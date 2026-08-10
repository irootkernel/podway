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
    AuthoringContext, ParsedNodeDefinition, ParsedProcedure, ParsedProcedureV2,
    ProcedureDocumentFormat, ProcedureFormatV1, ProcedureSourceLabel, ProcedureWarningPolicyV1,
    parse_procedure_document, parse_procedure_v1, sniff_procedure_schema, validate_procedure_v2,
    vet_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, AddItemV1, ArtifactLocationKindV1, ArtifactValueV1, AttachItemV1,
    AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptV1, AttemptValidityV2, AuthoringSeverity,
    BlockSessionV1, BlockerId, BlockerState, CancelSessionV1, CanonicalProcedureJsonV1,
    CanonicalProcedureSnapshotInputV1, CheckItemV1, ClearItemV1, CommandContextV1,
    CompleteSessionV1, DecisionRecordV2, DomainCommand, DomainError, DomainResult,
    GraphPlacementV2, ItemId, ItemMutationPreconditionsV1, ItemTypeV1, ItemValueV1, JobId,
    LocalArtifactVerificationV1, ProcedureSnapshotId, ProcedureSnapshotV1, ProcedureSourceLabelV1,
    ReasonV2, RemoveItemV1, ReopenSessionV1, ResetAllWorkspaceV1, ResetSessionV1,
    ResolvedEvidenceReferenceV2, RetrySessionV1, ReturnSessionV1, Revision, SessionAggregateV1,
    SessionAttemptV2, SessionCommandV1, SessionId, SessionLifecycle, SessionTraceV2, SetItemV1,
    Sha256Digest, SkipSessionV1, StageSpecV1, StartReplaceSessionV1, StartSessionV1,
    TraceSequenceV2, UnblockSessionV1, UncheckItemV1, UnixMillis, WorkspaceId, apply_transition_v1,
    canonicalize_json_v1, required_items_satisfied,
};
use podway_presets::lookup as lookup_embedded_preset_v1;
use podway_protocol::{
    ItemAddV1, ItemAttachSourceV1, ItemAttachV1, ItemCheckV1, ItemClearV1, ItemRemoveV1, ItemSetV1,
    ItemUncheckV1, ProcedureV2MutationCommandV1, ProcedureV2MutationRequestV1, RequestIdV1,
    Rfc3339MillisV1, SessionBlockV1, SessionCancelV1, SessionCompleteV1, SessionDecideV2,
    SessionMutationPreconditionsWireV1, SessionReopenV1, SessionResetV1, SessionRetryV1,
    SessionReturnV1, SessionReworkV2, SessionSkipV1, SessionStartSourceV1, SessionStartV1,
    SessionUnblockV1, SliceCommandV1, SliceRequestV1, WorktreeSelectorWireV1,
    canonical_procedure_v2_mutation_identity_v1, canonical_reset_all_identity_v1,
};
use podway_store::{
    ActiveItemMutationV2, AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1,
    AttemptMetadataV2, CanonicalExecutionJsonV1, ClaimedJobV1, DurableWorktreeIdentityV1,
    GraphMutationErrorV2, GraphNodeCounterV2, GraphSessionStateV2, GraphStartCurrentTaskV2,
    IdempotencyKeyV1, PersistedGraphMutationFailureV2, PersistedGraphTerminalOperationV2,
    PersistedResponseContextV1, PersistedSessionMutationV1, ProcedureSnapshotV2,
    RevisionAttemptItemPreconditionsV1, StateTransitionV1, StoreContractV1, StoreErrorV1,
    StoreGraphMutationContractV2, StoreGraphReadContractV2, StoreIdempotencyReadContractV1,
    StoreValueErrorV1, TerminalReceiptV1, TerminalResultV1, WorkerIdV1, WorkflowMemoryStateV2,
    WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const EXECUTION_DOCUMENT_VERSION_V1: u8 = 1;
const EXECUTION_DOCUMENT_VERSION_V2: u8 = 2;
const EXECUTION_DOCUMENT_VERSION_V3: u8 = 3;
const EXECUTION_DOCUMENT_VERSION_V4: u8 = 4;
const EXECUTION_DOCUMENT_VERSION_V5: u8 = 5;
const EXECUTION_DOCUMENT_VERSION_V6: u8 = 6;
const EXECUTION_DOCUMENT_VERSION_V7: u8 = 7;
const EXECUTION_DOCUMENT_VERSION_V8: u8 = 8;
const EXECUTION_DOCUMENT_VERSION_V9: u8 = 9;
const EXECUTION_DOCUMENT_VERSION_V10: u8 = 10;

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
    ProcedureV2Unsupported,
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

    pub const fn procedure_v2_unsupported() -> Self {
        Self::ProcedureV2Unsupported
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

    /// Reads, validates, and vets a relative Procedure v2 source under an already revalidated
    /// worktree. Implementations that do not own descriptor-safe filesystem authority remain
    /// closed by default.
    fn load_workspace_procedure_snapshot_v2(
        &self,
        _workspace: &WorkspaceBindingV1,
        _procedure: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV2, ProcedureV2SourceAdmissionErrorV1> {
        Err(ProcedureV2SourceAdmissionErrorV1::Rejected(
            ExecutionBoundaryErrorV1::procedure_v2_unsupported(),
        ))
    }

    /// Returns an admitted shipped Procedure v2 snapshot together with its independently pinned
    /// package digest. Production remains closed until a shipped v2 asset is registered.
    fn load_preset_snapshot_v2(
        &self,
        _preset: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<Option<(ProcedureSnapshotV2, Sha256Digest)>, ProcedureV2SourceAdmissionErrorV1>
    {
        Ok(None)
    }
}

/// Version-sensitive outcome of inspecting one custom Procedure source. A declared v1 document
/// is not a v2 failure: dispatch may safely fall back to the unchanged v1 start path.
#[derive(Debug)]
pub enum ProcedureV2SourceAdmissionErrorV1 {
    NotProcedureV2,
    SchemaInvalid { diagnostic_codes: Vec<String> },
    Rejected(ExecutionBoundaryErrorV1),
}

/// Failure before a confirmed Procedure v2 start can be durably admitted.
#[derive(Debug)]
pub enum ProcedureV2StartPreparationErrorV1 {
    Source(ProcedureV2SourceAdmissionErrorV1),
    DigestConfirmationRequired {
        procedure_digest: Sha256Digest,
    },
    ProcedureDigestMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    Domain(DomainError),
    InvalidStoreValue(StoreValueErrorV1),
    Execution(ExecutionErrorV1),
    PinnedPresetDigestMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
}

fn verify_pinned_procedure_v2_snapshot(
    snapshot: &ProcedureSnapshotV2,
    pinned_digest: &Sha256Digest,
) -> Result<(), ProcedureV2StartPreparationErrorV1> {
    let recomputed = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(snapshot.canonical_json().as_str().as_bytes())
    ))
    .map_err(ProcedureV2StartPreparationErrorV1::Domain)?;
    if &recomputed != snapshot.digest() || &recomputed != pinned_digest {
        return Err(
            ProcedureV2StartPreparationErrorV1::PinnedPresetDigestMismatch {
                expected: pinned_digest.clone(),
                actual: recomputed,
            },
        );
    }
    Ok(())
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
    if sniff_procedure_schema(source, format) == Some(podway_core::PROCEDURE_SCHEMA_V2) {
        return Err(ExecutionBoundaryErrorV1::procedure_v2_unsupported());
    }
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

/// Admits already bounded workspace bytes through the complete Procedure v2 configuration path.
/// This helper performs no filesystem I/O and creates no durable state.
pub fn workspace_procedure_snapshot_from_bytes_v2(
    procedure: &str,
    source: &[u8],
    snapshot_id: ProcedureSnapshotId,
    created_at: UnixMillis,
) -> Result<ProcedureSnapshotV2, ProcedureV2SourceAdmissionErrorV1> {
    let source_label = ProcedureSourceLabel::workspace_path(procedure).map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "workspace procedure path is invalid",
            },
        ))
    })?;
    let format =
        procedure_format_v1(procedure).map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
    if sniff_procedure_schema(source, format) != Some(podway_core::PROCEDURE_SCHEMA_V2) {
        return Err(ProcedureV2SourceAdmissionErrorV1::NotProcedureV2);
    }
    let source_text = std::str::from_utf8(source).map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::SchemaInvalid {
            diagnostic_codes: vec!["AUTHORING_SOURCE_NOT_UTF8".to_owned()],
        }
    })?;
    let parsed = match parse_procedure_document(source, format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Ok(ParsedProcedure::V1(_)) => {
            return Err(ProcedureV2SourceAdmissionErrorV1::NotProcedureV2);
        }
        Err(_) => {
            return Err(ProcedureV2SourceAdmissionErrorV1::SchemaInvalid {
                diagnostic_codes: vec!["AUTHORING_SCHEMA_INVALID".to_owned()],
            });
        }
    };
    let validated = validate_procedure_v2(parsed).map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::SchemaInvalid {
            diagnostic_codes: vec!["AUTHORING_SCHEMA_INVALID".to_owned()],
        }
    })?;
    let context = AuthoringContext::new(procedure, source_text, format);
    let diagnostics = vet_procedure_v2(&validated, &context);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity() == AuthoringSeverity::Error)
    {
        let mut diagnostic_codes: Vec<String> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity() == AuthoringSeverity::Error)
            .map(|diagnostic| diagnostic.code().as_str().to_owned())
            .take(256)
            .collect();
        diagnostic_codes.sort();
        diagnostic_codes.dedup();
        return Err(ProcedureV2SourceAdmissionErrorV1::SchemaInvalid { diagnostic_codes });
    }
    let canonical_json = CanonicalProcedureJsonV1::new(
        validated.canonical_json().as_str().to_owned(),
    )
    .map_err(|error| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
    })?;
    let source = ProcedureSourceLabelV1::new(source_label.display_label()).map_err(|error| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
    })?;
    ProcedureSnapshotV2::new(
        snapshot_id,
        canonical_json,
        validated.digest().clone(),
        source,
        created_at,
    )
    .map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "workspace Procedure v2 snapshot admission failed",
            },
        ))
    })
}

/// Resolves and confirms one custom Procedure v2 source, then constructs the complete fresh
/// in-memory graph session that a durable start transaction may persist. Source admission and
/// digest confirmation finish before this function returns any state to an admission caller.
#[allow(clippy::too_many_arguments)]
pub fn prepare_custom_procedure_v2_start(
    provider: &impl ProcedureProviderV1,
    workspace: &WorkspaceBindingV1,
    procedure: &str,
    expected_digest: Option<&Sha256Digest>,
    task_title: &str,
    session_id: SessionId,
    first_attempt_id: AttemptId,
    snapshot_id: ProcedureSnapshotId,
    created_at: UnixMillis,
) -> Result<GraphSessionStateV2, ProcedureV2StartPreparationErrorV1> {
    let snapshot = provider
        .load_workspace_procedure_snapshot_v2(workspace, procedure, snapshot_id, created_at)
        .map_err(ProcedureV2StartPreparationErrorV1::Source)?;
    let actual_digest = snapshot.digest().clone();
    match expected_digest {
        None => {
            return Err(
                ProcedureV2StartPreparationErrorV1::DigestConfirmationRequired {
                    procedure_digest: actual_digest,
                },
            );
        }
        Some(expected) if expected != &actual_digest => {
            return Err(
                ProcedureV2StartPreparationErrorV1::ProcedureDigestMismatch {
                    expected: expected.clone(),
                    actual: actual_digest,
                },
            );
        }
        Some(_) => {}
    }

    graph_session_state_from_procedure_v2_snapshot(
        snapshot,
        task_title,
        session_id,
        first_attempt_id,
        created_at,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_preset_procedure_v2_start(
    provider: &impl ProcedureProviderV1,
    preset: &str,
    task_title: &str,
    session_id: SessionId,
    first_attempt_id: AttemptId,
    snapshot_id: ProcedureSnapshotId,
    created_at: UnixMillis,
) -> Result<Option<GraphSessionStateV2>, ProcedureV2StartPreparationErrorV1> {
    let Some((snapshot, pinned_digest)) = provider
        .load_preset_snapshot_v2(preset, snapshot_id, created_at)
        .map_err(ProcedureV2StartPreparationErrorV1::Source)?
    else {
        return Ok(None);
    };
    verify_pinned_procedure_v2_snapshot(&snapshot, &pinned_digest)?;
    graph_session_state_from_procedure_v2_snapshot(
        snapshot,
        task_title,
        session_id,
        first_attempt_id,
        created_at,
    )
    .map(Some)
}

/// Reconstructs a fresh Procedure v2 graph session exclusively from immutable admitted data.
/// Claimed execution uses this path so restart never reopens or reparses the caller's source file.
pub fn graph_session_state_from_procedure_v2_snapshot(
    snapshot: ProcedureSnapshotV2,
    task_title: &str,
    session_id: SessionId,
    first_attempt_id: AttemptId,
    created_at: UnixMillis,
) -> Result<GraphSessionStateV2, ProcedureV2StartPreparationErrorV1> {
    let entry_graph_node_id = snapshot.entry_graph_node_id().clone();
    let attempt = SessionAttemptV2::new(
        first_attempt_id.clone(),
        entry_graph_node_id.clone(),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        None,
    )
    .map_err(ProcedureV2StartPreparationErrorV1::Domain)?;
    let trace = SessionTraceV2::from_parts(
        session_id,
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt],
    )
    .map_err(ProcedureV2StartPreparationErrorV1::Domain)?;
    let counters = snapshot
        .graph_nodes()
        .iter()
        .map(|node| {
            GraphNodeCounterV2::new(
                node.graph_node_id().clone(),
                u64::from(node.graph_node_id() == &entry_graph_node_id),
                0,
            )
        })
        .collect();
    let metadata = AttemptMetadataV2::new(first_attempt_id, created_at, None, None)
        .map_err(ProcedureV2StartPreparationErrorV1::InvalidStoreValue)?;
    let metadata = vec![metadata];
    let workflow_memory = WorkflowMemoryStateV2::initial_for_trace(&snapshot, &trace, &metadata)
        .map_err(ProcedureV2StartPreparationErrorV1::InvalidStoreValue)?;
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        task_title,
        snapshot,
        trace,
        counters,
        metadata,
        workflow_memory,
        created_at,
        None,
        None,
        None,
    )
    .map_err(ProcedureV2StartPreparationErrorV1::InvalidStoreValue)
}

#[derive(Clone, Debug)]
struct AdmittedProcedureV2StartV1 {
    selector: WorktreeSelectorWireV1,
    workspace_id: WorkspaceId,
    replace: bool,
    expected_current: GraphStartCurrentTaskV2,
    state: GraphSessionStateV2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedProcedureV2StartProjectionV1 {
    pub procedure_digest: Sha256Digest,
    pub session_id: SessionId,
    pub revision: Revision,
    pub entry_graph_node_id: podway_core::GraphNodeId,
    pub goal_tracking: bool,
}

pub fn admitted_procedure_v2_start_projection_v1(
    execution: &CanonicalExecutionJsonV1,
) -> Result<AdmittedProcedureV2StartProjectionV1, ExecutionErrorV1> {
    let admitted = decode_procedure_v2_start_execution_v1(execution.as_str())?;
    Ok(AdmittedProcedureV2StartProjectionV1 {
        procedure_digest: admitted.state.snapshot().digest().clone(),
        session_id: admitted.state.trace().session_id().clone(),
        revision: admitted.state.trace().revision(),
        entry_graph_node_id: admitted.state.snapshot().entry_graph_node_id().clone(),
        goal_tracking: admitted.state.snapshot().goal_tracking(),
    })
}

fn procedure_v2_start_execution_document_v1(
    admitted: &AdmittedProcedureV2StartV1,
    command: &SliceCommandV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let snapshot = admitted.state.snapshot();
    let expected_current = match &admitted.expected_current {
        GraphStartCurrentTaskV2::Absent => Value::Null,
        GraphStartCurrentTaskV2::Exact {
            session_id,
            session_revision,
        } => json!({
            "session_id": session_id,
            "session_revision": session_revision,
        }),
    };
    let document = json!({
        "command": command.command_name(),
        "execution_version": EXECUTION_DOCUMENT_VERSION_V6,
        "expected_current": expected_current,
        "first_attempt_id": admitted.state.trace().active_attempt().expect("fresh graph state has an active attempt").attempt_id(),
        "selector": admitted.selector,
        "session_id": admitted.state.trace().session_id(),
        "snapshot": {
            "canonical_json": snapshot.canonical_json().as_str(),
            "created_at": snapshot.created_at().get(),
            "digest": snapshot.digest(),
            "snapshot_id": snapshot.snapshot_id(),
            "source_label": snapshot.source().as_str(),
        },
        "task_title": admitted.state.task_title(),
        "workspace_id": admitted.workspace_id,
    });
    let canonical = canonicalize_json_v1(&document).map_err(|_| {
        ExecutionErrorV1::InvalidPersistedExecution {
            reason: "Procedure v2 execution document cannot be canonicalized",
        }
    })?;
    CanonicalExecutionJsonV1::new(canonical).map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn decode_procedure_v2_start_execution_v1(
    source: &str,
) -> Result<AdmittedProcedureV2StartV1, ExecutionErrorV1> {
    let root: Value = serde_json::from_str(source)
        .map_err(|_| invalid_execution_v1("Procedure v2 execution is not JSON"))?;
    let object = root
        .as_object()
        .ok_or_else(|| invalid_execution_v1("Procedure v2 execution root is invalid"))?;
    require_exact_keys_v1(
        object,
        &[
            "command",
            "execution_version",
            "expected_current",
            "first_attempt_id",
            "selector",
            "session_id",
            "snapshot",
            "task_title",
            "workspace_id",
        ],
    )?;
    if value_u64_v1(object, "execution_version")? != u64::from(EXECUTION_DOCUMENT_VERSION_V6) {
        return Err(invalid_execution_v1(
            "Procedure v2 execution version is unsupported",
        ));
    }
    let replace = match value_string_v1(object, "command")? {
        "session.start" => false,
        "session.start_replace" => true,
        _ => {
            return Err(invalid_execution_v1(
                "Procedure v2 execution command is invalid",
            ));
        }
    };
    let snapshot = value_object_v1(object, "snapshot")?;
    require_exact_keys_v1(
        snapshot,
        &[
            "canonical_json",
            "created_at",
            "digest",
            "snapshot_id",
            "source_label",
        ],
    )?;
    let snapshot = ProcedureSnapshotV2::new(
        value_typed_v1(snapshot, "snapshot_id")?,
        CanonicalProcedureJsonV1::new(value_string_v1(snapshot, "canonical_json")?.to_owned())
            .map_err(ExecutionErrorV1::BoundaryDomain)?,
        value_typed_v1(snapshot, "digest")?,
        ProcedureSourceLabelV1::new(value_string_v1(snapshot, "source_label")?.to_owned())
            .map_err(ExecutionErrorV1::BoundaryDomain)?,
        UnixMillis::new(value_u64_v1(snapshot, "created_at")?),
    )
    .map_err(ExecutionErrorV1::InvalidStoreValue)?;
    let expected_current = match value_v1(object, "expected_current")? {
        Value::Null => GraphStartCurrentTaskV2::Absent,
        Value::Object(expected) => {
            require_exact_keys_v1(expected, &["session_id", "session_revision"])?;
            GraphStartCurrentTaskV2::Exact {
                session_id: value_typed_v1(expected, "session_id")?,
                session_revision: value_typed_v1(expected, "session_revision")?,
            }
        }
        _ => {
            return Err(invalid_execution_v1(
                "Procedure v2 current-task fence is invalid",
            ));
        }
    };
    let task_title = value_string_v1(object, "task_title")?;
    let state = graph_session_state_from_procedure_v2_snapshot(
        snapshot,
        task_title,
        value_typed_v1(object, "session_id")?,
        value_typed_v1(object, "first_attempt_id")?,
        UnixMillis::new(value_u64_v1(
            value_object_v1(object, "snapshot")?,
            "created_at",
        )?),
    )
    .map_err(|_| invalid_execution_v1("Procedure v2 graph state cannot be reconstructed"))?;
    Ok(AdmittedProcedureV2StartV1 {
        selector: serde_json::from_value(value_v1(object, "selector")?.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 selector is invalid"))?,
        workspace_id: value_typed_v1(object, "workspace_id")?,
        replace,
        expected_current,
        state,
    })
}

#[derive(Clone, Debug)]
struct AdmittedProcedureV2MutationV1 {
    selector: WorktreeSelectorWireV1,
    workspace_id: WorkspaceId,
    command: SliceCommandV1,
    fresh_attempt_id: Option<AttemptId>,
    fresh_blocker_id: Option<BlockerId>,
    attached_artifact: Option<ArtifactValueV1>,
}

fn is_procedure_v2_graph_mutation_v7(command: &SliceCommandV1) -> bool {
    matches!(
        command,
        SliceCommandV1::SessionComplete(_)
            | SliceCommandV1::SessionSkip(_)
            | SliceCommandV1::SessionRetry(_)
            | SliceCommandV1::ItemCheck(_)
            | SliceCommandV1::ItemUncheck(_)
            | SliceCommandV1::ItemSet(_)
            | SliceCommandV1::ItemAdd(_)
            | SliceCommandV1::ItemRemove(_)
            | SliceCommandV1::ItemAttach(_)
            | SliceCommandV1::ItemClear(_)
    )
}

fn is_procedure_v2_graph_mutation_v8(command: &SliceCommandV1) -> bool {
    matches!(
        command,
        SliceCommandV1::SessionBlock(_)
            | SliceCommandV1::SessionUnblock(_)
            | SliceCommandV1::SessionCancel(_)
            | SliceCommandV1::SessionReset(_)
    )
}

fn is_procedure_v2_graph_mutation_v1(command: &SliceCommandV1) -> bool {
    is_procedure_v2_graph_mutation_v7(command) || is_procedure_v2_graph_mutation_v8(command)
}

fn validate_procedure_v2_block_reason_v1(reason: &str) -> Result<(), ExecutionErrorV1> {
    if reason.trim().is_empty() || reason.chars().count() > 1_000 {
        return Err(invalid_execution_v1(
            "Procedure v2 blocker reason is invalid",
        ));
    }
    Ok(())
}

fn procedure_v2_mutation_execution_document_v1(
    admitted: &AdmittedProcedureV2MutationV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let (preconditions, payload) = execution_components_v1(&admitted.command);
    let document = if is_procedure_v2_graph_mutation_v8(&admitted.command) {
        json!({
            "attached_artifact": admitted.attached_artifact.as_ref().map(artifact_value_v1),
            "command": admitted.command.command_name(),
            "execution_version": EXECUTION_DOCUMENT_VERSION_V8,
            "fresh_attempt_id": admitted.fresh_attempt_id,
            "fresh_blocker_id": admitted.fresh_blocker_id,
            "payload": payload,
            "preconditions": preconditions,
            "selector": admitted.selector,
            "workspace_id": admitted.workspace_id,
        })
    } else {
        json!({
            "attached_artifact": admitted.attached_artifact.as_ref().map(artifact_value_v1),
            "command": admitted.command.command_name(),
            "execution_version": EXECUTION_DOCUMENT_VERSION_V7,
            "fresh_attempt_id": admitted.fresh_attempt_id,
            "payload": payload,
            "preconditions": preconditions,
            "selector": admitted.selector,
            "workspace_id": admitted.workspace_id,
        })
    };
    let canonical = canonicalize_json_v1(&document).map_err(|_| {
        invalid_execution_v1("Procedure v2 mutation execution cannot be canonicalized")
    })?;
    CanonicalExecutionJsonV1::new(canonical).map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn decode_procedure_v2_mutation_execution_v1(
    source: &str,
) -> Result<AdmittedProcedureV2MutationV1, ExecutionErrorV1> {
    let root: Value = serde_json::from_str(source)
        .map_err(|_| invalid_execution_v1("Procedure v2 mutation execution is not JSON"))?;
    let object = root
        .as_object()
        .ok_or_else(|| invalid_execution_v1("Procedure v2 mutation execution root is invalid"))?;
    let execution_version = value_u64_v1(object, "execution_version")?;
    if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V7) {
        require_exact_keys_v1(
            object,
            &[
                "attached_artifact",
                "command",
                "execution_version",
                "fresh_attempt_id",
                "payload",
                "preconditions",
                "selector",
                "workspace_id",
            ],
        )?;
    } else if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V8) {
        require_exact_keys_v1(
            object,
            &[
                "attached_artifact",
                "command",
                "execution_version",
                "fresh_attempt_id",
                "fresh_blocker_id",
                "payload",
                "preconditions",
                "selector",
                "workspace_id",
            ],
        )?;
    } else {
        return Err(invalid_execution_v1(
            "Procedure v2 mutation execution version is unsupported",
        ));
    }
    let command = decode_command_components_v1(
        u64::from(EXECUTION_DOCUMENT_VERSION_V5),
        value_string_v1(object, "command")?,
        value_object_v1(object, "preconditions")?,
        value_object_v1(object, "payload")?,
    )?;
    if (execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V7)
        && !is_procedure_v2_graph_mutation_v7(&command))
        || (execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V8)
            && !is_procedure_v2_graph_mutation_v8(&command))
    {
        return Err(invalid_execution_v1(
            "Procedure v2 mutation command is outside the admitted set",
        ));
    }
    if let SliceCommandV1::SessionRetry(input) = &command {
        ReasonV2::new(input.reason.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 retry reason is invalid"))?;
    }
    if let SliceCommandV1::SessionSkip(input) = &command
        && let Some(reason) = &input.reason
    {
        ReasonV2::new(reason.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 skip reason is invalid"))?;
    }
    if let SliceCommandV1::SessionBlock(input) = &command {
        validate_procedure_v2_block_reason_v1(&input.reason)?;
    }
    if let SliceCommandV1::SessionCancel(input) = &command {
        ReasonV2::new(input.reason.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 cancel reason is invalid"))?;
    }
    if let SliceCommandV1::SessionReset(input) = &command
        && (input.dry_run || !input.confirmed)
    {
        return Err(invalid_execution_v1(
            "Procedure v2 durable reset is invalid",
        ));
    }
    let fresh_attempt_id = value_optional_typed_v1(object, "fresh_attempt_id")?;
    let fresh_blocker_id = if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V8) {
        value_optional_typed_v1(object, "fresh_blocker_id")?
    } else {
        None
    };
    let attached_artifact = match value_v1(object, "attached_artifact")? {
        Value::Null => None,
        Value::Object(value) => Some(decode_artifact_value_v1(value)?),
        _ => {
            return Err(invalid_execution_v1(
                "Procedure v2 attached artifact resolution is invalid",
            ));
        }
    };
    match &command {
        SliceCommandV1::SessionComplete(_) if attached_artifact.is_none() => {}
        SliceCommandV1::SessionSkip(_) if attached_artifact.is_none() => {}
        SliceCommandV1::SessionRetry(_)
            if attached_artifact.is_none() && fresh_attempt_id.is_some() => {}
        SliceCommandV1::SessionBlock(_)
            if attached_artifact.is_none()
                && fresh_attempt_id.is_none()
                && fresh_blocker_id.is_some() => {}
        SliceCommandV1::SessionUnblock(_)
        | SliceCommandV1::SessionCancel(_)
        | SliceCommandV1::SessionReset(_)
            if attached_artifact.is_none()
                && fresh_attempt_id.is_none()
                && fresh_blocker_id.is_none() => {}
        SliceCommandV1::ItemAttach(_)
            if attached_artifact.is_some() && fresh_attempt_id.is_none() => {}
        SliceCommandV1::ItemCheck(_)
        | SliceCommandV1::ItemUncheck(_)
        | SliceCommandV1::ItemSet(_)
        | SliceCommandV1::ItemAdd(_)
        | SliceCommandV1::ItemRemove(_)
        | SliceCommandV1::ItemClear(_)
            if attached_artifact.is_none() && fresh_attempt_id.is_none() => {}
        _ => {
            return Err(invalid_execution_v1(
                "Procedure v2 mutation resolution does not match the command",
            ));
        }
    }
    Ok(AdmittedProcedureV2MutationV1 {
        selector: serde_json::from_value(value_v1(object, "selector")?.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 mutation selector is invalid"))?,
        workspace_id: value_typed_v1(object, "workspace_id")?,
        command,
        fresh_attempt_id,
        fresh_blocker_id,
        attached_artifact,
    })
}

#[derive(Clone, Debug)]
struct AdmittedProcedureV2DecisionV1 {
    selector: WorktreeSelectorWireV1,
    workspace_id: WorkspaceId,
    command: SessionDecideV2,
    fresh_attempt_id: AttemptId,
}

fn procedure_v2_decision_execution_document_v1(
    admitted: &AdmittedProcedureV2DecisionV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let document = json!({
        "command": "session.decide",
        "execution_version": EXECUTION_DOCUMENT_VERSION_V9,
        "fresh_attempt_id": admitted.fresh_attempt_id,
        "payload": {
            "actor": admitted.command.actor,
            "option_id": admitted.command.option_id,
            "reason": admitted.command.reason,
        },
        "preconditions": {
            "attempt_id": admitted.command.preconditions.expected_attempt_id,
            "session_id": admitted.command.preconditions.expected_session_id,
            "session_revision": admitted.command.preconditions.expected_session_revision,
        },
        "selector": admitted.selector,
        "workspace_id": admitted.workspace_id,
    });
    let canonical = canonicalize_json_v1(&document).map_err(|_| {
        invalid_execution_v1("Procedure v2 decision execution cannot be canonicalized")
    })?;
    CanonicalExecutionJsonV1::new(canonical).map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn decode_procedure_v2_decision_execution_v1(
    source: &str,
) -> Result<AdmittedProcedureV2DecisionV1, ExecutionErrorV1> {
    let root: Value = serde_json::from_str(source)
        .map_err(|_| invalid_execution_v1("Procedure v2 decision execution is not JSON"))?;
    let object = root
        .as_object()
        .ok_or_else(|| invalid_execution_v1("Procedure v2 decision execution root is invalid"))?;
    require_exact_keys_v1(
        object,
        &[
            "command",
            "execution_version",
            "fresh_attempt_id",
            "payload",
            "preconditions",
            "selector",
            "workspace_id",
        ],
    )?;
    if value_u64_v1(object, "execution_version")? != u64::from(EXECUTION_DOCUMENT_VERSION_V9)
        || value_string_v1(object, "command")? != "session.decide"
    {
        return Err(invalid_execution_v1(
            "Procedure v2 decision execution identity is invalid",
        ));
    }
    let payload = value_object_v1(object, "payload")?;
    require_exact_keys_v1(payload, &["actor", "option_id", "reason"])?;
    let preconditions = value_object_v1(object, "preconditions")?;
    require_exact_keys_v1(
        preconditions,
        &["attempt_id", "session_id", "session_revision"],
    )?;
    let command = SessionDecideV2 {
        option_id: value_typed_v1(payload, "option_id")?,
        reason: value_string_v1(payload, "reason")?.to_owned(),
        actor: value_optional_string_v1(payload, "actor")?,
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: value_typed_v1(preconditions, "session_id")?,
            expected_session_revision: Revision::new(value_u64_v1(
                preconditions,
                "session_revision",
            )?),
            expected_attempt_id: value_typed_v1(preconditions, "attempt_id")?,
        },
    };
    ReasonV2::new(command.reason.clone())
        .map_err(|_| invalid_execution_v1("Procedure v2 decision reason is invalid"))?;
    command
        .actor
        .clone()
        .map(ActorAttributionV2::new)
        .transpose()
        .map_err(|_| invalid_execution_v1("Procedure v2 decision actor is invalid"))?;
    Ok(AdmittedProcedureV2DecisionV1 {
        selector: serde_json::from_value(value_v1(object, "selector")?.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 decision selector is invalid"))?,
        workspace_id: value_typed_v1(object, "workspace_id")?,
        command,
        fresh_attempt_id: value_typed_v1(object, "fresh_attempt_id")?,
    })
}

#[derive(Clone, Debug)]
struct AdmittedProcedureV2ReworkV1 {
    selector: WorktreeSelectorWireV1,
    workspace_id: WorkspaceId,
    command: SessionReworkV2,
    fresh_attempt_id: AttemptId,
}

fn procedure_v2_rework_execution_document_v1(
    admitted: &AdmittedProcedureV2ReworkV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let document = json!({
        "command": "session.rework",
        "execution_version": EXECUTION_DOCUMENT_VERSION_V10,
        "fresh_attempt_id": admitted.fresh_attempt_id,
        "payload": {
            "actor": admitted.command.actor,
            "reason": admitted.command.reason,
            "target_graph_node_id": admitted.command.target_graph_node_id,
        },
        "preconditions": {
            "attempt_id": admitted.command.preconditions.expected_attempt_id,
            "session_id": admitted.command.preconditions.expected_session_id,
            "session_revision": admitted.command.preconditions.expected_session_revision,
        },
        "selector": admitted.selector,
        "workspace_id": admitted.workspace_id,
    });
    let canonical = canonicalize_json_v1(&document).map_err(|_| {
        invalid_execution_v1("Procedure v2 rework execution cannot be canonicalized")
    })?;
    CanonicalExecutionJsonV1::new(canonical).map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn decode_procedure_v2_rework_execution_v1(
    source: &str,
) -> Result<AdmittedProcedureV2ReworkV1, ExecutionErrorV1> {
    let root: Value = serde_json::from_str(source)
        .map_err(|_| invalid_execution_v1("Procedure v2 rework execution is not JSON"))?;
    let object = root
        .as_object()
        .ok_or_else(|| invalid_execution_v1("Procedure v2 rework execution root is invalid"))?;
    require_exact_keys_v1(
        object,
        &[
            "command",
            "execution_version",
            "fresh_attempt_id",
            "payload",
            "preconditions",
            "selector",
            "workspace_id",
        ],
    )?;
    if value_u64_v1(object, "execution_version")? != u64::from(EXECUTION_DOCUMENT_VERSION_V10)
        || value_string_v1(object, "command")? != "session.rework"
    {
        return Err(invalid_execution_v1(
            "Procedure v2 rework execution identity is invalid",
        ));
    }
    let payload = value_object_v1(object, "payload")?;
    require_exact_keys_v1(payload, &["actor", "reason", "target_graph_node_id"])?;
    let preconditions = value_object_v1(object, "preconditions")?;
    require_exact_keys_v1(
        preconditions,
        &["attempt_id", "session_id", "session_revision"],
    )?;
    let command = SessionReworkV2 {
        target_graph_node_id: value_typed_v1(payload, "target_graph_node_id")?,
        reason: value_string_v1(payload, "reason")?.to_owned(),
        actor: value_optional_string_v1(payload, "actor")?,
        preconditions: podway_protocol::ReworkPreconditionsWireV2 {
            expected_session_id: value_typed_v1(preconditions, "session_id")?,
            expected_session_revision: Revision::new(value_u64_v1(
                preconditions,
                "session_revision",
            )?),
            expected_attempt_id: value_optional_typed_v1(preconditions, "attempt_id")?,
        },
    };
    ReasonV2::new(command.reason.clone())
        .map_err(|_| invalid_execution_v1("Procedure v2 rework reason is invalid"))?;
    command
        .actor
        .clone()
        .map(ActorAttributionV2::new)
        .transpose()
        .map_err(|_| invalid_execution_v1("Procedure v2 rework actor is invalid"))?;
    Ok(AdmittedProcedureV2ReworkV1 {
        selector: serde_json::from_value(value_v1(object, "selector")?.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 rework selector is invalid"))?,
        workspace_id: value_typed_v1(object, "workspace_id")?,
        command,
        fresh_attempt_id: value_typed_v1(object, "fresh_attempt_id")?,
    })
}

fn rework_record_projection_v1(record: &podway_core::ReworkRecordV2) -> Value {
    let mut value = json!({
        "trace_sequence": record.trace().get(),
        "kind": record.kind().as_str(),
        "from_graph_node_id": record.from_node(),
        "to_graph_node_id": record.to_node(),
        "target_attempt_id": record.target_attempt_id(),
        "reason": record.reason().as_str(),
        "reactivated": record.reactivated(),
        "recorded_at_ms": record.recorded_at().get(),
    });
    if let Some(actor) = record.actor() {
        value
            .as_object_mut()
            .expect("rework record projection is an object")
            .insert("actor".to_owned(), json!(actor.as_str()));
    }
    value
}

fn decision_record_projection_v1(record: &DecisionRecordV2) -> Result<Value, ExecutionErrorV1> {
    let references = record
        .evidence()
        .references()
        .iter()
        .map(|reference| {
            let mut value = json!({
                "source_graph_node_id": reference.source_node(),
            });
            let object = value
                .as_object_mut()
                .expect("evidence reference projection is an object");
            match reference {
                ResolvedEvidenceReferenceV2::Unresolved { .. } => {
                    object.insert("state".to_owned(), json!("unresolved"));
                }
                ResolvedEvidenceReferenceV2::Resolved(snapshot)
                | ResolvedEvidenceReferenceV2::Skipped(snapshot) => {
                    object.insert(
                        "state".to_owned(),
                        json!(
                            if matches!(reference, ResolvedEvidenceReferenceV2::Skipped(_)) {
                                "skipped"
                            } else {
                                "resolved"
                            }
                        ),
                    );
                    object.insert(
                        "source_attempt_id".to_owned(),
                        json!(snapshot.source_attempt_id()),
                    );
                    object.insert(
                        "source_attempt_number".to_owned(),
                        json!(snapshot.source_attempt_number().get()),
                    );
                    object.insert("items_digest".to_owned(), json!(snapshot.items_digest()));
                }
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>, ExecutionErrorV1>>()?;
    let mut value = json!({
        "trace_sequence": record.trace().get(),
        "session_id": record.session_id(),
        "session_revision": record.session_revision().get(),
        "procedure_schema": "podway.procedure/v2",
        "procedure_snapshot_id": record.procedure_snapshot_id(),
        "procedure_digest": record.procedure_digest(),
        "graph_node_id": record.graph_node_id(),
        "node_definition_id": record.node_definition_id(),
        "attempt_id": record.attempt_id(),
        "attempt_number": record.attempt_number().get(),
        "goal_revision": record.goal_revision().map(|revision| revision.get()),
        "option_id": record.selected_option(),
        "effect": record.route_effect().as_str(),
        "target_graph_node_id": record.route_target(),
        "reason": record.reason().as_str(),
        "recorded_at": rfc3339_millis_execution_v1(record.recorded_at())?,
        "references": references,
    });
    if let Some(actor) = record.actor() {
        value
            .as_object_mut()
            .expect("decision record projection is an object")
            .insert("actor".to_owned(), json!(actor.as_str()));
    }
    Ok(value)
}

fn rfc3339_millis_execution_v1(value: UnixMillis) -> Result<String, ExecutionErrorV1> {
    let seconds = value.get() / 1_000;
    let millis = value.get() % 1_000;
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let z = i128::from(days) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let timestamp =
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z");
    Rfc3339MillisV1::new(timestamp.clone())
        .map(|_| timestamp)
        .map_err(|_| invalid_execution_v1("Procedure v2 decision timestamp is invalid"))
}

fn artifact_value_v1(artifact: &ArtifactValueV1) -> Value {
    let location_kind = match artifact.location_kind() {
        ArtifactLocationKindV1::LocalPath => "local_path",
        ArtifactLocationKindV1::ExternalReference => "external_reference",
    };
    json!({
        "digest": artifact.digest(),
        "location": artifact.location(),
        "location_kind": location_kind,
        "media_type": artifact.media_type(),
        "size_bytes": artifact.size_bytes(),
    })
}

fn decode_artifact_value_v1(
    object: &Map<String, Value>,
) -> Result<ArtifactValueV1, ExecutionErrorV1> {
    require_exact_keys_v1(
        object,
        &[
            "digest",
            "location",
            "location_kind",
            "media_type",
            "size_bytes",
        ],
    )?;
    let location = value_string_v1(object, "location")?.to_owned();
    let digest = value_typed_v1(object, "digest")?;
    let size_bytes = value_u64_v1(object, "size_bytes")?;
    let media_type = value_string_v1(object, "media_type")?.to_owned();
    match value_string_v1(object, "location_kind")? {
        "local_path" => ArtifactValueV1::local_path(location, digest, size_bytes, media_type),
        "external_reference" => {
            ArtifactValueV1::external_reference(location, digest, size_bytes, media_type)
        }
        _ => {
            return Err(invalid_execution_v1(
                "Procedure v2 artifact location kind is invalid",
            ));
        }
    }
    .map_err(ExecutionErrorV1::BoundaryDomain)
}

fn rehydrate_procedure_v2_execution_v1(
    state: &GraphSessionStateV2,
) -> Result<ParsedProcedureV2, ExecutionErrorV1> {
    let parsed = parse_procedure_document(
        state.snapshot().canonical_json().as_str().as_bytes(),
        ProcedureDocumentFormat::Json,
    )
    .map_err(|_| invalid_execution_v1("Procedure v2 snapshot cannot be parsed"))?;
    let ParsedProcedure::V2(parsed) = parsed else {
        return Err(invalid_execution_v1(
            "Procedure v2 snapshot has an invalid schema",
        ));
    };
    let validated = validate_procedure_v2(parsed)
        .map_err(|_| invalid_execution_v1("Procedure v2 snapshot cannot be validated"))?;
    if validated.digest() != state.snapshot().digest()
        || validated.canonical_json().as_str() != state.snapshot().canonical_json().as_str()
    {
        return Err(invalid_execution_v1(
            "Procedure v2 snapshot identity is inconsistent",
        ));
    }
    Ok(validated.parsed().clone())
}

fn fresh_successor_attempt_id_v1<Ids: ExecutionIdSourceV1>(
    state: &GraphSessionStateV2,
    ids: &Ids,
) -> Result<Option<AttemptId>, ExecutionErrorV1> {
    let Some(active) = state.trace().active_attempt() else {
        return Ok(None);
    };
    let procedure = rehydrate_procedure_v2_execution_v1(state)?;
    Ok(match procedure.graph().placement(active.graph_node_id()) {
        Some(GraphPlacementV2::Action(action)) if !action.outcome().is_terminal() => {
            Some(ids.next_attempt_id())
        }
        Some(GraphPlacementV2::Action(_) | GraphPlacementV2::Decision(_)) => None,
        None => {
            return Err(invalid_execution_v1(
                "Procedure v2 active graph placement is absent",
            ));
        }
    })
}

fn required_local_artifacts_v2(
    state: &GraphSessionStateV2,
) -> Result<Vec<(ItemId, ArtifactValueV1)>, ExecutionErrorV1> {
    let Some(active) = state.trace().active_attempt() else {
        return Ok(Vec::new());
    };
    let procedure = rehydrate_procedure_v2_execution_v1(state)?;
    let placement = procedure
        .graph()
        .placement(active.graph_node_id())
        .ok_or_else(|| invalid_execution_v1("Procedure v2 active graph placement is absent"))?;
    let definition_id = match placement {
        GraphPlacementV2::Action(placement) => placement.definition(),
        GraphPlacementV2::Decision(placement) => placement.definition(),
    };
    let items = procedure
        .node_definitions()
        .iter()
        .find_map(|definition| match definition {
            ParsedNodeDefinition::Action(definition) if definition.id() == definition_id => {
                Some(definition.items())
            }
            ParsedNodeDefinition::Decision(definition) if definition.id() == definition_id => {
                Some(definition.items())
            }
            ParsedNodeDefinition::Action(_) | ParsedNodeDefinition::Decision(_) => None,
        })
        .ok_or_else(|| invalid_execution_v1("Procedure v2 active definition is absent"))?;
    let memory = state
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == active.attempt_id())
        .ok_or_else(|| invalid_execution_v1("Procedure v2 active workflow memory is absent"))?;
    if items.len() != memory.item_slots().len()
        || items
            .iter()
            .zip(memory.item_slots())
            .any(|(specification, slot)| {
                specification.id() != slot.item_id()
                    || specification.item_type() != slot.item_type()
            })
    {
        return Err(invalid_execution_v1(
            "Procedure v2 active items disagree with the snapshot",
        ));
    }
    Ok(items
        .iter()
        .zip(memory.item_slots())
        .filter_map(|(specification, slot)| {
            let artifact = slot.value().and_then(|value| value.as_artifact())?;
            (specification.common().required()
                && artifact.location_kind() == ArtifactLocationKindV1::LocalPath)
                .then(|| (slot.item_id().clone(), artifact.clone()))
        })
        .collect())
}

#[derive(Clone, Debug)]
struct PreparedProcedureV2ItemMutationV1 {
    item_id: ItemId,
    expected_attempt_id: AttemptId,
    expected_item_revision: Revision,
    mutation: ActiveItemMutationV2,
}

fn prepare_procedure_v2_item_mutation_v1(
    admitted: &AdmittedProcedureV2MutationV1,
) -> Result<PreparedProcedureV2ItemMutationV1, ExecutionErrorV1> {
    let (item_id, preconditions, mutation) = match &admitted.command {
        SliceCommandV1::ItemCheck(input) => (
            &input.item_id,
            &input.preconditions,
            ActiveItemMutationV2::Check,
        ),
        SliceCommandV1::ItemUncheck(input) => (
            &input.item_id,
            &input.preconditions,
            ActiveItemMutationV2::Uncheck,
        ),
        SliceCommandV1::ItemSet(input) => (
            &input.item_id,
            &input.preconditions,
            ActiveItemMutationV2::Set {
                value: input.value.clone(),
            },
        ),
        SliceCommandV1::ItemAdd(input) => (
            &input.item_id,
            &input.preconditions,
            ActiveItemMutationV2::Add {
                value: input.value.clone(),
            },
        ),
        SliceCommandV1::ItemRemove(input) => (
            &input.item_id,
            &input.preconditions,
            ActiveItemMutationV2::Remove {
                value: input.value.clone(),
                ignore_missing: input.ignore_missing,
            },
        ),
        SliceCommandV1::ItemAttach(input) => (
            &input.item_id,
            &input.preconditions,
            ActiveItemMutationV2::Attach {
                value: admitted.attached_artifact.clone().ok_or_else(|| {
                    invalid_execution_v1("Procedure v2 attachment resolution is absent")
                })?,
            },
        ),
        SliceCommandV1::ItemClear(input) => (
            &input.item_id,
            &input.preconditions,
            ActiveItemMutationV2::Clear,
        ),
        _ => {
            return Err(invalid_execution_v1(
                "Procedure v2 item preparation received another command",
            ));
        }
    };
    Ok(PreparedProcedureV2ItemMutationV1 {
        item_id: item_id.clone(),
        expected_attempt_id: preconditions.expected_attempt_id.clone(),
        expected_item_revision: preconditions.expected_item_revision,
        mutation,
    })
}

fn procedure_format_v1(procedure: &str) -> Result<ProcedureFormatV1, ExecutionBoundaryErrorV1> {
    match std::path::Path::new(procedure)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("json") => Ok(ProcedureFormatV1::Json),
        Some("yaml" | "yml") => Ok(ProcedureFormatV1::Yaml),
        _ => Err(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "workspace procedure source has an unsupported extension",
            },
        )),
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
    UnsupportedV2Capability {
        capability: &'static str,
        required_result_schema: &'static str,
    },
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
            Self::UnsupportedV2Capability { capability, .. } => {
                write!(formatter, "v2 capability is not enabled: {capability}")
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
            Self::UnsupportedV2Capability { .. }
            | Self::BoundaryTransient { .. }
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
    Existing(Box<AdmitOutcomeV1>),
    New(Box<PreparedWorkspaceResetAllV1>),
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
    /// Resolves a Procedure v2 start without admitting a job. This is the complete dry-run
    /// boundary: source bytes are validated and digest-confirmed, but no Store mutation occurs.
    pub fn prepare_procedure_v2_start_for_workspace(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &SliceRequestV1,
    ) -> Result<Option<AdmittedProcedureV2StartProjectionV1>, ProcedureV2StartPreparationErrorV1>
    {
        let start = match request.command() {
            SliceCommandV1::SessionStart(start) => start,
            SliceCommandV1::SessionStartReplace(replace) => &replace.start,
            _ => return Ok(None),
        };
        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
        if binding.identity() != expected_workspace.identity() {
            return Err(ProcedureV2StartPreparationErrorV1::Execution(
                invalid_execution_v1("dry-run workspace does not match the scheduler identity"),
            ));
        }
        let now = self.clock.now();
        let session_id = self.ids.next_session_id();
        let attempt_id = self.ids.next_attempt_id();
        let snapshot_id = self.ids.next_procedure_snapshot_id();
        let state = match &start.source {
            SessionStartSourceV1::Procedure { procedure } => prepare_custom_procedure_v2_start(
                &self.procedures,
                &binding,
                procedure,
                start.expected_procedure_digest.as_ref(),
                &start.task_title,
                session_id,
                attempt_id,
                snapshot_id,
                now,
            )?,
            SessionStartSourceV1::Preset { preset } => {
                let Some((snapshot, pinned_digest)) = self
                    .procedures
                    .load_preset_snapshot_v2(preset, snapshot_id, now)
                    .map_err(ProcedureV2StartPreparationErrorV1::Source)?
                else {
                    return Ok(None);
                };
                verify_pinned_procedure_v2_snapshot(&snapshot, &pinned_digest)?;
                graph_session_state_from_procedure_v2_snapshot(
                    snapshot,
                    &start.task_title,
                    session_id,
                    attempt_id,
                    now,
                )?
            }
        };
        Ok(Some(AdmittedProcedureV2StartProjectionV1 {
            procedure_digest: state.snapshot().digest().clone(),
            session_id: state.trace().session_id().clone(),
            revision: state.trace().revision(),
            entry_graph_node_id: state.snapshot().entry_graph_node_id().clone(),
            goal_tracking: state.snapshot().goal_tracking(),
        }))
    }

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
            None,
            ResetAllStoreAuthorityV1::readable(&self.store),
        )
    }

    pub fn prepare_workspace_reset_all_with_response_request_id(
        &self,
        request: &SliceRequestV1,
        previous_workspace: &DurableWorktreeIdentityV1,
        idempotency_key: IdempotencyKeyV1,
        response_request_id: RequestIdV1,
    ) -> Result<ResetAllPreparationOutcomeV1, ExecutionErrorV1> {
        self.prepare_workspace_reset_all_with_authority(
            request,
            previous_workspace,
            idempotency_key,
            Some(response_request_id),
            ResetAllStoreAuthorityV1::readable(&self.store),
        )
    }

    /// The manager-only recovery path permits no-Store preparation only with an unavailable proof
    /// issued for the exact source identity passed to this engine.
    pub(crate) fn prepare_workspace_reset_all_with_unavailable_store_and_response_request_id(
        &self,
        request: &SliceRequestV1,
        previous_workspace: &DurableWorktreeIdentityV1,
        idempotency_key: IdempotencyKeyV1,
        response_request_id: RequestIdV1,
        proof: ValidatedUnavailableStoreV1,
    ) -> Result<ResetAllPreparationOutcomeV1, ExecutionErrorV1> {
        self.prepare_workspace_reset_all_with_authority(
            request,
            previous_workspace,
            idempotency_key,
            Some(response_request_id),
            ResetAllStoreAuthorityV1::validated_unavailable(proof),
        )
    }

    fn prepare_workspace_reset_all_with_authority(
        &self,
        request: &SliceRequestV1,
        previous_workspace: &DurableWorktreeIdentityV1,
        idempotency_key: IdempotencyKeyV1,
        response_request_id: Option<RequestIdV1>,
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
                    return Ok(ResetAllPreparationOutcomeV1::Existing(Box::new(outcome)));
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
        let marker = match response_request_id {
            Some(response_request_id) => ResetMarkerV1::new_with_response_request_id(
                operation_id,
                idempotency_key,
                request_digest,
                previous_workspace.workspace_uuid().clone(),
                target_workspace_uuid,
                self.clock.now(),
                response_request_id,
            ),
            None => ResetMarkerV1::new(
                operation_id,
                idempotency_key,
                request_digest,
                previous_workspace.workspace_uuid().clone(),
                target_workspace_uuid,
                self.clock.now(),
            ),
        };
        Ok(ResetAllPreparationOutcomeV1::New(Box::new(
            PreparedWorkspaceResetAllV1 {
                marker,
                previous_workspace_uuid: previous_workspace.workspace_uuid().clone(),
                source: previous_workspace.clone(),
            },
        )))
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
        let result = self.admit_with_expected_workspace(None, request, idempotency_key, None);
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
        let result = self.admit_with_expected_workspace(
            Some(expected_workspace),
            request,
            idempotency_key,
            None,
        );
        self.emit_admission_result(&result);
        result
    }

    pub fn admit_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1> {
        let result = self.admit_with_expected_workspace(
            Some(expected_workspace),
            request,
            idempotency_key,
            response_context,
        );
        self.emit_admission_result(&result);
        result
    }

    fn admit_with_expected_workspace(
        &self,
        expected_workspace: Option<&WorkspaceBindingV1>,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
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
        let admitted = match response_context {
            Some(context) => admitted.with_response_context(context),
            None => admitted,
        };
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
                self.resolve_session_start(input, workspace, now, "session.start")
            }
            SliceCommandV1::SessionStartReplace(input) => {
                self.resolve_session_start(&input.start, workspace, now, "session.start_replace")
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
            | SliceCommandV1::JobLookup(_)
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
        capability: &'static str,
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
        .map_err(|error| match error {
            ExecutionBoundaryErrorV1::ProcedureV2Unsupported => {
                ExecutionErrorV1::UnsupportedV2Capability {
                    capability,
                    required_result_schema: "podway.session-start-result/v2",
                }
            }
            other => ExecutionErrorV1::from_boundary(other.into()),
        })?;
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
            | SliceCommandV1::JobLookup(_)
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

impl<Store, Ids, Clock, Procedures, Artifacts, Workspaces>
    DaemonExecutionEngineV1<Store, Ids, Clock, Procedures, Artifacts, Workspaces>
where
    Store: StoreContractV1
        + StoreIdempotencyReadContractV1
        + StoreGraphMutationContractV2
        + StoreGraphReadContractV2,
    Ids: ExecutionIdSourceV1,
    Clock: ExecutionClockV1,
    Procedures: ProcedureProviderV1,
    Artifacts: ArtifactVerifierV1,
    Workspaces: WorkspaceRevalidatorV1,
{
    /// Attempts the development-gated Procedure v2 start path. `None` means the source is not a
    /// Procedure v2 document and the unchanged v1 admission path may inspect it.
    pub fn admit_procedure_v2_start_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ProcedureV2StartPreparationErrorV1> {
        let (start, expected_current) = match request.command() {
            SliceCommandV1::SessionStart(start) => (start, GraphStartCurrentTaskV2::Absent),
            SliceCommandV1::SessionStartReplace(replace) => (
                &replace.start,
                GraphStartCurrentTaskV2::Exact {
                    session_id: replace.preconditions.expected_session_id.clone(),
                    session_revision: replace.preconditions.expected_session_revision,
                },
            ),
            _ => return Ok(None),
        };
        if let Some(existing) = self
            .store
            .read_idempotent_execution(expected_workspace.identity(), &idempotency_key)
            .map_err(ExecutionErrorV1::from)
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?
        {
            let Some(canonical_execution) = existing.canonical_execution() else {
                return Ok(None);
            };
            let version = serde_json::from_str::<Value>(canonical_execution.as_str())
                .ok()
                .and_then(|value| value.get("execution_version").and_then(Value::as_u64));
            if version != Some(u64::from(EXECUTION_DOCUMENT_VERSION_V6)) {
                return Ok(None);
            }
            let admitted = decode_procedure_v2_start_execution_v1(canonical_execution.as_str())
                .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
            if admitted.workspace_id != *expected_workspace.identity().workspace_uuid() {
                return Err(ProcedureV2StartPreparationErrorV1::Execution(
                    invalid_execution_v1("Procedure v2 replay workspace identity is invalid"),
                ));
            }
            let actual = request_digest_v1(
                request,
                expected_workspace.identity().workspace_uuid(),
                Some(admitted.state.snapshot().digest()),
            )
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
            if existing.request_digest() != &actual {
                return Err(ProcedureV2StartPreparationErrorV1::Execution(
                    ExecutionErrorV1::Store(StoreErrorV1::IdempotencyDigestConflictV1 {
                        expected: existing.request_digest().clone(),
                        actual,
                    }),
                ));
            }
            return Ok(Some(existing.outcome().clone()));
        }

        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
        if binding.identity() != expected_workspace.identity() {
            return Err(ProcedureV2StartPreparationErrorV1::Execution(
                ExecutionErrorV1::BoundaryDomain(DomainError::InvalidState {
                    reason: "revalidated workspace does not match the scheduler identity",
                }),
            ));
        }
        let now = self.clock.now();
        let session_id = self.ids.next_session_id();
        let first_attempt_id = self.ids.next_attempt_id();
        let snapshot_id = self.ids.next_procedure_snapshot_id();
        let state = match &start.source {
            SessionStartSourceV1::Procedure { procedure } => {
                match prepare_custom_procedure_v2_start(
                    &self.procedures,
                    &binding,
                    procedure,
                    start.expected_procedure_digest.as_ref(),
                    &start.task_title,
                    session_id,
                    first_attempt_id,
                    snapshot_id,
                    now,
                ) {
                    Ok(state) => state,
                    Err(ProcedureV2StartPreparationErrorV1::Source(
                        ProcedureV2SourceAdmissionErrorV1::NotProcedureV2,
                    )) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            SessionStartSourceV1::Preset { preset } => {
                let Some((snapshot, pinned_digest)) = self
                    .procedures
                    .load_preset_snapshot_v2(preset, snapshot_id, now)
                    .map_err(ProcedureV2StartPreparationErrorV1::Source)?
                else {
                    return Ok(None);
                };
                verify_pinned_procedure_v2_snapshot(&snapshot, &pinned_digest)?;
                graph_session_state_from_procedure_v2_snapshot(
                    snapshot,
                    &start.task_title,
                    session_id,
                    first_attempt_id,
                    now,
                )?
            }
        };
        let admitted = AdmittedProcedureV2StartV1 {
            selector: request.selector().clone(),
            workspace_id: binding.identity().workspace_uuid().clone(),
            replace: matches!(request.command(), SliceCommandV1::SessionStartReplace(_)),
            expected_current,
            state,
        };
        let canonical_execution =
            procedure_v2_start_execution_document_v1(&admitted, request.command())
                .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
        let request_digest = request_digest_v1(
            request,
            binding.identity().workspace_uuid(),
            Some(admitted.state.snapshot().digest()),
        )
        .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
        let durable = AdmitRequestV1::new_with_canonical_execution(
            command_for_admission_v1(request.command())
                .map_err(ProcedureV2StartPreparationErrorV1::Execution)?,
            idempotency_key,
            self.ids.next_job_id(),
            store_preconditions_v1(request.command())
                .map_err(ProcedureV2StartPreparationErrorV1::Execution)?,
            request_digest,
            now,
            canonical_execution,
        )
        .with_procedure_v2_execution()
        .with_session_identity(admission_session_identity_v1(request.command()));
        let durable = match response_context {
            Some(context) => durable.with_response_context(context),
            None => durable,
        };
        self.store
            .admit(binding.identity(), durable)
            .map(Some)
            .map_err(ExecutionErrorV1::from)
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)
    }

    /// Attempts the Procedure v2 complete, skip, retry, or item path. The immutable execution
    /// document freezes all generated IDs and attachment metadata before admission; an absent
    /// graph task preserves the unchanged v1 fallback.
    pub fn admit_procedure_v2_mutation_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        if !is_procedure_v2_graph_mutation_v1(request.command()) {
            return Ok(None);
        }
        if let Some(existing) = self
            .store
            .read_idempotent_execution(expected_workspace.identity(), &idempotency_key)?
        {
            let Some(canonical_execution) = existing.canonical_execution() else {
                return Ok(None);
            };
            let version = serde_json::from_str::<Value>(canonical_execution.as_str())
                .ok()
                .and_then(|value| value.get("execution_version").and_then(Value::as_u64));
            if !matches!(
                version,
                Some(version)
                    if version == u64::from(EXECUTION_DOCUMENT_VERSION_V7)
                        || version == u64::from(EXECUTION_DOCUMENT_VERSION_V8)
            ) {
                return Ok(None);
            }
            let admitted = decode_procedure_v2_mutation_execution_v1(canonical_execution.as_str())?;
            if admitted.workspace_id != *expected_workspace.identity().workspace_uuid() {
                return Err(invalid_execution_v1(
                    "Procedure v2 mutation replay workspace identity is invalid",
                ));
            }
            let actual = request_digest_v1(
                request,
                expected_workspace.identity().workspace_uuid(),
                None,
            )?;
            if existing.request_digest() != &actual {
                return Err(StoreErrorV1::IdempotencyDigestConflictV1 {
                    expected: existing.request_digest().clone(),
                    actual,
                }
                .into());
            }
            return Ok(Some(existing.outcome().clone()));
        }

        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)?;
        if binding.identity() != expected_workspace.identity() {
            return Err(ExecutionErrorV1::BoundaryDomain(
                DomainError::InvalidState {
                    reason: "revalidated workspace does not match the scheduler identity",
                },
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(binding.identity())?;
        let Some(state) = view.graph_state() else {
            return Ok(None);
        };
        if let Some(expected) = expected_session_id_v1(request.command())
            && state.trace().session_id() != expected
        {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: expected.clone(),
                actual: Some(state.trace().session_id().clone()),
            });
        }
        match request.command() {
            SliceCommandV1::SessionBlock(input) => {
                validate_procedure_v2_block_reason_v1(&input.reason)?
            }
            SliceCommandV1::SessionCancel(input) => {
                ReasonV2::new(input.reason.clone()).map_err(ExecutionErrorV1::BoundaryDomain)?;
            }
            SliceCommandV1::SessionReset(input) if input.dry_run || !input.confirmed => {
                return Err(invalid_execution_v1(
                    "Procedure v2 durable reset requires confirmation and cannot be dry-run",
                ));
            }
            _ => {}
        }
        let attached_artifact = match request.command() {
            SliceCommandV1::ItemAttach(input) => Some(match &input.source {
                ItemAttachSourceV1::Path { path, media_type } => {
                    let artifact = self
                        .artifacts
                        .hash_local_artifact(&binding, path, media_type.as_deref())
                        .map_err(BoundaryDispositionV1::from)
                        .map_err(ExecutionErrorV1::from_boundary)?;
                    validate_local_attached_artifact_v1(path, media_type.as_deref(), &artifact)
                        .map_err(ExecutionErrorV1::BoundaryDomain)?;
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
                .map_err(ExecutionErrorV1::BoundaryDomain)?,
            }),
            _ => None,
        };
        let fresh_attempt_id = if matches!(
            request.command(),
            SliceCommandV1::SessionComplete(_) | SliceCommandV1::SessionSkip(_)
        ) {
            if let SliceCommandV1::SessionSkip(input) = request.command()
                && let Some(reason) = &input.reason
            {
                ReasonV2::new(reason.clone()).map_err(ExecutionErrorV1::BoundaryDomain)?;
            }
            fresh_successor_attempt_id_v1(state, &self.ids)?
        } else if let SliceCommandV1::SessionRetry(input) = request.command() {
            ReasonV2::new(input.reason.clone()).map_err(ExecutionErrorV1::BoundaryDomain)?;
            Some(self.ids.next_attempt_id())
        } else {
            None
        };
        let fresh_blocker_id = matches!(request.command(), SliceCommandV1::SessionBlock(_))
            .then(|| self.ids.next_blocker_id());
        let admitted = AdmittedProcedureV2MutationV1 {
            selector: request.selector().clone(),
            workspace_id: binding.identity().workspace_uuid().clone(),
            command: request.command().clone(),
            fresh_attempt_id,
            fresh_blocker_id,
            attached_artifact,
        };
        let canonical_execution = procedure_v2_mutation_execution_document_v1(&admitted)?;
        let request_digest = request_digest_v1(request, binding.identity().workspace_uuid(), None)?;
        let durable = AdmitRequestV1::new_with_canonical_execution(
            command_for_admission_v1(request.command())?,
            idempotency_key,
            self.ids.next_job_id(),
            store_preconditions_v1(request.command())?,
            request_digest,
            self.clock.now(),
            canonical_execution,
        )
        .with_procedure_v2_execution()
        .with_session_identity(admission_session_identity_v1(request.command()));
        let durable = match response_context {
            Some(context) => durable.with_response_context(context),
            None => durable,
        };
        self.store
            .admit(binding.identity(), durable)
            .map(Some)
            .map_err(Into::into)
    }

    /// Durably admits the typed `session.decide` route without widening the legacy slice command.
    pub fn admit_procedure_v2_typed_mutation_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &ProcedureV2MutationRequestV1,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        match request.command() {
            ProcedureV2MutationCommandV1::SessionDecide(command) => self
                .admit_procedure_v2_decision_for_workspace_with_response_context(
                    expected_workspace,
                    request,
                    command,
                    idempotency_key,
                    response_context,
                ),
            ProcedureV2MutationCommandV1::SessionRework(command) => self
                .admit_procedure_v2_rework_for_workspace_with_response_context(
                    expected_workspace,
                    request,
                    command,
                    idempotency_key,
                    response_context,
                ),
            ProcedureV2MutationCommandV1::GoalDefine(_)
            | ProcedureV2MutationCommandV1::GoalRevise(_)
            | ProcedureV2MutationCommandV1::GoalAssessCriterion(_) => Ok(None),
        }
    }

    fn admit_procedure_v2_decision_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &ProcedureV2MutationRequestV1,
        command: &podway_protocol::SessionDecideV2,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        if let Some(existing) = self
            .store
            .read_idempotent_execution(expected_workspace.identity(), &idempotency_key)?
        {
            let Some(canonical_execution) = existing.canonical_execution() else {
                return Ok(None);
            };
            let version = serde_json::from_str::<Value>(canonical_execution.as_str())
                .ok()
                .and_then(|value| value.get("execution_version").and_then(Value::as_u64));
            if version != Some(u64::from(EXECUTION_DOCUMENT_VERSION_V9)) {
                return Ok(None);
            }
            let admitted = decode_procedure_v2_decision_execution_v1(canonical_execution.as_str())?;
            if admitted.workspace_id != *expected_workspace.identity().workspace_uuid() {
                return Err(invalid_execution_v1(
                    "Procedure v2 decision replay workspace identity is invalid",
                ));
            }
            let actual = procedure_v2_typed_mutation_request_digest_v1(
                request,
                expected_workspace.identity().workspace_uuid(),
            )?;
            if existing.request_digest() != &actual {
                return Err(StoreErrorV1::IdempotencyDigestConflictV1 {
                    expected: existing.request_digest().clone(),
                    actual,
                }
                .into());
            }
            return Ok(Some(existing.outcome().clone()));
        }

        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)?;
        if binding.identity() != expected_workspace.identity() {
            return Err(ExecutionErrorV1::BoundaryDomain(
                DomainError::InvalidState {
                    reason: "revalidated workspace does not match the scheduler identity",
                },
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(binding.identity())?;
        let Some(state) = view.graph_state() else {
            return Ok(None);
        };
        if state.trace().session_id() != &command.preconditions.expected_session_id {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: command.preconditions.expected_session_id.clone(),
                actual: Some(state.trace().session_id().clone()),
            });
        }
        ReasonV2::new(command.reason.clone()).map_err(ExecutionErrorV1::BoundaryDomain)?;
        command
            .actor
            .clone()
            .map(ActorAttributionV2::new)
            .transpose()
            .map_err(ExecutionErrorV1::BoundaryDomain)?;
        let admitted = AdmittedProcedureV2DecisionV1 {
            selector: request.selector().clone(),
            workspace_id: binding.identity().workspace_uuid().clone(),
            command: command.clone(),
            fresh_attempt_id: self.ids.next_attempt_id(),
        };
        let canonical_execution = procedure_v2_decision_execution_document_v1(&admitted)?;
        let request_digest = procedure_v2_typed_mutation_request_digest_v1(
            request,
            binding.identity().workspace_uuid(),
        )?;
        let preconditions = RevisionAttemptItemPreconditionsV1::new(
            Some(command.preconditions.expected_session_revision),
            Some(command.preconditions.expected_attempt_id.clone()),
            None,
            None,
        )
        .map_err(ExecutionErrorV1::InvalidStoreValue)?;
        let durable = AdmitRequestV1::new_with_canonical_execution(
            DomainCommand::SessionDecide,
            idempotency_key,
            self.ids.next_job_id(),
            preconditions,
            request_digest,
            self.clock.now(),
            canonical_execution,
        )
        .with_procedure_v2_execution()
        .with_session_identity(AdmissionSessionIdentityV1::Exact(
            command.preconditions.expected_session_id.clone(),
        ));
        let durable = match response_context {
            Some(context) => durable.with_response_context(context),
            None => durable,
        };
        self.store
            .admit(binding.identity(), durable)
            .map(Some)
            .map_err(Into::into)
    }

    fn admit_procedure_v2_rework_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &ProcedureV2MutationRequestV1,
        command: &SessionReworkV2,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        if let Some(existing) = self
            .store
            .read_idempotent_execution(expected_workspace.identity(), &idempotency_key)?
        {
            let Some(canonical_execution) = existing.canonical_execution() else {
                return Ok(None);
            };
            let version = serde_json::from_str::<Value>(canonical_execution.as_str())
                .ok()
                .and_then(|value| value.get("execution_version").and_then(Value::as_u64));
            if version != Some(u64::from(EXECUTION_DOCUMENT_VERSION_V10)) {
                return Ok(None);
            }
            let admitted = decode_procedure_v2_rework_execution_v1(canonical_execution.as_str())?;
            if admitted.workspace_id != *expected_workspace.identity().workspace_uuid() {
                return Err(invalid_execution_v1(
                    "Procedure v2 rework replay workspace identity is invalid",
                ));
            }
            let actual = procedure_v2_typed_mutation_request_digest_v1(
                request,
                expected_workspace.identity().workspace_uuid(),
            )?;
            if existing.request_digest() != &actual {
                return Err(StoreErrorV1::IdempotencyDigestConflictV1 {
                    expected: existing.request_digest().clone(),
                    actual,
                }
                .into());
            }
            return Ok(Some(existing.outcome().clone()));
        }

        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)?;
        if binding.identity() != expected_workspace.identity() {
            return Err(ExecutionErrorV1::BoundaryDomain(
                DomainError::InvalidState {
                    reason: "revalidated workspace does not match the scheduler identity",
                },
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(binding.identity())?;
        let Some(state) = view.graph_state() else {
            return Ok(None);
        };
        if state.trace().session_id() != &command.preconditions.expected_session_id {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: command.preconditions.expected_session_id.clone(),
                actual: Some(state.trace().session_id().clone()),
            });
        }
        match (
            state.trace().lifecycle(),
            command.preconditions.expected_attempt_id.as_ref(),
        ) {
            (SessionLifecycle::Running, Some(_))
            | (SessionLifecycle::Completed, None)
            | (SessionLifecycle::Cancelled, _) => {}
            (SessionLifecycle::Running, None) => {
                return Err(ExecutionErrorV1::BoundaryDomain(
                    DomainError::InvalidState {
                        reason: "running Procedure v2 manual rework requires an attempt fence",
                    },
                ));
            }
            (SessionLifecycle::Completed, Some(_)) => {
                return Err(ExecutionErrorV1::BoundaryDomain(
                    DomainError::InvalidState {
                        reason: "completed Procedure v2 manual rework forbids an attempt fence",
                    },
                ));
            }
        }
        ReasonV2::new(command.reason.clone()).map_err(ExecutionErrorV1::BoundaryDomain)?;
        command
            .actor
            .clone()
            .map(ActorAttributionV2::new)
            .transpose()
            .map_err(ExecutionErrorV1::BoundaryDomain)?;
        let admitted = AdmittedProcedureV2ReworkV1 {
            selector: request.selector().clone(),
            workspace_id: binding.identity().workspace_uuid().clone(),
            command: command.clone(),
            fresh_attempt_id: self.ids.next_attempt_id(),
        };
        let canonical_execution = procedure_v2_rework_execution_document_v1(&admitted)?;
        let request_digest = procedure_v2_typed_mutation_request_digest_v1(
            request,
            binding.identity().workspace_uuid(),
        )?;
        let preconditions = RevisionAttemptItemPreconditionsV1::new(
            Some(command.preconditions.expected_session_revision),
            command.preconditions.expected_attempt_id.clone(),
            None,
            None,
        )
        .map_err(ExecutionErrorV1::InvalidStoreValue)?;
        let durable = AdmitRequestV1::new_with_canonical_execution(
            DomainCommand::SessionRework,
            idempotency_key,
            self.ids.next_job_id(),
            preconditions,
            request_digest,
            self.clock.now(),
            canonical_execution,
        )
        .with_procedure_v2_execution()
        .with_session_identity(AdmissionSessionIdentityV1::Exact(
            command.preconditions.expected_session_id.clone(),
        ));
        let durable = match response_context {
            Some(context) => durable.with_response_context(context),
            None => durable,
        };
        self.store
            .admit(binding.identity(), durable)
            .map(Some)
            .map_err(Into::into)
    }

    /// Claims either durable flavor, reconstructing Procedure v2 exclusively from the admitted
    /// execution document before committing graph state and the terminal receipt atomically.
    pub fn execute_next_with_graph_v2(
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
        if claimed.execution().execution_flavor()
            == podway_store::DurableExecutionFlavorV1::LegacyV1
        {
            return self.execute_claimed(&workspace, claimed, now).map(Some);
        }
        if workspace.identity() != claimed.claim().identity() {
            return Err(invalid_execution_v1(
                "scheduler workspace does not match the Procedure v2 claim",
            ));
        }
        let document: Value =
            serde_json::from_str(claimed.execution().canonical_execution().as_str())
                .map_err(|_| invalid_execution_v1("Procedure v2 execution is not JSON"))?;
        let version = document
            .get("execution_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid_execution_v1("Procedure v2 execution version is absent"))?;
        let receipt = match version {
            version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V6) => {
                let admitted = decode_procedure_v2_start_execution_v1(
                    claimed.execution().canonical_execution().as_str(),
                )?;
                if admitted.workspace_id != *claimed.claim().identity().workspace_uuid() {
                    return Err(invalid_execution_v1(
                        "Procedure v2 execution workspace does not match the claim",
                    ));
                }
                validate_procedure_v2_durable_execution_v1(&admitted, claimed.execution())?;
                self.store.commit_graph_start_terminal_v2(
                    claimed.claim().clone(),
                    admitted.expected_current,
                    admitted.state,
                    now,
                )?
            }
            version
                if version == u64::from(EXECUTION_DOCUMENT_VERSION_V7)
                    || version == u64::from(EXECUTION_DOCUMENT_VERSION_V8) =>
            {
                let admitted = decode_procedure_v2_mutation_execution_v1(
                    claimed.execution().canonical_execution().as_str(),
                )?;
                self.execute_procedure_v2_mutation_claimed(&workspace, &claimed, admitted, now)?
            }
            version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V9) => {
                let admitted = decode_procedure_v2_decision_execution_v1(
                    claimed.execution().canonical_execution().as_str(),
                )?;
                self.execute_procedure_v2_decision_claimed(&workspace, &claimed, admitted, now)?
            }
            version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V10) => {
                let admitted = decode_procedure_v2_rework_execution_v1(
                    claimed.execution().canonical_execution().as_str(),
                )?;
                self.execute_procedure_v2_rework_claimed(&claimed, admitted, now)?
            }
            _ => {
                return Err(invalid_execution_v1(
                    "Procedure v2 execution version is unsupported",
                ));
            }
        };
        Ok(Some(receipt))
    }

    fn execute_procedure_v2_rework_claimed(
        &self,
        claimed: &ClaimedJobV1,
        admitted: AdmittedProcedureV2ReworkV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        if admitted.workspace_id != *claimed.claim().identity().workspace_uuid() {
            return Err(invalid_execution_v1(
                "Procedure v2 rework workspace does not match the claim",
            ));
        }
        let expected_preconditions = RevisionAttemptItemPreconditionsV1::new(
            Some(admitted.command.preconditions.expected_session_revision),
            admitted.command.preconditions.expected_attempt_id.clone(),
            None,
            None,
        )
        .map_err(ExecutionErrorV1::InvalidStoreValue)?;
        if claimed.execution().command() != &DomainCommand::SessionRework
            || claimed.execution().preconditions() != &expected_preconditions
            || claimed.execution().session_identity()
                != &AdmissionSessionIdentityV1::Exact(
                    admitted.command.preconditions.expected_session_id.clone(),
                )
        {
            return Err(invalid_execution_v1(
                "Procedure v2 rework document does not match durable admission metadata",
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(claimed.claim().identity())?;
        let actual_session_id = view
            .graph_state()
            .map(|state| state.trace().session_id().clone());
        if actual_session_id.as_ref() != Some(&admitted.command.preconditions.expected_session_id) {
            return self.commit_domain_failure(
                claimed,
                Revision::ZERO,
                DomainError::SessionIdentityMismatch {
                    expected: admitted.command.preconditions.expected_session_id.clone(),
                    actual: actual_session_id,
                },
                now,
            );
        }
        let state = view
            .graph_state()
            .expect("matching Procedure v2 session identity requires graph state");
        let reason = ReasonV2::new(admitted.command.reason)
            .map_err(|_| invalid_execution_v1("Procedure v2 rework reason is invalid"))?;
        let actor = admitted
            .command
            .actor
            .map(ActorAttributionV2::new)
            .transpose()
            .map_err(|_| invalid_execution_v1("Procedure v2 rework actor is invalid"))?;
        let outcome = match state.manual_rework_v2(
            admitted.command.preconditions.expected_session_revision,
            admitted.command.preconditions.expected_attempt_id.as_ref(),
            admitted.command.target_graph_node_id,
            admitted.fresh_attempt_id,
            reason,
            actor,
            now,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
            }
        };
        let operation = PersistedGraphTerminalOperationV2::rework(rework_record_projection_v1(
            outcome.record(),
        ))
        .map_err(|_| invalid_execution_v1("Procedure v2 rework operation cannot be persisted"))?;
        let result = DomainResult::SessionChanged {
            session_id: state.trace().session_id().clone(),
            revision_before: state.trace().revision(),
            revision_after: outcome.state().trace().revision(),
            changed: true,
        };
        self.store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                state.workspace_revision(),
                state.trace().revision(),
                Some(outcome.into_state()),
                TerminalResultV1::Success(result),
                operation,
                now,
            )
            .map_err(Into::into)
    }

    fn execute_procedure_v2_decision_claimed(
        &self,
        workspace: &WorkspaceBindingV1,
        claimed: &ClaimedJobV1,
        admitted: AdmittedProcedureV2DecisionV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        if admitted.workspace_id != *claimed.claim().identity().workspace_uuid() {
            return Err(invalid_execution_v1(
                "Procedure v2 decision workspace does not match the claim",
            ));
        }
        let expected_preconditions = RevisionAttemptItemPreconditionsV1::new(
            Some(admitted.command.preconditions.expected_session_revision),
            Some(admitted.command.preconditions.expected_attempt_id.clone()),
            None,
            None,
        )
        .map_err(ExecutionErrorV1::InvalidStoreValue)?;
        if claimed.execution().command() != &DomainCommand::SessionDecide
            || claimed.execution().preconditions() != &expected_preconditions
            || claimed.execution().session_identity()
                != &AdmissionSessionIdentityV1::Exact(
                    admitted.command.preconditions.expected_session_id.clone(),
                )
        {
            return Err(invalid_execution_v1(
                "Procedure v2 decision document does not match durable admission metadata",
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(claimed.claim().identity())?;
        let actual_session_id = view
            .graph_state()
            .map(|state| state.trace().session_id().clone());
        if actual_session_id.as_ref() != Some(&admitted.command.preconditions.expected_session_id) {
            return self.commit_domain_failure(
                claimed,
                Revision::ZERO,
                DomainError::SessionIdentityMismatch {
                    expected: admitted.command.preconditions.expected_session_id.clone(),
                    actual: actual_session_id,
                },
                now,
            );
        }
        let state = view
            .graph_state()
            .expect("matching Procedure v2 session identity requires graph state");
        let reason = ReasonV2::new(admitted.command.reason.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 decision reason is invalid"))?;
        let actor = admitted
            .command
            .actor
            .clone()
            .map(ActorAttributionV2::new)
            .transpose()
            .map_err(|_| invalid_execution_v1("Procedure v2 decision actor is invalid"))?;
        let outcome = match state.decide_active_route_v2(
            admitted.command.preconditions.expected_session_revision,
            &admitted.command.preconditions.expected_attempt_id,
            admitted.command.option_id,
            admitted.fresh_attempt_id,
            Some(reason),
            actor,
            now,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
            }
        };
        for (item_id, artifact) in required_local_artifacts_v2(state)? {
            if let Err(error) = self
                .artifacts
                .revalidate_local_artifact(workspace, &item_id, &artifact)
            {
                return match error {
                    ExecutionBoundaryErrorV1::Domain(DomainError::ArtifactChanged) => {
                        self.commit_graph_artifact_changed_failure_v2(claimed, state, now)
                    }
                    error => Err(ExecutionErrorV1::from_boundary(error.into())),
                };
            }
        }
        let operation = PersistedGraphTerminalOperationV2::decide(
            decision_record_projection_v1(outcome.decision_record())?,
            outcome.to_attempt_id().clone(),
        )
        .map_err(|_| invalid_execution_v1("Procedure v2 decision operation cannot be persisted"))?;
        let result = DomainResult::SessionChanged {
            session_id: state.trace().session_id().clone(),
            revision_before: state.trace().revision(),
            revision_after: outcome.state().trace().revision(),
            changed: true,
        };
        self.store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                state.workspace_revision(),
                state.trace().revision(),
                Some(outcome.into_state()),
                TerminalResultV1::Success(result),
                operation,
                now,
            )
            .map_err(Into::into)
    }

    fn execute_procedure_v2_mutation_claimed(
        &self,
        workspace: &WorkspaceBindingV1,
        claimed: &ClaimedJobV1,
        admitted: AdmittedProcedureV2MutationV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        if admitted.workspace_id != *claimed.claim().identity().workspace_uuid() {
            return Err(invalid_execution_v1(
                "Procedure v2 mutation workspace does not match the claim",
            ));
        }
        validate_procedure_v2_mutation_durable_execution_v1(&admitted, claimed.execution())?;
        let view = self
            .store
            .read_graph_workspace_view_v2(claimed.claim().identity())?;
        let expected_session = expected_session_id_v1(&admitted.command).ok_or_else(|| {
            invalid_execution_v1("Procedure v2 mutation has no session identity fence")
        })?;
        let actual_session_id = view
            .graph_state()
            .map(|state| state.trace().session_id().clone());
        if actual_session_id.as_ref() != Some(expected_session) {
            return self.commit_domain_failure(
                claimed,
                Revision::ZERO,
                DomainError::SessionIdentityMismatch {
                    expected: expected_session.clone(),
                    actual: actual_session_id,
                },
                now,
            );
        }
        let state = view
            .graph_state()
            .expect("matching Procedure v2 session identity requires graph state");
        let expected_workspace_revision = state.workspace_revision();
        let expected_session_revision = state.trace().revision();
        match &admitted.command {
            SliceCommandV1::SessionComplete(input) => {
                let outcome = match state.complete_active_action_v2(
                    input.preconditions.expected_session_revision,
                    &input.preconditions.expected_attempt_id,
                    admitted.fresh_attempt_id.clone(),
                    now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                };
                for (item_id, artifact) in required_local_artifacts_v2(state)? {
                    if let Err(error) = self
                        .artifacts
                        .revalidate_local_artifact(workspace, &item_id, &artifact)
                    {
                        return match error {
                            ExecutionBoundaryErrorV1::Domain(DomainError::ArtifactChanged) => {
                                self.commit_graph_artifact_changed_failure_v2(claimed, state, now)
                            }
                            error => Err(ExecutionErrorV1::from_boundary(error.into())),
                        };
                    }
                }
                let operation = PersistedGraphTerminalOperationV2::complete(
                    outcome.from_graph_node_id().clone(),
                    outcome.from_attempt_id().clone(),
                    outcome.to_graph_node_id().cloned(),
                    outcome.to_attempt_id().cloned(),
                )
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 complete operation cannot be persisted")
                })?;
                let result = DomainResult::SessionChanged {
                    session_id: state.trace().session_id().clone(),
                    revision_before: state.trace().revision(),
                    revision_after: outcome.state().trace().revision(),
                    changed: true,
                };
                self.store
                    .commit_graph_mutation_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        Some(outcome.into_state()),
                        TerminalResultV1::Success(result),
                        operation,
                        now,
                    )
                    .map_err(Into::into)
            }
            SliceCommandV1::SessionSkip(input) => {
                let reason = input
                    .reason
                    .clone()
                    .map(ReasonV2::new)
                    .transpose()
                    .map_err(|_| invalid_execution_v1("Procedure v2 skip reason is invalid"))?;
                let outcome = match state.skip_active_action_v2(
                    input.preconditions.expected_session_revision,
                    &input.preconditions.expected_attempt_id,
                    admitted.fresh_attempt_id.clone(),
                    reason.clone(),
                    now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                };
                let operation = PersistedGraphTerminalOperationV2::skip(
                    outcome.from_graph_node_id().clone(),
                    outcome.from_attempt_id().clone(),
                    outcome.to_graph_node_id().cloned(),
                    outcome.to_attempt_id().cloned(),
                    reason,
                )
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 skip operation cannot be persisted")
                })?;
                let result = DomainResult::SessionChanged {
                    session_id: state.trace().session_id().clone(),
                    revision_before: state.trace().revision(),
                    revision_after: outcome.state().trace().revision(),
                    changed: true,
                };
                self.store
                    .commit_graph_mutation_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        Some(outcome.into_state()),
                        TerminalResultV1::Success(result),
                        operation,
                        now,
                    )
                    .map_err(Into::into)
            }
            SliceCommandV1::SessionRetry(input) => {
                let reason = ReasonV2::new(input.reason.clone())
                    .map_err(|_| invalid_execution_v1("Procedure v2 retry reason is invalid"))?;
                let fresh_attempt_id = admitted.fresh_attempt_id.clone().ok_or_else(|| {
                    invalid_execution_v1(
                        "Procedure v2 retry execution has no fresh attempt identity",
                    )
                })?;
                let outcome = match state.retry_active_attempt_v2(
                    input.preconditions.expected_session_revision,
                    &input.preconditions.expected_attempt_id,
                    fresh_attempt_id,
                    reason.clone(),
                    now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                };
                let operation = PersistedGraphTerminalOperationV2::retry(
                    outcome.graph_node_id().clone(),
                    outcome.from_attempt_id().clone(),
                    outcome.to_attempt_id().clone(),
                    reason,
                )
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 retry operation cannot be persisted")
                })?;
                let result = DomainResult::SessionChanged {
                    session_id: state.trace().session_id().clone(),
                    revision_before: state.trace().revision(),
                    revision_after: outcome.state().trace().revision(),
                    changed: true,
                };
                self.store
                    .commit_graph_mutation_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        Some(outcome.into_state()),
                        TerminalResultV1::Success(result),
                        operation,
                        now,
                    )
                    .map_err(Into::into)
            }
            SliceCommandV1::SessionBlock(input) => {
                let blocker_id = admitted.fresh_blocker_id.clone().ok_or_else(|| {
                    invalid_execution_v1(
                        "Procedure v2 block execution has no fresh blocker identity",
                    )
                })?;
                let outcome = match state.block_active_attempt_v2(
                    input.preconditions.expected_session_revision,
                    &input.preconditions.expected_attempt_id,
                    blocker_id,
                    input.reason.clone(),
                    now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                };
                let operation = PersistedGraphTerminalOperationV2::block(
                    outcome.graph_node_id().clone(),
                    outcome.attempt_id().clone(),
                    outcome.blocker_id().clone(),
                    input.reason.clone(),
                )
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 block operation cannot be persisted")
                })?;
                let result = DomainResult::SessionChanged {
                    session_id: state.trace().session_id().clone(),
                    revision_before: state.trace().revision(),
                    revision_after: outcome.state().trace().revision(),
                    changed: true,
                };
                self.store
                    .commit_graph_mutation_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        Some(outcome.into_state()),
                        TerminalResultV1::Success(result),
                        operation,
                        now,
                    )
                    .map_err(Into::into)
            }
            SliceCommandV1::SessionUnblock(input) => {
                let outcome = match state.unblock_active_attempt_v2(
                    input.preconditions.expected_session_revision,
                    &input.preconditions.expected_attempt_id,
                    input.blocker_id.as_ref(),
                    input.all,
                    now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                };
                let operation = PersistedGraphTerminalOperationV2::unblock(
                    outcome.graph_node_id().clone(),
                    outcome.attempt_id().clone(),
                    input.all,
                    outcome.blocker_ids().to_vec(),
                )
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 unblock operation cannot be persisted")
                })?;
                let result = DomainResult::SessionChanged {
                    session_id: state.trace().session_id().clone(),
                    revision_before: state.trace().revision(),
                    revision_after: outcome.state().trace().revision(),
                    changed: true,
                };
                self.store
                    .commit_graph_mutation_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        Some(outcome.into_state()),
                        TerminalResultV1::Success(result),
                        operation,
                        now,
                    )
                    .map_err(Into::into)
            }
            SliceCommandV1::SessionCancel(input) => {
                let reason = ReasonV2::new(input.reason.clone())
                    .map_err(|_| invalid_execution_v1("Procedure v2 cancel reason is invalid"))?;
                let outcome = match state.cancel_active_session_v2(
                    input.preconditions.expected_session_revision,
                    &input.preconditions.expected_attempt_id,
                    reason.clone(),
                    now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                };
                let operation = PersistedGraphTerminalOperationV2::cancel(
                    outcome.graph_node_id().clone(),
                    outcome.attempt_id().clone(),
                    reason,
                )
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 cancel operation cannot be persisted")
                })?;
                let result = DomainResult::SessionChanged {
                    session_id: state.trace().session_id().clone(),
                    revision_before: state.trace().revision(),
                    revision_after: outcome.state().trace().revision(),
                    changed: true,
                };
                self.store
                    .commit_graph_mutation_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        Some(outcome.into_state()),
                        TerminalResultV1::Success(result),
                        operation,
                        now,
                    )
                    .map_err(Into::into)
            }
            SliceCommandV1::SessionReset(input) => {
                if input.preconditions.expected_session_revision != state.trace().revision() {
                    return self.commit_persisted_graph_mutation_failure_v2(
                        claimed,
                        state,
                        PersistedGraphMutationFailureV2::SessionRevisionConflict {
                            expected: input.preconditions.expected_session_revision,
                            actual: state.trace().revision(),
                        },
                        now,
                    );
                }
                self.store
                    .commit_graph_reset_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        state.trace().session_id().clone(),
                        now,
                    )
                    .map_err(Into::into)
            }
            SliceCommandV1::ItemCheck(_)
            | SliceCommandV1::ItemUncheck(_)
            | SliceCommandV1::ItemSet(_)
            | SliceCommandV1::ItemAdd(_)
            | SliceCommandV1::ItemRemove(_)
            | SliceCommandV1::ItemAttach(_)
            | SliceCommandV1::ItemClear(_) => {
                let prepared = prepare_procedure_v2_item_mutation_v1(&admitted)?;
                let active = state.trace().active_attempt().ok_or_else(|| {
                    invalid_execution_v1("Procedure v2 claimed item mutation has no active attempt")
                })?;
                let outcome = match state.mutate_active_item_v2(
                    &prepared.expected_attempt_id,
                    &prepared.item_id,
                    prepared.expected_item_revision,
                    prepared.mutation,
                    now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                };
                let operation = PersistedGraphTerminalOperationV2::item_mutation(
                    active.graph_node_id().clone(),
                    active.attempt_id().clone(),
                    active.number(),
                    prepared.item_id.clone(),
                    outcome.value_digest().cloned(),
                )
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 item operation cannot be persisted")
                })?;
                let result = DomainResult::ItemChanged {
                    session_id: state.trace().session_id().clone(),
                    item_id: prepared.item_id,
                    revision_before: state.trace().revision(),
                    revision_after: outcome.state().trace().revision(),
                    changed: outcome.changed(),
                };
                let next_state = outcome.changed().then(|| outcome.into_state());
                self.store
                    .commit_graph_mutation_terminal_v2(
                        claimed.claim().clone(),
                        expected_workspace_revision,
                        expected_session_revision,
                        next_state,
                        TerminalResultV1::Success(result),
                        operation,
                        now,
                    )
                    .map_err(Into::into)
            }
            _ => Err(invalid_execution_v1(
                "Procedure v2 claimed mutation command is invalid",
            )),
        }
    }

    fn commit_graph_mutation_failure_v2(
        &self,
        claimed: &ClaimedJobV1,
        state: &GraphSessionStateV2,
        error: &GraphMutationErrorV2,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        let failure = PersistedGraphMutationFailureV2::try_from(error)
            .map_err(|_| invalid_execution_v1("Procedure v2 graph failure cannot be persisted"))?;
        self.commit_persisted_graph_mutation_failure_v2(claimed, state, failure, now)
    }

    fn commit_graph_artifact_changed_failure_v2(
        &self,
        claimed: &ClaimedJobV1,
        state: &GraphSessionStateV2,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        self.commit_persisted_graph_mutation_failure_v2(
            claimed,
            state,
            PersistedGraphMutationFailureV2::ArtifactChanged,
            now,
        )
    }

    fn commit_persisted_graph_mutation_failure_v2(
        &self,
        claimed: &ClaimedJobV1,
        state: &GraphSessionStateV2,
        failure: PersistedGraphMutationFailureV2,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        let operation = PersistedGraphTerminalOperationV2::failure(failure).map_err(|_| {
            invalid_execution_v1("Procedure v2 failure operation cannot be persisted")
        })?;
        self.store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                state.workspace_revision(),
                state.trace().revision(),
                None,
                TerminalResultV1::Failure(DomainError::InvalidState {
                    reason: "Procedure v2 graph mutation failed",
                }),
                operation,
                now,
            )
            .map_err(Into::into)
    }
}

fn validate_procedure_v2_durable_execution_v1(
    admitted: &AdmittedProcedureV2StartV1,
    execution: &podway_store::ClaimedExecutionV1,
) -> Result<(), ExecutionErrorV1> {
    let matches = match (
        admitted.replace,
        &admitted.expected_current,
        execution.command(),
        execution.session_identity(),
        execution.preconditions().expected_session_revision(),
    ) {
        (
            false,
            GraphStartCurrentTaskV2::Absent,
            podway_store::CommandV1::SessionStart,
            AdmissionSessionIdentityV1::Absent,
            None,
        ) => true,
        (
            true,
            GraphStartCurrentTaskV2::Exact {
                session_id: inner_session_id,
                session_revision: inner_revision,
            },
            podway_store::CommandV1::SessionStartReplace,
            AdmissionSessionIdentityV1::Exact(outer_session_id),
            Some(outer_revision),
        ) => inner_session_id == outer_session_id && *inner_revision == outer_revision,
        _ => false,
    };
    if !matches {
        return Err(invalid_execution_v1(
            "Procedure v2 execution document does not match durable admission metadata",
        ));
    }
    Ok(())
}

fn validate_procedure_v2_mutation_durable_execution_v1(
    admitted: &AdmittedProcedureV2MutationV1,
    execution: &podway_store::ClaimedExecutionV1,
) -> Result<(), ExecutionErrorV1> {
    let command = command_for_admission_v1(&admitted.command)?;
    let preconditions = store_preconditions_v1(&admitted.command)?;
    let session_identity = admission_session_identity_v1(&admitted.command);
    if execution.command() != &command
        || execution.preconditions() != &preconditions
        || execution.session_identity() != &session_identity
    {
        return Err(invalid_execution_v1(
            "Procedure v2 mutation document does not match durable admission metadata",
        ));
    }
    Ok(())
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
            ExecutionBoundaryErrorV1::ProcedureV2Unsupported => {
                Self::Domain(DomainError::InvalidState {
                    reason: "Procedure v2 capability escaped start admission resolution",
                })
            }
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
        | SliceCommandV1::JobLookup(_)
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
        | SliceCommandV1::JobLookup(_)
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
        | SliceCommandV1::JobLookup(_)
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
        | SliceCommandV1::JobLookup(_)
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
        | DomainCommand::SessionReset
        | DomainCommand::SessionDecide
        | DomainCommand::SessionRework => DomainResult::SessionChanged {
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

fn procedure_v2_typed_mutation_request_digest_v1(
    request: &ProcedureV2MutationRequestV1,
    workspace_id: &WorkspaceId,
) -> Result<Sha256Digest, ExecutionErrorV1> {
    let canonical =
        canonical_procedure_v2_mutation_identity_v1(request, workspace_id).map_err(|_| {
            ExecutionErrorV1::InvalidPersistedExecution {
                reason: "Procedure v2 mutation identity cannot be canonicalized",
            }
        })?;
    Sha256Digest::new(format!("sha256:{}", sha256_hex_v1(canonical.as_bytes()))).map_err(|_| {
        ExecutionErrorV1::InvalidPersistedExecution {
            reason: "Procedure v2 mutation identity digest is invalid",
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
        | SliceCommandV1::JobLookup(_)
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
        | SliceCommandV1::JobLookup(_)
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
    use super::*;

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

    #[test]
    fn v2run001_inner_execution_fence_must_match_durable_metadata() {
        let snapshot = workspace_procedure_snapshot_from_bytes_v2(
            "workflow.json",
            br#"{
                "schema":"podway.procedure/v2",
                "id":"durable-fence",
                "version":"1",
                "name":"Durable fence",
                "purpose":"Reject mismatched persisted execution metadata.",
                "node_definitions":{"work":{"type":"action","title":"Work","intent":"Work."}},
                "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}
            }"#,
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000901").unwrap(),
            UnixMillis::new(10),
        )
        .unwrap();
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000902").unwrap();
        let state = graph_session_state_from_procedure_v2_snapshot(
            snapshot,
            "Durable fence",
            session_id.clone(),
            AttemptId::new("00000000-0000-4000-8000-000000000903").unwrap(),
            UnixMillis::new(10),
        )
        .unwrap();
        let admitted = AdmittedProcedureV2StartV1 {
            selector: WorktreeSelectorWireV1::new(b"/tmp/worktree", "/tmp/worktree", None).unwrap(),
            workspace_id: WorkspaceId::new("00000000-0000-4000-8000-000000000904").unwrap(),
            replace: true,
            expected_current: GraphStartCurrentTaskV2::Exact {
                session_id: session_id.clone(),
                session_revision: Revision::new(2),
            },
            state,
        };
        let canonical_execution = procedure_v2_start_execution_document_v1(
            &admitted,
            &SliceCommandV1::SessionStartReplace(podway_protocol::SessionStartReplaceV1 {
                start: podway_protocol::SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "workflow.json".to_owned(),
                    },
                    task_title: "Durable fence".to_owned(),
                    expected_procedure_digest: Some(admitted.state.snapshot().digest().clone()),
                    dry_run: false,
                },
                confirmed: true,
                preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(2),
                },
            }),
        )
        .unwrap();
        let durable = podway_store::ClaimedExecutionV1::new_procedure_v2(
            podway_store::CommandV1::SessionStartReplace,
            RevisionAttemptItemPreconditionsV1::new(Some(Revision::new(1)), None, None, None)
                .unwrap(),
            canonical_execution,
            AdmissionSessionIdentityV1::Exact(session_id),
        );

        assert!(matches!(
            validate_procedure_v2_durable_execution_v1(&admitted, &durable),
            Err(ExecutionErrorV1::InvalidPersistedExecution { .. })
        ));
    }

    #[test]
    fn v2run004_retry_execution_freezes_fresh_identity_and_fails_closed() {
        let admitted = AdmittedProcedureV2MutationV1 {
            selector: WorktreeSelectorWireV1::new(b"/tmp/worktree", "/tmp/worktree", None).unwrap(),
            workspace_id: WorkspaceId::new("00000000-0000-4000-8000-000000000904").unwrap(),
            command: SliceCommandV1::SessionRetry(SessionRetryV1 {
                reason: "Retry with clean attempt-local state.".to_owned(),
                preconditions: podway_protocol::SessionMutationPreconditionsWireV1 {
                    expected_session_id: SessionId::new("00000000-0000-4000-8000-000000000902")
                        .unwrap(),
                    expected_session_revision: Revision::new(7),
                    expected_attempt_id: AttemptId::new("00000000-0000-4000-8000-000000000903")
                        .unwrap(),
                },
            }),
            fresh_attempt_id: Some(AttemptId::new("00000000-0000-4000-8000-000000000905").unwrap()),
            fresh_blocker_id: None,
            attached_artifact: None,
        };
        let canonical = procedure_v2_mutation_execution_document_v1(&admitted).unwrap();
        assert_eq!(
            decode_procedure_v2_mutation_execution_v1(canonical.as_str())
                .unwrap()
                .fresh_attempt_id,
            admitted.fresh_attempt_id
        );

        let mut missing_fresh: Value = serde_json::from_str(canonical.as_str()).unwrap();
        missing_fresh["fresh_attempt_id"] = Value::Null;
        assert!(decode_procedure_v2_mutation_execution_v1(&missing_fresh.to_string()).is_err());

        let mut oversized_reason: Value = serde_json::from_str(canonical.as_str()).unwrap();
        oversized_reason["payload"]["reason"] = json!("x".repeat(2_001));
        assert!(decode_procedure_v2_mutation_execution_v1(&oversized_reason.to_string()).is_err());
    }

    #[test]
    fn v2run005_skip_execution_preserves_optional_reason_and_fresh_identity() {
        let admitted = AdmittedProcedureV2MutationV1 {
            selector: WorktreeSelectorWireV1::new(b"/tmp/worktree", "/tmp/worktree", None).unwrap(),
            workspace_id: WorkspaceId::new("00000000-0000-4000-8000-000000000914").unwrap(),
            command: SliceCommandV1::SessionSkip(SessionSkipV1 {
                reason: Some("Skip the optional placement.".to_owned()),
                preconditions: podway_protocol::SessionMutationPreconditionsWireV1 {
                    expected_session_id: SessionId::new("00000000-0000-4000-8000-000000000912")
                        .unwrap(),
                    expected_session_revision: Revision::new(7),
                    expected_attempt_id: AttemptId::new("00000000-0000-4000-8000-000000000913")
                        .unwrap(),
                },
            }),
            fresh_attempt_id: Some(AttemptId::new("00000000-0000-4000-8000-000000000915").unwrap()),
            fresh_blocker_id: None,
            attached_artifact: None,
        };
        let canonical = procedure_v2_mutation_execution_document_v1(&admitted).unwrap();
        let decoded = decode_procedure_v2_mutation_execution_v1(canonical.as_str()).unwrap();
        assert_eq!(decoded.fresh_attempt_id, admitted.fresh_attempt_id);
        let SliceCommandV1::SessionSkip(decoded) = decoded.command else {
            panic!("decoded command is not session.skip");
        };
        assert_eq!(
            decoded.reason.as_deref(),
            Some("Skip the optional placement.")
        );

        let mut no_reason: Value = serde_json::from_str(canonical.as_str()).unwrap();
        no_reason["payload"]["reason"] = Value::Null;
        assert!(decode_procedure_v2_mutation_execution_v1(&no_reason.to_string()).is_ok());

        let mut blank_reason: Value = serde_json::from_str(canonical.as_str()).unwrap();
        blank_reason["payload"]["reason"] = json!("   ");
        assert!(decode_procedure_v2_mutation_execution_v1(&blank_reason.to_string()).is_err());

        let mut oversized_reason: Value = serde_json::from_str(canonical.as_str()).unwrap();
        oversized_reason["payload"]["reason"] = json!("x".repeat(2_001));
        assert!(decode_procedure_v2_mutation_execution_v1(&oversized_reason.to_string()).is_err());
    }

    #[test]
    fn v2drw001_decision_execution_freezes_every_semantic_field_and_fresh_identity() {
        let admitted = AdmittedProcedureV2DecisionV1 {
            selector: WorktreeSelectorWireV1::new(b"/tmp/worktree", "/tmp/worktree", None).unwrap(),
            workspace_id: WorkspaceId::new("00000000-0000-4000-8000-000000000924").unwrap(),
            command: SessionDecideV2 {
                option_id: podway_core::OptionId::new("accept").unwrap(),
                reason: "The resolved evidence supports this route.".to_owned(),
                actor: Some("reviewer".to_owned()),
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: SessionId::new("00000000-0000-4000-8000-000000000922")
                        .unwrap(),
                    expected_session_revision: Revision::new(7),
                    expected_attempt_id: AttemptId::new("00000000-0000-4000-8000-000000000923")
                        .unwrap(),
                },
            },
            fresh_attempt_id: AttemptId::new("00000000-0000-4000-8000-000000000925").unwrap(),
        };
        let canonical = procedure_v2_decision_execution_document_v1(&admitted).unwrap();
        let decoded = decode_procedure_v2_decision_execution_v1(canonical.as_str()).unwrap();
        assert_eq!(decoded.command, admitted.command);
        assert_eq!(decoded.fresh_attempt_id, admitted.fresh_attempt_id);

        let mut missing_fresh: Value = serde_json::from_str(canonical.as_str()).unwrap();
        missing_fresh["fresh_attempt_id"] = Value::Null;
        assert!(decode_procedure_v2_decision_execution_v1(&missing_fresh.to_string()).is_err());

        let mut blank_actor: Value = serde_json::from_str(canonical.as_str()).unwrap();
        blank_actor["payload"]["actor"] = json!("   ");
        assert!(decode_procedure_v2_decision_execution_v1(&blank_actor.to_string()).is_err());
    }
}
