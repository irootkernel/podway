//! Daemon-owned execution of durably admitted G005 mutations.
//!
//! This module deliberately accepts only protocol slice commands at admission. New admissions store
//! a canonical, self-contained execution document. Legacy v1/v2 documents fail closed because they
//! lack immutable admission resolutions. Query commands remain outside of this execution path.

use std::{error::Error, fmt};

use podway_config::ProcedureWarningPolicyV1;
use podway_core::{
    AddItemV1, ArtifactLocationKindV1, ArtifactValueV1, AttachItemV1, AttemptId, AttemptV1,
    BlockSessionV1, BlockerId, BlockerState, CanonicalProcedureJsonV1,
    CanonicalProcedureSnapshotInputV1, CheckItemV1, CommandContextV1, CompleteSessionV1,
    DomainCommand, DomainError, DomainResult, ItemId, ItemMutationPreconditionsV1, ItemTypeV1,
    ItemValueV1, JobId, LocalArtifactVerificationV1, ProcedureSnapshotId, ProcedureSnapshotV1,
    ProcedureSourceLabelV1, RetrySessionV1, ReturnSessionV1, Revision, SessionAggregateV1,
    SessionCommandV1, SessionId, SetItemV1, Sha256Digest, StageSpecV1, UnblockSessionV1,
    UnixMillis, WorkspaceId, apply_transition_v1, canonicalize_json_v1, required_items_satisfied,
};
use podway_presets::lookup as lookup_embedded_preset_v1;
use podway_protocol::{
    ItemAddV1, ItemAttachPathV1, ItemCheckV1, ItemSetV1, SessionBlockV1, SessionCompleteV1,
    SessionRetryV1, SessionReturnV1, SessionUnblockV1, SliceCommandV1, SliceRequestV1,
    WorktreeSelectorWireV1,
};
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, CanonicalExecutionJsonV1, ClaimedJobV1,
    DurableWorktreeIdentityV1, IdempotencyKeyV1, PersistedSessionMutationV1,
    RevisionAttemptItemPreconditionsV1, StateTransitionV1, StoreContractV1, StoreErrorV1,
    StoreIdempotencyReadContractV1, StoreValueErrorV1, TerminalReceiptV1, TerminalResultV1,
    WorkerIdV1, WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const EXECUTION_DOCUMENT_VERSION_V1: u8 = 1;
const EXECUTION_DOCUMENT_VERSION_V2: u8 = 2;
const EXECUTION_DOCUMENT_VERSION_V3: u8 = 3;

#[derive(Clone, Debug)]
enum AdmissionResolutionV1 {
    None,
    PresetStart {
        snapshot: Box<ProcedureSnapshotV1>,
        session_id: SessionId,
        first_attempt_id: AttemptId,
    },
    SessionBlock {
        blocker_id: BlockerId,
    },
    SessionRetry {
        next_attempt_id: AttemptId,
    },
    SessionReturn {
        destination_attempt_id: AttemptId,
    },
    SessionComplete {
        next_attempt_id: AttemptId,
    },
}

/// A boundary outcome that can either become a durable domain failure or leave the claim for
/// recovery. A transient outcome never causes the engine to commit a terminal result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionBoundaryErrorV1 {
    Domain(DomainError),
    Transient { operation: &'static str },
}

impl ExecutionBoundaryErrorV1 {
    pub const fn domain(error: DomainError) -> Self {
        Self::Domain(error)
    }

    pub const fn transient(operation: &'static str) -> Self {
        Self::Transient { operation }
    }
}

/// The sole source of generated domain IDs for an execution worker.
pub trait ExecutionIdSourceV1: Send + Sync {
    fn next_job_id(&self) -> JobId;
    fn next_session_id(&self) -> SessionId;
    fn next_attempt_id(&self) -> AttemptId;
    fn next_blocker_id(&self) -> BlockerId;
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId;
}

/// The daemon clock used while preparing new admissions and executing claimed jobs.
pub trait ExecutionClockV1: Send + Sync {
    fn now(&self) -> UnixMillis;
}

/// Loads an admitted preset through the public preset/config path and returns its immutable core
/// snapshot. Implementations must not apply daemon-only preset semantics.
pub trait ProcedureProviderV1: Send + Sync {
    fn load_preset_snapshot(
        &self,
        preset: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1>;
}

/// The production built-in preset provider. It deliberately uses the public preset validation and
/// config-to-core snapshot APIs rather than duplicating embedded preset handling in the daemon.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbeddedPresetProcedureProviderV1;

impl ProcedureProviderV1 for EmbeddedPresetProcedureProviderV1 {
    fn load_preset_snapshot(
        &self,
        preset: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        let preset = lookup_embedded_preset_v1(preset).ok_or(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "requested preset is unavailable",
            },
        ))?;
        preset
            .validate()
            .and_then(|preset| {
                preset.into_snapshot_v1(snapshot_id, created_at, ProcedureWarningPolicyV1::Accept)
            })
            .map_err(|_| {
                ExecutionBoundaryErrorV1::domain(DomainError::InvalidState {
                    reason: "embedded preset admission failed",
                })
            })
    }
}

/// Revalidates workspace evidence into the durable identity and lossless workspace root accepted
/// by the Store. Selector evidence is admission intent/audit data only; a manager-selected binding
/// is the sole execution authority. Display paths are diagnostic-only and must never be used as a
/// lookup key.
pub trait WorkspaceRevalidatorV1: Send + Sync {
    fn revalidate(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<WorkspaceBindingV1, ExecutionBoundaryErrorV1>;

    /// Revalidates the manager-selected binding immediately before a Store claim. Implementations
    /// must verify the complete durable identity and root evidence, not merely its workspace UUID.
    ///
    /// The default deliberately fails closed for adapters that have not implemented this execution
    /// authority boundary.
    fn revalidate_binding(
        &self,
        binding: &WorkspaceBindingV1,
    ) -> Result<WorkspaceBindingV1, ExecutionBoundaryErrorV1> {
        let _ = binding;
        Err(ExecutionBoundaryErrorV1::transient(
            "revalidate manager workspace binding",
        ))
    }
}

/// Streams a local artifact through a verified worktree boundary. The same operation is used when
/// attaching a path and again at completion, preventing a stored digest from standing in for a
/// current filesystem check.
pub trait ArtifactVerifierV1: Send + Sync {
    fn hash_local_artifact(
        &self,
        workspace: &WorkspaceBindingV1,
        path: &str,
        requested_media_type: Option<&str>,
    ) -> Result<ArtifactValueV1, ExecutionBoundaryErrorV1>;

    fn revalidate_local_artifact(
        &self,
        workspace: &WorkspaceBindingV1,
        item_id: &ItemId,
        artifact: &ArtifactValueV1,
    ) -> Result<LocalArtifactVerificationV1, ExecutionBoundaryErrorV1>;
}

/// Errors surfaced by admission or worker execution. Domain failures are represented by a durable
/// [`TerminalReceiptV1`], not by this error type.
#[derive(Debug)]
pub enum ExecutionErrorV1 {
    BoundaryDomain(DomainError),
    BoundaryTransient { operation: &'static str },
    InvalidPersistedExecution { reason: &'static str },
    InvalidStoreValue(StoreValueErrorV1),
    Store(StoreErrorV1),
}

impl fmt::Display for ExecutionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BoundaryDomain(error) => {
                write!(formatter, "execution boundary rejected request: {error}")
            }
            Self::BoundaryTransient { operation } => {
                write!(
                    formatter,
                    "execution boundary is transiently unavailable: {operation}"
                )
            }
            Self::InvalidPersistedExecution { reason } => {
                write!(formatter, "invalid persisted execution document: {reason}")
            }
            Self::InvalidStoreValue(error) => {
                write!(formatter, "invalid Store execution value: {error}")
            }
            Self::Store(error) => write!(formatter, "Store execution failure: {error}"),
        }
    }
}

impl Error for ExecutionErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BoundaryDomain(error) => Some(error),
            Self::BoundaryTransient { .. } | Self::InvalidPersistedExecution { .. } => None,
            Self::InvalidStoreValue(error) => Some(error),
            Self::Store(error) => Some(error),
        }
    }
}

impl From<StoreErrorV1> for ExecutionErrorV1 {
    fn from(error: StoreErrorV1) -> Self {
        Self::Store(error)
    }
}

/// The daemon-owned production mutation executor. The Store remains protocol-free: protocol data
/// is decoded only at the daemon boundary and translated to typed core commands before commit.
pub struct DaemonExecutionEngineV1<Store, Ids, Clock, Procedures, Artifacts, Workspaces>
where
    Store: StoreContractV1 + StoreIdempotencyReadContractV1,
    Ids: ExecutionIdSourceV1,
    Clock: ExecutionClockV1,
    Procedures: ProcedureProviderV1,
    Artifacts: ArtifactVerifierV1,
    Workspaces: WorkspaceRevalidatorV1,
{
    store: Store,
    ids: Ids,
    clock: Clock,
    procedures: Procedures,
    artifacts: Artifacts,
    workspaces: Workspaces,
}

impl<Store, Ids, Clock, Procedures, Artifacts, Workspaces>
    DaemonExecutionEngineV1<Store, Ids, Clock, Procedures, Artifacts, Workspaces>
where
    Store: StoreContractV1 + StoreIdempotencyReadContractV1,
    Ids: ExecutionIdSourceV1,
    Clock: ExecutionClockV1,
    Procedures: ProcedureProviderV1,
    Artifacts: ArtifactVerifierV1,
    Workspaces: WorkspaceRevalidatorV1,
{
    pub fn new(
        store: Store,
        ids: Ids,
        clock: Clock,
        procedures: Procedures,
        artifacts: Artifacts,
        workspaces: Workspaces,
    ) -> Self {
        Self {
            store,
            ids,
            clock,
            procedures,
            artifacts,
            workspaces,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Revalidates the selector, creates the complete immutable execution document, and asks the
    /// Store to durably admit the typed domain command. This selector-only boundary intentionally
    /// cannot pre-read an idempotency outcome: it has no durable workspace identity before
    /// revalidation. Existing idempotency outcomes are returned unchanged and never execute a
    /// second time.
    pub fn admit(
        &self,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1> {
        self.admit_with_expected_workspace(None, request, idempotency_key)
    }

    /// The scheduler binding supplies a durable workspace identity, so an exact idempotency replay
    /// is read from the Store before selector revalidation. New admissions still revalidate the
    /// selector.
    pub fn admit_for_workspace(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1> {
        self.admit_with_expected_workspace(Some(expected_workspace), request, idempotency_key)
    }

    fn admit_with_expected_workspace(
        &self,
        expected_workspace: Option<&WorkspaceBindingV1>,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1> {
        if let Some(expected_workspace) = expected_workspace {
            let request_digest =
                request_digest_v1(request, expected_workspace.identity().workspace_uuid())?;
            if let Some(outcome) = self.store.read_idempotent_outcome(
                expected_workspace.identity(),
                &idempotency_key,
                &request_digest,
            )? {
                return Ok(outcome);
            }
        }
        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)?;
        if expected_workspace.is_some_and(|expected| expected.identity() != binding.identity()) {
            return Err(ExecutionErrorV1::BoundaryDomain(
                DomainError::InvalidState {
                    reason: "revalidated workspace does not match the scheduler identity",
                },
            ));
        }
        let command = command_for_admission_v1(request.command())?;
        let preconditions = store_preconditions_v1(request.command())?;
        let now = self.clock.now();
        let resolution = self.admission_resolution(request.command(), now)?;
        let canonical_execution =
            canonical_execution_document_v1(request, binding.identity(), &resolution)?;
        let request_digest = request_digest_v1(request, binding.identity().workspace_uuid())?;
        let admitted = AdmitRequestV1::new_with_canonical_execution(
            command,
            idempotency_key,
            self.ids.next_job_id(),
            preconditions,
            request_digest,
            now,
            canonical_execution,
        );
        self.store
            .admit(binding.identity(), admitted)
            .map_err(Into::into)
    }

    /// Revalidates the manager-selected workspace before claiming at most one job. A transient
    /// boundary error leaves the Store queue untouched for recovery; a domain error is committed
    /// only after a claim has been made for a valid workspace.
    pub fn execute_next(
        &self,
        scheduled_workspace: &WorkspaceBindingV1,
        worker: WorkerIdV1,
    ) -> Result<Option<TerminalReceiptV1>, ExecutionErrorV1> {
        let workspace = self
            .bound_manager_workspace(scheduled_workspace)
            .map_err(ExecutionErrorV1::from_boundary)?;
        let now = self.clock.now();
        let Some(claimed) = self.store.claim_next(workspace.identity(), worker, now)? else {
            return Ok(None);
        };
        self.execute_claimed(&workspace, claimed, now).map(Some)
    }

    fn execute_claimed(
        &self,
        workspace: &WorkspaceBindingV1,
        claimed: ClaimedJobV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        let expected_workspace_revision = claimed
            .current_session()
            .map_or(Revision::ZERO, SessionAggregateV1::revision);
        let (_selector, workspace_id, command, resolution) =
            decode_execution_document_v1(claimed.execution().canonical_execution().as_str())?;
        if workspace_id != *claimed.claim().identity().workspace_uuid() {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "workspace identity does not match the claim",
            });
        }
        if workspace.identity() != claimed.claim().identity() {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "scheduler workspace does not match the claim",
            });
        }

        let admitted_command = match command_for_admission_v1(&command) {
            Ok(command) => command,
            Err(_) => {
                return Err(ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "command is not admitted for execution",
                });
            }
        };
        let admitted_preconditions = match store_preconditions_v1(&command) {
            Ok(preconditions) => preconditions,
            Err(_) => {
                return Err(ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "command preconditions cannot be reconstructed",
                });
            }
        };
        if claimed.execution().command() != &admitted_command
            || claimed.execution().preconditions() != &admitted_preconditions
        {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "document does not match Store execution metadata",
            });
        }

        if matches!(command, SliceCommandV1::WorkspaceInit) {
            return self.commit_workspace_initialization(
                &claimed,
                expected_workspace_revision,
                now,
            );
        }

        let session_command = match self.prepare_session_command(
            &command,
            &resolution,
            claimed.current_session(),
            workspace,
            now,
        ) {
            Ok(command) => command,
            Err(BoundaryDispositionV1::Domain(error)) => {
                return self.commit_domain_failure(
                    &claimed,
                    expected_workspace_revision,
                    error,
                    now,
                );
            }
            Err(BoundaryDispositionV1::Transient { operation }) => {
                return Err(ExecutionErrorV1::BoundaryTransient { operation });
            }
        };

        let context = CommandContextV1 {
            expected_revision: expected_revision_v1(&command),
            now,
        };
        let outcome =
            match apply_transition_v1(claimed.current_session(), &session_command, context) {
                Ok(outcome) => outcome,
                Err(error) => {
                    return self.commit_domain_failure(
                        &claimed,
                        expected_workspace_revision,
                        error,
                        now,
                    );
                }
            };
        let transition = match state_transition_v1(claimed.current_session(), &outcome) {
            Ok(transition) => transition,
            Err(error) => {
                return self.commit_domain_failure(
                    &claimed,
                    expected_workspace_revision,
                    error,
                    now,
                );
            }
        };
        let result = match domain_result_v1(
            &admitted_command,
            &outcome,
            claimed.claim().identity().workspace_uuid(),
        ) {
            Ok(result) => result,
            Err(error) => {
                return self.commit_domain_failure(
                    &claimed,
                    expected_workspace_revision,
                    error,
                    now,
                );
            }
        };
        self.store
            .commit_terminal(
                claimed.claim().clone(),
                expected_workspace_revision,
                Some(transition),
                TerminalResultV1::Success(result),
                now,
            )
            .map_err(Into::into)
    }

    fn commit_workspace_initialization(
        &self,
        claimed: &ClaimedJobV1,
        expected_workspace_revision: Revision,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        if expected_workspace_revision != Revision::ZERO || claimed.current_session().is_some() {
            return self.commit_domain_failure(
                claimed,
                expected_workspace_revision,
                DomainError::InvalidState {
                    reason: "workspace initialization requires an empty workspace",
                },
                now,
            );
        }
        let transition = StateTransitionV1::new_persisted(
            None,
            Revision::ZERO,
            Revision::ZERO,
            PersistedSessionMutationV1::Unchanged,
        )
        .map_err(ExecutionErrorV1::InvalidStoreValue)?;
        let result = DomainResult::WorkspaceInitialized {
            workspace_id: claimed.claim().identity().workspace_uuid().clone(),
            revision: Revision::ZERO,
        };
        self.store
            .commit_terminal(
                claimed.claim().clone(),
                Revision::ZERO,
                Some(transition),
                TerminalResultV1::Success(result),
                now,
            )
            .map_err(Into::into)
    }

    fn commit_domain_failure(
        &self,
        claimed: &ClaimedJobV1,
        expected_workspace_revision: Revision,
        error: DomainError,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        self.store
            .commit_terminal(
                claimed.claim().clone(),
                expected_workspace_revision,
                None,
                TerminalResultV1::Failure(error),
                now,
            )
            .map_err(Into::into)
    }

    fn bound_workspace(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<WorkspaceBindingV1, BoundaryDispositionV1> {
        let binding = self
            .workspaces
            .revalidate(selector)
            .map_err(BoundaryDispositionV1::from)?;
        if selector
            .expected_uuid()
            .is_some_and(|expected| expected != binding.identity().workspace_uuid())
        {
            return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                reason: "selector workspace UUID does not match revalidated identity",
            }));
        }
        Ok(binding)
    }

    fn bound_manager_workspace(
        &self,
        scheduled_workspace: &WorkspaceBindingV1,
    ) -> Result<WorkspaceBindingV1, BoundaryDispositionV1> {
        let binding = self
            .workspaces
            .revalidate_binding(scheduled_workspace)
            .map_err(BoundaryDispositionV1::from)?;
        if &binding != scheduled_workspace {
            return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                reason: "revalidated workspace does not match the scheduler binding",
            }));
        }
        Ok(binding)
    }
    fn admission_resolution(
        &self,
        command: &SliceCommandV1,
        now: UnixMillis,
    ) -> Result<AdmissionResolutionV1, ExecutionErrorV1> {
        match command {
            SliceCommandV1::PresetStart(input) => {
                let snapshot = self
                    .procedures
                    .load_preset_snapshot(&input.preset, self.ids.next_procedure_snapshot_id(), now)
                    .map_err(|error| ExecutionErrorV1::from_boundary(error.into()))?;
                Ok(AdmissionResolutionV1::PresetStart {
                    snapshot: Box::new(snapshot),
                    session_id: self.ids.next_session_id(),
                    first_attempt_id: self.ids.next_attempt_id(),
                })
            }
            SliceCommandV1::SessionBlock(_) => Ok(AdmissionResolutionV1::SessionBlock {
                blocker_id: self.ids.next_blocker_id(),
            }),
            SliceCommandV1::SessionRetry(_) => Ok(AdmissionResolutionV1::SessionRetry {
                next_attempt_id: self.ids.next_attempt_id(),
            }),
            SliceCommandV1::SessionReturn(_) => Ok(AdmissionResolutionV1::SessionReturn {
                destination_attempt_id: self.ids.next_attempt_id(),
            }),
            SliceCommandV1::SessionComplete(_) => Ok(AdmissionResolutionV1::SessionComplete {
                next_attempt_id: self.ids.next_attempt_id(),
            }),
            SliceCommandV1::WorkspaceInit
            | SliceCommandV1::ItemCheck(_)
            | SliceCommandV1::ItemSet(_)
            | SliceCommandV1::ItemAdd(_)
            | SliceCommandV1::ItemAttachPath(_)
            | SliceCommandV1::SessionUnblock(_)
            | SliceCommandV1::Status
            | SliceCommandV1::Next => Ok(AdmissionResolutionV1::None),
        }
    }

    fn prepare_session_command(
        &self,
        command: &SliceCommandV1,
        resolution: &AdmissionResolutionV1,
        prior: Option<&SessionAggregateV1>,
        workspace: &WorkspaceBindingV1,
        _now: UnixMillis,
    ) -> Result<SessionCommandV1, BoundaryDispositionV1> {
        match command {
            SliceCommandV1::PresetStart(input) => {
                let (snapshot, session_id, first_attempt_id) = match resolution {
                    AdmissionResolutionV1::PresetStart {
                        snapshot,
                        session_id,
                        first_attempt_id,
                    } => (
                        snapshot.as_ref().clone(),
                        session_id.clone(),
                        first_attempt_id.clone(),
                    ),
                    _ => {
                        return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                            reason: "persisted admission resolution does not match preset start",
                        }));
                    }
                };
                Ok(SessionCommandV1::Start(podway_core::StartSessionV1 {
                    task_title: input.task_title.clone(),
                    snapshot,
                    session_id,
                    first_attempt_id,
                }))
            }
            SliceCommandV1::ItemCheck(input) => Ok(SessionCommandV1::Check(CheckItemV1 {
                item_id: input.item_id.clone(),
                preconditions: core_item_preconditions_v1(&input.preconditions),
            })),
            SliceCommandV1::ItemSet(input) => {
                let value = set_item_value_v1(prior, &input.item_id, &input.value)
                    .map_err(BoundaryDispositionV1::Domain)?;
                Ok(SessionCommandV1::Set(SetItemV1 {
                    item_id: input.item_id.clone(),
                    value,
                    preconditions: core_item_preconditions_v1(&input.preconditions),
                }))
            }
            SliceCommandV1::ItemAdd(input) => Ok(SessionCommandV1::Add(AddItemV1 {
                item_id: input.item_id.clone(),
                value: input.value.clone(),
                preconditions: core_item_preconditions_v1(&input.preconditions),
            })),
            SliceCommandV1::ItemAttachPath(input) => {
                let artifact = self
                    .artifacts
                    .hash_local_artifact(workspace, &input.path, input.media_type.as_deref())
                    .map_err(BoundaryDispositionV1::from)?;
                validate_attached_artifact_v1(input, &artifact)
                    .map_err(BoundaryDispositionV1::Domain)?;
                Ok(SessionCommandV1::Attach(AttachItemV1 {
                    item_id: input.item_id.clone(),
                    value: artifact,
                    preconditions: core_item_preconditions_v1(&input.preconditions),
                }))
            }
            SliceCommandV1::SessionBlock(input) => {
                let blocker_id = match resolution {
                    AdmissionResolutionV1::SessionBlock { blocker_id } => blocker_id.clone(),
                    _ => {
                        return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                            reason: "persisted admission resolution does not match session block",
                        }));
                    }
                };
                Ok(SessionCommandV1::Block(BlockSessionV1 {
                    expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                    blocker_id,
                    reason: input.reason.clone(),
                }))
            }
            SliceCommandV1::SessionUnblock(input) => {
                Ok(SessionCommandV1::Unblock(UnblockSessionV1 {
                    expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                    blocker_id: input.blocker_id.clone(),
                    unblock_all: input.all,
                }))
            }
            SliceCommandV1::SessionRetry(input) => {
                let next_attempt_id = match resolution {
                    AdmissionResolutionV1::SessionRetry { next_attempt_id } => {
                        next_attempt_id.clone()
                    }
                    _ => {
                        return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                            reason: "persisted admission resolution does not match session retry",
                        }));
                    }
                };
                Ok(SessionCommandV1::Retry(RetrySessionV1 {
                    expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                    reason: input.reason.clone(),
                    next_attempt_id,
                }))
            }
            SliceCommandV1::SessionReturn(input) => {
                let destination_attempt_id = match resolution {
                    AdmissionResolutionV1::SessionReturn {
                        destination_attempt_id,
                    } => destination_attempt_id.clone(),
                    _ => {
                        return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                            reason: "persisted admission resolution does not match session return",
                        }));
                    }
                };
                Ok(SessionCommandV1::Return(ReturnSessionV1 {
                    expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                    destination_stage_id: input.destination_stage_id.clone(),
                    reason: input.reason.clone(),
                    destination_attempt_id,
                }))
            }
            SliceCommandV1::SessionComplete(input) => {
                let (stage, attempt) =
                    active_stage_attempt_v1(prior).map_err(BoundaryDispositionV1::Domain)?;
                if attempt
                    .blockers()
                    .iter()
                    .any(|blocker| blocker.state() == BlockerState::Open)
                {
                    return Err(BoundaryDispositionV1::Domain(DomainError::BlockersPresent));
                }
                if !required_items_satisfied(stage, attempt.item_slots()) {
                    return Err(BoundaryDispositionV1::Domain(
                        DomainError::RequiredItemsMissing,
                    ));
                }
                if !attempt.is_ready_to_complete(stage) {
                    return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                        reason: "the active attempt is not ready to complete",
                    }));
                }
                let verifications = self.local_artifact_verifications(attempt, workspace)?;
                let next_attempt_id = if is_final_stage_v1(prior, stage)
                    .map_err(BoundaryDispositionV1::Domain)?
                {
                    None
                } else {
                    Some(match resolution {
                        AdmissionResolutionV1::SessionComplete { next_attempt_id } => {
                            next_attempt_id.clone()
                        }
                        _ => {
                            return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                                reason: "persisted admission resolution does not match session completion",
                            }));
                        }
                    })
                };
                Ok(SessionCommandV1::Complete(CompleteSessionV1 {
                    expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                    next_attempt_id,
                    local_artifact_verifications: verifications,
                }))
            }
            SliceCommandV1::WorkspaceInit | SliceCommandV1::Status | SliceCommandV1::Next => {
                Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                    reason: "command is not a session mutation",
                }))
            }
        }
    }

    fn local_artifact_verifications(
        &self,
        attempt: &AttemptV1,
        workspace: &WorkspaceBindingV1,
    ) -> Result<Vec<LocalArtifactVerificationV1>, BoundaryDispositionV1> {
        let mut verifications = Vec::new();
        for slot in attempt.item_slots() {
            let Some(artifact) = slot.value().and_then(ItemValueV1::as_artifact) else {
                continue;
            };
            if artifact.location_kind() != ArtifactLocationKindV1::LocalPath {
                continue;
            }
            let verification = self
                .artifacts
                .revalidate_local_artifact(workspace, slot.item_id(), artifact)
                .map_err(BoundaryDispositionV1::from)?;
            if verification.item_id != *slot.item_id() {
                return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                    reason: "artifact verifier returned a different item identifier",
                }));
            }
            verifications.push(verification);
        }
        Ok(verifications)
    }
}

#[derive(Debug)]
enum BoundaryDispositionV1 {
    Domain(DomainError),
    Transient { operation: &'static str },
}

impl From<ExecutionBoundaryErrorV1> for BoundaryDispositionV1 {
    fn from(error: ExecutionBoundaryErrorV1) -> Self {
        match error {
            ExecutionBoundaryErrorV1::Domain(error) => Self::Domain(error),
            ExecutionBoundaryErrorV1::Transient { operation } => Self::Transient { operation },
        }
    }
}

impl ExecutionErrorV1 {
    fn from_boundary(error: BoundaryDispositionV1) -> Self {
        match error {
            BoundaryDispositionV1::Domain(error) => Self::BoundaryDomain(error),
            BoundaryDispositionV1::Transient { operation } => Self::BoundaryTransient { operation },
        }
    }
}

fn command_for_admission_v1(command: &SliceCommandV1) -> Result<DomainCommand, ExecutionErrorV1> {
    let command = match command {
        SliceCommandV1::WorkspaceInit => DomainCommand::WorkspaceInitialize,
        SliceCommandV1::PresetStart(_) => DomainCommand::SessionStart,
        SliceCommandV1::ItemCheck(input) => DomainCommand::ItemCheck {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemSet(input) => DomainCommand::ItemSet {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemAdd(input) => DomainCommand::ItemAdd {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemAttachPath(input) => DomainCommand::ItemAttach {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::SessionBlock(_) => DomainCommand::SessionBlock,
        SliceCommandV1::SessionUnblock(_) => DomainCommand::SessionUnblock,
        SliceCommandV1::SessionRetry(_) => DomainCommand::SessionRetry,
        SliceCommandV1::SessionReturn(_) => DomainCommand::SessionReturn,
        SliceCommandV1::SessionComplete(_) => DomainCommand::SessionComplete,
        SliceCommandV1::Status | SliceCommandV1::Next => {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "read command reached the mutation executor",
            });
        }
    };
    Ok(command)
}

fn store_preconditions_v1(
    command: &SliceCommandV1,
) -> Result<RevisionAttemptItemPreconditionsV1, ExecutionErrorV1> {
    let (session_revision, attempt_id, item_id, item_revision) = match command {
        SliceCommandV1::ItemCheck(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemSet(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemAdd(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemAttachPath(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::SessionBlock(input) => session_store_preconditions_v1(&input.preconditions),
        SliceCommandV1::SessionUnblock(input) => {
            session_store_preconditions_v1(&input.preconditions)
        }
        SliceCommandV1::SessionRetry(input) => session_store_preconditions_v1(&input.preconditions),
        SliceCommandV1::SessionReturn(input) => {
            session_store_preconditions_v1(&input.preconditions)
        }
        SliceCommandV1::SessionComplete(input) => {
            session_store_preconditions_v1(&input.preconditions)
        }
        SliceCommandV1::WorkspaceInit | SliceCommandV1::PresetStart(_) => (None, None, None, None),
        SliceCommandV1::Status | SliceCommandV1::Next => {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "read command has no Store mutation preconditions",
            });
        }
    };
    RevisionAttemptItemPreconditionsV1::new(session_revision, attempt_id, item_id, item_revision)
        .map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn item_store_preconditions_v1(
    item_id: &ItemId,
    preconditions: &podway_protocol::ItemMutationPreconditionsWireV1,
) -> (
    Option<Revision>,
    Option<AttemptId>,
    Option<ItemId>,
    Option<Revision>,
) {
    (
        None,
        Some(preconditions.expected_attempt_id.clone()),
        Some(item_id.clone()),
        Some(preconditions.expected_item_revision),
    )
}

fn session_store_preconditions_v1(
    preconditions: &podway_protocol::SessionMutationPreconditionsWireV1,
) -> (
    Option<Revision>,
    Option<AttemptId>,
    Option<ItemId>,
    Option<Revision>,
) {
    (
        Some(preconditions.expected_session_revision),
        Some(preconditions.expected_attempt_id.clone()),
        None,
        None,
    )
}

fn expected_revision_v1(command: &SliceCommandV1) -> Revision {
    match command {
        SliceCommandV1::SessionBlock(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionUnblock(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionRetry(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionReturn(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionComplete(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::WorkspaceInit | SliceCommandV1::PresetStart(_) => Revision::ZERO,
        SliceCommandV1::ItemCheck(_)
        | SliceCommandV1::ItemSet(_)
        | SliceCommandV1::ItemAdd(_)
        | SliceCommandV1::ItemAttachPath(_)
        | SliceCommandV1::Status
        | SliceCommandV1::Next => Revision::ZERO,
    }
}

fn core_item_preconditions_v1(
    preconditions: &podway_protocol::ItemMutationPreconditionsWireV1,
) -> ItemMutationPreconditionsV1 {
    ItemMutationPreconditionsV1 {
        expected_attempt_id: preconditions.expected_attempt_id.clone(),
        expected_item_revision: preconditions.expected_item_revision,
    }
}

fn set_item_value_v1(
    prior: Option<&SessionAggregateV1>,
    item_id: &ItemId,
    wire_value: &str,
) -> Result<ItemValueV1, DomainError> {
    let (stage, _) = active_stage_attempt_v1(prior)?;
    let specification = stage
        .item(item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: item_id.clone(),
        })?;
    match specification.item_type() {
        ItemTypeV1::Text => Ok(ItemValueV1::text(wire_value)),
        ItemTypeV1::Choice => ItemValueV1::choice(wire_value),
        ItemTypeV1::Integer => wire_value
            .parse::<i64>()
            .map(ItemValueV1::integer)
            .map_err(|_| DomainError::InvalidState {
                reason: "integer item values must be canonical signed integers",
            }),
        ItemTypeV1::Confirm | ItemTypeV1::List | ItemTypeV1::Artifact => {
            Err(DomainError::InvalidState {
                reason: "set is not valid for this item type",
            })
        }
    }
}

fn validate_attached_artifact_v1(
    input: &ItemAttachPathV1,
    artifact: &ArtifactValueV1,
) -> Result<(), DomainError> {
    if artifact.location_kind() != ArtifactLocationKindV1::LocalPath
        || artifact.location() != input.path
    {
        return Err(DomainError::InvalidState {
            reason: "artifact verifier did not preserve the requested local path",
        });
    }
    if input
        .media_type
        .as_deref()
        .is_some_and(|media_type| media_type != artifact.media_type())
    {
        return Err(DomainError::InvalidState {
            reason: "artifact verifier did not preserve the requested media type",
        });
    }
    if input.media_type.is_none() && artifact.media_type().is_empty() {
        return Err(DomainError::InvalidState {
            reason: "artifact verifier did not provide a media type",
        });
    }
    Ok(())
}

fn active_stage_attempt_v1(
    prior: Option<&SessionAggregateV1>,
) -> Result<(&StageSpecV1, &AttemptV1), DomainError> {
    let session = prior.ok_or(DomainError::InvalidState {
        reason: "a session is required",
    })?;
    let stage_id = session.active_stage_id().ok_or(DomainError::InvalidState {
        reason: "a running session requires an active stage",
    })?;
    let attempt_id = session
        .active_attempt_id()
        .ok_or(DomainError::InvalidState {
            reason: "a running session requires an active attempt",
        })?;
    let stage = session
        .snapshot()
        .stages()
        .iter()
        .find(|stage| stage.id() == stage_id)
        .ok_or(DomainError::InvalidState {
            reason: "active stage is absent from the procedure snapshot",
        })?;
    let attempt = session
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == attempt_id)
        .ok_or(DomainError::InvalidState {
            reason: "active attempt is absent from the session",
        })?;
    Ok((stage, attempt))
}

fn is_final_stage_v1(
    prior: Option<&SessionAggregateV1>,
    stage: &StageSpecV1,
) -> Result<bool, DomainError> {
    let session = prior.ok_or(DomainError::InvalidState {
        reason: "a session is required",
    })?;
    let stage_index = session
        .snapshot()
        .stages()
        .iter()
        .position(|candidate| candidate.id() == stage.id())
        .ok_or(DomainError::InvalidState {
            reason: "active stage is absent from the procedure snapshot",
        })?;
    Ok(stage_index + 1 == session.snapshot().stages().len())
}

fn state_transition_v1(
    prior: Option<&SessionAggregateV1>,
    outcome: &podway_core::TransitionOutcomeV1,
) -> Result<StateTransitionV1, DomainError> {
    let previous_workspace_revision = prior.map_or(Revision::ZERO, SessionAggregateV1::revision);
    let resulting_workspace_revision = outcome
        .revision_after()
        .unwrap_or(previous_workspace_revision);
    let next = outcome.next_aggregate().ok_or(DomainError::InvalidState {
        reason: "admitted session transition has no next aggregate",
    })?;
    let persisted_session_mutation = if outcome.changed() {
        PersistedSessionMutationV1::Replace(next.clone())
    } else {
        PersistedSessionMutationV1::Unchanged
    };
    StateTransitionV1::new_persisted(
        Some(next.session_id().clone()),
        previous_workspace_revision,
        resulting_workspace_revision,
        persisted_session_mutation,
    )
    .map_err(|_| DomainError::InvalidState {
        reason: "transition cannot be persisted",
    })
}

fn domain_result_v1(
    command: &DomainCommand,
    outcome: &podway_core::TransitionOutcomeV1,
    workspace_id: &WorkspaceId,
) -> Result<DomainResult, DomainError> {
    let next = outcome.next_aggregate().ok_or(DomainError::InvalidState {
        reason: "admitted session transition has no next aggregate",
    })?;
    let revision_before = outcome.revision_before().unwrap_or(Revision::ZERO);
    let revision_after = outcome.revision_after().unwrap_or(revision_before);
    let changed = outcome.changed();
    let result = match command {
        DomainCommand::ItemCheck { item_id }
        | DomainCommand::ItemSet { item_id }
        | DomainCommand::ItemAdd { item_id }
        | DomainCommand::ItemAttach { item_id } => DomainResult::ItemChanged {
            session_id: next.session_id().clone(),
            item_id: item_id.clone(),
            revision_before,
            revision_after,
            changed,
        },
        DomainCommand::SessionStart
        | DomainCommand::SessionComplete
        | DomainCommand::SessionRetry
        | DomainCommand::SessionReturn
        | DomainCommand::SessionBlock
        | DomainCommand::SessionUnblock => DomainResult::SessionChanged {
            session_id: next.session_id().clone(),
            revision_before,
            revision_after,
            changed,
        },
        DomainCommand::WorkspaceInitialize => DomainResult::WorkspaceInitialized {
            workspace_id: workspace_id.clone(),
            revision: Revision::ZERO,
        },
        _ => {
            return Err(DomainError::InvalidState {
                reason: "domain command is outside the admitted execution set",
            });
        }
    };
    Ok(result)
}

fn canonical_execution_document_v1(
    request: &SliceRequestV1,
    identity: &DurableWorktreeIdentityV1,
    resolution: &AdmissionResolutionV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let (preconditions, payload) = execution_components_v1(request.command());
    let document = json!({
        "command": request.command().command_name(),
        "execution": admission_resolution_value_v1(resolution),
        "execution_version": EXECUTION_DOCUMENT_VERSION_V3,
        "payload": payload,
        "preconditions": preconditions,
        "selector": request.selector(),
        "workspace_id": identity.workspace_uuid(),
    });
    let canonical = canonicalize_json_v1(&document).map_err(|_| {
        ExecutionErrorV1::InvalidPersistedExecution {
            reason: "admitted execution document cannot be canonicalized",
        }
    })?;
    CanonicalExecutionJsonV1::new(canonical).map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn request_digest_v1(
    request: &SliceRequestV1,
    workspace_id: &WorkspaceId,
) -> Result<Sha256Digest, ExecutionErrorV1> {
    let canonical = podway_protocol::canonical_mutation_identity_v1(request, workspace_id)
        .map_err(|_| ExecutionErrorV1::InvalidPersistedExecution {
            reason: "request identity cannot be canonicalized",
        })?;
    Sha256Digest::new(format!("sha256:{}", sha256_hex_v1(canonical.as_bytes()))).map_err(|_| {
        ExecutionErrorV1::InvalidPersistedExecution {
            reason: "request identity digest is invalid",
        }
    })
}

fn execution_components_v1(command: &SliceCommandV1) -> (Value, Value) {
    match command {
        SliceCommandV1::WorkspaceInit => (json!({}), json!({})),
        SliceCommandV1::PresetStart(input) => (
            json!({}),
            json!({"preset": input.preset, "task_title": input.task_title}),
        ),
        SliceCommandV1::ItemCheck(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({"item_id": input.item_id}),
        ),
        SliceCommandV1::ItemSet(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({"item_id": input.item_id, "value": input.value}),
        ),
        SliceCommandV1::ItemAdd(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({"item_id": input.item_id, "value": input.value}),
        ),
        SliceCommandV1::ItemAttachPath(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({
                "item_id": input.item_id,
                "media_type": input.media_type,
                "path": input.path,
            }),
        ),
        SliceCommandV1::SessionBlock(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"reason": input.reason}),
        ),
        SliceCommandV1::SessionUnblock(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"all": input.all, "blocker_id": input.blocker_id}),
        ),
        SliceCommandV1::SessionRetry(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"reason": input.reason}),
        ),
        SliceCommandV1::SessionReturn(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"destination_stage_id": input.destination_stage_id, "reason": input.reason}),
        ),
        SliceCommandV1::SessionComplete(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({}),
        ),
        SliceCommandV1::Status | SliceCommandV1::Next => (json!({}), json!({})),
    }
}

fn item_preconditions_value_v1(
    preconditions: &podway_protocol::ItemMutationPreconditionsWireV1,
) -> Value {
    json!({
        "attempt_id": preconditions.expected_attempt_id,
        "item_revision": preconditions.expected_item_revision,
    })
}

fn session_preconditions_value_v1(
    preconditions: &podway_protocol::SessionMutationPreconditionsWireV1,
) -> Value {
    json!({
        "attempt_id": preconditions.expected_attempt_id,
        "session_revision": preconditions.expected_session_revision,
    })
}
fn admission_resolution_value_v1(resolution: &AdmissionResolutionV1) -> Value {
    match resolution {
        AdmissionResolutionV1::None => json!({"kind": "none"}),
        AdmissionResolutionV1::PresetStart {
            snapshot,
            session_id,
            first_attempt_id,
        } => json!({
            "first_attempt_id": first_attempt_id.as_str(),
            "kind": "preset_start",
            "session_id": session_id.as_str(),
            "snapshot": {
                "canonical_json": snapshot.canonical_json().as_str(),
                "created_at": snapshot.created_at().get(),
                "digest": snapshot.digest().as_str(),
                "name": snapshot.name(),
                "procedure_id": snapshot.procedure_id(),
                "procedure_version": snapshot.procedure_version(),
                "snapshot_id": snapshot.snapshot_id().as_str(),
                "source_label": snapshot.source_label().as_str(),
            },
        }),
        AdmissionResolutionV1::SessionBlock { blocker_id } => {
            json!({"blocker_id": blocker_id.as_str(), "kind": "session_block"})
        }
        AdmissionResolutionV1::SessionRetry { next_attempt_id } => {
            json!({"kind": "session_retry", "next_attempt_id": next_attempt_id.as_str()})
        }
        AdmissionResolutionV1::SessionReturn {
            destination_attempt_id,
        } => {
            json!({
                "destination_attempt_id": destination_attempt_id.as_str(),
                "kind": "session_return",
            })
        }
        AdmissionResolutionV1::SessionComplete { next_attempt_id } => {
            json!({"kind": "session_complete", "next_attempt_id": next_attempt_id.as_str()})
        }
    }
}

fn decode_admission_resolution_v1(
    command: &SliceCommandV1,
    object: &Map<String, Value>,
) -> Result<AdmissionResolutionV1, ExecutionErrorV1> {
    let kind = value_string_v1(object, "kind")?;
    match command {
        SliceCommandV1::PresetStart(_) => {
            require_exact_keys_v1(
                object,
                &["first_attempt_id", "kind", "session_id", "snapshot"],
            )?;
            if kind != "preset_start" {
                return Err(invalid_execution_v1(
                    "execution resolution kind does not match command",
                ));
            }
            Ok(AdmissionResolutionV1::PresetStart {
                snapshot: Box::new(decode_snapshot_v1(value_object_v1(object, "snapshot")?)?),
                session_id: value_typed_v1(object, "session_id")?,
                first_attempt_id: value_typed_v1(object, "first_attempt_id")?,
            })
        }
        SliceCommandV1::SessionBlock(_) => {
            require_exact_keys_v1(object, &["blocker_id", "kind"])?;
            if kind != "session_block" {
                return Err(invalid_execution_v1(
                    "execution resolution kind does not match command",
                ));
            }
            Ok(AdmissionResolutionV1::SessionBlock {
                blocker_id: value_typed_v1(object, "blocker_id")?,
            })
        }
        SliceCommandV1::SessionRetry(_) => {
            require_exact_keys_v1(object, &["kind", "next_attempt_id"])?;
            if kind != "session_retry" {
                return Err(invalid_execution_v1(
                    "execution resolution kind does not match command",
                ));
            }
            Ok(AdmissionResolutionV1::SessionRetry {
                next_attempt_id: value_typed_v1(object, "next_attempt_id")?,
            })
        }
        SliceCommandV1::SessionReturn(_) => {
            require_exact_keys_v1(object, &["destination_attempt_id", "kind"])?;
            if kind != "session_return" {
                return Err(invalid_execution_v1(
                    "execution resolution kind does not match command",
                ));
            }
            Ok(AdmissionResolutionV1::SessionReturn {
                destination_attempt_id: value_typed_v1(object, "destination_attempt_id")?,
            })
        }
        SliceCommandV1::SessionComplete(_) => {
            require_exact_keys_v1(object, &["kind", "next_attempt_id"])?;
            if kind != "session_complete" {
                return Err(invalid_execution_v1(
                    "execution resolution kind does not match command",
                ));
            }
            Ok(AdmissionResolutionV1::SessionComplete {
                next_attempt_id: value_typed_v1(object, "next_attempt_id")?,
            })
        }
        SliceCommandV1::WorkspaceInit
        | SliceCommandV1::ItemCheck(_)
        | SliceCommandV1::ItemSet(_)
        | SliceCommandV1::ItemAdd(_)
        | SliceCommandV1::ItemAttachPath(_)
        | SliceCommandV1::SessionUnblock(_)
        | SliceCommandV1::Status
        | SliceCommandV1::Next => {
            require_exact_keys_v1(object, &["kind"])?;
            if kind != "none" {
                return Err(invalid_execution_v1(
                    "execution resolution kind does not match command",
                ));
            }
            Ok(AdmissionResolutionV1::None)
        }
    }
}

fn decode_snapshot_v1(
    object: &Map<String, Value>,
) -> Result<ProcedureSnapshotV1, ExecutionErrorV1> {
    require_exact_keys_v1(
        object,
        &[
            "canonical_json",
            "created_at",
            "digest",
            "name",
            "procedure_id",
            "procedure_version",
            "snapshot_id",
            "source_label",
        ],
    )?;
    let canonical_json =
        CanonicalProcedureJsonV1::new(value_string_v1(object, "canonical_json")?.to_owned())
            .map_err(|_| invalid_execution_v1("snapshot canonical procedure is invalid"))?;
    let source_label =
        ProcedureSourceLabelV1::new(value_string_v1(object, "source_label")?.to_owned())
            .map_err(|_| invalid_execution_v1("snapshot source label is invalid"))?;
    ProcedureSnapshotV1::from_canonical_json(CanonicalProcedureSnapshotInputV1 {
        snapshot_id: value_typed_v1(object, "snapshot_id")?,
        schema_id: "podway.procedure/v1".to_owned(),
        procedure_id: value_string_v1(object, "procedure_id")?.to_owned(),
        procedure_version: value_string_v1(object, "procedure_version")?.to_owned(),
        name: value_string_v1(object, "name")?.to_owned(),
        source_label,
        canonical_json,
        digest: value_typed_v1(object, "digest")?,
        created_at: UnixMillis::new(value_u64_v1(object, "created_at")?),
    })
    .map_err(|_| invalid_execution_v1("snapshot content identity is invalid"))
}

fn decode_execution_document_v1(
    source: &str,
) -> Result<
    (
        WorktreeSelectorWireV1,
        WorkspaceId,
        SliceCommandV1,
        AdmissionResolutionV1,
    ),
    ExecutionErrorV1,
> {
    let root =
        serde_json::from_str::<Value>(source).map_err(|_| invalid_execution_v1("not JSON"))?;
    let object = root
        .as_object()
        .ok_or_else(|| invalid_execution_v1("root is not an object"))?;
    let version = value_u64_v1(object, "execution_version")?;
    let resolution = match version {
        version
            if version == u64::from(EXECUTION_DOCUMENT_VERSION_V1)
                || version == u64::from(EXECUTION_DOCUMENT_VERSION_V2) =>
        {
            require_exact_keys_v1(
                object,
                &[
                    "command",
                    "execution_version",
                    "payload",
                    "preconditions",
                    "selector",
                    "workspace_id",
                ],
            )?;
            return Err(invalid_execution_v1(
                "legacy execution document lacks immutable admission resolution",
            ));
        }
        version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V3) => {
            require_exact_keys_v1(
                object,
                &[
                    "command",
                    "execution",
                    "execution_version",
                    "payload",
                    "preconditions",
                    "selector",
                    "workspace_id",
                ],
            )?;
            AdmissionResolutionV1::None
        }
        _ => return Err(invalid_execution_v1("unsupported execution version")),
    };
    let selector =
        serde_json::from_value::<WorktreeSelectorWireV1>(value_v1(object, "selector")?.clone())
            .map_err(|_| invalid_execution_v1("selector is invalid"))?;
    let workspace_id =
        serde_json::from_value::<WorkspaceId>(value_v1(object, "workspace_id")?.clone())
            .map_err(|_| invalid_execution_v1("workspace ID is invalid"))?;
    let command = value_string_v1(object, "command")?;
    let preconditions = value_object_v1(object, "preconditions")?;
    let payload = value_object_v1(object, "payload")?;
    let command = decode_command_components_v1(command, preconditions, payload)?;
    let resolution = match resolution {
        AdmissionResolutionV1::None => {
            decode_admission_resolution_v1(&command, value_object_v1(object, "execution")?)?
        }
        _ => unreachable!("only the v3 placeholder resolution is constructed here"),
    };
    Ok((selector, workspace_id, command, resolution))
}

fn decode_command_components_v1(
    command: &str,
    preconditions: &Map<String, Value>,
    payload: &Map<String, Value>,
) -> Result<SliceCommandV1, ExecutionErrorV1> {
    match command {
        "workspace.init" => {
            require_exact_keys_v1(preconditions, &[])?;
            require_exact_keys_v1(payload, &[])?;
            Ok(SliceCommandV1::WorkspaceInit)
        }
        "preset.start" => {
            require_exact_keys_v1(preconditions, &[])?;
            require_exact_keys_v1(payload, &["preset", "task_title"])?;
            Ok(SliceCommandV1::PresetStart(
                podway_protocol::PresetStartV1 {
                    preset: value_string_v1(payload, "preset")?.to_owned(),
                    task_title: value_string_v1(payload, "task_title")?.to_owned(),
                },
            ))
        }
        "item.check" => {
            require_exact_keys_v1(payload, &["item_id"])?;
            Ok(SliceCommandV1::ItemCheck(ItemCheckV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        "item.set" => {
            require_exact_keys_v1(payload, &["item_id", "value"])?;
            Ok(SliceCommandV1::ItemSet(ItemSetV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                value: value_string_v1(payload, "value")?.to_owned(),
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        "item.add" => {
            require_exact_keys_v1(payload, &["item_id", "value"])?;
            Ok(SliceCommandV1::ItemAdd(ItemAddV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                value: value_string_v1(payload, "value")?.to_owned(),
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        "item.attach_path" => {
            require_exact_keys_v1(payload, &["item_id", "media_type", "path"])?;
            Ok(SliceCommandV1::ItemAttachPath(ItemAttachPathV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                path: value_string_v1(payload, "path")?.to_owned(),
                media_type: value_optional_string_v1(payload, "media_type")?,
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        "session.block" => Ok(SliceCommandV1::SessionBlock(SessionBlockV1 {
            reason: required_payload_string_v1(payload, &["reason"], "reason")?,
            preconditions: decode_session_preconditions_v1(preconditions)?,
        })),
        "session.unblock" => {
            require_exact_keys_v1(payload, &["all", "blocker_id"])?;
            Ok(SliceCommandV1::SessionUnblock(SessionUnblockV1 {
                blocker_id: value_optional_typed_v1(payload, "blocker_id")?,
                all: value_bool_v1(payload, "all")?,
                preconditions: decode_session_preconditions_v1(preconditions)?,
            }))
        }
        "session.retry" => Ok(SliceCommandV1::SessionRetry(SessionRetryV1 {
            reason: required_payload_string_v1(payload, &["reason"], "reason")?,
            preconditions: decode_session_preconditions_v1(preconditions)?,
        })),
        "session.return" => {
            require_exact_keys_v1(payload, &["destination_stage_id", "reason"])?;
            Ok(SliceCommandV1::SessionReturn(SessionReturnV1 {
                destination_stage_id: value_typed_v1(payload, "destination_stage_id")?,
                reason: value_string_v1(payload, "reason")?.to_owned(),
                preconditions: decode_session_preconditions_v1(preconditions)?,
            }))
        }
        "session.complete" => {
            require_exact_keys_v1(payload, &[])?;
            Ok(SliceCommandV1::SessionComplete(SessionCompleteV1 {
                preconditions: decode_session_preconditions_v1(preconditions)?,
            }))
        }
        _ => Err(invalid_execution_v1("command is outside the admitted set")),
    }
}

fn decode_item_preconditions_v1(
    object: &Map<String, Value>,
) -> Result<podway_protocol::ItemMutationPreconditionsWireV1, ExecutionErrorV1> {
    require_exact_keys_v1(object, &["attempt_id", "item_revision"])?;
    Ok(podway_protocol::ItemMutationPreconditionsWireV1 {
        expected_attempt_id: value_typed_v1(object, "attempt_id")?,
        expected_item_revision: value_typed_v1(object, "item_revision")?,
    })
}

fn decode_session_preconditions_v1(
    object: &Map<String, Value>,
) -> Result<podway_protocol::SessionMutationPreconditionsWireV1, ExecutionErrorV1> {
    require_exact_keys_v1(object, &["attempt_id", "session_revision"])?;
    Ok(podway_protocol::SessionMutationPreconditionsWireV1 {
        expected_attempt_id: value_typed_v1(object, "attempt_id")?,
        expected_session_revision: value_typed_v1(object, "session_revision")?,
    })
}

fn required_payload_string_v1(
    payload: &Map<String, Value>,
    keys: &[&str],
    field: &'static str,
) -> Result<String, ExecutionErrorV1> {
    require_exact_keys_v1(payload, keys)?;
    Ok(value_string_v1(payload, field)?.to_owned())
}

fn require_exact_keys_v1(
    object: &Map<String, Value>,
    expected: &[&str],
) -> Result<(), ExecutionErrorV1> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid_execution_v1(
            "object fields do not match the execution schema",
        ));
    }
    Ok(())
}

fn value_v1<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Value, ExecutionErrorV1> {
    object
        .get(field)
        .ok_or_else(|| invalid_execution_v1("required field is absent"))
}

fn value_object_v1<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Map<String, Value>, ExecutionErrorV1> {
    value_v1(object, field)?
        .as_object()
        .ok_or_else(|| invalid_execution_v1("field is not an object"))
}

fn value_string_v1<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, ExecutionErrorV1> {
    value_v1(object, field)?
        .as_str()
        .ok_or_else(|| invalid_execution_v1("field is not a string"))
}

fn value_u64_v1(object: &Map<String, Value>, field: &'static str) -> Result<u64, ExecutionErrorV1> {
    value_v1(object, field)?
        .as_u64()
        .ok_or_else(|| invalid_execution_v1("field is not an unsigned integer"))
}

fn value_bool_v1(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<bool, ExecutionErrorV1> {
    value_v1(object, field)?
        .as_bool()
        .ok_or_else(|| invalid_execution_v1("field is not a boolean"))
}

fn value_typed_v1<T: serde::de::DeserializeOwned>(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<T, ExecutionErrorV1> {
    serde_json::from_value(value_v1(object, field)?.clone())
        .map_err(|_| invalid_execution_v1("field has an invalid typed value"))
}

fn value_optional_typed_v1<T: serde::de::DeserializeOwned>(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<T>, ExecutionErrorV1> {
    let value = value_v1(object, field)?;
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|_| invalid_execution_v1("optional field has an invalid typed value"))
}

fn value_optional_string_v1(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, ExecutionErrorV1> {
    value_optional_typed_v1(object, field)
}

fn invalid_execution_v1(reason: &'static str) -> ExecutionErrorV1 {
    ExecutionErrorV1::InvalidPersistedExecution { reason }
}

/// Derives the Store request digest with the workspace's audited SHA-256 implementation.
fn sha256_hex_v1(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut rendered = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use fmt::Write as _;
        write!(&mut rendered, "{byte:02x}").expect("writing to a String cannot fail");
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::sha256_hex_v1;

    #[test]
    fn sha256_matches_the_standard_empty_and_short_vectors() {
        assert_eq!(
            sha256_hex_v1(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex_v1(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
