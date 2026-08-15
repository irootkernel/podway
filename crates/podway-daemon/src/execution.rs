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
    ProcedureDocumentFormat, ProcedureSourceLabel, parse_procedure_document,
    sniff_procedure_schema, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, ArtifactLocationKindV1, ArtifactValueV1, AttemptId, AttemptLifecycle,
    AttemptNumberV2, AttemptValidityV2, AuthoringSeverity, BlockerId, CanonicalProcedureJsonV1,
    CriterionAssessmentReasonV2, CriterionAssessmentResultV2, CriterionCitationV2,
    CriterionStatusV2, DecisionRecordV2, DomainCommand, DomainError, DomainResult,
    GoalAssessmentRecordV2, GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2,
    GoalRevisionReasonV2, GoalStatementV2, GraphPlacementV2, ItemId, JobId, ProcedureSnapshotId,
    ProcedureSourceLabelV1, ReasonV2, ResolvedEvidenceReferenceV2, Revision, SessionAttemptV2,
    SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis,
    WorkspaceId, canonicalize_json_v1,
};
use podway_presets::{EmbeddedPresetV2, catalog_v2};
use podway_protocol::{
    GoalAssessCriterionV2, GoalCriterionWireV2, GoalDefineV2, GoalReviseV2, ItemAddV1,
    ItemAttachSourceV1, ItemAttachV1, ItemCheckV1, ItemClearV1, ItemRemoveV1, ItemSetV1,
    ItemUncheckV1, ProcedureV2MutationCommandV1, ProcedureV2MutationRequestV1,
    ProcedureV2StartCommandV1, ProcedureV2StartRequestV1, RequestIdV1, Rfc3339MillisV1,
    SessionBlockV1, SessionCancelV1, SessionCompleteV1, SessionDecideV2,
    SessionMutationPreconditionsWireV1, SessionResetV1, SessionRetryV1, SessionReworkV2,
    SessionSkipV1, SessionStartSourceV1, SessionStartV1, SessionUnblockV1, SliceCommandV1,
    SliceRequestV1, WorktreeSelectorWireV1, canonical_procedure_v2_mutation_identity_v1,
    canonical_procedure_v2_start_identity_v1, canonical_reset_all_identity_v1,
};
use podway_store::{
    ActiveItemMutationV2, AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1,
    AttemptMetadataV2, CanonicalExecutionJsonV1, ClaimedJobV1, DurableWorktreeIdentityV1,
    GraphMutationErrorV2, GraphNodeCounterV2, GraphSessionStateV2, GraphStartCurrentTaskV2,
    IdempotencyKeyV1, PersistedGraphMutationFailureV2, PersistedGraphTerminalOperationV2,
    PersistedResponseContextV1, ProcedureSnapshotV2, RevisionAttemptItemPreconditionsV1,
    StateTransitionV1, StoreContractV1, StoreErrorV1, StoreGraphMutationContractV2,
    StoreGraphReadContractV2, StoreIdempotencyReadContractV1, StoreValueErrorV1, TerminalReceiptV1,
    TerminalResultV1, WorkerIdV1, WorkflowMemoryStateV2, WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const EXECUTION_DOCUMENT_VERSION_V5: u8 = 5;
const EXECUTION_DOCUMENT_VERSION_V6: u8 = 6;
const EXECUTION_DOCUMENT_VERSION_V7: u8 = 7;
const EXECUTION_DOCUMENT_VERSION_V8: u8 = 8;
const EXECUTION_DOCUMENT_VERSION_V9: u8 = 9;
const EXECUTION_DOCUMENT_VERSION_V10: u8 = 10;
const EXECUTION_DOCUMENT_VERSION_V11: u8 = 11;
const EXECUTION_DOCUMENT_VERSION_V12: u8 = 12;
const EXECUTION_DOCUMENT_VERSION_V13: u8 = 13;
const EXECUTION_DOCUMENT_VERSION_V14: u8 = 14;
const EXECUTION_DOCUMENT_VERSION_V15: u8 = 15;

#[derive(Clone, Debug)]
enum AdmissionResolutionV1 {
    None,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalArtifactVerificationV2 {
    pub item_id: ItemId,
    pub location: String,
    pub digest: Sha256Digest,
    pub size_bytes: u64,
}

/// Outcome of inspecting one custom Procedure v2 source.
#[derive(Debug)]
pub enum ProcedureV2SourceAdmissionErrorV1 {
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
    GoalMutation(GraphMutationErrorV2),
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

fn embedded_preset_snapshot_v2(
    preset: EmbeddedPresetV2,
    snapshot_id: ProcedureSnapshotId,
    created_at: UnixMillis,
) -> Result<(ProcedureSnapshotV2, Sha256Digest), ProcedureV2SourceAdmissionErrorV1> {
    let admitted = preset.validate_source().map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "embedded Procedure v2 preset admission failed",
            },
        ))
    })?;
    let canonical_json = CanonicalProcedureJsonV1::new(
        admitted.canonical_json().as_str().to_owned(),
    )
    .map_err(|error| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
    })?;
    let source_label = preset.metadata.source_label().map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "embedded Procedure v2 preset source label is invalid",
            },
        ))
    })?;
    let source = ProcedureSourceLabelV1::new(source_label.display_label()).map_err(|error| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
    })?;
    let pinned_digest = admitted.pinned_digest().clone();
    let snapshot = ProcedureSnapshotV2::new(
        snapshot_id,
        canonical_json,
        admitted.digest().clone(),
        source,
        created_at,
    )
    .map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "embedded Procedure v2 preset snapshot admission failed",
            },
        ))
    })?;
    Ok((snapshot, pinned_digest))
}

impl ProcedureProviderV1 for EmbeddedPresetProcedureProviderV1 {
    fn load_preset_snapshot_v2(
        &self,
        preset: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<Option<(ProcedureSnapshotV2, Sha256Digest)>, ProcedureV2SourceAdmissionErrorV1>
    {
        let Some(preset) = catalog_v2().lookup(preset) else {
            return Ok(None);
        };
        embedded_preset_snapshot_v2(preset, snapshot_id, created_at).map(Some)
    }
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
    let format = procedure_document_format(procedure)
        .map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
    if sniff_procedure_schema(source, format) != Some(podway_core::PROCEDURE_SCHEMA_V2) {
        return Err(ProcedureV2SourceAdmissionErrorV1::SchemaInvalid {
            diagnostic_codes: vec!["AUTHORING_SCHEMA_INVALID".to_owned()],
        });
    }
    let source_text = std::str::from_utf8(source).map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::SchemaInvalid {
            diagnostic_codes: vec!["AUTHORING_SOURCE_NOT_UTF8".to_owned()],
        }
    })?;
    let parsed = match parse_procedure_document(source, format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
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

pub fn bind_initial_goal_for_start_v2(
    state: GraphSessionStateV2,
    initial_goal: &podway_protocol::InitialGoalWireV2,
    now: UnixMillis,
) -> Result<GraphSessionStateV2, ProcedureV2StartPreparationErrorV1> {
    let statement = GoalStatementV2::new(initial_goal.goal.clone())
        .map_err(ProcedureV2StartPreparationErrorV1::Domain)?;
    let criteria = goal_definition_from_wire_v2(&initial_goal.criteria)
        .map_err(ProcedureV2StartPreparationErrorV1::Domain)?;
    let actor = initial_goal
        .actor
        .clone()
        .map(ActorAttributionV2::new)
        .transpose()
        .map_err(ProcedureV2StartPreparationErrorV1::Domain)?;
    state
        .bind_initial_goal_at_start_v2(statement, criteria, actor, now)
        .map(|outcome| outcome.into_state())
        .map_err(ProcedureV2StartPreparationErrorV1::GoalMutation)
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

fn procedure_v2_typed_start_execution_document_v1(
    admitted: &AdmittedProcedureV2StartV1,
    request: &ProcedureV2StartRequestV1,
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
    let initial_goal = match request.command() {
        ProcedureV2StartCommandV1::SessionStart(start) => start.initial_goal.as_ref(),
        ProcedureV2StartCommandV1::SessionStartReplace(replace) => {
            replace.start.initial_goal.as_ref()
        }
    }
    .map(|goal| {
        json!({
            "actor": goal.actor,
            "criteria": goal.criteria,
            "goal": goal.goal,
        })
    });
    let document = json!({
        "command": request.command().command_name(),
        "execution_version": EXECUTION_DOCUMENT_VERSION_V12,
        "expected_current": expected_current,
        "first_attempt_id": admitted.state.trace().active_attempt().expect("fresh graph state has an active attempt").attempt_id(),
        "initial_goal": initial_goal,
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
        invalid_execution_v1("Procedure v2 goal-bearing start execution cannot be canonicalized")
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
    let execution_version = value_u64_v1(object, "execution_version")?;
    let keys = if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V12) {
        &[
            "command",
            "execution_version",
            "expected_current",
            "first_attempt_id",
            "initial_goal",
            "selector",
            "session_id",
            "snapshot",
            "task_title",
            "workspace_id",
        ][..]
    } else {
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
        ][..]
    };
    require_exact_keys_v1(object, keys)?;
    if execution_version != u64::from(EXECUTION_DOCUMENT_VERSION_V6)
        && execution_version != u64::from(EXECUTION_DOCUMENT_VERSION_V12)
    {
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
    let mut state = graph_session_state_from_procedure_v2_snapshot(
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
    if execution_version == u64::from(EXECUTION_DOCUMENT_VERSION_V12) {
        let initial_goal = value_v1(object, "initial_goal")?;
        if !initial_goal.is_null() {
            let goal = initial_goal
                .as_object()
                .ok_or_else(|| invalid_execution_v1("Procedure v2 initial goal is invalid"))?;
            require_exact_keys_v1(goal, &["actor", "criteria", "goal"])?;
            let statement = GoalStatementV2::new(value_string_v1(goal, "goal")?.to_owned())
                .map_err(ExecutionErrorV1::BoundaryDomain)?;
            let criteria = decode_goal_criteria_v2(value_v1(goal, "criteria")?)?;
            let actor = optional_string_v1(goal, "actor")?
                .map(|actor| ActorAttributionV2::new(actor.to_owned()))
                .transpose()
                .map_err(ExecutionErrorV1::BoundaryDomain)?;
            state = state
                .bind_initial_goal_at_start_v2(statement, criteria, actor, state.created_at())
                .map_err(|_| {
                    invalid_execution_v1("Procedure v2 initial goal cannot be reconstructed")
                })?
                .into_state();
        }
    }
    Ok(AdmittedProcedureV2StartV1 {
        selector: serde_json::from_value(value_v1(object, "selector")?.clone())
            .map_err(|_| invalid_execution_v1("Procedure v2 selector is invalid"))?,
        workspace_id: value_typed_v1(object, "workspace_id")?,
        replace,
        expected_current,
        state,
    })
}

fn decode_typed_start_replay_execution_v1(
    execution: &CanonicalExecutionJsonV1,
    expected_workspace_id: &WorkspaceId,
) -> Result<Option<AdmittedProcedureV2StartV1>, ExecutionErrorV1> {
    let version = serde_json::from_str::<Value>(execution.as_str())
        .ok()
        .and_then(|value| value.get("execution_version").and_then(Value::as_u64));
    if version != Some(u64::from(EXECUTION_DOCUMENT_VERSION_V12)) {
        return Ok(None);
    }
    let admitted = decode_procedure_v2_start_execution_v1(execution.as_str())?;
    if &admitted.workspace_id != expected_workspace_id {
        return Err(invalid_execution_v1(
            "typed Procedure v2 start replay workspace identity is invalid",
        ));
    }
    Ok(Some(admitted))
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
    execution_version: u8,
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
        "execution_version": EXECUTION_DOCUMENT_VERSION_V14,
        "fresh_attempt_id": admitted.fresh_attempt_id,
        "payload": {
            "actor": admitted.command.actor,
            "option_id": admitted.command.option_id,
            "reason": admitted.command.reason,
        },
        "preconditions": {
            "attempt_id": admitted.command.preconditions.expected_attempt_id,
            "goal_revision": admitted.command.expected_goal_revision,
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
    let version = value_u64_v1(object, "execution_version")?;
    if !matches!(version, value if value == u64::from(EXECUTION_DOCUMENT_VERSION_V9) || value == u64::from(EXECUTION_DOCUMENT_VERSION_V14))
        || value_string_v1(object, "command")? != "session.decide"
    {
        return Err(invalid_execution_v1(
            "Procedure v2 decision execution identity is invalid",
        ));
    }
    let payload = value_object_v1(object, "payload")?;
    require_exact_keys_v1(payload, &["actor", "option_id", "reason"])?;
    let preconditions = value_object_v1(object, "preconditions")?;
    match version {
        value if value == u64::from(EXECUTION_DOCUMENT_VERSION_V9) => require_exact_keys_v1(
            preconditions,
            &["attempt_id", "session_id", "session_revision"],
        )?,
        value if value == u64::from(EXECUTION_DOCUMENT_VERSION_V14) => require_exact_keys_v1(
            preconditions,
            &[
                "attempt_id",
                "goal_revision",
                "session_id",
                "session_revision",
            ],
        )?,
        _ => unreachable!("decision execution version was admitted above"),
    }
    let expected_goal_revision = if version == u64::from(EXECUTION_DOCUMENT_VERSION_V14) {
        let value = value_optional_typed_v1::<u64>(preconditions, "goal_revision")?;
        if value == Some(0) {
            return Err(invalid_execution_v1(
                "Procedure v2 decision goal revision is invalid",
            ));
        }
        value
    } else {
        None
    };
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
        expected_goal_revision,
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
        execution_version: u8::try_from(version).map_err(|_| {
            invalid_execution_v1("Procedure v2 decision execution version is invalid")
        })?,
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

#[derive(Clone, Debug)]
enum AdmittedProcedureV2GoalMutationV1 {
    Define {
        selector: WorktreeSelectorWireV1,
        workspace_id: WorkspaceId,
        command: GoalDefineV2,
    },
    Revise {
        selector: WorktreeSelectorWireV1,
        workspace_id: WorkspaceId,
        command: GoalReviseV2,
        fresh_attempt_id: AttemptId,
    },
}

fn procedure_v2_goal_execution_document_v1(
    admitted: &AdmittedProcedureV2GoalMutationV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let document = match admitted {
        AdmittedProcedureV2GoalMutationV1::Define {
            selector,
            workspace_id,
            command,
        } => json!({
            "command": "goal.define",
            "execution_version": EXECUTION_DOCUMENT_VERSION_V11,
            "fresh_attempt_id": null,
            "payload": { "actor": command.actor, "criteria": command.criteria, "goal": command.goal },
            "preconditions": {
                "attempt_id": null,
                "goal_revision": null,
                "session_id": command.preconditions.expected_session_id,
                "session_revision": command.preconditions.expected_session_revision,
            },
            "selector": selector,
            "workspace_id": workspace_id,
        }),
        AdmittedProcedureV2GoalMutationV1::Revise {
            selector,
            workspace_id,
            command,
            fresh_attempt_id,
        } => json!({
            "command": "goal.revise",
            "execution_version": EXECUTION_DOCUMENT_VERSION_V11,
            "fresh_attempt_id": fresh_attempt_id,
            "payload": {
                "actor": command.actor,
                "criteria": command.criteria,
                "goal": command.goal,
                "reactivate": command.reactivate,
                "reason": command.reason,
                "target_graph_node_id": command.target_graph_node_id,
            },
            "preconditions": {
                "attempt_id": command.preconditions.expected_attempt_id,
                "goal_revision": command.preconditions.expected_goal_revision,
                "session_id": command.preconditions.expected_session_id,
                "session_revision": command.preconditions.expected_session_revision,
            },
            "selector": selector,
            "workspace_id": workspace_id,
        }),
    };
    let canonical = canonicalize_json_v1(&document)
        .map_err(|_| invalid_execution_v1("Procedure v2 goal execution cannot be canonicalized"))?;
    CanonicalExecutionJsonV1::new(canonical).map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn decode_goal_criteria_wire_v2(
    value: &Value,
) -> Result<Vec<GoalCriterionWireV2>, ExecutionErrorV1> {
    let values = value
        .as_array()
        .ok_or_else(|| invalid_execution_v1("Procedure v2 goal criteria are invalid"))?;
    values
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| invalid_execution_v1("Procedure v2 goal criterion is invalid"))?;
            require_exact_keys_v1(object, &["criterion_id", "statement"])?;
            Ok(GoalCriterionWireV2 {
                criterion_id: value_typed_v1(object, "criterion_id")?,
                statement: value_string_v1(object, "statement")?.to_owned(),
            })
        })
        .collect()
}

fn decode_procedure_v2_goal_execution_v1(
    source: &str,
) -> Result<AdmittedProcedureV2GoalMutationV1, ExecutionErrorV1> {
    let root: Value = serde_json::from_str(source)
        .map_err(|_| invalid_execution_v1("Procedure v2 goal execution is not JSON"))?;
    let object = root
        .as_object()
        .ok_or_else(|| invalid_execution_v1("Procedure v2 goal execution root is invalid"))?;
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
    if value_u64_v1(object, "execution_version")? != u64::from(EXECUTION_DOCUMENT_VERSION_V11) {
        return Err(invalid_execution_v1(
            "Procedure v2 goal execution version is invalid",
        ));
    }
    let payload = value_object_v1(object, "payload")?;
    let preconditions = value_object_v1(object, "preconditions")?;
    require_exact_keys_v1(
        preconditions,
        &[
            "attempt_id",
            "goal_revision",
            "session_id",
            "session_revision",
        ],
    )?;
    let selector = serde_json::from_value(value_v1(object, "selector")?.clone())
        .map_err(|_| invalid_execution_v1("Procedure v2 goal selector is invalid"))?;
    let workspace_id = value_typed_v1(object, "workspace_id")?;
    match value_string_v1(object, "command")? {
        "goal.define" => {
            require_exact_keys_v1(payload, &["actor", "criteria", "goal"])?;
            if !value_v1(object, "fresh_attempt_id")?.is_null()
                || !value_v1(preconditions, "attempt_id")?.is_null()
                || !value_v1(preconditions, "goal_revision")?.is_null()
            {
                return Err(invalid_execution_v1(
                    "Procedure v2 goal definition resolutions are invalid",
                ));
            }
            Ok(AdmittedProcedureV2GoalMutationV1::Define {
                selector,
                workspace_id,
                command: GoalDefineV2 {
                    goal: value_string_v1(payload, "goal")?.to_owned(),
                    criteria: decode_goal_criteria_wire_v2(value_v1(payload, "criteria")?)?,
                    actor: value_optional_string_v1(payload, "actor")?,
                    preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                        expected_session_id: value_typed_v1(preconditions, "session_id")?,
                        expected_session_revision: Revision::new(value_u64_v1(
                            preconditions,
                            "session_revision",
                        )?),
                    },
                },
            })
        }
        "goal.revise" => {
            require_exact_keys_v1(
                payload,
                &[
                    "actor",
                    "criteria",
                    "goal",
                    "reactivate",
                    "reason",
                    "target_graph_node_id",
                ],
            )?;
            let expected_goal_revision = value_u64_v1(preconditions, "goal_revision")?;
            if expected_goal_revision == 0 {
                return Err(invalid_execution_v1(
                    "Procedure v2 goal revision is invalid",
                ));
            }
            Ok(AdmittedProcedureV2GoalMutationV1::Revise {
                selector,
                workspace_id,
                fresh_attempt_id: value_typed_v1(object, "fresh_attempt_id")?,
                command: GoalReviseV2 {
                    goal: value_string_v1(payload, "goal")?.to_owned(),
                    criteria: decode_goal_criteria_wire_v2(value_v1(payload, "criteria")?)?,
                    target_graph_node_id: value_typed_v1(payload, "target_graph_node_id")?,
                    reason: value_string_v1(payload, "reason")?.to_owned(),
                    actor: value_optional_string_v1(payload, "actor")?,
                    reactivate: value_v1(payload, "reactivate")?.as_bool().ok_or_else(|| {
                        invalid_execution_v1("Procedure v2 goal reactivate flag is invalid")
                    })?,
                    preconditions: podway_protocol::GoalRevisionPreconditionsWireV2 {
                        expected_session_id: value_typed_v1(preconditions, "session_id")?,
                        expected_session_revision: Revision::new(value_u64_v1(
                            preconditions,
                            "session_revision",
                        )?),
                        expected_attempt_id: value_optional_typed_v1(preconditions, "attempt_id")?,
                        expected_goal_revision,
                    },
                },
            })
        }
        _ => Err(invalid_execution_v1("Procedure v2 goal command is invalid")),
    }
}

fn validate_goal_claim_workspace_v1(
    admitted: &AdmittedProcedureV2GoalMutationV1,
    claimed_workspace_id: &WorkspaceId,
) -> Result<(), ExecutionErrorV1> {
    let admitted_workspace_id = match admitted {
        AdmittedProcedureV2GoalMutationV1::Define { workspace_id, .. }
        | AdmittedProcedureV2GoalMutationV1::Revise { workspace_id, .. } => workspace_id,
    };
    if admitted_workspace_id != claimed_workspace_id {
        return Err(invalid_execution_v1(
            "Procedure v2 goal workspace does not match the claim",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct AdmittedProcedureV2CriterionAssessmentV1 {
    selector: WorktreeSelectorWireV1,
    workspace_id: WorkspaceId,
    command: GoalAssessCriterionV2,
}

fn procedure_v2_criterion_assessment_execution_document_v1(
    admitted: &AdmittedProcedureV2CriterionAssessmentV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let command = &admitted.command;
    let document = json!({
        "command": "goal.assess_criterion",
        "execution_version": EXECUTION_DOCUMENT_VERSION_V13,
        "payload": {
            "actor": command.actor,
            "criterion_id": command.criterion_id,
            "evidence": command.evidence,
            "items": command.items,
            "reason": command.reason,
            "status": command.status,
        },
        "preconditions": {
            "attempt_id": command.preconditions.expected_attempt_id,
            "goal_revision": command.expected_goal_revision,
            "session_id": command.preconditions.expected_session_id,
            "session_revision": command.preconditions.expected_session_revision,
        },
        "selector": admitted.selector,
        "workspace_id": admitted.workspace_id,
    });
    let canonical = canonicalize_json_v1(&document).map_err(|_| {
        invalid_execution_v1("Procedure v2 criterion assessment execution cannot be canonicalized")
    })?;
    CanonicalExecutionJsonV1::new(canonical).map_err(ExecutionErrorV1::InvalidStoreValue)
}

fn decode_procedure_v2_criterion_assessment_execution_v1(
    source: &str,
) -> Result<AdmittedProcedureV2CriterionAssessmentV1, ExecutionErrorV1> {
    let root: Value = serde_json::from_str(source).map_err(|_| {
        invalid_execution_v1("Procedure v2 criterion assessment execution is not JSON")
    })?;
    let object = root.as_object().ok_or_else(|| {
        invalid_execution_v1("Procedure v2 criterion assessment execution root is invalid")
    })?;
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
    if value_u64_v1(object, "execution_version")? != u64::from(EXECUTION_DOCUMENT_VERSION_V13)
        || value_string_v1(object, "command")? != "goal.assess_criterion"
    {
        return Err(invalid_execution_v1(
            "Procedure v2 criterion assessment execution identity is invalid",
        ));
    }
    let payload = value_object_v1(object, "payload")?;
    require_exact_keys_v1(
        payload,
        &[
            "actor",
            "criterion_id",
            "evidence",
            "items",
            "reason",
            "status",
        ],
    )?;
    let preconditions = value_object_v1(object, "preconditions")?;
    require_exact_keys_v1(
        preconditions,
        &[
            "attempt_id",
            "goal_revision",
            "session_id",
            "session_revision",
        ],
    )?;
    let command = GoalAssessCriterionV2 {
        criterion_id: value_typed_v1(payload, "criterion_id")?,
        status: value_string_v1(payload, "status")?.to_owned(),
        reason: value_string_v1(payload, "reason")?.to_owned(),
        evidence: value_typed_v1(payload, "evidence")?,
        items: value_typed_v1(payload, "items")?,
        actor: value_optional_string_v1(payload, "actor")?,
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: value_typed_v1(preconditions, "session_id")?,
            expected_session_revision: Revision::new(value_u64_v1(
                preconditions,
                "session_revision",
            )?),
            expected_attempt_id: value_typed_v1(preconditions, "attempt_id")?,
        },
        expected_goal_revision: value_u64_v1(preconditions, "goal_revision")?,
    };
    validate_criterion_assessment_command_v2(&command)?;
    Ok(AdmittedProcedureV2CriterionAssessmentV1 {
        selector: serde_json::from_value(value_v1(object, "selector")?.clone()).map_err(|_| {
            invalid_execution_v1("Procedure v2 criterion assessment selector is invalid")
        })?,
        workspace_id: value_typed_v1(object, "workspace_id")?,
        command,
    })
}

fn criterion_assessment_result_from_wire_v2(
    command: &GoalAssessCriterionV2,
) -> Result<CriterionAssessmentResultV2, ExecutionErrorV1> {
    let status = command
        .status
        .parse::<CriterionStatusV2>()
        .map_err(|_| invalid_execution_v1("Procedure v2 criterion assessment status is invalid"))?;
    let citations = command
        .evidence
        .iter()
        .cloned()
        .map(CriterionCitationV2::Evidence)
        .chain(command.items.iter().cloned().map(CriterionCitationV2::Item))
        .collect();
    CriterionAssessmentResultV2::new(
        command.criterion_id.clone(),
        status,
        CriterionAssessmentReasonV2::new(command.reason.clone())
            .map_err(ExecutionErrorV1::BoundaryDomain)?,
        citations,
    )
    .map_err(ExecutionErrorV1::BoundaryDomain)
}

fn validate_criterion_assessment_command_v2(
    command: &GoalAssessCriterionV2,
) -> Result<(), ExecutionErrorV1> {
    if command.expected_goal_revision == 0 {
        return Err(invalid_execution_v1(
            "Procedure v2 criterion assessment goal revision is invalid",
        ));
    }
    criterion_assessment_result_from_wire_v2(command)?;
    command
        .actor
        .clone()
        .map(ActorAttributionV2::new)
        .transpose()
        .map_err(ExecutionErrorV1::BoundaryDomain)?;
    if command
        .evidence
        .iter()
        .enumerate()
        .any(|(index, value)| command.evidence[..index].contains(value))
        || command
            .items
            .iter()
            .enumerate()
            .any(|(index, value)| command.items[..index].contains(value))
    {
        return Err(invalid_execution_v1(
            "Procedure v2 criterion assessment citations are duplicated",
        ));
    }
    Ok(())
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

fn goal_revision_record_projection_v1(
    record: &podway_core::GoalRevisionRecordV2,
) -> Result<Value, ExecutionErrorV1> {
    let criteria = record
        .criteria()
        .criteria()
        .iter()
        .map(|criterion| {
            json!({
                "criterion_id": criterion.id(),
                "statement": criterion.statement(),
            })
        })
        .collect::<Vec<_>>();
    let mut value = if record.revision() == GoalRevisionNumberV2::FIRST {
        json!({
            "goal_revision": record.revision().get(),
            "statement": record.statement().as_str(),
            "criteria": criteria,
            "recorded_at": rfc3339_millis_execution_v1(record.created_at())?,
        })
    } else {
        json!({
            "goal_revision": record.revision().get(),
            "statement": record.statement().as_str(),
            "criteria": criteria,
            "reason": record.reason().expect("later goal revision has a reason").as_str(),
            "recorded_at": rfc3339_millis_execution_v1(record.created_at())?,
            "rework_to": record.rework_to().expect("later goal revision has a target"),
            "reactivated": record.reactivated(),
        })
    };
    if let Some(actor) = record.actor() {
        value
            .as_object_mut()
            .expect("goal record is an object")
            .insert("actor".to_owned(), json!(actor.as_str()));
    }
    Ok(value)
}

fn criterion_assessment_record_projection_v1(
    outcome: &podway_store::GraphCriterionAssessmentOutcomeV2,
) -> Result<Value, ExecutionErrorV1> {
    let assessment = outcome.assessment();
    let result = assessment.result();
    let citations = result
        .citations()
        .iter()
        .map(|citation| match citation {
            CriterionCitationV2::Evidence(graph_node_id) => {
                json!({"kind":"evidence", "graph_node_id":graph_node_id})
            }
            CriterionCitationV2::Item(item_id) => {
                json!({"kind":"item", "item_id":item_id})
            }
        })
        .collect::<Vec<_>>();
    let mut value = json!({
        "graph_node_id": outcome.graph_node_id(),
        "attempt_id": outcome.attempt_id(),
        "goal_revision": outcome.goal_revision().get(),
        "mode": result.mode().as_str(),
        "result": {
            "criterion_id": result.criterion_id(),
            "status": result.status().as_str(),
            "reason": result.reason().as_str(),
            "citations": citations,
        },
        "recorded_at": rfc3339_millis_execution_v1(assessment.recorded_at())?,
        "complete": outcome.complete(),
    });
    if let Some(actor) = assessment.actor() {
        value
            .as_object_mut()
            .expect("criterion assessment projection is an object")
            .insert("actor".to_owned(), json!(actor.as_str()));
    }
    if let Some(determined) = outcome.determined_outcome() {
        value
            .as_object_mut()
            .expect("criterion assessment projection is an object")
            .insert("determined_outcome".to_owned(), json!(determined.as_str()));
    }
    Ok(value)
}

fn decision_record_projection_v1(
    record: &DecisionRecordV2,
    assessment: Option<&GoalAssessmentRecordV2>,
) -> Result<Value, ExecutionErrorV1> {
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
    if let Some(assessment) = assessment {
        let criterion_results = assessment
            .criterion_results()
            .iter()
            .map(|result| {
                let citations = result
                    .citations()
                    .iter()
                    .map(|citation| match citation {
                        CriterionCitationV2::Evidence(graph_node_id) => json!({
                            "reference_graph_node_id": graph_node_id,
                        }),
                        CriterionCitationV2::Item(item_id) => json!({
                            "local_item_id": item_id,
                        }),
                    })
                    .collect::<Vec<_>>();
                json!({
                    "criterion_id": result.criterion_id(),
                    "status": result.status().as_str(),
                    "reason": result.reason().as_str(),
                    "citations": citations,
                })
            })
            .collect::<Vec<_>>();
        let object = value
            .as_object_mut()
            .expect("decision record projection is an object");
        object.insert("assessment".to_owned(), json!("session_goal"));
        object.insert(
            "assessment_mode".to_owned(),
            json!(assessment.mode().as_str()),
        );
        object.insert(
            "goal_outcome".to_owned(),
            json!(assessment.outcome().as_str()),
        );
        object.insert(
            "criterion_results".to_owned(),
            Value::Array(criterion_results),
        );
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
    let ParsedProcedure::V2(parsed) = parsed;
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

fn procedure_document_format(
    procedure: &str,
) -> Result<ProcedureDocumentFormat, ExecutionBoundaryErrorV1> {
    match std::path::Path::new(procedure)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("json") => Ok(ProcedureDocumentFormat::Json),
        Some("yaml" | "yml") => Ok(ProcedureDocumentFormat::Yaml),
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
    ) -> Result<LocalArtifactVerificationV2, ExecutionBoundaryErrorV1>;
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
        if !matches!(request.command(), SliceCommandV1::WorkspaceInit(_)) {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "only workspace initialization uses the bootstrap executor",
            });
        }
        let binding = self
            .bound_workspace(request.selector())
            .map_err(ExecutionErrorV1::from_boundary)?;
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
        let request_digest = request_digest_v1(request, binding.identity().workspace_uuid(), None)?;
        if let Some(outcome) = self.store.read_idempotent_outcome(
            binding.identity(),
            &idempotency_key,
            &request_digest,
        )? {
            return Ok(outcome);
        }
        let command = command_for_admission_v1(request.command())?;
        let preconditions = store_preconditions_v1(request.command())?;
        let now = self.clock.now();
        let canonical_execution = canonical_execution_document_v1(
            request,
            binding.identity(),
            &AdmissionResolutionV1::None,
        )?;
        let admitted = AdmitRequestV1::new_with_canonical_execution(
            command,
            idempotency_key,
            self.ids.next_job_id(),
            preconditions,
            request_digest,
            now,
            canonical_execution,
        );
        let admitted = match response_context {
            Some(context) => admitted.with_response_context(context),
            None => admitted,
        };
        let outcome = self
            .store
            .admit(binding.identity(), admitted)
            .map_err(ExecutionErrorV1::from)?;
        Ok(outcome)
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

        if !matches!(command, SliceCommandV1::WorkspaceInit(_))
            || !matches!(resolution, AdmissionResolutionV1::None)
        {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "bootstrap executor received a non-initialization command",
            });
        }
        let admitted_command = command_for_admission_v1(&command)?;
        let admitted_preconditions = store_preconditions_v1(&command)?;
        if claimed.execution().command() != &admitted_command
            || claimed.execution().preconditions() != &admitted_preconditions
        {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "document does not match Store execution metadata",
            });
        }

        self.commit_workspace_initialization(&claimed, now)
    }

    fn commit_workspace_initialization(
        &self,
        claimed: &ClaimedJobV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        let transition = StateTransitionV1::new_persisted(None, Revision::ZERO, Revision::ZERO)
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
    /// Attempts the Procedure v2 start path after runtime admission.
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
            SessionStartSourceV1::Procedure { procedure } => prepare_custom_procedure_v2_start(
                &self.procedures,
                &binding,
                procedure,
                start.expected_procedure_digest.as_ref(),
                &start.task_title,
                session_id,
                first_attempt_id,
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

    /// Durably admits the typed start boundary used when revision 1 is supplied at session start.
    pub fn admit_procedure_v2_typed_start_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &ProcedureV2StartRequestV1,
        idempotency_key: IdempotencyKeyV1,
        response_context: Option<PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ProcedureV2StartPreparationErrorV1> {
        let (start, initial_goal, expected_current, command) = match request.command() {
            ProcedureV2StartCommandV1::SessionStart(input) => (
                &input.start,
                input.initial_goal.as_ref(),
                GraphStartCurrentTaskV2::Absent,
                DomainCommand::SessionStart,
            ),
            ProcedureV2StartCommandV1::SessionStartReplace(input) => (
                &input.start.start,
                input.start.initial_goal.as_ref(),
                GraphStartCurrentTaskV2::Exact {
                    session_id: input.preconditions.expected_session_id.clone(),
                    session_revision: input.preconditions.expected_session_revision,
                },
                DomainCommand::SessionStartReplace,
            ),
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
            let Some(admitted) = decode_typed_start_replay_execution_v1(
                canonical_execution,
                expected_workspace.identity().workspace_uuid(),
            )
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?
            else {
                return Ok(None);
            };
            let actual = procedure_v2_typed_start_request_digest_v1(
                request,
                expected_workspace.identity().workspace_uuid(),
                admitted.state.snapshot().digest(),
            )
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
            if existing.request_digest() != &actual {
                return Err(ProcedureV2StartPreparationErrorV1::Execution(
                    StoreErrorV1::IdempotencyDigestConflictV1 {
                        expected: existing.request_digest().clone(),
                        actual,
                    }
                    .into(),
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
                invalid_execution_v1("typed Procedure v2 start workspace identity changed"),
            ));
        }
        let now = self.clock.now();
        let session_id = self.ids.next_session_id();
        let first_attempt_id = self.ids.next_attempt_id();
        let snapshot_id = self.ids.next_procedure_snapshot_id();
        let mut state = match &start.source {
            SessionStartSourceV1::Procedure { procedure } => prepare_custom_procedure_v2_start(
                &self.procedures,
                &binding,
                procedure,
                start.expected_procedure_digest.as_ref(),
                &start.task_title,
                session_id,
                first_attempt_id,
                snapshot_id,
                now,
            )?,
            SessionStartSourceV1::Preset { preset } => {
                let Some(state) = prepare_preset_procedure_v2_start(
                    &self.procedures,
                    preset,
                    &start.task_title,
                    session_id,
                    first_attempt_id,
                    snapshot_id,
                    now,
                )?
                else {
                    return Ok(None);
                };
                state
            }
        };
        if let Some(initial_goal) = initial_goal {
            state = bind_initial_goal_for_start_v2(state, initial_goal, now)?;
        }
        let admitted = AdmittedProcedureV2StartV1 {
            selector: request.selector().clone(),
            workspace_id: binding.identity().workspace_uuid().clone(),
            replace: matches!(
                request.command(),
                ProcedureV2StartCommandV1::SessionStartReplace(_)
            ),
            expected_current,
            state,
        };
        let canonical_execution =
            procedure_v2_typed_start_execution_document_v1(&admitted, request)
                .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
        let request_digest = procedure_v2_typed_start_request_digest_v1(
            request,
            binding.identity().workspace_uuid(),
            admitted.state.snapshot().digest(),
        )
        .map_err(ProcedureV2StartPreparationErrorV1::Execution)?;
        let preconditions = match request.command() {
            ProcedureV2StartCommandV1::SessionStart(_) => {
                RevisionAttemptItemPreconditionsV1::new(None, None, None, None)
                    .map_err(ProcedureV2StartPreparationErrorV1::InvalidStoreValue)?
            }
            ProcedureV2StartCommandV1::SessionStartReplace(input) => {
                RevisionAttemptItemPreconditionsV1::new(
                    Some(input.preconditions.expected_session_revision),
                    None,
                    None,
                    None,
                )
                .map_err(ProcedureV2StartPreparationErrorV1::InvalidStoreValue)?
            }
        };
        let durable = AdmitRequestV1::new_with_canonical_execution(
            command,
            idempotency_key,
            self.ids.next_job_id(),
            preconditions,
            request_digest,
            now,
            canonical_execution,
        )
        .with_procedure_v2_execution()
        .with_session_identity(match request.command() {
            ProcedureV2StartCommandV1::SessionStart(_) => AdmissionSessionIdentityV1::Absent,
            ProcedureV2StartCommandV1::SessionStartReplace(input) => {
                AdmissionSessionIdentityV1::Exact(input.preconditions.expected_session_id.clone())
            }
        });
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
    /// document freezes all generated IDs and attachment metadata before admission.
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
            let expected = expected_session_id_v1(request.command()).ok_or_else(|| {
                invalid_execution_v1("Procedure v2 graph mutation is missing a session identity")
            })?;
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: expected.clone(),
                actual: None,
            });
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
            | ProcedureV2MutationCommandV1::GoalRevise(_) => self
                .admit_procedure_v2_goal_mutation_for_workspace_with_response_context(
                    expected_workspace,
                    request,
                    idempotency_key,
                    response_context,
                ),
            ProcedureV2MutationCommandV1::GoalAssessCriterion(command) => self
                .admit_procedure_v2_criterion_assessment_for_workspace_with_response_context(
                    expected_workspace,
                    request,
                    command,
                    idempotency_key,
                    response_context,
                ),
        }
    }

    fn admit_procedure_v2_criterion_assessment_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &ProcedureV2MutationRequestV1,
        command: &GoalAssessCriterionV2,
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
            let admitted = decode_procedure_v2_criterion_assessment_execution_v1(
                canonical_execution.as_str(),
            )?;
            if admitted.workspace_id != *expected_workspace.identity().workspace_uuid() {
                return Err(invalid_execution_v1(
                    "Procedure v2 criterion assessment replay workspace identity is invalid",
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
            return Err(invalid_execution_v1(
                "Procedure v2 criterion assessment workspace identity changed",
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(binding.identity())?;
        let Some(state) = view.graph_state() else {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: command.preconditions.expected_session_id.clone(),
                actual: None,
            });
        };
        if state.trace().session_id() != &command.preconditions.expected_session_id {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: command.preconditions.expected_session_id.clone(),
                actual: Some(state.trace().session_id().clone()),
            });
        }
        if let Some(active) = state.trace().active_attempt() {
            let exact_active_fences = state.trace().revision()
                == command.preconditions.expected_session_revision
                && active.attempt_id() == &command.preconditions.expected_attempt_id;
            let expected_goal_revision = GoalRevisionNumberV2::new(command.expected_goal_revision);
            let exact_goal_fence = state.goal_state().current_revision()
                == Some(expected_goal_revision)
                && active.goal_revision() == Some(expected_goal_revision);
            let active_is_goal_assessment = state
                .snapshot()
                .graph_nodes()
                .iter()
                .find(|node| node.graph_node_id() == active.graph_node_id())
                .is_some_and(|node| node.goal_assessment());
            if state.snapshot().goal_tracking()
                && exact_active_fences
                && exact_goal_fence
                && !active_is_goal_assessment
            {
                return Err(ExecutionErrorV1::BoundaryDomain(
                    DomainError::InvalidState {
                        reason: "criterion assessment requires an active goal-assessment decision",
                    },
                ));
            }
        }
        validate_criterion_assessment_command_v2(command)?;
        let admitted = AdmittedProcedureV2CriterionAssessmentV1 {
            selector: request.selector().clone(),
            workspace_id: binding.identity().workspace_uuid().clone(),
            command: command.clone(),
        };
        let canonical_execution =
            procedure_v2_criterion_assessment_execution_document_v1(&admitted)?;
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
            DomainCommand::GoalAssessCriterion,
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

    fn admit_procedure_v2_goal_mutation_for_workspace_with_response_context(
        &self,
        expected_workspace: &WorkspaceBindingV1,
        request: &ProcedureV2MutationRequestV1,
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
            let admitted = decode_procedure_v2_goal_execution_v1(canonical_execution.as_str())?;
            let workspace_id = match &admitted {
                AdmittedProcedureV2GoalMutationV1::Define { workspace_id, .. }
                | AdmittedProcedureV2GoalMutationV1::Revise { workspace_id, .. } => workspace_id,
            };
            if workspace_id != expected_workspace.identity().workspace_uuid() {
                return Err(invalid_execution_v1(
                    "Procedure v2 goal replay workspace identity is invalid",
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
            return Err(invalid_execution_v1(
                "Procedure v2 goal workspace identity changed",
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(binding.identity())?;
        let Some(state) = view.graph_state() else {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: procedure_v2_mutation_session_id_v1(request.command()).clone(),
                actual: None,
            });
        };
        let (admitted, domain_command, preconditions, session_id) = match request.command() {
            ProcedureV2MutationCommandV1::GoalDefine(command) => {
                GoalStatementV2::new(command.goal.clone())
                    .map_err(ExecutionErrorV1::BoundaryDomain)?;
                goal_definition_from_wire_v2(&command.criteria)
                    .map_err(ExecutionErrorV1::BoundaryDomain)?;
                command
                    .actor
                    .clone()
                    .map(ActorAttributionV2::new)
                    .transpose()
                    .map_err(ExecutionErrorV1::BoundaryDomain)?;
                (
                    AdmittedProcedureV2GoalMutationV1::Define {
                        selector: request.selector().clone(),
                        workspace_id: binding.identity().workspace_uuid().clone(),
                        command: command.clone(),
                    },
                    DomainCommand::GoalDefine,
                    RevisionAttemptItemPreconditionsV1::new(
                        Some(command.preconditions.expected_session_revision),
                        None,
                        None,
                        None,
                    )
                    .map_err(ExecutionErrorV1::InvalidStoreValue)?,
                    command.preconditions.expected_session_id.clone(),
                )
            }
            ProcedureV2MutationCommandV1::GoalRevise(command) => {
                if state.trace().lifecycle() == podway_core::SessionLifecycle::Running
                    && command.preconditions.expected_attempt_id.is_none()
                {
                    return Err(ExecutionErrorV1::BoundaryDomain(
                        DomainError::InvalidState {
                            reason: "running Procedure v2 goal revision requires an attempt fence",
                        },
                    ));
                }
                GoalStatementV2::new(command.goal.clone())
                    .map_err(ExecutionErrorV1::BoundaryDomain)?;
                goal_definition_from_wire_v2(&command.criteria)
                    .map_err(ExecutionErrorV1::BoundaryDomain)?;
                GoalRevisionReasonV2::new(command.reason.clone())
                    .map_err(ExecutionErrorV1::BoundaryDomain)?;
                command
                    .actor
                    .clone()
                    .map(ActorAttributionV2::new)
                    .transpose()
                    .map_err(ExecutionErrorV1::BoundaryDomain)?;
                (
                    AdmittedProcedureV2GoalMutationV1::Revise {
                        selector: request.selector().clone(),
                        workspace_id: binding.identity().workspace_uuid().clone(),
                        command: command.clone(),
                        fresh_attempt_id: self.ids.next_attempt_id(),
                    },
                    DomainCommand::GoalRevise,
                    RevisionAttemptItemPreconditionsV1::new(
                        Some(command.preconditions.expected_session_revision),
                        command.preconditions.expected_attempt_id.clone(),
                        None,
                        None,
                    )
                    .map_err(ExecutionErrorV1::InvalidStoreValue)?,
                    command.preconditions.expected_session_id.clone(),
                )
            }
            _ => return Ok(None),
        };
        if state.trace().session_id() != &session_id {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: session_id,
                actual: Some(state.trace().session_id().clone()),
            });
        }
        let canonical_execution = procedure_v2_goal_execution_document_v1(&admitted)?;
        let request_digest = procedure_v2_typed_mutation_request_digest_v1(
            request,
            binding.identity().workspace_uuid(),
        )?;
        let durable = AdmitRequestV1::new_with_canonical_execution(
            domain_command,
            idempotency_key,
            self.ids.next_job_id(),
            preconditions,
            request_digest,
            self.clock.now(),
            canonical_execution,
        )
        .with_procedure_v2_execution()
        .with_session_identity(AdmissionSessionIdentityV1::Exact(session_id));
        let durable = match response_context {
            Some(context) => durable.with_response_context(context),
            None => durable,
        };
        self.store
            .admit(binding.identity(), durable)
            .map(Some)
            .map_err(Into::into)
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
            if !matches!(
                version,
                Some(value)
                    if value == u64::from(EXECUTION_DOCUMENT_VERSION_V9)
                        || value == u64::from(EXECUTION_DOCUMENT_VERSION_V14)
            ) {
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
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: command.preconditions.expected_session_id.clone(),
                actual: None,
            });
        };
        if state.trace().session_id() != &command.preconditions.expected_session_id {
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: command.preconditions.expected_session_id.clone(),
                actual: Some(state.trace().session_id().clone()),
            });
        }
        let active = state
            .trace()
            .active_attempt()
            .ok_or_else(|| invalid_execution_v1("Procedure v2 decision has no active attempt"))?;
        let goal_assessment = state
            .snapshot()
            .graph_nodes()
            .iter()
            .find(|node| node.graph_node_id() == active.graph_node_id())
            .ok_or_else(|| invalid_execution_v1("Procedure v2 decision node is absent"))?
            .goal_assessment();
        let exact_active_fences = state.trace().revision()
            == command.preconditions.expected_session_revision
            && active.attempt_id() == &command.preconditions.expected_attempt_id;
        if exact_active_fences && goal_assessment && command.expected_goal_revision.is_none() {
            return Err(ExecutionErrorV1::BoundaryDomain(
                DomainError::InvalidState {
                    reason: "goal-assessment decisions require a goal revision precondition",
                },
            ));
        }
        ReasonV2::new(command.reason.clone()).map_err(ExecutionErrorV1::BoundaryDomain)?;
        command
            .actor
            .clone()
            .map(ActorAttributionV2::new)
            .transpose()
            .map_err(ExecutionErrorV1::BoundaryDomain)?;
        let admitted = AdmittedProcedureV2DecisionV1 {
            execution_version: EXECUTION_DOCUMENT_VERSION_V14,
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
            return Err(ExecutionErrorV1::SessionIdentityMismatch {
                expected: command.preconditions.expected_session_id.clone(),
                actual: None,
            });
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
            version
                if version == u64::from(EXECUTION_DOCUMENT_VERSION_V9)
                    || version == u64::from(EXECUTION_DOCUMENT_VERSION_V14) =>
            {
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
            version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V11) => {
                let admitted = decode_procedure_v2_goal_execution_v1(
                    claimed.execution().canonical_execution().as_str(),
                )?;
                self.execute_procedure_v2_goal_mutation_claimed(&claimed, admitted, now)?
            }
            version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V12) => {
                let admitted = decode_procedure_v2_start_execution_v1(
                    claimed.execution().canonical_execution().as_str(),
                )?;
                if admitted.workspace_id != *claimed.claim().identity().workspace_uuid() {
                    return Err(invalid_execution_v1(
                        "Procedure v2 typed start workspace does not match the claim",
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
            version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V13) => {
                let admitted = decode_procedure_v2_criterion_assessment_execution_v1(
                    claimed.execution().canonical_execution().as_str(),
                )?;
                self.execute_procedure_v2_criterion_assessment_claimed(&claimed, admitted, now)?
            }
            _ => {
                return Err(invalid_execution_v1(
                    "Procedure v2 execution version is unsupported",
                ));
            }
        };
        Ok(Some(receipt))
    }

    fn execute_procedure_v2_criterion_assessment_claimed(
        &self,
        claimed: &ClaimedJobV1,
        admitted: AdmittedProcedureV2CriterionAssessmentV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        if admitted.workspace_id != *claimed.claim().identity().workspace_uuid() {
            return Err(invalid_execution_v1(
                "Procedure v2 criterion assessment workspace does not match the claim",
            ));
        }
        let command = admitted.command;
        let expected_preconditions = RevisionAttemptItemPreconditionsV1::new(
            Some(command.preconditions.expected_session_revision),
            Some(command.preconditions.expected_attempt_id.clone()),
            None,
            None,
        )
        .map_err(ExecutionErrorV1::InvalidStoreValue)?;
        if claimed.execution().command() != &DomainCommand::GoalAssessCriterion
            || claimed.execution().preconditions() != &expected_preconditions
            || claimed.execution().session_identity()
                != &AdmissionSessionIdentityV1::Exact(
                    command.preconditions.expected_session_id.clone(),
                )
        {
            return Err(invalid_execution_v1(
                "Procedure v2 criterion assessment document does not match durable metadata",
            ));
        }
        let view = self
            .store
            .read_graph_workspace_view_v2(claimed.claim().identity())?;
        let state = view.graph_state().ok_or_else(|| {
            invalid_execution_v1("Procedure v2 criterion assessment has no graph session")
        })?;
        if state.trace().session_id() != &command.preconditions.expected_session_id {
            return self.commit_domain_failure(
                claimed,
                Revision::ZERO,
                DomainError::SessionIdentityMismatch {
                    expected: command.preconditions.expected_session_id,
                    actual: Some(state.trace().session_id().clone()),
                },
                now,
            );
        }
        let revision_before = state.trace().revision();
        let result = criterion_assessment_result_from_wire_v2(&command)?;
        let outcome = state.assess_goal_criterion_v2(
            command.preconditions.expected_session_revision,
            &command.preconditions.expected_attempt_id,
            GoalRevisionNumberV2::new(command.expected_goal_revision),
            result,
            command
                .actor
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(ExecutionErrorV1::BoundaryDomain)?,
            now,
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
            }
        };
        let operation = PersistedGraphTerminalOperationV2::goal_assess_criterion(
            criterion_assessment_record_projection_v1(&outcome)?,
        )
        .map_err(|_| invalid_execution_v1("criterion assessment terminal operation is invalid"))?;
        let next = outcome.into_state();
        let result = DomainResult::SessionChanged {
            session_id: state.trace().session_id().clone(),
            revision_before,
            revision_after: next.trace().revision(),
            changed: true,
        };
        self.store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                state.workspace_revision(),
                revision_before,
                Some(next),
                TerminalResultV1::Success(result),
                operation,
                now,
            )
            .map_err(Into::into)
    }

    fn execute_procedure_v2_goal_mutation_claimed(
        &self,
        claimed: &ClaimedJobV1,
        admitted: AdmittedProcedureV2GoalMutationV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, ExecutionErrorV1> {
        validate_goal_claim_workspace_v1(&admitted, claimed.claim().identity().workspace_uuid())?;
        let view = self
            .store
            .read_graph_workspace_view_v2(claimed.claim().identity())?;
        let state = view.graph_state().ok_or_else(|| {
            invalid_execution_v1("Procedure v2 goal mutation has no graph session")
        })?;
        let (expected_session_id, expected_preconditions, domain_command) = match &admitted {
            AdmittedProcedureV2GoalMutationV1::Define { command, .. } => (
                &command.preconditions.expected_session_id,
                RevisionAttemptItemPreconditionsV1::new(
                    Some(command.preconditions.expected_session_revision),
                    None,
                    None,
                    None,
                )
                .map_err(ExecutionErrorV1::InvalidStoreValue)?,
                DomainCommand::GoalDefine,
            ),
            AdmittedProcedureV2GoalMutationV1::Revise { command, .. } => (
                &command.preconditions.expected_session_id,
                RevisionAttemptItemPreconditionsV1::new(
                    Some(command.preconditions.expected_session_revision),
                    command.preconditions.expected_attempt_id.clone(),
                    None,
                    None,
                )
                .map_err(ExecutionErrorV1::InvalidStoreValue)?,
                DomainCommand::GoalRevise,
            ),
        };
        if claimed.execution().command() != &domain_command
            || claimed.execution().preconditions() != &expected_preconditions
            || claimed.execution().session_identity()
                != &AdmissionSessionIdentityV1::Exact(expected_session_id.clone())
        {
            return Err(invalid_execution_v1(
                "Procedure v2 goal document does not match durable metadata",
            ));
        }
        if state.trace().session_id() != expected_session_id {
            return self.commit_domain_failure(
                claimed,
                Revision::ZERO,
                DomainError::SessionIdentityMismatch {
                    expected: expected_session_id.clone(),
                    actual: Some(state.trace().session_id().clone()),
                },
                now,
            );
        }
        let revision_before = state.trace().revision();
        let outcome = match admitted {
            AdmittedProcedureV2GoalMutationV1::Define { command, .. } => {
                let result = state.define_goal_v2(
                    command.preconditions.expected_session_revision,
                    GoalStatementV2::new(command.goal).map_err(ExecutionErrorV1::BoundaryDomain)?,
                    goal_definition_from_wire_v2(&command.criteria)
                        .map_err(ExecutionErrorV1::BoundaryDomain)?,
                    command
                        .actor
                        .map(ActorAttributionV2::new)
                        .transpose()
                        .map_err(ExecutionErrorV1::BoundaryDomain)?,
                    now,
                );
                match result {
                    Ok(outcome) => {
                        let operation = PersistedGraphTerminalOperationV2::goal_define(
                            goal_revision_record_projection_v1(outcome.revision())?,
                        )
                        .map_err(|_| {
                            invalid_execution_v1("goal definition terminal operation is invalid")
                        })?;
                        (outcome.into_state(), operation)
                    }
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                }
            }
            AdmittedProcedureV2GoalMutationV1::Revise {
                command,
                fresh_attempt_id,
                ..
            } => {
                let result = state.revise_goal_v2(
                    command.preconditions.expected_session_revision,
                    command.preconditions.expected_attempt_id.as_ref(),
                    GoalRevisionNumberV2::new(command.preconditions.expected_goal_revision),
                    GoalStatementV2::new(command.goal).map_err(ExecutionErrorV1::BoundaryDomain)?,
                    goal_definition_from_wire_v2(&command.criteria)
                        .map_err(ExecutionErrorV1::BoundaryDomain)?,
                    command.target_graph_node_id,
                    fresh_attempt_id,
                    GoalRevisionReasonV2::new(command.reason)
                        .map_err(ExecutionErrorV1::BoundaryDomain)?,
                    command
                        .actor
                        .map(ActorAttributionV2::new)
                        .transpose()
                        .map_err(ExecutionErrorV1::BoundaryDomain)?,
                    command.reactivate,
                    now,
                );
                match result {
                    Ok(outcome) => {
                        let operation = PersistedGraphTerminalOperationV2::goal_revise(
                            goal_revision_record_projection_v1(outcome.revision())?,
                            outcome.target_attempt_id().clone(),
                        )
                        .map_err(|_| {
                            invalid_execution_v1("goal revision terminal operation is invalid")
                        })?;
                        (outcome.into_state(), operation)
                    }
                    Err(error) => {
                        return self.commit_graph_mutation_failure_v2(claimed, state, &error, now);
                    }
                }
            }
        };
        let result = DomainResult::SessionChanged {
            session_id: state.trace().session_id().clone(),
            revision_before,
            revision_after: outcome.0.trace().revision(),
            changed: true,
        };
        self.store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                state.workspace_revision(),
                revision_before,
                Some(outcome.0),
                TerminalResultV1::Success(result),
                outcome.1,
                now,
            )
            .map_err(Into::into)
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
        let outcome = match admitted.execution_version {
            EXECUTION_DOCUMENT_VERSION_V9 => state.decide_active_route_v2(
                admitted.command.preconditions.expected_session_revision,
                &admitted.command.preconditions.expected_attempt_id,
                admitted.command.option_id,
                admitted.fresh_attempt_id,
                Some(reason),
                actor,
                now,
            ),
            EXECUTION_DOCUMENT_VERSION_V14 => state.decide_active_route_with_goal_revision_v2(
                admitted.command.preconditions.expected_session_revision,
                &admitted.command.preconditions.expected_attempt_id,
                admitted.command.option_id,
                admitted.fresh_attempt_id,
                admitted
                    .command
                    .expected_goal_revision
                    .map(GoalRevisionNumberV2::new),
                Some(reason),
                actor,
                now,
            ),
            _ => {
                return Err(invalid_execution_v1(
                    "Procedure v2 decision execution version is invalid",
                ));
            }
        };
        let outcome = match outcome {
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
            decision_record_projection_v1(
                outcome.decision_record(),
                outcome.goal_assessment_record(),
            )?,
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
        SliceCommandV1::SessionBlock(_) => DomainCommand::SessionBlock,
        SliceCommandV1::SessionUnblock(_) => DomainCommand::SessionUnblock,
        SliceCommandV1::SessionCancel(_) => DomainCommand::SessionCancel,
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
        | SliceCommandV1::SessionObserve(_)
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
        | SliceCommandV1::SessionObserve(_)
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
        SliceCommandV1::SessionBlock(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionUnblock(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionCancel(input) => Some(&input.preconditions.expected_session_id),
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
        | SliceCommandV1::SessionObserve(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobLookup(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => None,
    }
}

fn procedure_v2_mutation_session_id_v1(command: &ProcedureV2MutationCommandV1) -> &SessionId {
    match command {
        ProcedureV2MutationCommandV1::SessionDecide(input) => {
            &input.preconditions.expected_session_id
        }
        ProcedureV2MutationCommandV1::SessionRework(input) => {
            &input.preconditions.expected_session_id
        }
        ProcedureV2MutationCommandV1::GoalDefine(input) => &input.preconditions.expected_session_id,
        ProcedureV2MutationCommandV1::GoalRevise(input) => &input.preconditions.expected_session_id,
        ProcedureV2MutationCommandV1::GoalAssessCriterion(input) => {
            &input.preconditions.expected_session_id
        }
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

fn canonical_execution_document_v1(
    request: &SliceRequestV1,
    identity: &DurableWorktreeIdentityV1,
    _resolution: &AdmissionResolutionV1,
) -> Result<CanonicalExecutionJsonV1, ExecutionErrorV1> {
    let (preconditions, payload) = execution_components_v1(request.command());
    let document = json!({
        "command": request.command().command_name(),
        "execution": { "kind": "workspace_bootstrap" },
        "execution_version": EXECUTION_DOCUMENT_VERSION_V15,
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

fn procedure_v2_typed_start_request_digest_v1(
    request: &ProcedureV2StartRequestV1,
    workspace_id: &WorkspaceId,
    procedure_digest: &Sha256Digest,
) -> Result<Sha256Digest, ExecutionErrorV1> {
    let canonical =
        canonical_procedure_v2_start_identity_v1(request, workspace_id, procedure_digest).map_err(
            |_| invalid_execution_v1("Procedure v2 start identity cannot be canonicalized"),
        )?;
    Sha256Digest::new(format!("sha256:{}", sha256_hex_v1(canonical.as_bytes())))
        .map_err(|_| invalid_execution_v1("Procedure v2 start identity digest is invalid"))
}

fn goal_definition_from_wire_v2(
    criteria: &[GoalCriterionWireV2],
) -> Result<GoalDefinitionV2, DomainError> {
    GoalDefinitionV2::new(
        criteria
            .iter()
            .map(|criterion| {
                GoalCriterionV2::new(criterion.criterion_id.clone(), criterion.statement.clone())
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
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
        | SliceCommandV1::SessionObserve(_)
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
        version if version == u64::from(EXECUTION_DOCUMENT_VERSION_V15) => {
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
            let execution = value_object_v1(object, "execution")?;
            require_exact_keys_v1(execution, &["kind"])?;
            if value_string_v1(execution, "kind")? != "workspace_bootstrap" {
                return Err(invalid_execution_v1(
                    "invalid workspace bootstrap execution",
                ));
            }
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
    if !matches!(command, SliceCommandV1::WorkspaceInit(_)) {
        return Err(invalid_execution_v1(
            "workspace bootstrap document contains another command",
        ));
    }
    Ok((version, selector, workspace_id, command, resolution))
}

/// Returns the immutable Procedure digest embedded in an admitted start execution document.
///
/// This deliberately decodes the complete document rather than consulting the source path again.
pub(crate) fn admitted_start_procedure_digest_v1(
    execution: &CanonicalExecutionJsonV1,
) -> Result<Option<Sha256Digest>, ExecutionErrorV1> {
    if matches!(serde_json::from_str::<Value>(execution.as_str())
        .ok()
        .and_then(|value| value.get("execution_version").and_then(Value::as_u64))
        , Some(version) if version == u64::from(EXECUTION_DOCUMENT_VERSION_V6)
            || version == u64::from(EXECUTION_DOCUMENT_VERSION_V12))
    {
        return decode_procedure_v2_start_execution_v1(execution.as_str())
            .map(|admitted| Some(admitted.state.snapshot().digest().clone()));
    }
    Ok(None)
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

fn optional_string_v1<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<Option<&'a str>, ExecutionErrorV1> {
    match value_v1(object, field)? {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err(invalid_execution_v1("persisted optional string is invalid")),
    }
}

fn decode_goal_criteria_v2(value: &Value) -> Result<GoalDefinitionV2, ExecutionErrorV1> {
    let values = value
        .as_array()
        .ok_or_else(|| invalid_execution_v1("Procedure v2 goal criteria are invalid"))?;
    let criteria = values
        .iter()
        .map(|value| {
            let criterion = value
                .as_object()
                .ok_or_else(|| invalid_execution_v1("Procedure v2 goal criterion is invalid"))?;
            require_exact_keys_v1(criterion, &["criterion_id", "statement"])?;
            GoalCriterionV2::new(
                value_typed_v1(criterion, "criterion_id")?,
                value_string_v1(criterion, "statement")?.to_owned(),
            )
            .map_err(ExecutionErrorV1::BoundaryDomain)
        })
        .collect::<Result<Vec<_>, _>>()?;
    GoalDefinitionV2::new(criteria).map_err(ExecutionErrorV1::BoundaryDomain)
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
    fn v2dog003_embedded_snapshot_preserves_pin_mismatch_for_the_final_fence() {
        let preset = EmbeddedPresetV2 {
            shipped_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ..catalog_v2().lookup("bug-fix-v2").unwrap()
        };
        let (snapshot, pinned) = embedded_preset_snapshot_v2(
            preset,
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000999").unwrap(),
            UnixMillis::new(1),
        )
        .expect("source-valid preset must reach the independent runtime digest fence");
        assert_ne!(snapshot.digest(), &pinned);
        assert!(matches!(
            verify_pinned_procedure_v2_snapshot(&snapshot, &pinned),
            Err(ProcedureV2StartPreparationErrorV1::PinnedPresetDigestMismatch {
                expected,
                actual,
            }) if expected == pinned && actual == *snapshot.digest()
        ));
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
            execution_version: EXECUTION_DOCUMENT_VERSION_V14,
            selector: WorktreeSelectorWireV1::new(b"/tmp/worktree", "/tmp/worktree", None).unwrap(),
            workspace_id: WorkspaceId::new("00000000-0000-4000-8000-000000000924").unwrap(),
            command: SessionDecideV2 {
                option_id: podway_core::OptionId::new("accept").unwrap(),
                reason: "The resolved evidence supports this route.".to_owned(),
                actor: Some("reviewer".to_owned()),
                expected_goal_revision: None,
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
        assert_eq!(decoded.execution_version, EXECUTION_DOCUMENT_VERSION_V14);
        assert_eq!(decoded.command, admitted.command);
        assert_eq!(decoded.fresh_attempt_id, admitted.fresh_attempt_id);

        let document: Value = serde_json::from_str(canonical.as_str()).unwrap();
        assert_eq!(document["execution_version"], json!(14));
        assert_eq!(document["preconditions"]["goal_revision"], Value::Null);

        let mut legacy = document.clone();
        legacy["execution_version"] = json!(9);
        legacy["preconditions"]
            .as_object_mut()
            .unwrap()
            .remove("goal_revision");
        let legacy = decode_procedure_v2_decision_execution_v1(&legacy.to_string()).unwrap();
        assert_eq!(legacy.execution_version, EXECUTION_DOCUMENT_VERSION_V9);
        assert_eq!(legacy.command.expected_goal_revision, None);

        let mut zero_goal_revision = document.clone();
        zero_goal_revision["preconditions"]["goal_revision"] = json!(0);
        assert!(
            decode_procedure_v2_decision_execution_v1(&zero_goal_revision.to_string()).is_err()
        );

        let mut missing_fresh: Value = serde_json::from_str(canonical.as_str()).unwrap();
        missing_fresh["fresh_attempt_id"] = Value::Null;
        assert!(decode_procedure_v2_decision_execution_v1(&missing_fresh.to_string()).is_err());

        let mut blank_actor: Value = serde_json::from_str(canonical.as_str()).unwrap();
        blank_actor["payload"]["actor"] = json!("   ");
        assert!(decode_procedure_v2_decision_execution_v1(&blank_actor.to_string()).is_err());
    }

    #[test]
    fn v2gol002_criterion_assessment_execution_revalidates_closed_payload() {
        let admitted = AdmittedProcedureV2CriterionAssessmentV1 {
            selector: WorktreeSelectorWireV1::new(b"/tmp/worktree", "/tmp/worktree", None).unwrap(),
            workspace_id: WorkspaceId::new("00000000-0000-4000-8000-000000000934").unwrap(),
            command: GoalAssessCriterionV2 {
                criterion_id: podway_core::CriterionId::new("correct").unwrap(),
                status: "satisfied".to_owned(),
                reason: "The durable result is attributable and uniquely cited.".to_owned(),
                evidence: vec![podway_core::GraphNodeId::new("perform").unwrap()],
                items: vec![podway_core::ItemId::new("proof").unwrap()],
                actor: Some("reviewer".to_owned()),
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: SessionId::new("00000000-0000-4000-8000-000000000932")
                        .unwrap(),
                    expected_session_revision: Revision::new(7),
                    expected_attempt_id: AttemptId::new("00000000-0000-4000-8000-000000000933")
                        .unwrap(),
                },
                expected_goal_revision: 1,
            },
        };
        let canonical = procedure_v2_criterion_assessment_execution_document_v1(&admitted).unwrap();
        assert!(decode_procedure_v2_criterion_assessment_execution_v1(canonical.as_str()).is_ok());

        let mut duplicate_evidence: Value = serde_json::from_str(canonical.as_str()).unwrap();
        duplicate_evidence["payload"]["evidence"] = json!(["perform", "perform"]);
        assert!(
            decode_procedure_v2_criterion_assessment_execution_v1(&duplicate_evidence.to_string())
                .is_err()
        );

        let mut duplicate_items: Value = serde_json::from_str(canonical.as_str()).unwrap();
        duplicate_items["payload"]["items"] = json!(["proof", "proof"]);
        assert!(
            decode_procedure_v2_criterion_assessment_execution_v1(&duplicate_items.to_string())
                .is_err()
        );

        let mut blank_actor: Value = serde_json::from_str(canonical.as_str()).unwrap();
        blank_actor["payload"]["actor"] = json!("   ");
        assert!(
            decode_procedure_v2_criterion_assessment_execution_v1(&blank_actor.to_string())
                .is_err()
        );

        let mut zero_goal_revision: Value = serde_json::from_str(canonical.as_str()).unwrap();
        zero_goal_revision["preconditions"]["goal_revision"] = json!(0);
        assert!(
            decode_procedure_v2_criterion_assessment_execution_v1(&zero_goal_revision.to_string())
                .is_err()
        );
    }
}

#[cfg(test)]
mod int_v2gol_epic_execution_integrity;
