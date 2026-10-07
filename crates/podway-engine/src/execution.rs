//! Host-independent Procedure preparation, execution boundaries, and request identities.

use std::{error::Error, fmt};

use podway_config::{
    AuthoringContext, ParsedNodeDefinition, ParsedProcedure, ParsedProcedureV2,
    ProcedureDocumentFormat, ProcedureSourceLabel, parse_procedure_document,
    sniff_procedure_schema, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, ArtifactLocationKindV1, ArtifactValueV1, AttemptId, AttemptLifecycle,
    AttemptNumberV2, AttemptValidityV2, AuthoringDiagnostic, AuthoringSeverity, BlockerId,
    CanonicalProcedureJsonV1, DomainError, GoalCriterionV2, GoalDefinitionV2, GoalStatementV2,
    GraphPlacementV2, ItemId, JobId, ProcedureSnapshotId, ProcedureSourceLabelV1, Revision,
    SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2,
    UnixMillis, WorkspaceId,
};
use podway_presets::{EmbeddedPresetV2, catalog_v2};
use podway_protocol::{
    GoalCriterionWireV2, ItemRecordManyDispositionV1, ItemRecordValueV1,
    ProcedureV2MutationRequestV1, ProcedureV2StartRequestV1, SliceCommandV1, SliceRequestV1,
    WorktreeSelectorWireV1, canonical_procedure_v2_mutation_identity_v1,
    canonical_procedure_v2_start_identity_v1, canonical_reset_all_identity_v1,
};
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, AttemptMetadataV2,
    DurableWorktreeIdentityV1, GraphMutationErrorV2, GraphNodeCounterV2, GraphSessionStateV2,
    ProcedureSnapshotV2, StoreErrorV1, StoreValueErrorV1, WorkflowMemoryStateV2,
    WorkspaceBindingV1,
};
use sha2::{Digest as _, Sha256};

/// A boundary outcome that can either become a durable domain failure or leave the claim for
/// recovery. A transient outcome never causes the engine to commit a terminal result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionBoundaryErrorV1 {
    Domain(DomainError),
    LocalArtifact(LocalArtifactFailureV1),
    ProcedureV2Unsupported,
    WorkspaceIdentityMismatch {
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    Transient {
        operation: &'static str,
    },
}

/// Filesystem failure categories preserved by the direct artifact boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalArtifactFailureV1 {
    NotFound,
    Unreadable,
    PathOutsideWorktree,
    PathUnsafe,
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

/// The host-supplied source of generated domain IDs.
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

/// The host-supplied clock used for domain preparation and execution.
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
    PresetNotFound,
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

pub fn verify_pinned_procedure_v2_snapshot(
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
    prepare_workspace_procedure_source_v2(procedure, source)?.into_snapshot(snapshot_id, created_at)
}

#[derive(Clone, Debug)]
pub struct PreparedProcedureSourceV2 {
    canonical_json: CanonicalProcedureJsonV1,
    digest: Sha256Digest,
    source: ProcedureSourceLabelV1,
    source_bytes_digest: Option<Sha256Digest>,
    goal_tracking: bool,
    entry_graph_node_id: podway_core::GraphNodeId,
}

impl PreparedProcedureSourceV2 {
    pub fn goal_tracking(&self) -> bool {
        self.goal_tracking
    }
    pub fn entry_graph_node_id(&self) -> &podway_core::GraphNodeId {
        &self.entry_graph_node_id
    }
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub fn source_bytes_digest(&self) -> Option<&Sha256Digest> {
        self.source_bytes_digest.as_ref()
    }
    pub fn into_snapshot(
        self,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV2, ProcedureV2SourceAdmissionErrorV1> {
        ProcedureSnapshotV2::new(
            snapshot_id,
            self.canonical_json,
            self.digest,
            self.source,
            created_at,
        )
        .map_err(|_| {
            ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
                DomainError::InvalidState {
                    reason: "Procedure v2 snapshot admission failed",
                },
            ))
        })
    }
}

pub fn prepare_preset_procedure_source_v2(
    preset: &str,
) -> Result<Option<PreparedProcedureSourceV2>, ProcedureV2SourceAdmissionErrorV1> {
    let Some(preset) = catalog_v2().lookup(preset) else {
        return Ok(None);
    };
    let admitted = preset.validate().map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "embedded Procedure v2 preset admission failed",
            },
        ))
    })?;
    let label = preset.metadata.source_label().map_err(|_| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
            DomainError::InvalidState {
                reason: "embedded preset label is invalid",
            },
        ))
    })?;
    Ok(Some(PreparedProcedureSourceV2 {
        canonical_json: CanonicalProcedureJsonV1::new(admitted.canonical_json().as_str()).map_err(
            |error| {
                ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
            },
        )?,
        digest: admitted.digest().clone(),
        source: ProcedureSourceLabelV1::new(label.display_label()).map_err(|error| {
            ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
        })?,
        source_bytes_digest: None,
        goal_tracking: admitted
            .parsed()
            .goal_tracking()
            .is_some_and(|opt_in| opt_in.is_enabled()),
        entry_graph_node_id: admitted.parsed().graph().entry().clone(),
    }))
}

pub fn prepare_workspace_procedure_source_v2(
    procedure: &str,
    source: &[u8],
) -> Result<PreparedProcedureSourceV2, ProcedureV2SourceAdmissionErrorV1> {
    let source_bytes_digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(source)))
        .expect("SHA-256 produces a canonical digest");
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
        let diagnostic_codes = distinct_error_diagnostic_codes(&diagnostics);
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
    Ok(PreparedProcedureSourceV2 {
        canonical_json,
        digest: validated.digest().clone(),
        source,
        source_bytes_digest: Some(source_bytes_digest),
        goal_tracking: validated
            .parsed()
            .goal_tracking()
            .is_some_and(|opt_in| opt_in.is_enabled()),
        entry_graph_node_id: validated.parsed().graph().entry().clone(),
    })
}

fn distinct_error_diagnostic_codes(diagnostics: &[AuthoringDiagnostic]) -> Vec<String> {
    let mut diagnostic_codes: Vec<String> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity() == AuthoringSeverity::Error)
        .map(|diagnostic| diagnostic.code().as_str().to_owned())
        .collect();
    diagnostic_codes.sort();
    diagnostic_codes.dedup();
    diagnostic_codes.truncate(256);
    diagnostic_codes
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
) -> Result<GraphSessionStateV2, ProcedureV2StartPreparationErrorV1> {
    let (snapshot, pinned_digest) = provider
        .load_preset_snapshot_v2(preset, snapshot_id, created_at)
        .map_err(ProcedureV2StartPreparationErrorV1::Source)?
        .ok_or(ProcedureV2StartPreparationErrorV1::PresetNotFound)?;
    verify_pinned_procedure_v2_snapshot(&snapshot, &pinned_digest)?;
    graph_session_state_from_procedure_v2_snapshot(
        snapshot,
        task_title,
        session_id,
        first_attempt_id,
        created_at,
    )
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

pub fn graph_prepared_session_state_from_procedure_v2_snapshot(
    snapshot: ProcedureSnapshotV2,
    task_title: &str,
    session_id: SessionId,
    created_at: UnixMillis,
) -> Result<GraphSessionStateV2, ProcedureV2StartPreparationErrorV1> {
    GraphSessionStateV2::prepared(
        Revision::new(1),
        task_title,
        snapshot,
        session_id,
        created_at,
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
pub struct ResolvedBatchArtifactV1 {
    pub item_id: ItemId,
    pub artifact: ArtifactValueV1,
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

pub fn fresh_successor_attempt_id_v1<Ids: ExecutionIdSourceV1>(
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

pub fn required_local_artifacts_v2(
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
    let values = memory
        .item_slots()
        .iter()
        .map(|slot| slot.value())
        .collect::<Vec<_>>();
    Ok(items
        .iter()
        .zip(memory.item_slots())
        .filter_map(|(specification, slot)| {
            let artifact = slot.value().and_then(|value| value.as_artifact())?;
            (specification.required_now(items, &values)
                && artifact.location_kind() == ArtifactLocationKindV1::LocalPath)
                .then(|| (slot.item_id().clone(), artifact.clone()))
        })
        .collect())
}

#[derive(Clone, Debug)]
pub struct PreparedProcedureV2ItemMutationV1 {
    item_id: ItemId,
    expected_attempt_id: AttemptId,
    expected_item_revision: Revision,
    mutation: ActiveItemMutationV2,
}

impl PreparedProcedureV2ItemMutationV1 {
    pub fn into_parts(self) -> (ItemId, AttemptId, Revision, ActiveItemMutationV2) {
        (
            self.item_id,
            self.expected_attempt_id,
            self.expected_item_revision,
            self.mutation,
        )
    }
}

pub fn prepare_procedure_v2_item_mutation_v1(
    command: &SliceCommandV1,
    attached_artifact: Option<&ArtifactValueV1>,
) -> Result<PreparedProcedureV2ItemMutationV1, ExecutionErrorV1> {
    let (item_id, preconditions, mutation) = match command {
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
                value: attached_artifact.cloned().ok_or_else(|| {
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

pub fn prepare_procedure_v2_item_mutations_v1(
    command: &SliceCommandV1,
    attached_artifacts: &[ResolvedBatchArtifactV1],
) -> Result<Vec<ActiveItemMutationRequestV2>, ExecutionErrorV1> {
    let SliceCommandV1::ItemRecordMany(input) = command else {
        return Err(invalid_execution_v1(
            "Procedure v2 item batch preparation received another command",
        ));
    };
    let expected_artifact_items = input
        .operations
        .iter()
        .filter(|operation| {
            matches!(
                operation.disposition,
                ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Artifact { .. }
                }
            )
        })
        .map(|operation| operation.item_id.clone())
        .collect::<Vec<_>>();
    if expected_artifact_items.len() != attached_artifacts.len()
        || expected_artifact_items
            .iter()
            .zip(attached_artifacts)
            .any(|(expected, resolved)| expected != &resolved.item_id)
    {
        return Err(invalid_execution_v1(
            "Procedure v2 item batch artifact resolutions do not match the command",
        ));
    }
    let mut artifact_index = 0usize;
    input
        .operations
        .iter()
        .map(|operation| {
            let mutation = match &operation.disposition {
                ItemRecordManyDispositionV1::Clear { clear: true } => ActiveItemMutationV2::Clear,
                ItemRecordManyDispositionV1::Clear { clear: false } => {
                    return Err(invalid_execution_v1(
                        "Procedure v2 item batch clear disposition is invalid",
                    ));
                }
                ItemRecordManyDispositionV1::Record { record } => match record {
                    ItemRecordValueV1::Confirm { value: true } => ActiveItemMutationV2::Check,
                    ItemRecordValueV1::Confirm { value: false } => ActiveItemMutationV2::Uncheck,
                    ItemRecordValueV1::Text { value } | ItemRecordValueV1::Choice { value } => {
                        ActiveItemMutationV2::Set {
                            value: value.clone(),
                        }
                    }
                    ItemRecordValueV1::Integer { value } => ActiveItemMutationV2::Set {
                        value: value.to_string(),
                    },
                    ItemRecordValueV1::List { value } => ActiveItemMutationV2::ReplaceList {
                        values: value.clone(),
                    },
                    ItemRecordValueV1::Artifact { .. } => {
                        let resolved = attached_artifacts.get(artifact_index).ok_or_else(|| {
                            invalid_execution_v1(
                                "Procedure v2 item batch artifact resolution is absent",
                            )
                        })?;
                        artifact_index += 1;
                        ActiveItemMutationV2::Attach {
                            value: resolved.artifact.clone(),
                        }
                    }
                    ItemRecordValueV1::CheckResult { value } => {
                        ActiveItemMutationV2::RecordCheckResult {
                            value: value.clone(),
                        }
                    }
                },
            };
            Ok(ActiveItemMutationRequestV2::new(
                operation.item_id.clone(),
                operation.expected_item_revision,
                mutation,
            ))
        })
        .collect()
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
/// [`podway_store::TerminalReceiptV1`], not by this error type.
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
pub fn request_digest_v1(
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

pub fn procedure_v2_typed_mutation_request_digest_v1(
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

pub fn procedure_v2_typed_start_request_digest_v1(
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

pub fn goal_definition_from_wire_v2(
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
pub fn reset_all_request_digest_v1(
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

pub fn invalid_execution_v1(reason: &'static str) -> ExecutionErrorV1 {
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
    fn schema_admission_deduplicates_error_codes_before_applying_the_wire_cap() {
        use podway_core::{AuthoringDiagnosticCode, SourceLocation};

        let repeated = AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            "workflow.yaml",
            SourceLocation::document_start(),
            "$",
            "Invalid schema.",
            "Correct the schema.",
        );
        let distinct = AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::UnreachableGraphNode,
            "workflow.yaml",
            SourceLocation::document_start(),
            "graph",
            "Node is unreachable.",
            "Connect the node.",
        );
        let warning = AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::WeakPurposeGuidance,
            "workflow.yaml",
            SourceLocation::document_start(),
            "purpose",
            "Purpose is weak.",
            "Make the purpose specific.",
        );
        let mut diagnostics = Vec::with_capacity(601);
        for _ in 0..300 {
            diagnostics.push(repeated.clone());
            diagnostics.push(distinct.clone());
        }
        diagnostics.push(warning);

        assert_eq!(
            distinct_error_diagnostic_codes(&diagnostics),
            vec![
                "AUTHORING_SCHEMA_INVALID".to_owned(),
                "UNREACHABLE_GRAPH_NODE".to_owned(),
            ],
        );
    }

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
}
