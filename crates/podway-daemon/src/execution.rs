//! Daemon-owned execution of durably admitted G006 mutations.
//!
//! This module deliberately accepts only durable protocol commands at admission. New admissions store
//! a canonical, self-contained execution document. Legacy v1/v2 documents fail closed because they
//! lack immutable admission resolutions. Query commands remain outside of this execution path.

use std::{error::Error, fmt};

use crate::{
    observability::{EventOperationV1, EventOutcomeV1, EventRecordV1, ObservabilityEmitterV1},
    workspace::ResetMarkerV1,
};
use podway_config::{
    ProcedureFormatV1, ProcedureSourceLabel, ProcedureWarningPolicyV1, parse_procedure_v1,
};
use podway_core::{
    AddItemV1, ArtifactLocationKindV1, ArtifactValueV1, AttachItemV1, AttemptId, AttemptV1,
    BlockSessionV1, BlockerId, BlockerState, CancelSessionV1, CanonicalProcedureJsonV1,
    CanonicalProcedureSnapshotInputV1, CheckItemV1, ClearItemV1, CommandContextV1,
    CompleteSessionV1, DomainCommand, DomainError, DomainResult, ItemId,
    ItemMutationPreconditionsV1, ItemTypeV1, ItemValueV1, JobId, LocalArtifactVerificationV1,
    ProcedureSnapshotId, ProcedureSnapshotV1, ProcedureSourceLabelV1, RemoveItemV1,
    ReopenSessionV1, ResetAllWorkspaceV1, ResetSessionV1, RetrySessionV1, ReturnSessionV1,
    Revision, SessionAggregateV1, SessionCommandV1, SessionId, SetItemV1, Sha256Digest,
    SkipSessionV1, StageSpecV1, StartReplaceSessionV1, StartSessionV1, UnblockSessionV1,
    UncheckItemV1, UnixMillis, WorkspaceId, apply_transition_v1, canonicalize_json_v1,
    required_items_satisfied,
};
use podway_presets::lookup as lookup_embedded_preset_v1;
use podway_protocol::{
    ItemAddV1, ItemAttachSourceV1, ItemAttachV1, ItemCheckV1, ItemClearV1, ItemRemoveV1, ItemSetV1,
    ItemUncheckV1, SessionBlockV1, SessionCancelV1, SessionCompleteV1, SessionReopenV1,
    SessionResetV1, SessionRetryV1, SessionReturnV1, SessionSkipV1, SessionStartSourceV1,
    SessionStartV1, SessionUnblockV1, SliceCommandV1, SliceRequestV1, WorktreeSelectorWireV1,
    canonical_reset_all_identity_v1,
};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1, CanonicalExecutionJsonV1,
    ClaimedJobV1, DurableWorktreeIdentityV1, IdempotencyKeyV1, PersistedSessionMutationV1,
    RevisionAttemptItemPreconditionsV1, StateTransitionV1, StoreContractV1, StoreErrorV1,
    StoreIdempotencyReadContractV1, StoreValueErrorV1, TerminalReceiptV1, TerminalResultV1,
    WorkerIdV1, WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const EXECUTION_DOCUMENT_VERSION_V1: u8 = 1;
const EXECUTION_DOCUMENT_VERSION_V2: u8 = 2;
const EXECUTION_DOCUMENT_VERSION_V3: u8 = 3;
const EXECUTION_DOCUMENT_VERSION_V4: u8 = 4;
const EXECUTION_DOCUMENT_VERSION_V5: u8 = 5;

#[derive(Clone, Debug)]
enum AdmissionResolutionV1 {
    None,
    SessionStart {
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
    SessionSkip {
        next_attempt_id: AttemptId,
    },
    SessionReopen {
        destination_attempt_id: AttemptId,
    },
}

/// A boundary outcome that can either become a durable domain failure or leave the claim for
/// recovery. A transient outcome never causes the engine to commit a terminal result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionBoundaryErrorV1 {
    Domain(DomainError),
    WorkspaceIdentityMismatch {
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    Transient {
        operation: &'static str,
    },
}

impl ExecutionBoundaryErrorV1 {
    pub const fn domain(error: DomainError) -> Self {
        Self::Domain(error)
    }

    pub const fn transient(operation: &'static str) -> Self {
        Self::Transient { operation }
    }

    pub fn workspace_identity_mismatch(expected: WorkspaceId, actual: WorkspaceId) -> Self {
        Self::WorkspaceIdentityMismatch { expected, actual }
    }
}

/// The sole source of generated domain IDs for an execution worker.
pub trait ExecutionIdSourceV1: Send + Sync {
    fn next_job_id(&self) -> JobId;
    /// Generates a fresh workspace UUID for destructive reset replacement. Existing ID sources
    /// remain compatible by deriving the UUID from their next UUID-shaped job ID.
    fn next_workspace_id(&self) -> WorkspaceId {
        let generated = self.next_job_id();
        WorkspaceId::new(generated.as_str())
            .expect("execution job ID must satisfy the workspace UUID contract")
    }
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
    /// Reads and validates a relative procedure source under an already revalidated worktree.
    /// The returned snapshot is persisted at admission and never reconstructed during replay.
    fn load_workspace_procedure_snapshot(
        &self,
        workspace: &WorkspaceBindingV1,
        procedure: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1>;
}

/// The production provider loads preset and worktree-relative procedure sources through their public
/// config admission paths, returning the immutable snapshot retained in the execution document.
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

    fn load_workspace_procedure_snapshot(
        &self,
        _workspace: &WorkspaceBindingV1,
        _procedure: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        Err(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "workspace procedure sources require native workspace authority",
            },
        ))
    }
}

pub(crate) fn workspace_procedure_snapshot_from_bytes_v1(
    procedure: &str,
    source: &[u8],
    snapshot_id: ProcedureSnapshotId,
    created_at: UnixMillis,
) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
    let source_label = ProcedureSourceLabel::workspace_path(procedure).map_err(|_| {
        ExecutionBoundaryErrorV1::domain(DomainError::InvalidState {
            reason: "workspace procedure path is invalid",
        })
    })?;
    let format = match std::path::Path::new(procedure)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("json") => ProcedureFormatV1::Json,
        Some("yaml" | "yml") => ProcedureFormatV1::Yaml,
        _ => Err(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "workspace procedure source has an unsupported extension",
            },
        ))?,
    };
    parse_procedure_v1(source, format)
        .and_then(|procedure| {
            procedure.into_snapshot_v1(
                snapshot_id,
                source_label,
                created_at,
                ProcedureWarningPolicyV1::Accept,
            )
        })
        .map_err(|_| {
            ExecutionBoundaryErrorV1::domain(DomainError::InvalidState {
                reason: "workspace procedure admission failed",
            })
        })
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
    BoundaryTransient {
        operation: &'static str,
    },
    SessionIdentityMismatch {
        expected: SessionId,
        actual: Option<SessionId>,
    },
    WorkspaceIdentityMismatch {
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    ProcedureDigestMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    InvalidPersistedExecution {
        reason: &'static str,
    },
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
            Self::SessionIdentityMismatch { .. } => {
                formatter.write_str("execution target session identity does not match")
            }
            Self::WorkspaceIdentityMismatch { .. } => {
                formatter.write_str("execution target workspace identity does not match")
            }
            Self::ProcedureDigestMismatch { .. } => {
                formatter.write_str("canonical Procedure digest does not match the expectation")
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
            Self::BoundaryTransient { .. }
            | Self::SessionIdentityMismatch { .. }
            | Self::WorkspaceIdentityMismatch { .. }
            | Self::ProcedureDigestMismatch { .. }
            | Self::InvalidPersistedExecution { .. } => None,
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
/// Immutable inputs for publishing a new reset marker after the old Store has been retired.
/// Opaque capability produced by Store-first reset preparation. It binds the exact source identity
/// inspected during preparation, so it cannot be replayed against another worktree generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedWorkspaceResetAllV1 {
    marker: ResetMarkerV1,
    previous_workspace_uuid: WorkspaceId,
    source: DurableWorktreeIdentityV1,
}

impl PreparedWorkspaceResetAllV1 {
    pub fn marker(&self) -> &ResetMarkerV1 {
        &self.marker
    }

    pub fn previous_workspace_uuid(&self) -> &WorkspaceId {
        &self.previous_workspace_uuid
    }

    pub(crate) fn matches_source(&self, source: &DurableWorktreeIdentityV1) -> bool {
        &self.source == source
    }
}

/// Store-first reset-all preparation. Replays retain the Store's exact immutable outcome; new
/// requests carry only the marker publication inputs and never admit into the old Store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResetAllPreparationOutcomeV1 {
    Existing(AdmitOutcomeV1),
    New(PreparedWorkspaceResetAllV1),
}

/// Observational result of inspecting the exact reset-source database. It carries no authority to
/// bypass Store-first replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResetStoreInspectionV1 {
    Readable,
    Absent,
    Unreadable(StoreErrorV1),
}

/// Manager-issued proof that a particular inspected source has no readable Store. Its fields and
/// constructors are crate-private: external callers can observe an inspection but cannot turn one
/// into reset authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidatedUnavailableStoreV1 {
    source: DurableWorktreeIdentityV1,
    inspection: ResetStoreInspectionV1,
}

impl ValidatedUnavailableStoreV1 {
    pub(crate) fn absent(source: DurableWorktreeIdentityV1) -> Self {
        Self {
            source,
            inspection: ResetStoreInspectionV1::Absent,
        }
    }

    pub(crate) fn unreadable(source: DurableWorktreeIdentityV1, error: StoreErrorV1) -> Self {
        Self {
            source,
            inspection: ResetStoreInspectionV1::Unreadable(error),
        }
    }

    fn matches_source(&self, source: &DurableWorktreeIdentityV1) -> bool {
        &self.source == source
            && matches!(
                &self.inspection,
                ResetStoreInspectionV1::Absent | ResetStoreInspectionV1::Unreadable(_)
            )
    }
}

/// Typed internal reset Store authority. The public preparation entry point always uses the engine's
/// own Store; only the runtime manager can issue the unavailable variant.
pub(crate) struct ResetAllStoreAuthorityV1<'store, Store> {
    kind: ResetAllStoreAuthorityKindV1<'store, Store>,
}

enum ResetAllStoreAuthorityKindV1<'store, Store> {
    Readable(&'store Store),
    ValidatedUnavailable(ValidatedUnavailableStoreV1),
}

impl<'store, Store> ResetAllStoreAuthorityV1<'store, Store> {
    fn readable(store: &'store Store) -> Self {
        Self {
            kind: ResetAllStoreAuthorityKindV1::Readable(store),
        }
    }

    pub(crate) fn validated_unavailable(proof: ValidatedUnavailableStoreV1) -> Self {
        Self {
            kind: ResetAllStoreAuthorityKindV1::ValidatedUnavailable(proof),
        }
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
    observability: Option<ObservabilityEmitterV1>,
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
        Self::with_observability(store, ids, clock, procedures, artifacts, workspaces, None)
    }

    /// Adds an optional non-authoritative typed event producer.
    pub fn with_observability(
        store: Store,
        ids: Ids,
        clock: Clock,
        procedures: Procedures,
        artifacts: Artifacts,
        workspaces: Workspaces,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Self {
        Self {
            store,
            ids,
            clock,
            procedures,
            artifacts,
            workspaces,
            observability,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
    /// Prepares a destructive reset without admitting it to this engine's old-generation Store.
    /// Store-first replay is mandatory on this public path.
    pub fn prepare_workspace_reset_all(
        &self,
        request: &SliceRequestV1,
        previous_workspace: &DurableWorktreeIdentityV1,
        idempotency_key: IdempotencyKeyV1,
    ) -> Result<ResetAllPreparationOutcomeV1, ExecutionErrorV1> {
        self.prepare_workspace_reset_all_with_authority(
            request,
            previous_workspace,
            idempotency_key,
            ResetAllStoreAuthorityV1::readable(&self.store),
        )
    }

    /// The manager-only recovery path permits no-Store preparation only with an unavailable proof
    /// issued for the exact source identity passed to this engine.
    pub(crate) fn prepare_workspace_reset_all_with_unavailable_store(
        &self,
        request: &SliceRequestV1,
        previous_workspace: &DurableWorktreeIdentityV1,
        idempotency_key: IdempotencyKeyV1,
        proof: ValidatedUnavailableStoreV1,
    ) -> Result<ResetAllPreparationOutcomeV1, ExecutionErrorV1> {
        self.prepare_workspace_reset_all_with_authority(
            request,
            previous_workspace,
            idempotency_key,
            ResetAllStoreAuthorityV1::validated_unavailable(proof),
        )
    }

    fn prepare_workspace_reset_all_with_authority(
        &self,
        request: &SliceRequestV1,
        previous_workspace: &DurableWorktreeIdentityV1,
        idempotency_key: IdempotencyKeyV1,
        store_authority: ResetAllStoreAuthorityV1<'_, Store>,
    ) -> Result<ResetAllPreparationOutcomeV1, ExecutionErrorV1> {
        let SliceCommandV1::WorkspaceResetAll(reset) = request.command() else {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "non-reset command reached reset preparation",
            });
        };
        let request_digest = reset_all_request_digest_v1(request, previous_workspace)?;
        match store_authority.kind {
            ResetAllStoreAuthorityKindV1::Readable(store) => {
                if let Some(outcome) = store.read_idempotent_outcome(
                    previous_workspace,
                    &idempotency_key,
                    &request_digest,
                )? {
                    self.emit(
                        EventOperationV1::IdempotentReplay,
                        EventOutcomeV1::Succeeded,
                    );
                    return Ok(ResetAllPreparationOutcomeV1::Existing(outcome));
                }
                let expected_workspace_uuid =
                    reset.preconditions.expected_workspace_id.as_ref().ok_or(
                        ExecutionErrorV1::BoundaryDomain(DomainError::InvalidState {
                            reason:
                                "workspace reset requires the previous workspace UUID when the Store is readable",
                        }),
                )?;
                if expected_workspace_uuid != previous_workspace.workspace_uuid() {
                    return Err(ExecutionErrorV1::WorkspaceIdentityMismatch {
                        expected: expected_workspace_uuid.clone(),
                        actual: previous_workspace.workspace_uuid().clone(),
                    });
                }
            }
            ResetAllStoreAuthorityKindV1::ValidatedUnavailable(proof) => {
                if !proof.matches_source(previous_workspace) {
                    return Err(ExecutionErrorV1::InvalidPersistedExecution {
                        reason: "unavailable Store proof does not match the inspected reset source",
                    });
                }
                if let Some(expected) = reset.preconditions.expected_workspace_id.as_ref()
                    && expected != previous_workspace.workspace_uuid()
                {
                    return Err(ExecutionErrorV1::WorkspaceIdentityMismatch {
                        expected: expected.clone(),
                        actual: previous_workspace.workspace_uuid().clone(),
                    });
                }
            }
        }
        if !reset.confirmed {
            return Err(ExecutionErrorV1::BoundaryDomain(
                DomainError::InvalidState {
                    reason: "workspace reset requires explicit confirmation",
                },
            ));
        }
        let operation_id = self.ids.next_job_id();
        let target_workspace_uuid = self.ids.next_workspace_id();
        let marker = ResetMarkerV1::new(
            operation_id,
            idempotency_key,
            request_digest,
            previous_workspace.workspace_uuid().clone(),
            target_workspace_uuid,
            self.clock.now(),
        );
        Ok(ResetAllPreparationOutcomeV1::New(
            PreparedWorkspaceResetAllV1 {
                marker,
                previous_workspace_uuid: previous_workspace.workspace_uuid().clone(),
                source: previous_workspace.clone(),
            },
        ))
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
        let result = self.admit_with_expected_workspace(None, request, idempotency_key);
        self.emit_admission_result(&result);
        result
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
        let result =
            self.admit_with_expected_workspace(Some(expected_workspace), request, idempotency_key);
        self.emit_admission_result(&result);
        result
    }

    fn admit_with_expected_workspace(
        &self,
        expected_workspace: Option<&WorkspaceBindingV1>,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1> {
        if matches!(request.command(), SliceCommandV1::WorkspaceResetAll(_)) {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "workspace reset must be prepared through the maintenance path",
            });
        }
        let is_start = matches!(
            request.command(),
            SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_)
        );
        let expected_procedure_digest = expected_start_procedure_digest_v1(request.command());
        if let Some(expected_workspace) = expected_workspace
            && let Some(outcome) = self.read_admission_idempotent_outcome(
                expected_workspace.identity(),
                request,
                &idempotency_key,
                expected_procedure_digest,
            )?
        {
            return Ok(outcome);
        }
        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)?;
        if expected_workspace.is_none()
            && let Some(outcome) = self.read_admission_idempotent_outcome(
                binding.identity(),
                request,
                &idempotency_key,
                expected_procedure_digest,
            )?
        {
            return Ok(outcome);
        }
        if let Some(expected) = expected_workspace
            && expected.identity() != binding.identity()
        {
            if expected.identity().workspace_uuid() != binding.identity().workspace_uuid() {
                return Err(ExecutionErrorV1::WorkspaceIdentityMismatch {
                    expected: expected.identity().workspace_uuid().clone(),
                    actual: binding.identity().workspace_uuid().clone(),
                });
            }
            return Err(ExecutionErrorV1::BoundaryDomain(
                DomainError::InvalidState {
                    reason: "revalidated workspace does not match the scheduler identity",
                },
            ));
        }
        if !is_start {
            self.enforce_admission_session_identity(request.command(), &binding)?;
        }
        let command = command_for_admission_v1(request.command())?;
        let preconditions = store_preconditions_v1(request.command())?;
        let now = self.clock.now();
        let resolution = self.admission_resolution(request.command(), &binding, now)?;
        let canonical_execution =
            canonical_execution_document_v1(request, binding.identity(), &resolution)?;
        let procedure_digest = match &resolution {
            AdmissionResolutionV1::SessionStart { snapshot, .. } => Some(snapshot.digest()),
            _ => None,
        };
        let request_digest = request_digest_v1(
            request,
            binding.identity().workspace_uuid(),
            procedure_digest,
        )?;
        if is_start {
            if let Some(outcome) = self.store.read_idempotent_outcome(
                binding.identity(),
                &idempotency_key,
                &request_digest,
            )? {
                return Ok(outcome);
            }
            self.enforce_admission_session_identity(request.command(), &binding)?;
        }
        let admitted = AdmitRequestV1::new_with_canonical_execution(
            command,
            idempotency_key,
            self.ids.next_job_id(),
            preconditions,
            request_digest,
            now,
            canonical_execution,
        )
        .with_session_identity(admission_session_identity_v1(request.command()));
        let admitted = match &resolution {
            AdmissionResolutionV1::SessionStart { snapshot, .. } => {
                admitted.with_admitted_procedure_snapshot(snapshot.as_ref().clone())
            }
            _ => admitted,
        };
        let outcome = self
            .store
            .admit(binding.identity(), admitted)
            .map_err(ExecutionErrorV1::from)?;
        Ok(outcome)
    }

    fn read_admission_idempotent_outcome(
        &self,
        identity: &DurableWorktreeIdentityV1,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
        expected_procedure_digest: Option<&Sha256Digest>,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        if matches!(
            request.command(),
            SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_)
        ) && expected_procedure_digest.is_none()
        {
            return self.read_unresolved_start_idempotent_outcome(
                identity,
                request,
                idempotency_key,
            );
        }
        let request_digest = request_digest_v1(
            request,
            identity.workspace_uuid(),
            expected_procedure_digest,
        )?;
        self.store
            .read_idempotent_outcome(identity, idempotency_key, &request_digest)
            .map_err(ExecutionErrorV1::from)
    }

    fn read_unresolved_start_idempotent_outcome(
        &self,
        identity: &DurableWorktreeIdentityV1,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        let Some(existing) = self
            .store
            .read_idempotent_execution(identity, idempotency_key)?
        else {
            return Ok(None);
        };
        let (execution_version, procedure_digest) =
            if let Some(canonical_execution) = existing.canonical_execution() {
                let (
                    execution_version,
                    _selector,
                    persisted_workspace_id,
                    persisted_command,
                    resolution,
                ) = decode_execution_document_v1(canonical_execution.as_str())?;
                if persisted_workspace_id != *identity.workspace_uuid() {
                    return Err(invalid_execution_v1(
                        "idempotent execution workspace identity is invalid",
                    ));
                }
                if !matches!(
                    persisted_command,
                    SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_)
                ) {
                    return Ok(None);
                }
                let AdmissionResolutionV1::SessionStart { snapshot, .. } = resolution else {
                    return Err(invalid_execution_v1(
                        "idempotent start has no admitted Procedure snapshot",
                    ));
                };
                (execution_version, snapshot.digest().clone())
            } else if let Some(start_identity) = existing.retained_start_identity() {
                if !matches!(
                    request.command(),
                    SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_)
                ) {
                    return Ok(None);
                }
                (
                    u64::from(start_identity.execution_version()),
                    start_identity.procedure_digest().clone(),
                )
            } else {
                return Ok(None);
            };
        let actual = if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V4) {
            request_digest_v1(request, identity.workspace_uuid(), None)?
        } else {
            request_digest_v1(request, identity.workspace_uuid(), Some(&procedure_digest))?
        };
        if existing.request_digest() != &actual {
            return Err(ExecutionErrorV1::Store(
                StoreErrorV1::IdempotencyDigestConflictV1 {
                    expected: existing.request_digest().clone(),
                    actual,
                },
            ));
        }
        Ok(Some(existing.outcome().clone()))
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
        let claimed = match self.store.claim_next(workspace.identity(), worker, now) {
            Ok(Some(claimed)) => {
                self.emit(EventOperationV1::JobClaim, EventOutcomeV1::Succeeded);
                claimed
            }
            Ok(None) => {
                self.emit(EventOperationV1::JobClaim, EventOutcomeV1::Rejected);
                return Ok(None);
            }
            Err(error) => {
                self.emit(EventOperationV1::JobClaim, EventOutcomeV1::Failed);
                return Err(error.into());
            }
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
        let (_execution_version, _selector, workspace_id, command, resolution) =
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

        if matches!(command, SliceCommandV1::WorkspaceInit(_)) {
            return self.commit_workspace_initialization(
                &claimed,
                expected_workspace_revision,
                now,
            );
        }

        if let Err(error) = enforce_session_identity_v1(&command, claimed.current_session()) {
            return self.commit_domain_failure(&claimed, expected_workspace_revision, error, now);
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
            Err(BoundaryDispositionV1::WorkspaceIdentityMismatch { expected, actual }) => {
                return Err(ExecutionErrorV1::WorkspaceIdentityMismatch { expected, actual });
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
        let transition =
            match state_transition_v1(&admitted_command, claimed.current_session(), &outcome) {
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
            claimed.current_session(),
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
        let receipt = self
            .store
            .commit_terminal(
                claimed.claim().clone(),
                expected_workspace_revision,
                Some(transition),
                TerminalResultV1::Success(result),
                now,
            )
            .map_err(ExecutionErrorV1::from)?;
        self.emit(EventOperationV1::JobTerminal, EventOutcomeV1::Succeeded);
        Ok(receipt)
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
        let receipt = self
            .store
            .commit_terminal(
                claimed.claim().clone(),
                Revision::ZERO,
                Some(transition),
                TerminalResultV1::Success(result),
                now,
            )
            .map_err(ExecutionErrorV1::from)?;
        self.emit(EventOperationV1::JobTerminal, EventOutcomeV1::Succeeded);
        Ok(receipt)
    }

    fn commit_domain_failure(
        &self,
        claimed: &ClaimedJobV1,
        expected_workspace_revision: Revision,
        error: DomainError,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        let receipt = self
            .store
            .commit_terminal(
                claimed.claim().clone(),
                expected_workspace_revision,
                None,
                TerminalResultV1::Failure(error),
                now,
            )
            .map_err(ExecutionErrorV1::from)?;
        self.emit(EventOperationV1::JobTerminal, EventOutcomeV1::Failed);
        Ok(receipt)
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
            return Err(BoundaryDispositionV1::WorkspaceIdentityMismatch {
                expected: selector.expected_uuid().cloned().expect("checked above"),
                actual: binding.identity().workspace_uuid().clone(),
            });
        }
        Ok(binding)
    }

    fn enforce_admission_session_identity(
        &self,
        command: &SliceCommandV1,
        workspace: &WorkspaceBindingV1,
    ) -> Result<(), ExecutionErrorV1> {
        if expected_session_id_v1(command).is_none() {
            return Ok(());
        }
        let view = self.store.read_workspace_view(workspace.identity())?;
        if view.identity() != workspace.identity()
            || view.state().workspace_id() != workspace.identity().workspace_uuid()
        {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "Store read returned a different workspace identity before admission",
            });
        }
        match enforce_session_identity_v1(command, view.current_session()) {
            Err(DomainError::SessionIdentityMismatch { expected, actual }) => {
                Err(ExecutionErrorV1::SessionIdentityMismatch { expected, actual })
            }
            Err(error) => Err(ExecutionErrorV1::BoundaryDomain(error)),
            Ok(()) => Ok(()),
        }
    }

    fn emit_admission_result(&self, result: &Result<AdmitOutcomeV1, ExecutionErrorV1>) {
        let (operation, outcome) = match result {
            Ok(AdmitOutcomeV1::New(_)) => {
                (EventOperationV1::JobAdmission, EventOutcomeV1::Succeeded)
            }
            Ok(AdmitOutcomeV1::Existing(_)) => (
                EventOperationV1::IdempotentReplay,
                EventOutcomeV1::Succeeded,
            ),
            Err(_) => (EventOperationV1::JobAdmission, EventOutcomeV1::Rejected),
        };
        self.emit(operation, outcome);
    }

    fn emit(&self, operation: EventOperationV1, outcome: EventOutcomeV1) {
        if let Some(observability) = &self.observability {
            observability.emit(EventRecordV1::new(operation, outcome));
        }
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
        workspace: &WorkspaceBindingV1,
        now: UnixMillis,
    ) -> Result<AdmissionResolutionV1, ExecutionErrorV1> {
        match command {
            SliceCommandV1::SessionStart(input) => {
                self.resolve_session_start(input, workspace, now)
            }
            SliceCommandV1::SessionStartReplace(input) => {
                self.resolve_session_start(&input.start, workspace, now)
            }
            SliceCommandV1::SessionBlock(_) => Ok(AdmissionResolutionV1::SessionBlock {
                blocker_id: self.ids.next_blocker_id(),
            }),
            SliceCommandV1::SessionSkip(_) => Ok(AdmissionResolutionV1::SessionSkip {
                next_attempt_id: self.ids.next_attempt_id(),
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
            SliceCommandV1::SessionReopen(_) => Ok(AdmissionResolutionV1::SessionReopen {
                destination_attempt_id: self.ids.next_attempt_id(),
            }),
            SliceCommandV1::WorkspaceInit(_)
            | SliceCommandV1::WorkspaceResetAll(_)
            | SliceCommandV1::ItemCheck(_)
            | SliceCommandV1::ItemUncheck(_)
            | SliceCommandV1::ItemSet(_)
            | SliceCommandV1::ItemAdd(_)
            | SliceCommandV1::ItemRemove(_)
            | SliceCommandV1::ItemAttach(_)
            | SliceCommandV1::ItemClear(_)
            | SliceCommandV1::SessionUnblock(_)
            | SliceCommandV1::SessionCancel(_)
            | SliceCommandV1::SessionReset(_) => Ok(AdmissionResolutionV1::None),
            SliceCommandV1::WorkspaceDoctor(_)
            | SliceCommandV1::WorkspaceShow(_)
            | SliceCommandV1::WorkspaceRepair(_)
            | SliceCommandV1::SessionStatus(_)
            | SliceCommandV1::SessionNext(_)
            | SliceCommandV1::JobList(_)
            | SliceCommandV1::JobStatus(_)
            | SliceCommandV1::JobWait(_)
            | SliceCommandV1::JobCancel(_) => Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "non-durable command reached the mutation executor",
            }),
        }
    }

    fn resolve_session_start(
        &self,
        input: &SessionStartV1,
        workspace: &WorkspaceBindingV1,
        now: UnixMillis,
    ) -> Result<AdmissionResolutionV1, ExecutionErrorV1> {
        let snapshot_id = self.ids.next_procedure_snapshot_id();
        let snapshot = match &input.source {
            SessionStartSourceV1::Preset { preset } => {
                self.procedures
                    .load_preset_snapshot(preset, snapshot_id, now)
            }
            SessionStartSourceV1::Procedure { procedure } => self
                .procedures
                .load_workspace_procedure_snapshot(workspace, procedure, snapshot_id, now),
        }
        .map_err(|error| ExecutionErrorV1::from_boundary(error.into()))?;
        if let Some(expected) = input.expected_procedure_digest.as_ref()
            && snapshot.digest() != expected
        {
            return Err(ExecutionErrorV1::ProcedureDigestMismatch {
                expected: expected.clone(),
                actual: snapshot.digest().clone(),
            });
        }
        Ok(AdmissionResolutionV1::SessionStart {
            snapshot: Box::new(snapshot),
            session_id: self.ids.next_session_id(),
            first_attempt_id: self.ids.next_attempt_id(),
        })
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
            SliceCommandV1::SessionStart(input) => Ok(SessionCommandV1::Start(
                start_session_input_v1(input, resolution)?,
            )),
            SliceCommandV1::SessionStartReplace(input) => {
                Ok(SessionCommandV1::StartReplace(StartReplaceSessionV1 {
                    expected_session_id: input.preconditions.expected_session_id.clone(),
                    confirmed: input.confirmed,
                    start: start_session_input_v1(&input.start, resolution)?,
                }))
            }
            SliceCommandV1::ItemCheck(input) => Ok(SessionCommandV1::Check(CheckItemV1 {
                item_id: input.item_id.clone(),
                preconditions: core_item_preconditions_v1(&input.preconditions),
            })),
            SliceCommandV1::ItemUncheck(input) => Ok(SessionCommandV1::Uncheck(UncheckItemV1 {
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
            SliceCommandV1::ItemRemove(input) => Ok(SessionCommandV1::Remove(RemoveItemV1 {
                item_id: input.item_id.clone(),
                value: input.value.clone(),
                ignore_missing: input.ignore_missing,
                preconditions: core_item_preconditions_v1(&input.preconditions),
            })),
            SliceCommandV1::ItemAttach(input) => {
                let artifact = match &input.source {
                    ItemAttachSourceV1::Path { path, media_type } => {
                        let artifact = self
                            .artifacts
                            .hash_local_artifact(workspace, path, media_type.as_deref())
                            .map_err(BoundaryDispositionV1::from)?;
                        validate_local_attached_artifact_v1(path, media_type.as_deref(), &artifact)
                            .map_err(BoundaryDispositionV1::Domain)?;
                        artifact
                    }
                    ItemAttachSourceV1::OpaqueReference {
                        reference,
                        digest,
                        size_bytes,
                        media_type,
                    } => ArtifactValueV1::external_reference(
                        reference,
                        digest.clone(),
                        *size_bytes,
                        media_type,
                    )
                    .map_err(BoundaryDispositionV1::Domain)?,
                };
                Ok(SessionCommandV1::Attach(AttachItemV1 {
                    item_id: input.item_id.clone(),
                    value: artifact,
                    preconditions: core_item_preconditions_v1(&input.preconditions),
                }))
            }
            SliceCommandV1::ItemClear(input) => Ok(SessionCommandV1::Clear(ClearItemV1 {
                item_id: input.item_id.clone(),
                preconditions: core_item_preconditions_v1(&input.preconditions),
            })),
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
            SliceCommandV1::SessionSkip(input) => {
                let (stage, _) =
                    active_stage_attempt_v1(prior).map_err(BoundaryDispositionV1::Domain)?;
                let next_attempt_id = if is_final_stage_v1(prior, stage)
                    .map_err(BoundaryDispositionV1::Domain)?
                {
                    None
                } else {
                    Some(match resolution {
                        AdmissionResolutionV1::SessionSkip { next_attempt_id } => {
                            next_attempt_id.clone()
                        }
                        _ => {
                            return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                                reason: "persisted admission resolution does not match session skip",
                            }));
                        }
                    })
                };
                Ok(SessionCommandV1::Skip(SkipSessionV1 {
                    expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                    reason: input.reason.clone(),
                    next_attempt_id,
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
            SliceCommandV1::SessionCancel(input) => Ok(SessionCommandV1::Cancel(CancelSessionV1 {
                expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                reason: input.reason.clone(),
            })),
            SliceCommandV1::SessionReopen(input) => {
                let destination_attempt_id = match resolution {
                    AdmissionResolutionV1::SessionReopen {
                        destination_attempt_id,
                    } => destination_attempt_id.clone(),
                    _ => {
                        return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                            reason: "persisted admission resolution does not match session reopen",
                        }));
                    }
                };
                Ok(SessionCommandV1::Reopen(ReopenSessionV1 {
                    expected_session_id: input.preconditions.expected_session_id.clone(),
                    destination_stage_id: input.destination_stage_id.clone(),
                    reason: input.reason.clone(),
                    destination_attempt_id,
                }))
            }
            SliceCommandV1::SessionReset(input) => Ok(SessionCommandV1::Reset(ResetSessionV1 {
                expected_session_id: input.preconditions.expected_session_id.clone(),
                confirmed: input.confirmed,
            })),
            SliceCommandV1::WorkspaceResetAll(input) => {
                if input
                    .preconditions
                    .expected_workspace_id
                    .as_ref()
                    .is_some_and(|expected| expected != workspace.identity().workspace_uuid())
                {
                    return Err(BoundaryDispositionV1::Domain(DomainError::InvalidState {
                        reason: "workspace reset precondition does not match the claim",
                    }));
                }
                Ok(SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
                    workspace_id: input.preconditions.expected_workspace_id.clone(),
                    confirmed: input.confirmed,
                }))
            }
            SliceCommandV1::WorkspaceInit(_)
            | SliceCommandV1::WorkspaceDoctor(_)
            | SliceCommandV1::WorkspaceShow(_)
            | SliceCommandV1::WorkspaceRepair(_)
            | SliceCommandV1::SessionStatus(_)
            | SliceCommandV1::SessionNext(_)
            | SliceCommandV1::JobList(_)
            | SliceCommandV1::JobStatus(_)
            | SliceCommandV1::JobWait(_)
            | SliceCommandV1::JobCancel(_) => {
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
fn start_session_input_v1(
    input: &SessionStartV1,
    resolution: &AdmissionResolutionV1,
) -> Result<StartSessionV1, BoundaryDispositionV1> {
    let (snapshot, session_id, first_attempt_id) = match resolution {
        AdmissionResolutionV1::SessionStart {
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
                reason: "persisted admission resolution does not match session start",
            }));
        }
    };
    Ok(StartSessionV1 {
        task_title: input.task_title.clone(),
        snapshot,
        session_id,
        first_attempt_id,
    })
}

#[derive(Debug)]
enum BoundaryDispositionV1 {
    Domain(DomainError),
    WorkspaceIdentityMismatch {
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    Transient {
        operation: &'static str,
    },
}

impl From<ExecutionBoundaryErrorV1> for BoundaryDispositionV1 {
    fn from(error: ExecutionBoundaryErrorV1) -> Self {
        match error {
            ExecutionBoundaryErrorV1::Domain(error) => Self::Domain(error),
            ExecutionBoundaryErrorV1::WorkspaceIdentityMismatch { expected, actual } => {
                Self::WorkspaceIdentityMismatch { expected, actual }
            }
            ExecutionBoundaryErrorV1::Transient { operation } => Self::Transient { operation },
        }
    }
}

impl ExecutionErrorV1 {
    fn from_boundary(error: BoundaryDispositionV1) -> Self {
        match error {
            BoundaryDispositionV1::Domain(error) => Self::BoundaryDomain(error),
            BoundaryDispositionV1::WorkspaceIdentityMismatch { expected, actual } => {
                Self::WorkspaceIdentityMismatch { expected, actual }
            }
            BoundaryDispositionV1::Transient { operation } => Self::BoundaryTransient { operation },
        }
    }
}

fn command_for_admission_v1(command: &SliceCommandV1) -> Result<DomainCommand, ExecutionErrorV1> {
    let command = match command {
        SliceCommandV1::WorkspaceInit(_) => DomainCommand::WorkspaceInitialize,
        SliceCommandV1::WorkspaceResetAll(_) => {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "workspace reset must be prepared through the maintenance path",
            });
        }
        SliceCommandV1::SessionStart(_) => DomainCommand::SessionStart,
        SliceCommandV1::SessionStartReplace(_) => DomainCommand::SessionStartReplace,
        SliceCommandV1::SessionComplete(_) => DomainCommand::SessionComplete,
        SliceCommandV1::SessionSkip(_) => DomainCommand::SessionSkip,
        SliceCommandV1::SessionRetry(_) => DomainCommand::SessionRetry,
        SliceCommandV1::SessionReturn(_) => DomainCommand::SessionReturn,
        SliceCommandV1::SessionBlock(_) => DomainCommand::SessionBlock,
        SliceCommandV1::SessionUnblock(_) => DomainCommand::SessionUnblock,
        SliceCommandV1::SessionCancel(_) => DomainCommand::SessionCancel,
        SliceCommandV1::SessionReopen(_) => DomainCommand::SessionReopen,
        SliceCommandV1::SessionReset(_) => DomainCommand::SessionReset,
        SliceCommandV1::ItemCheck(input) => DomainCommand::ItemCheck {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemUncheck(input) => DomainCommand::ItemUncheck {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemSet(input) => DomainCommand::ItemSet {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemAdd(input) => DomainCommand::ItemAdd {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemRemove(input) => DomainCommand::ItemRemove {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemAttach(input) => DomainCommand::ItemAttach {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::ItemClear(input) => DomainCommand::ItemClear {
            item_id: input.item_id.clone(),
        },
        SliceCommandV1::WorkspaceDoctor(_)
        | SliceCommandV1::WorkspaceShow(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::SessionStatus(_)
        | SliceCommandV1::SessionNext(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "non-durable command reached the mutation executor",
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
        SliceCommandV1::ItemUncheck(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemSet(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemAdd(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemRemove(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemAttach(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::ItemClear(input) => {
            item_store_preconditions_v1(&input.item_id, &input.preconditions)
        }
        SliceCommandV1::SessionComplete(input) => {
            session_store_preconditions_v1(&input.preconditions)
        }
        SliceCommandV1::SessionSkip(input) => session_store_preconditions_v1(&input.preconditions),
        SliceCommandV1::SessionRetry(input) => session_store_preconditions_v1(&input.preconditions),
        SliceCommandV1::SessionReturn(input) => {
            session_store_preconditions_v1(&input.preconditions)
        }
        SliceCommandV1::SessionBlock(input) => session_store_preconditions_v1(&input.preconditions),
        SliceCommandV1::SessionUnblock(input) => {
            session_store_preconditions_v1(&input.preconditions)
        }
        SliceCommandV1::SessionCancel(input) => {
            session_store_preconditions_v1(&input.preconditions)
        }
        SliceCommandV1::SessionStartReplace(input) => {
            session_revision_store_preconditions_v1(input.preconditions.expected_session_revision)
        }
        SliceCommandV1::SessionReopen(input) => {
            session_revision_store_preconditions_v1(input.preconditions.expected_session_revision)
        }
        SliceCommandV1::SessionReset(input) => {
            session_revision_store_preconditions_v1(input.preconditions.expected_session_revision)
        }
        SliceCommandV1::WorkspaceInit(_)
        | SliceCommandV1::WorkspaceResetAll(_)
        | SliceCommandV1::SessionStart(_) => (None, None, None, None),
        SliceCommandV1::WorkspaceDoctor(_)
        | SliceCommandV1::WorkspaceShow(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::SessionStatus(_)
        | SliceCommandV1::SessionNext(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "non-durable command has no Store mutation preconditions",
            });
        }
    };
    RevisionAttemptItemPreconditionsV1::new(session_revision, attempt_id, item_id, item_revision)
        .map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn expected_session_id_v1(command: &SliceCommandV1) -> Option<&SessionId> {
    match command {
        SliceCommandV1::SessionStartReplace(input) => {
            Some(&input.preconditions.expected_session_id)
        }
        SliceCommandV1::SessionComplete(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionSkip(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionRetry(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionReturn(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionBlock(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionUnblock(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionCancel(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionReopen(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionReset(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::ItemCheck(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::ItemUncheck(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::ItemSet(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::ItemAdd(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::ItemRemove(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::ItemAttach(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::ItemClear(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::WorkspaceInit(_)
        | SliceCommandV1::WorkspaceResetAll(_)
        | SliceCommandV1::SessionStart(_)
        | SliceCommandV1::WorkspaceDoctor(_)
        | SliceCommandV1::WorkspaceShow(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::SessionStatus(_)
        | SliceCommandV1::SessionNext(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => None,
    }
}

fn expected_start_procedure_digest_v1(command: &SliceCommandV1) -> Option<&Sha256Digest> {
    match command {
        SliceCommandV1::SessionStart(input) => input.expected_procedure_digest.as_ref(),
        SliceCommandV1::SessionStartReplace(input) => {
            input.start.expected_procedure_digest.as_ref()
        }
        _ => None,
    }
}

fn admission_session_identity_v1(command: &SliceCommandV1) -> AdmissionSessionIdentityV1 {
    if matches!(command, SliceCommandV1::SessionStart(_)) {
        AdmissionSessionIdentityV1::Absent
    } else if let Some(expected) = expected_session_id_v1(command) {
        AdmissionSessionIdentityV1::Exact(expected.clone())
    } else {
        AdmissionSessionIdentityV1::Any
    }
}

fn enforce_session_identity_v1(
    command: &SliceCommandV1,
    current: Option<&SessionAggregateV1>,
) -> Result<(), DomainError> {
    let Some(expected) = expected_session_id_v1(command) else {
        return Ok(());
    };
    let actual = current.map(SessionAggregateV1::session_id).cloned();
    if actual.as_ref() != Some(expected) {
        return Err(DomainError::SessionIdentityMismatch {
            expected: expected.clone(),
            actual,
        });
    }
    Ok(())
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

fn session_revision_store_preconditions_v1(
    expected_session_revision: Revision,
) -> (
    Option<Revision>,
    Option<AttemptId>,
    Option<ItemId>,
    Option<Revision>,
) {
    (Some(expected_session_revision), None, None, None)
}

fn expected_revision_v1(command: &SliceCommandV1) -> Revision {
    match command {
        SliceCommandV1::SessionStartReplace(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionComplete(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionSkip(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionRetry(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionReturn(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionBlock(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionUnblock(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionCancel(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionReopen(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::SessionReset(input) => input.preconditions.expected_session_revision,
        SliceCommandV1::WorkspaceInit(_)
        | SliceCommandV1::WorkspaceResetAll(_)
        | SliceCommandV1::SessionStart(_)
        | SliceCommandV1::ItemCheck(_)
        | SliceCommandV1::ItemUncheck(_)
        | SliceCommandV1::ItemSet(_)
        | SliceCommandV1::ItemAdd(_)
        | SliceCommandV1::ItemRemove(_)
        | SliceCommandV1::ItemAttach(_)
        | SliceCommandV1::ItemClear(_)
        | SliceCommandV1::WorkspaceDoctor(_)
        | SliceCommandV1::WorkspaceShow(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::SessionStatus(_)
        | SliceCommandV1::SessionNext(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => Revision::ZERO,
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

fn validate_local_attached_artifact_v1(
    path: &str,
    media_type: Option<&str>,
    artifact: &ArtifactValueV1,
) -> Result<(), DomainError> {
    if artifact.location_kind() != ArtifactLocationKindV1::LocalPath || artifact.location() != path
    {
        return Err(DomainError::InvalidState {
            reason: "artifact verifier did not preserve the requested local path",
        });
    }
    if media_type.is_some_and(|media_type| media_type != artifact.media_type()) {
        return Err(DomainError::InvalidState {
            reason: "artifact verifier did not preserve the requested media type",
        });
    }
    if media_type.is_none() && artifact.media_type().is_empty() {
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
    command: &DomainCommand,
    prior: Option<&SessionAggregateV1>,
    outcome: &podway_core::TransitionOutcomeV1,
) -> Result<StateTransitionV1, DomainError> {
    let previous_workspace_revision = prior.map_or(Revision::ZERO, SessionAggregateV1::revision);
    let (session_id, resulting_workspace_revision, persisted_session_mutation) =
        match outcome.next_aggregate() {
            Some(next) => {
                let resulting_workspace_revision = outcome
                    .revision_after()
                    .unwrap_or(previous_workspace_revision);
                let persisted_session_mutation = if outcome.changed() {
                    match prior {
                        Some(prior)
                            if matches!(command, DomainCommand::SessionStartReplace)
                                && prior.session_id() != next.session_id() =>
                        {
                            PersistedSessionMutationV1::ReplaceFresh(next.clone())
                        }
                        _ => PersistedSessionMutationV1::Replace(next.clone()),
                    }
                } else {
                    PersistedSessionMutationV1::Unchanged
                };
                (
                    Some(next.session_id().clone()),
                    resulting_workspace_revision,
                    persisted_session_mutation,
                )
            }
            None if outcome.changed() => (None, Revision::ZERO, PersistedSessionMutationV1::Clear),
            None => {
                return Err(DomainError::InvalidState {
                    reason: "admitted session transition has no next aggregate",
                });
            }
        };
    StateTransitionV1::new_persisted(
        session_id,
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
    prior: Option<&SessionAggregateV1>,
    outcome: &podway_core::TransitionOutcomeV1,
    workspace_id: &WorkspaceId,
) -> Result<DomainResult, DomainError> {
    let session_id = outcome
        .next_aggregate()
        .or(prior)
        .map(|session| session.session_id().clone());
    let revision_before = outcome.revision_before().unwrap_or(Revision::ZERO);
    let revision_after = if matches!(command, DomainCommand::SessionReset) {
        Revision::ZERO
    } else {
        outcome.revision_after().unwrap_or(revision_before)
    };
    let changed = outcome.changed();
    let result = match command {
        DomainCommand::ItemCheck { item_id }
        | DomainCommand::ItemUncheck { item_id }
        | DomainCommand::ItemSet { item_id }
        | DomainCommand::ItemAdd { item_id }
        | DomainCommand::ItemRemove { item_id }
        | DomainCommand::ItemAttach { item_id }
        | DomainCommand::ItemClear { item_id } => DomainResult::ItemChanged {
            session_id: session_id.clone().ok_or(DomainError::InvalidState {
                reason: "admitted session transition has no session aggregate",
            })?,
            item_id: item_id.clone(),
            revision_before,
            revision_after,
            changed,
        },
        DomainCommand::SessionStart
        | DomainCommand::SessionStartReplace
        | DomainCommand::SessionComplete
        | DomainCommand::SessionSkip
        | DomainCommand::SessionRetry
        | DomainCommand::SessionReturn
        | DomainCommand::SessionBlock
        | DomainCommand::SessionUnblock
        | DomainCommand::SessionCancel
        | DomainCommand::SessionReopen
        | DomainCommand::SessionReset => DomainResult::SessionChanged {
            session_id: session_id.ok_or(DomainError::InvalidState {
                reason: "admitted session transition has no session aggregate",
            })?,
            revision_before,
            revision_after,
            changed,
        },
        DomainCommand::WorkspaceInitialize => DomainResult::WorkspaceInitialized {
            workspace_id: workspace_id.clone(),
            revision: Revision::ZERO,
        },
        DomainCommand::WorkspaceResetAll => DomainResult::WorkspaceReset {
            workspace_id: workspace_id.clone(),
            revision: Revision::ZERO,
        },
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
        "execution_version": EXECUTION_DOCUMENT_VERSION_V5,
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
    resolved_procedure_digest: Option<&Sha256Digest>,
) -> Result<Sha256Digest, ExecutionErrorV1> {
    let canonical = match resolved_procedure_digest {
        Some(digest) => {
            podway_protocol::canonical_start_mutation_identity_v1(request, workspace_id, digest)
        }
        None => podway_protocol::canonical_mutation_identity_v1(request, workspace_id),
    }
    .map_err(|_| ExecutionErrorV1::InvalidPersistedExecution {
        reason: "request identity cannot be canonicalized",
    })?;
    Sha256Digest::new(format!("sha256:{}", sha256_hex_v1(canonical.as_bytes()))).map_err(|_| {
        ExecutionErrorV1::InvalidPersistedExecution {
            reason: "request identity digest is invalid",
        }
    })
}
fn reset_all_request_digest_v1(
    request: &SliceRequestV1,
    previous_workspace: &DurableWorktreeIdentityV1,
) -> Result<Sha256Digest, ExecutionErrorV1> {
    let canonical = canonical_reset_all_identity_v1(
        request,
        previous_workspace.common_dir_identity(),
        previous_workspace.worktree_admin_identity(),
    )
    .map_err(|_| ExecutionErrorV1::InvalidPersistedExecution {
        reason: "reset request identity cannot be canonicalized",
    })?;
    Sha256Digest::new(format!("sha256:{}", sha256_hex_v1(canonical.as_bytes()))).map_err(|_| {
        ExecutionErrorV1::InvalidPersistedExecution {
            reason: "reset request identity digest is invalid",
        }
    })
}

fn execution_components_v1(command: &SliceCommandV1) -> (Value, Value) {
    match command {
        SliceCommandV1::WorkspaceInit(input) => (json!({}), json!({"repair": input.repair})),
        SliceCommandV1::WorkspaceResetAll(input) => (
            workspace_reset_all_preconditions_value_v1(&input.preconditions),
            json!({"confirmed": input.confirmed}),
        ),
        SliceCommandV1::SessionStart(input) => (
            json!({}),
            json!({
                "expected_procedure_digest": input.expected_procedure_digest,
                "source": session_start_source_value_v1(&input.source),
                "task_title": input.task_title,
            }),
        ),
        SliceCommandV1::SessionStartReplace(input) => (
            session_identity_preconditions_value_v1(&input.preconditions),
            json!({
                "confirmed": input.confirmed,
                "expected_procedure_digest": input.start.expected_procedure_digest,
                "source": session_start_source_value_v1(&input.start.source),
                "task_title": input.start.task_title,
            }),
        ),
        SliceCommandV1::SessionComplete(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({}),
        ),
        SliceCommandV1::SessionSkip(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"reason": input.reason}),
        ),
        SliceCommandV1::SessionRetry(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"reason": input.reason}),
        ),
        SliceCommandV1::SessionReturn(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"destination_stage_id": input.destination_stage_id, "reason": input.reason}),
        ),
        SliceCommandV1::SessionBlock(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"reason": input.reason}),
        ),
        SliceCommandV1::SessionUnblock(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"all": input.all, "blocker_id": input.blocker_id}),
        ),
        SliceCommandV1::SessionCancel(input) => (
            session_preconditions_value_v1(&input.preconditions),
            json!({"reason": input.reason}),
        ),
        SliceCommandV1::SessionReopen(input) => (
            session_revision_preconditions_value_v1(&input.preconditions),
            json!({"destination_stage_id": input.destination_stage_id, "reason": input.reason}),
        ),
        SliceCommandV1::SessionReset(input) => (
            session_identity_preconditions_value_v1(&input.preconditions),
            json!({"confirmed": input.confirmed}),
        ),
        SliceCommandV1::ItemCheck(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({"item_id": input.item_id}),
        ),
        SliceCommandV1::ItemUncheck(input) => (
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
        SliceCommandV1::ItemRemove(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({
                "ignore_missing": input.ignore_missing,
                "item_id": input.item_id,
                "value": input.value,
            }),
        ),
        SliceCommandV1::ItemAttach(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({
                "item_id": input.item_id,
                "source": item_attach_source_value_v1(&input.source),
            }),
        ),
        SliceCommandV1::ItemClear(input) => (
            item_preconditions_value_v1(&input.preconditions),
            json!({"item_id": input.item_id}),
        ),
        SliceCommandV1::WorkspaceDoctor(_)
        | SliceCommandV1::WorkspaceShow(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::SessionStatus(_)
        | SliceCommandV1::SessionNext(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => {
            unreachable!("non-durable commands are rejected at admission")
        }
    }
}

fn item_preconditions_value_v1(
    preconditions: &podway_protocol::ItemMutationPreconditionsWireV1,
) -> Value {
    json!({
        "session_id": preconditions.expected_session_id,
        "attempt_id": preconditions.expected_attempt_id,
        "item_revision": preconditions.expected_item_revision,
    })
}

fn session_preconditions_value_v1(
    preconditions: &podway_protocol::SessionMutationPreconditionsWireV1,
) -> Value {
    json!({
        "session_id": preconditions.expected_session_id,
        "attempt_id": preconditions.expected_attempt_id,
        "session_revision": preconditions.expected_session_revision,
    })
}

fn session_identity_preconditions_value_v1(
    preconditions: &podway_protocol::SessionIdentityPreconditionsWireV1,
) -> Value {
    json!({
        "session_id": preconditions.expected_session_id,
        "session_revision": preconditions.expected_session_revision,
    })
}

fn session_revision_preconditions_value_v1(
    preconditions: &podway_protocol::SessionRevisionPreconditionsWireV1,
) -> Value {
    json!({
        "session_id": preconditions.expected_session_id,
        "session_revision": preconditions.expected_session_revision,
    })
}

fn workspace_reset_all_preconditions_value_v1(
    preconditions: &podway_protocol::WorkspaceResetAllPreconditionsWireV1,
) -> Value {
    json!({"workspace_id": preconditions.expected_workspace_id})
}

fn session_start_source_value_v1(source: &SessionStartSourceV1) -> Value {
    match source {
        SessionStartSourceV1::Preset { preset } => json!({"preset": preset}),
        SessionStartSourceV1::Procedure { procedure } => json!({"procedure": procedure}),
    }
}

fn item_attach_source_value_v1(source: &ItemAttachSourceV1) -> Value {
    match source {
        ItemAttachSourceV1::Path { path, media_type } => {
            json!({"media_type": media_type, "path": path})
        }
        ItemAttachSourceV1::OpaqueReference {
            reference,
            digest,
            size_bytes,
            media_type,
        } => json!({
            "digest": digest,
            "media_type": media_type,
            "reference": reference,
            "size_bytes": size_bytes,
        }),
    }
}
fn admission_resolution_value_v1(resolution: &AdmissionResolutionV1) -> Value {
    match resolution {
        AdmissionResolutionV1::None => json!({"kind": "none"}),
        AdmissionResolutionV1::SessionStart {
            snapshot,
            session_id,
            first_attempt_id,
        } => json!({
            "first_attempt_id": first_attempt_id.as_str(),
            "kind": "session_start",
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
        AdmissionResolutionV1::SessionSkip { next_attempt_id } => {
            json!({"kind": "session_skip", "next_attempt_id": next_attempt_id.as_str()})
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
        AdmissionResolutionV1::SessionReopen {
            destination_attempt_id,
        } => {
            json!({
                "destination_attempt_id": destination_attempt_id.as_str(),
                "kind": "session_reopen",
            })
        }
    }
}

fn decode_admission_resolution_v1(
    command: &SliceCommandV1,
    object: &Map<String, Value>,
) -> Result<AdmissionResolutionV1, ExecutionErrorV1> {
    let kind = value_string_v1(object, "kind")?;
    match command {
        SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_) => {
            require_exact_keys_v1(
                object,
                &["first_attempt_id", "kind", "session_id", "snapshot"],
            )?;
            if kind != "session_start" {
                return Err(invalid_execution_v1(
                    "execution resolution kind does not match command",
                ));
            }
            Ok(AdmissionResolutionV1::SessionStart {
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
        SliceCommandV1::SessionSkip(_) => {
            decode_next_attempt_resolution_v1(object, kind, "session_skip", |next_attempt_id| {
                AdmissionResolutionV1::SessionSkip { next_attempt_id }
            })
        }
        SliceCommandV1::SessionRetry(_) => {
            decode_next_attempt_resolution_v1(object, kind, "session_retry", |next_attempt_id| {
                AdmissionResolutionV1::SessionRetry { next_attempt_id }
            })
        }
        SliceCommandV1::SessionReturn(_) => decode_destination_attempt_resolution_v1(
            object,
            kind,
            "session_return",
            |destination_attempt_id| AdmissionResolutionV1::SessionReturn {
                destination_attempt_id,
            },
        ),
        SliceCommandV1::SessionComplete(_) => {
            decode_next_attempt_resolution_v1(object, kind, "session_complete", |next_attempt_id| {
                AdmissionResolutionV1::SessionComplete { next_attempt_id }
            })
        }
        SliceCommandV1::SessionReopen(_) => decode_destination_attempt_resolution_v1(
            object,
            kind,
            "session_reopen",
            |destination_attempt_id| AdmissionResolutionV1::SessionReopen {
                destination_attempt_id,
            },
        ),
        SliceCommandV1::WorkspaceInit(_)
        | SliceCommandV1::WorkspaceResetAll(_)
        | SliceCommandV1::ItemCheck(_)
        | SliceCommandV1::ItemUncheck(_)
        | SliceCommandV1::ItemSet(_)
        | SliceCommandV1::ItemAdd(_)
        | SliceCommandV1::ItemRemove(_)
        | SliceCommandV1::ItemAttach(_)
        | SliceCommandV1::ItemClear(_)
        | SliceCommandV1::SessionUnblock(_)
        | SliceCommandV1::SessionCancel(_)
        | SliceCommandV1::SessionReset(_)
        | SliceCommandV1::WorkspaceDoctor(_)
        | SliceCommandV1::WorkspaceShow(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::SessionStatus(_)
        | SliceCommandV1::SessionNext(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => {
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

fn decode_next_attempt_resolution_v1<F>(
    object: &Map<String, Value>,
    kind: &str,
    expected_kind: &'static str,
    constructor: F,
) -> Result<AdmissionResolutionV1, ExecutionErrorV1>
where
    F: FnOnce(AttemptId) -> AdmissionResolutionV1,
{
    require_exact_keys_v1(object, &["kind", "next_attempt_id"])?;
    if kind != expected_kind {
        return Err(invalid_execution_v1(
            "execution resolution kind does not match command",
        ));
    }
    Ok(constructor(value_typed_v1(object, "next_attempt_id")?))
}

fn decode_destination_attempt_resolution_v1<F>(
    object: &Map<String, Value>,
    kind: &str,
    expected_kind: &'static str,
    constructor: F,
) -> Result<AdmissionResolutionV1, ExecutionErrorV1>
where
    F: FnOnce(AttemptId) -> AdmissionResolutionV1,
{
    require_exact_keys_v1(object, &["destination_attempt_id", "kind"])?;
    if kind != expected_kind {
        return Err(invalid_execution_v1(
            "execution resolution kind does not match command",
        ));
    }
    Ok(constructor(value_typed_v1(
        object,
        "destination_attempt_id",
    )?))
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
        u64,
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
            return Err(invalid_execution_v1(
                "legacy execution document lacks session identity fences",
            ));
        }
        version
            if version == u64::from(EXECUTION_DOCUMENT_VERSION_V4)
                || version == u64::from(EXECUTION_DOCUMENT_VERSION_V5) =>
        {
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
    let command = decode_command_components_v1(version, command, preconditions, payload)?;
    let resolution = match resolution {
        AdmissionResolutionV1::None => {
            decode_admission_resolution_v1(&command, value_object_v1(object, "execution")?)?
        }
        _ => unreachable!("only the v4 placeholder resolution is constructed here"),
    };
    Ok((version, selector, workspace_id, command, resolution))
}

/// Returns the immutable Procedure digest embedded in an admitted start execution document.
///
/// This deliberately decodes the complete document rather than consulting the source path again.
pub(crate) fn admitted_start_procedure_digest_v1(
    execution: &CanonicalExecutionJsonV1,
) -> Result<Option<Sha256Digest>, ExecutionErrorV1> {
    let (_, _, _, command, resolution) = decode_execution_document_v1(execution.as_str())?;
    if !matches!(
        command,
        SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_)
    ) {
        return Ok(None);
    }
    let AdmissionResolutionV1::SessionStart { snapshot, .. } = resolution else {
        return Err(invalid_execution_v1(
            "admitted start has no Procedure snapshot",
        ));
    };
    Ok(Some(snapshot.digest().clone()))
}

fn decode_command_components_v1(
    execution_version: u64,
    command: &str,
    preconditions: &Map<String, Value>,
    payload: &Map<String, Value>,
) -> Result<SliceCommandV1, ExecutionErrorV1> {
    match command {
        "workspace.init" => {
            require_exact_keys_v1(preconditions, &[])?;
            require_exact_keys_v1(payload, &["repair"])?;
            Ok(SliceCommandV1::WorkspaceInit(
                podway_protocol::WorkspaceInitV1 {
                    repair: value_bool_v1(payload, "repair")?,
                },
            ))
        }
        "workspace.reset_all" => {
            require_exact_keys_v1(payload, &["confirmed"])?;
            Ok(SliceCommandV1::WorkspaceResetAll(
                podway_protocol::WorkspaceResetAllV1 {
                    confirmed: value_bool_v1(payload, "confirmed")?,
                    preconditions: decode_workspace_reset_all_preconditions_v1(preconditions)?,
                },
            ))
        }
        "session.start" => {
            require_exact_keys_v1(preconditions, &[])?;
            let expected_procedure_digest =
                if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V5) {
                    require_exact_keys_v1(
                        payload,
                        &["expected_procedure_digest", "source", "task_title"],
                    )?;
                    value_optional_typed_v1(payload, "expected_procedure_digest")?
                } else {
                    require_exact_keys_v1(payload, &["source", "task_title"])?;
                    None
                };
            Ok(SliceCommandV1::SessionStart(SessionStartV1 {
                source: decode_session_start_source_v1(value_object_v1(payload, "source")?)?,
                expected_procedure_digest,
                task_title: value_string_v1(payload, "task_title")?.to_owned(),
                dry_run: false,
            }))
        }
        "session.start_replace" => {
            let expected_procedure_digest =
                if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V5) {
                    require_exact_keys_v1(
                        payload,
                        &[
                            "confirmed",
                            "expected_procedure_digest",
                            "source",
                            "task_title",
                        ],
                    )?;
                    value_optional_typed_v1(payload, "expected_procedure_digest")?
                } else {
                    require_exact_keys_v1(payload, &["confirmed", "source", "task_title"])?;
                    None
                };
            Ok(SliceCommandV1::SessionStartReplace(
                podway_protocol::SessionStartReplaceV1 {
                    start: SessionStartV1 {
                        source: decode_session_start_source_v1(value_object_v1(
                            payload, "source",
                        )?)?,
                        expected_procedure_digest,
                        task_title: value_string_v1(payload, "task_title")?.to_owned(),
                        dry_run: false,
                    },
                    confirmed: value_bool_v1(payload, "confirmed")?,
                    preconditions: decode_session_identity_preconditions_v1(preconditions)?,
                },
            ))
        }
        "session.complete" => {
            require_exact_keys_v1(payload, &[])?;
            Ok(SliceCommandV1::SessionComplete(SessionCompleteV1 {
                preconditions: decode_session_preconditions_v1(preconditions)?,
            }))
        }
        "session.skip" => {
            require_exact_keys_v1(payload, &["reason"])?;
            Ok(SliceCommandV1::SessionSkip(SessionSkipV1 {
                reason: value_optional_string_v1(payload, "reason")?,
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
                dry_run: false,
                preconditions: decode_session_preconditions_v1(preconditions)?,
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
        "session.cancel" => Ok(SliceCommandV1::SessionCancel(SessionCancelV1 {
            reason: required_payload_string_v1(payload, &["reason"], "reason")?,
            preconditions: decode_session_preconditions_v1(preconditions)?,
        })),
        "session.reopen" => {
            require_exact_keys_v1(payload, &["destination_stage_id", "reason"])?;
            Ok(SliceCommandV1::SessionReopen(SessionReopenV1 {
                destination_stage_id: value_typed_v1(payload, "destination_stage_id")?,
                reason: value_string_v1(payload, "reason")?.to_owned(),
                dry_run: false,
                preconditions: decode_session_revision_preconditions_v1(preconditions)?,
            }))
        }
        "session.reset" => {
            require_exact_keys_v1(payload, &["confirmed"])?;
            Ok(SliceCommandV1::SessionReset(SessionResetV1 {
                confirmed: value_bool_v1(payload, "confirmed")?,
                dry_run: false,
                preconditions: decode_session_identity_preconditions_v1(preconditions)?,
            }))
        }
        "item.check" => {
            require_exact_keys_v1(payload, &["item_id"])?;
            Ok(SliceCommandV1::ItemCheck(ItemCheckV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        "item.uncheck" => {
            require_exact_keys_v1(payload, &["item_id"])?;
            Ok(SliceCommandV1::ItemUncheck(ItemUncheckV1 {
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
        "item.remove" => {
            require_exact_keys_v1(payload, &["ignore_missing", "item_id", "value"])?;
            Ok(SliceCommandV1::ItemRemove(ItemRemoveV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                value: value_string_v1(payload, "value")?.to_owned(),
                ignore_missing: value_bool_v1(payload, "ignore_missing")?,
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        "item.attach" => {
            require_exact_keys_v1(payload, &["item_id", "source"])?;
            Ok(SliceCommandV1::ItemAttach(ItemAttachV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                source: decode_item_attach_source_v1(value_object_v1(payload, "source")?)?,
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        "item.clear" => {
            require_exact_keys_v1(payload, &["item_id"])?;
            Ok(SliceCommandV1::ItemClear(ItemClearV1 {
                item_id: value_typed_v1(payload, "item_id")?,
                preconditions: decode_item_preconditions_v1(preconditions)?,
            }))
        }
        _ => Err(invalid_execution_v1("command is outside the admitted set")),
    }
}

fn decode_session_start_source_v1(
    object: &Map<String, Value>,
) -> Result<SessionStartSourceV1, ExecutionErrorV1> {
    if object.contains_key("preset") {
        require_exact_keys_v1(object, &["preset"])?;
        return Ok(SessionStartSourceV1::Preset {
            preset: value_string_v1(object, "preset")?.to_owned(),
        });
    }
    if object.contains_key("procedure") {
        require_exact_keys_v1(object, &["procedure"])?;
        return Ok(SessionStartSourceV1::Procedure {
            procedure: value_string_v1(object, "procedure")?.to_owned(),
        });
    }
    Err(invalid_execution_v1("session start source is invalid"))
}

fn decode_item_attach_source_v1(
    object: &Map<String, Value>,
) -> Result<ItemAttachSourceV1, ExecutionErrorV1> {
    if object.contains_key("path") {
        require_exact_keys_v1(object, &["media_type", "path"])?;
        return Ok(ItemAttachSourceV1::Path {
            path: value_string_v1(object, "path")?.to_owned(),
            media_type: value_optional_string_v1(object, "media_type")?,
        });
    }
    if object.contains_key("reference") {
        require_exact_keys_v1(object, &["digest", "media_type", "reference", "size_bytes"])?;
        return Ok(ItemAttachSourceV1::OpaqueReference {
            reference: value_string_v1(object, "reference")?.to_owned(),
            digest: value_typed_v1(object, "digest")?,
            size_bytes: value_u64_v1(object, "size_bytes")?,
            media_type: value_string_v1(object, "media_type")?.to_owned(),
        });
    }
    Err(invalid_execution_v1("item attachment source is invalid"))
}

fn decode_item_preconditions_v1(
    object: &Map<String, Value>,
) -> Result<podway_protocol::ItemMutationPreconditionsWireV1, ExecutionErrorV1> {
    require_exact_keys_v1(object, &["attempt_id", "item_revision", "session_id"])?;
    Ok(podway_protocol::ItemMutationPreconditionsWireV1 {
        expected_session_id: value_typed_v1(object, "session_id")?,
        expected_attempt_id: value_typed_v1(object, "attempt_id")?,
        expected_item_revision: value_typed_v1(object, "item_revision")?,
    })
}

fn decode_session_preconditions_v1(
    object: &Map<String, Value>,
) -> Result<podway_protocol::SessionMutationPreconditionsWireV1, ExecutionErrorV1> {
    require_exact_keys_v1(object, &["attempt_id", "session_id", "session_revision"])?;
    Ok(podway_protocol::SessionMutationPreconditionsWireV1 {
        expected_session_id: value_typed_v1(object, "session_id")?,
        expected_attempt_id: value_typed_v1(object, "attempt_id")?,
        expected_session_revision: value_typed_v1(object, "session_revision")?,
    })
}

fn decode_session_identity_preconditions_v1(
    object: &Map<String, Value>,
) -> Result<podway_protocol::SessionIdentityPreconditionsWireV1, ExecutionErrorV1> {
    require_exact_keys_v1(object, &["session_id", "session_revision"])?;
    Ok(podway_protocol::SessionIdentityPreconditionsWireV1 {
        expected_session_id: value_typed_v1(object, "session_id")?,
        expected_session_revision: value_typed_v1(object, "session_revision")?,
    })
}

fn decode_session_revision_preconditions_v1(
    object: &Map<String, Value>,
) -> Result<podway_protocol::SessionRevisionPreconditionsWireV1, ExecutionErrorV1> {
    require_exact_keys_v1(object, &["session_id", "session_revision"])?;
    Ok(podway_protocol::SessionRevisionPreconditionsWireV1 {
        expected_session_id: value_typed_v1(object, "session_id")?,
        expected_session_revision: value_typed_v1(object, "session_revision")?,
    })
}

fn decode_workspace_reset_all_preconditions_v1(
    object: &Map<String, Value>,
) -> Result<podway_protocol::WorkspaceResetAllPreconditionsWireV1, ExecutionErrorV1> {
    require_exact_keys_v1(object, &["workspace_id"])?;
    Ok(podway_protocol::WorkspaceResetAllPreconditionsWireV1 {
        expected_workspace_id: value_optional_typed_v1(object, "workspace_id")?,
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
