//! Host-independent Procedure preparation and direct execution boundaries.

use std::{error::Error, fmt};

use podway_config::{
    AuthoringContext, ParsedNodeDefinition, ParsedProcedure, ParsedProcedureV2,
    ProcedureDocumentFormat, ProcedureSourceLabel, ValidatedProcedureV2, parse_procedure_document,
    sniff_procedure_schema, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, ArtifactLocationKindV1, ArtifactValueV1, AttemptId, AttemptLifecycle,
    AttemptNumberV2, AttemptValidityV2, AuthoringDiagnostic, AuthoringSeverity,
    CanonicalProcedureJsonV1, DomainError, GoalCriterionV2, GoalDefinitionV2, GoalStatementV2,
    GraphPlacementV2, ItemId, ProcedureSnapshotId, ProcedureSourceLabelV1, Revision,
    SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2,
    UnixMillis,
};
use podway_presets::{EmbeddedPresetV2, catalog_v2};
use podway_protocol::{
    GoalCriterionWireV2, ItemRecordManyDispositionV1, ItemRecordValueV1, SliceCommandV1,
};
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, AttemptMetadataV2, GraphMutationErrorV2,
    GraphNodeCounterV2, GraphSessionStateV2, ProcedureSnapshotV2, StoreValueErrorV1,
    WorkflowMemoryStateV2, WorkspaceBindingV1,
};
use sha2::{Digest as _, Sha256};

/// A preparation rejection at a domain or native boundary. Failed direct mutations retain no
/// success outcome; callers may retry a transient boundary after refreshing their observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionBoundaryErrorV1 {
    Domain(DomainError),
    LocalArtifact(LocalArtifactFailureV1),
    ProcedureV2Unsupported,
    Transient { operation: &'static str },
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
    let snapshot = podway_store::admit_procedure_snapshot_v2(
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
    validated: ValidatedProcedureV2,
    source: ProcedureSourceLabelV1,
    source_bytes_digest: Option<Sha256Digest>,
}

impl PreparedProcedureSourceV2 {
    pub fn goal_tracking(&self) -> bool {
        self.validated
            .parsed()
            .goal_tracking()
            .is_some_and(|opt_in| opt_in.is_enabled())
    }
    pub fn entry_graph_node_id(&self) -> &podway_core::GraphNodeId {
        self.validated.parsed().graph().entry()
    }
    pub fn digest(&self) -> &Sha256Digest {
        self.validated.digest()
    }
    pub fn source_bytes_digest(&self) -> Option<&Sha256Digest> {
        self.source_bytes_digest.as_ref()
    }
    pub fn into_snapshot(
        self,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV2, ProcedureV2SourceAdmissionErrorV1> {
        let runtime = self.validated.runtime_v2().map_err(|_| {
            ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(
                DomainError::InvalidState {
                    reason: "Procedure v2 snapshot admission failed",
                },
            ))
        })?;
        Ok(ProcedureSnapshotV2::from_runtime(
            snapshot_id,
            runtime,
            self.source,
            created_at,
        ))
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
    // Preserve bounded canonical admission before source-label construction.
    CanonicalProcedureJsonV1::new(admitted.canonical_json().as_str()).map_err(|error| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
    })?;
    Ok(Some(PreparedProcedureSourceV2 {
        validated: admitted.into_procedure(),
        source: ProcedureSourceLabelV1::new(label.display_label()).map_err(|error| {
            ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
        })?,
        source_bytes_digest: None,
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
    CanonicalProcedureJsonV1::new(validated.canonical_json().as_str().to_owned()).map_err(
        |error| {
            ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
        },
    )?;
    let source = ProcedureSourceLabelV1::new(source_label.display_label()).map_err(|error| {
        ProcedureV2SourceAdmissionErrorV1::Rejected(ExecutionBoundaryErrorV1::domain(error))
    })?;
    Ok(PreparedProcedureSourceV2 {
        validated,
        source,
        source_bytes_digest: Some(source_bytes_digest),
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
/// Pure preparation uses only the admitted immutable snapshot.
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
        .map_err(|error| ProcedureV2StartPreparationErrorV1::InvalidStoreValue(error.into()))?;
    let metadata = vec![metadata];
    let workflow_memory = WorkflowMemoryStateV2::initial_for_trace(&snapshot, &trace, &metadata)
        .map_err(|error| ProcedureV2StartPreparationErrorV1::InvalidStoreValue(error.into()))?;
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
    .map_err(|error| ProcedureV2StartPreparationErrorV1::InvalidStoreValue(error.into()))
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
    .map_err(|error| ProcedureV2StartPreparationErrorV1::InvalidStoreValue(error.into()))
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

/// Rehashes local artifacts through a verified worktree boundary for independent deep integrity
/// checks. Direct mutations use prepared descriptor stamps through the native workspace adapter.
pub trait ArtifactVerifierV1: Send + Sync {
    fn revalidate_local_artifact(
        &self,
        workspace: &WorkspaceBindingV1,
        item_id: &ItemId,
        artifact: &ArtifactValueV1,
    ) -> Result<LocalArtifactVerificationV2, ExecutionBoundaryErrorV1>;
}

/// Failures surfaced by pure preparation and direct execution boundaries.
#[derive(Debug)]
pub enum ExecutionErrorV1 {
    InvalidPersistedExecution { reason: &'static str },
}

impl fmt::Display for ExecutionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPersistedExecution { reason } => {
                write!(formatter, "invalid persisted execution document: {reason}")
            }
        }
    }
}

impl Error for ExecutionErrorV1 {}

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
pub fn invalid_execution_v1(reason: &'static str) -> ExecutionErrorV1 {
    ExecutionErrorV1::InvalidPersistedExecution { reason }
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
