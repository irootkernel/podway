//! Pure Procedure v2 graph/session state and transitions.
use crate::ProcedureStateErrorV2;
use crate::goal_state_v2::{
    AttemptCriterionAssessmentStateV2, CriterionAssessmentStateV2, GoalStateV2,
    validate_goal_revision_target_v2, validate_goal_state_v2,
};
use crate::workflow_v2::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, EvidenceLookupErrorV2, EvidenceReadbackV2,
    GraphMutationErrorV2, SelectedEvidenceItemV2, SnapshotMemoryModelV2, WorkflowMemoryStateV2,
    validate_workflow_memory_v2,
};
use crate::{
    ActorAttributionV2, AdvanceTerminalV2, AttemptId, AttemptLifecycle, BlockerId,
    CanonicalProcedureJsonV1, CriterionAssessmentResultV2, CriterionCitationV2,
    GoalAssessmentRecordV2, GoalDefinitionV2, GoalOutcome, GoalRevisionNumberV2,
    GoalRevisionReasonV2, GoalRevisionRecordV2, GoalStatementV2, GraphNodeId, ItemId,
    NodeDefinitionId, NodeKindV2, OptionId, ProcedureSnapshotId, ProcedureSourceLabelV1, ReasonV2,
    Revision, SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest,
    TraceSequenceV2, UnixMillis, canonicalize_json_v1, verify_canonical_json_v1,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
const PROCEDURE_SCHEMA_V2: &str = "podway.procedure/v2";
const MAX_TASK_TITLE_CHARACTERS_V2: usize = 500;
const MAX_TERMINAL_REASON_CHARACTERS_V2: usize = 2_000;
fn invalid(reason: &'static str) -> ProcedureStateErrorV2 {
    ProcedureStateErrorV2::new(reason)
}
fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, ProcedureStateErrorV2> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("canonical Procedure v2 metadata is incomplete"))
}

/// Exact canonical bytes, digest and their bounded parsed document.
/// Shape admission belongs to the adapter; constructing this value does not
/// admit derived workflow or goal declarations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalProcedureDocumentV2 {
    canonical_json: CanonicalProcedureJsonV1,
    digest: Sha256Digest,
    document: Value,
}
impl CanonicalProcedureDocumentV2 {
    pub fn new(
        canonical_json: CanonicalProcedureJsonV1,
        digest: Sha256Digest,
    ) -> Result<Self, ProcedureStateErrorV2> {
        verify_canonical_json_v1(canonical_json.as_str().as_bytes())
            .map_err(|_| invalid("Procedure v2 snapshot JSON must be canonical"))?;
        let computed = format!(
            "sha256:{:x}",
            Sha256::digest(canonical_json.as_str().as_bytes())
        );
        if computed != digest.as_str() {
            return Err(invalid(
                "Procedure v2 snapshot digest does not match its canonical JSON",
            ));
        }

        let document: Value = serde_json::from_str(canonical_json.as_str())
            .map_err(|_| invalid("Procedure v2 snapshot JSON is not a document"))?;
        Ok(Self {
            canonical_json,
            digest,
            document,
        })
    }
    pub fn as_value(&self) -> &Value {
        &self.document
    }
}
/// Immutable placement metadata derived from one canonical Procedure v2 snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphNodeSnapshotV2 {
    graph_node_id: GraphNodeId,
    node_definition_id: NodeDefinitionId,
    placement_index: u32,
    node_kind: NodeKindV2,
    goal_assessment: bool,
    canonical_placement_json: String,
}

impl GraphNodeSnapshotV2 {
    pub fn graph_node_id(&self) -> &GraphNodeId {
        &self.graph_node_id
    }

    pub fn node_definition_id(&self) -> &NodeDefinitionId {
        &self.node_definition_id
    }

    pub const fn placement_index(&self) -> u32 {
        self.placement_index
    }

    pub const fn node_kind(&self) -> NodeKindV2 {
        self.node_kind
    }

    pub const fn goal_assessment(&self) -> bool {
        self.goal_assessment
    }

    pub fn canonical_placement_json(&self) -> &str {
        &self.canonical_placement_json
    }
}

/// A canonical immutable Procedure v2 snapshot ready for relational persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureSnapshotV2 {
    snapshot_id: ProcedureSnapshotId,
    procedure_id: String,
    procedure_version: String,
    name: String,
    purpose: String,
    digest: Sha256Digest,
    canonical_json: CanonicalProcedureJsonV1,
    source: ProcedureSourceLabelV1,
    goal_tracking: bool,
    entry_graph_node_id: GraphNodeId,
    graph_nodes: Vec<GraphNodeSnapshotV2>,
    memory_model: Arc<Result<SnapshotMemoryModelV2, ProcedureStateErrorV2>>,
    created_at: UnixMillis,
}

impl ProcedureSnapshotV2 {
    /// Constructs pure runtime state from independently shape-admitted bytes.
    /// Keeps derived declaration failures deferred until their existing callers.
    pub fn from_admitted_document(
        snapshot_id: ProcedureSnapshotId,
        document: CanonicalProcedureDocumentV2,
        source: ProcedureSourceLabelV1,
        created_at: UnixMillis,
    ) -> Result<Self, ProcedureStateErrorV2> {
        let CanonicalProcedureDocumentV2 {
            canonical_json,
            digest,
            document,
        } = document;
        let document = document
            .as_object()
            .ok_or_else(|| invalid("Procedure v2 snapshot must be a JSON object"))?;
        if required_string(document, "schema")? != PROCEDURE_SCHEMA_V2 {
            return Err(invalid("Procedure v2 snapshot has the wrong schema"));
        }
        let procedure_id = required_string(document, "id")?.to_owned();
        let procedure_version = required_string(document, "version")?.to_owned();
        let name = required_string(document, "name")?.to_owned();
        let purpose = required_string(document, "purpose")?.to_owned();
        let goal_tracking = match document.get("goal_tracking") {
            None => false,
            Some(Value::Bool(true)) => true,
            _ => return Err(invalid("Procedure v2 goal tracking must be absent or true")),
        };
        let definitions = document
            .get("node_definitions")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("Procedure v2 node definitions are missing"))?;
        let graph = document
            .get("graph")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("Procedure v2 graph is missing"))?;
        let entry_graph_node_id = GraphNodeId::new(required_string(graph, "entry")?.to_owned())
            .map_err(|_| invalid("Procedure v2 entry graph node identity is invalid"))?;
        let placements = graph
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("Procedure v2 graph placements are missing"))?;
        if placements.is_empty() || placements.len() > crate::MAX_GRAPH_NODES_PER_PROCEDURE_V2 {
            return Err(invalid("Procedure v2 graph node count is out of bounds"));
        }

        let mut graph_nodes = Vec::with_capacity(placements.len());
        let mut seen = BTreeSet::new();
        for (index, placement) in placements.iter().enumerate() {
            let placement_object = placement
                .as_object()
                .ok_or_else(|| invalid("Procedure v2 graph placement must be an object"))?;
            let graph_node_id =
                GraphNodeId::new(required_string(placement_object, "id")?.to_owned())
                    .map_err(|_| invalid("Procedure v2 graph node identity is invalid"))?;
            if !seen.insert(graph_node_id.clone()) {
                return Err(invalid("Procedure v2 graph node identities must be unique"));
            }
            let definition_text = required_string(placement_object, "use")?;
            let node_definition_id = NodeDefinitionId::new(definition_text.to_owned())
                .map_err(|_| invalid("Procedure v2 node definition identity is invalid"))?;
            let definition = definitions
                .get(definition_text)
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("Procedure v2 graph placement names no definition"))?;
            let node_kind = match required_string(definition, "type")? {
                "action" => NodeKindV2::Action,
                "decision" => NodeKindV2::Decision,
                _ => return Err(invalid("Procedure v2 node definition kind is invalid")),
            };
            let goal_assessment = definition
                .get("assessment")
                .and_then(Value::as_object)
                .and_then(|assessment| assessment.get("target"))
                .and_then(Value::as_str)
                .is_some_and(|target| target == "session_goal");
            let canonical_placement_json = canonicalize_json_v1(placement)
                .map_err(|_| invalid("Procedure v2 graph placement is not canonicalizable"))?;
            graph_nodes.push(GraphNodeSnapshotV2 {
                graph_node_id,
                node_definition_id,
                placement_index: u32::try_from(index)
                    .map_err(|_| invalid("Procedure v2 graph placement index is out of bounds"))?,
                node_kind,
                goal_assessment,
                canonical_placement_json,
            });
        }
        if !seen.contains(&entry_graph_node_id) {
            return Err(invalid("Procedure v2 entry graph node is absent"));
        }

        // This derived model belongs to these immutable canonical bytes. Keep
        // parse errors for the same workflow-memory admission point as before.
        let memory_model = Arc::new(SnapshotMemoryModelV2::parse(
            canonical_json.as_str(),
            &graph_nodes,
        ));

        Ok(Self {
            snapshot_id,
            procedure_id,
            procedure_version,
            name,
            purpose,
            digest,
            canonical_json,
            source,
            goal_tracking,
            entry_graph_node_id,
            graph_nodes,
            memory_model,
            created_at,
        })
    }

    pub fn snapshot_id(&self) -> &ProcedureSnapshotId {
        &self.snapshot_id
    }
    pub fn procedure_id(&self) -> &str {
        &self.procedure_id
    }
    pub fn procedure_version(&self) -> &str {
        &self.procedure_version
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub fn canonical_json(&self) -> &CanonicalProcedureJsonV1 {
        &self.canonical_json
    }
    pub fn source(&self) -> &ProcedureSourceLabelV1 {
        &self.source
    }
    pub const fn goal_tracking(&self) -> bool {
        self.goal_tracking
    }
    pub fn entry_graph_node_id(&self) -> &GraphNodeId {
        &self.entry_graph_node_id
    }
    pub fn graph_nodes(&self) -> &[GraphNodeSnapshotV2] {
        &self.graph_nodes
    }
    pub const fn created_at(&self) -> UnixMillis {
        self.created_at
    }

    pub fn graph_node(&self, id: &GraphNodeId) -> Option<&GraphNodeSnapshotV2> {
        self.graph_nodes
            .iter()
            .find(|node| node.graph_node_id() == id)
    }

    pub fn validate_memory_declarations_v2(&self) -> Result<(), ProcedureStateErrorV2> {
        self.memory_model().map(|_| ())
    }
    pub fn item_declarations_v2(
        &self,
        node: &GraphNodeId,
    ) -> Result<&[crate::ItemSpecV2], ProcedureStateErrorV2> {
        self.memory_model()?.items(node)
    }

    pub(crate) fn memory_model(&self) -> Result<&SnapshotMemoryModelV2, ProcedureStateErrorV2> {
        self.memory_model.as_ref().as_ref().map_err(Clone::clone)
    }
}

/// Durable per-node counters for one current Procedure v2 session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphNodeCounterV2 {
    graph_node_id: GraphNodeId,
    attempt_count: u64,
    rework_traversal_count: u64,
}

impl GraphNodeCounterV2 {
    pub fn new(
        graph_node_id: GraphNodeId,
        attempt_count: u64,
        rework_traversal_count: u64,
    ) -> Self {
        Self {
            graph_node_id,
            attempt_count,
            rework_traversal_count,
        }
    }
    pub fn graph_node_id(&self) -> &GraphNodeId {
        &self.graph_node_id
    }
    pub const fn attempt_count(&self) -> u64 {
        self.attempt_count
    }
    pub const fn rework_traversal_count(&self) -> u64 {
        self.rework_traversal_count
    }
}

/// Timestamp and terminal-reason metadata paired with one domain attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptMetadataV2 {
    attempt_id: AttemptId,
    started_at: UnixMillis,
    ended_at: Option<UnixMillis>,
    terminal_reason: Option<String>,
}

impl AttemptMetadataV2 {
    pub fn new(
        attempt_id: AttemptId,
        started_at: UnixMillis,
        ended_at: Option<UnixMillis>,
        terminal_reason: Option<String>,
    ) -> Result<Self, ProcedureStateErrorV2> {
        if let Some(reason) = terminal_reason.as_deref() {
            let count = reason.chars().count();
            if reason.trim().is_empty() || count > MAX_TERMINAL_REASON_CHARACTERS_V2 {
                return Err(invalid("Procedure v2 terminal reason is invalid"));
            }
        }
        Ok(Self {
            attempt_id,
            started_at,
            ended_at,
            terminal_reason,
        })
    }
    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }
    pub const fn started_at(&self) -> UnixMillis {
        self.started_at
    }
    pub const fn ended_at(&self) -> Option<UnixMillis> {
        self.ended_at
    }
    pub fn terminal_reason(&self) -> Option<&str> {
        self.terminal_reason.as_deref()
    }
}

/// Complete coherent graph/action state of the current Procedure v2 task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphSessionStateV2 {
    workspace_revision: Revision,
    task_title: String,
    snapshot: ProcedureSnapshotV2,
    trace: SessionTraceV2,
    counters: Vec<GraphNodeCounterV2>,
    attempt_metadata: Vec<AttemptMetadataV2>,
    workflow_memory: WorkflowMemoryStateV2,
    goal_state: GoalStateV2,
    created_at: UnixMillis,
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphInitialGoalV2 {
    statement: GoalStatementV2,
    criteria: GoalDefinitionV2,
    actor: Option<ActorAttributionV2>,
}

impl GraphInitialGoalV2 {
    pub fn new(
        statement: GoalStatementV2,
        criteria: GoalDefinitionV2,
        actor: Option<ActorAttributionV2>,
    ) -> Self {
        Self {
            statement,
            criteria,
            actor,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphBeginOutcomeV2 {
    state: GraphSessionStateV2,
    attempt_id: AttemptId,
    goal_revision: Option<GoalRevisionNumberV2>,
}

impl GraphBeginOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }

    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub const fn goal_revision(&self) -> Option<GoalRevisionNumberV2> {
        self.goal_revision
    }
}

/// Pure result of mutating one item slot of the current Procedure v2 attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphItemMutationOutcomeV2 {
    state: GraphSessionStateV2,
    changed: bool,
    item_revision: Revision,
    value_digest: Option<Sha256Digest>,
}

/// Item-ID-ordered result of one operation in an atomic active-item mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphItemMutationEntryV2 {
    item_id: ItemId,
    changed: bool,
    item_revision: Revision,
    value_digest: Option<Sha256Digest>,
}

impl GraphItemMutationEntryV2 {
    pub fn item_id(&self) -> &ItemId {
        &self.item_id
    }
    pub const fn changed(&self) -> bool {
        self.changed
    }
    pub const fn item_revision(&self) -> Revision {
        self.item_revision
    }
    pub fn value_digest(&self) -> Option<&Sha256Digest> {
        self.value_digest.as_ref()
    }
}

/// Pure result of atomically mutating a bounded set of current-attempt item slots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphItemsMutationOutcomeV2 {
    state: GraphSessionStateV2,
    changed: bool,
    items: Vec<GraphItemMutationEntryV2>,
}

impl GraphItemsMutationOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub const fn changed(&self) -> bool {
        self.changed
    }
    pub fn items(&self) -> &[GraphItemMutationEntryV2] {
        &self.items
    }
}

impl GraphItemMutationOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub const fn changed(&self) -> bool {
        self.changed
    }
    pub const fn item_revision(&self) -> Revision {
        self.item_revision
    }
    pub fn value_digest(&self) -> Option<&Sha256Digest> {
        self.value_digest.as_ref()
    }
}

/// Pure result of completing one active Procedure v2 action placement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphActionCompletionOutcomeV2 {
    state: GraphSessionStateV2,
    from_graph_node_id: GraphNodeId,
    from_attempt_id: AttemptId,
    to_graph_node_id: Option<GraphNodeId>,
    to_attempt_id: Option<AttemptId>,
}

/// Pure result of retrying the active Procedure v2 action or decision placement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphRetryOutcomeV2 {
    state: GraphSessionStateV2,
    graph_node_id: GraphNodeId,
    from_attempt_id: AttemptId,
    to_attempt_id: AttemptId,
}

/// Pure result of skipping the active Procedure v2 action placement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphActionSkipOutcomeV2 {
    state: GraphSessionStateV2,
    from_graph_node_id: GraphNodeId,
    from_attempt_id: AttemptId,
    to_graph_node_id: Option<GraphNodeId>,
    to_attempt_id: Option<AttemptId>,
}

/// Pure result of selecting one route from the active Procedure v2 decision placement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphDecisionOutcomeV2 {
    state: GraphSessionStateV2,
    decision: crate::DecisionRecordV2,
    goal_assessment: Option<GoalAssessmentRecordV2>,
    declared_rework: Option<crate::ReworkRecordV2>,
    from_graph_node_id: GraphNodeId,
    from_attempt_id: AttemptId,
    to_graph_node_id: GraphNodeId,
    to_attempt_id: AttemptId,
}

/// Pure result of manually re-entering one allowed valid-trace target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphManualReworkOutcomeV2 {
    state: GraphSessionStateV2,
    record: crate::ReworkRecordV2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphGoalDefinitionOutcomeV2 {
    state: GraphSessionStateV2,
    revision: GoalRevisionRecordV2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphGoalRevisionOutcomeV2 {
    state: GraphSessionStateV2,
    revision: GoalRevisionRecordV2,
    target_attempt_id: AttemptId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphCriterionAssessmentOutcomeV2 {
    state: GraphSessionStateV2,
    assessment: CriterionAssessmentStateV2,
    graph_node_id: GraphNodeId,
    attempt_id: AttemptId,
    goal_revision: GoalRevisionNumberV2,
    complete: bool,
    determined_outcome: Option<GoalOutcome>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphBlockOutcomeV2 {
    state: GraphSessionStateV2,
    graph_node_id: GraphNodeId,
    attempt_id: AttemptId,
    blocker_id: BlockerId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphUnblockOutcomeV2 {
    state: GraphSessionStateV2,
    graph_node_id: GraphNodeId,
    attempt_id: AttemptId,
    blocker_ids: Vec<BlockerId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphCancelOutcomeV2 {
    state: GraphSessionStateV2,
    graph_node_id: GraphNodeId,
    attempt_id: AttemptId,
}

macro_rules! graph_memory_outcome_accessors {
    ($name:ident) => {
        impl $name {
            pub fn state(&self) -> &GraphSessionStateV2 {
                &self.state
            }
            pub fn into_state(self) -> GraphSessionStateV2 {
                self.state
            }
            pub fn graph_node_id(&self) -> &GraphNodeId {
                &self.graph_node_id
            }
            pub fn attempt_id(&self) -> &AttemptId {
                &self.attempt_id
            }
        }
    };
}
graph_memory_outcome_accessors!(GraphBlockOutcomeV2);
graph_memory_outcome_accessors!(GraphUnblockOutcomeV2);
graph_memory_outcome_accessors!(GraphCancelOutcomeV2);

impl GraphBlockOutcomeV2 {
    pub fn blocker_id(&self) -> &BlockerId {
        &self.blocker_id
    }
}
impl GraphUnblockOutcomeV2 {
    pub fn blocker_ids(&self) -> &[BlockerId] {
        &self.blocker_ids
    }
}

impl GraphActionSkipOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn from_graph_node_id(&self) -> &GraphNodeId {
        &self.from_graph_node_id
    }
    pub fn from_attempt_id(&self) -> &AttemptId {
        &self.from_attempt_id
    }
    pub fn to_graph_node_id(&self) -> Option<&GraphNodeId> {
        self.to_graph_node_id.as_ref()
    }
    pub fn to_attempt_id(&self) -> Option<&AttemptId> {
        self.to_attempt_id.as_ref()
    }
}

impl GraphDecisionOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn decision_record(&self) -> &crate::DecisionRecordV2 {
        &self.decision
    }
    pub fn goal_assessment_record(&self) -> Option<&GoalAssessmentRecordV2> {
        self.goal_assessment.as_ref()
    }
    pub fn declared_rework_record(&self) -> Option<&crate::ReworkRecordV2> {
        self.declared_rework.as_ref()
    }
    pub fn from_graph_node_id(&self) -> &GraphNodeId {
        &self.from_graph_node_id
    }
    pub fn from_attempt_id(&self) -> &AttemptId {
        &self.from_attempt_id
    }
    pub fn to_graph_node_id(&self) -> &GraphNodeId {
        &self.to_graph_node_id
    }
    pub fn to_attempt_id(&self) -> &AttemptId {
        &self.to_attempt_id
    }
}

impl GraphManualReworkOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn record(&self) -> &crate::ReworkRecordV2 {
        &self.record
    }
    pub fn from_graph_node_id(&self) -> &GraphNodeId {
        self.record.from_node()
    }
    pub fn to_graph_node_id(&self) -> &GraphNodeId {
        self.record.to_node()
    }
    pub fn to_attempt_id(&self) -> &AttemptId {
        self.record.target_attempt_id()
    }
    pub const fn reactivated(&self) -> bool {
        self.record.reactivated()
    }
}

impl GraphGoalDefinitionOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn revision(&self) -> &GoalRevisionRecordV2 {
        &self.revision
    }
}

impl GraphGoalRevisionOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn revision(&self) -> &GoalRevisionRecordV2 {
        &self.revision
    }
    pub fn target_attempt_id(&self) -> &AttemptId {
        &self.target_attempt_id
    }
}

impl GraphCriterionAssessmentOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn assessment(&self) -> &CriterionAssessmentStateV2 {
        &self.assessment
    }
    pub fn graph_node_id(&self) -> &GraphNodeId {
        &self.graph_node_id
    }
    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }
    pub const fn goal_revision(&self) -> GoalRevisionNumberV2 {
        self.goal_revision
    }
    pub const fn complete(&self) -> bool {
        self.complete
    }
    pub const fn determined_outcome(&self) -> Option<GoalOutcome> {
        self.determined_outcome
    }
}

impl GraphRetryOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn graph_node_id(&self) -> &GraphNodeId {
        &self.graph_node_id
    }
    pub fn from_attempt_id(&self) -> &AttemptId {
        &self.from_attempt_id
    }
    pub fn to_attempt_id(&self) -> &AttemptId {
        &self.to_attempt_id
    }
}

impl GraphActionCompletionOutcomeV2 {
    pub fn state(&self) -> &GraphSessionStateV2 {
        &self.state
    }
    pub fn into_state(self) -> GraphSessionStateV2 {
        self.state
    }
    pub fn from_graph_node_id(&self) -> &GraphNodeId {
        &self.from_graph_node_id
    }
    pub fn from_attempt_id(&self) -> &AttemptId {
        &self.from_attempt_id
    }
    pub fn to_graph_node_id(&self) -> Option<&GraphNodeId> {
        self.to_graph_node_id.as_ref()
    }
    pub fn to_attempt_id(&self) -> Option<&AttemptId> {
        self.to_attempt_id.as_ref()
    }
}

impl GraphSessionStateV2 {
    pub fn accept_delegation_successor_v1(
        &self,
        attempt_id: &AttemptId,
    ) -> Result<Self, GraphMutationErrorV2> {
        if self.trace.lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning);
        }
        let active = self
            .trace
            .active_attempt()
            .ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        if active.attempt_id() != attempt_id {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: attempt_id.clone(),
                actual: Some(active.attempt_id().clone()),
            });
        }
        let placement = self
            .snapshot
            .graph_nodes()
            .iter()
            .find(|node| node.graph_node_id() == active.graph_node_id())
            .ok_or_else(|| invalid("delegation placement is absent"))?;
        let document: Value = serde_json::from_str(placement.canonical_placement_json())
            .map_err(|_| invalid("delegation placement is malformed"))?;
        if document.get("delegation").is_none() {
            return Err(invalid("delegation acceptance requires a delegated action").into());
        }
        self.workflow_memory
            .validate_active_action_ready_v2(&self.snapshot, &self.trace)?;
        let trace = SessionTraceV2::from_parts(
            self.trace.session_id().clone(),
            self.trace.lifecycle(),
            self.trace
                .revision()
                .checked_next()
                .map_err(|_| invalid("session revision overflowed"))?,
            self.trace.attempts().to_vec(),
        )
        .map_err(|_| invalid("delegation acceptance trace is invalid"))?;
        Ok(Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(|_| invalid("workspace revision overflowed"))?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            self.workflow_memory.clone(),
            self.goal_state.clone(),
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?)
    }

    pub fn prepared(
        workspace_revision: Revision,
        task_title: impl Into<String>,
        snapshot: ProcedureSnapshotV2,
        session_id: SessionId,
        created_at: UnixMillis,
    ) -> Result<Self, ProcedureStateErrorV2> {
        let counters = snapshot
            .graph_nodes()
            .iter()
            .map(|node| GraphNodeCounterV2::new(node.graph_node_id().clone(), 0, 0))
            .collect();
        Self::new(
            workspace_revision,
            task_title,
            snapshot,
            SessionTraceV2::prepared(session_id),
            counters,
            Vec::new(),
            created_at,
            None,
            None,
            None,
        )
    }

    pub fn begin_v2(
        &self,
        expected_session_revision: Revision,
        attempt_id: AttemptId,
        initial_goal: Option<GraphInitialGoalV2>,
        now: UnixMillis,
    ) -> Result<GraphBeginOutcomeV2, GraphMutationErrorV2> {
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        if self.trace.lifecycle() != SessionLifecycle::Prepared {
            return Err(GraphMutationErrorV2::SessionNotPrepared);
        }
        if initial_goal.is_some() && !self.snapshot.goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled);
        }

        let goal_revision = initial_goal.as_ref().map(|_| GoalRevisionNumberV2::FIRST);
        let mut trace = self.trace.clone();
        trace
            .begin(
                self.snapshot.entry_graph_node_id().clone(),
                attempt_id.clone(),
                goal_revision,
            )
            .map_err(GraphMutationErrorV2::Domain)?;
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                GraphNodeCounterV2::new(
                    counter.graph_node_id().clone(),
                    u64::from(counter.graph_node_id() == self.snapshot.entry_graph_node_id()),
                    0,
                )
            })
            .collect::<Vec<_>>();
        let metadata = vec![AttemptMetadataV2::new(attempt_id.clone(), now, None, None)?];
        let workflow_memory =
            WorkflowMemoryStateV2::initial_for_trace(&self.snapshot, &trace, &metadata)?;
        let goal_state = match initial_goal {
            Some(goal) => {
                let revision = GoalRevisionRecordV2::new(
                    GoalRevisionNumberV2::FIRST,
                    None,
                    goal.statement,
                    goal.criteria,
                    None,
                    None,
                    false,
                    goal.actor,
                    TraceSequenceV2::FIRST,
                    now,
                )?;
                GoalStateV2::new(
                    Some(GoalRevisionNumberV2::FIRST),
                    vec![revision],
                    Vec::new(),
                    Vec::new(),
                )?
            }
            None => GoalStateV2::empty(),
        };
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            counters,
            metadata,
            workflow_memory,
            goal_state,
            self.created_at,
            None,
            None,
            None,
        )?;
        Ok(GraphBeginOutcomeV2 {
            state,
            attempt_id,
            goal_revision,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace_revision: Revision,
        task_title: impl Into<String>,
        snapshot: ProcedureSnapshotV2,
        trace: SessionTraceV2,
        counters: Vec<GraphNodeCounterV2>,
        attempt_metadata: Vec<AttemptMetadataV2>,
        created_at: UnixMillis,
        completed_at: Option<UnixMillis>,
        cancelled_at: Option<UnixMillis>,
        cancel_reason: Option<String>,
    ) -> Result<Self, ProcedureStateErrorV2> {
        let task_title = task_title.into();
        let workflow_memory =
            WorkflowMemoryStateV2::empty_for_trace(&snapshot, &trace, &attempt_metadata)?;
        Self::new_with_workflow_memory(
            workspace_revision,
            task_title,
            snapshot,
            trace,
            counters,
            attempt_metadata,
            workflow_memory,
            created_at,
            completed_at,
            cancelled_at,
            cancel_reason,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_workflow_memory(
        workspace_revision: Revision,
        task_title: impl Into<String>,
        snapshot: ProcedureSnapshotV2,
        trace: SessionTraceV2,
        counters: Vec<GraphNodeCounterV2>,
        attempt_metadata: Vec<AttemptMetadataV2>,
        workflow_memory: WorkflowMemoryStateV2,
        created_at: UnixMillis,
        completed_at: Option<UnixMillis>,
        cancelled_at: Option<UnixMillis>,
        cancel_reason: Option<String>,
    ) -> Result<Self, ProcedureStateErrorV2> {
        Self::new_with_goal_state(
            workspace_revision,
            task_title,
            snapshot,
            trace,
            counters,
            attempt_metadata,
            workflow_memory,
            GoalStateV2::empty(),
            created_at,
            completed_at,
            cancelled_at,
            cancel_reason,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_goal_state(
        workspace_revision: Revision,
        task_title: impl Into<String>,
        snapshot: ProcedureSnapshotV2,
        trace: SessionTraceV2,
        counters: Vec<GraphNodeCounterV2>,
        attempt_metadata: Vec<AttemptMetadataV2>,
        workflow_memory: WorkflowMemoryStateV2,
        goal_state: GoalStateV2,
        created_at: UnixMillis,
        completed_at: Option<UnixMillis>,
        cancelled_at: Option<UnixMillis>,
        cancel_reason: Option<String>,
    ) -> Result<Self, ProcedureStateErrorV2> {
        let task_title = task_title.into();
        if task_title.trim().is_empty() || task_title.chars().count() > MAX_TASK_TITLE_CHARACTERS_V2
        {
            return Err(invalid("Procedure v2 task title is invalid"));
        }
        if workspace_revision == Revision::ZERO
            || (trace.lifecycle() == SessionLifecycle::Prepared)
                != (trace.revision() == Revision::ZERO)
        {
            return Err(invalid("Procedure v2 persisted revisions are inconsistent"));
        }
        validate_session_metadata(
            trace.lifecycle(),
            created_at,
            completed_at,
            cancelled_at,
            cancel_reason.as_deref(),
        )?;
        validate_graph_state_members(&snapshot, &trace, &counters, &attempt_metadata)?;
        validate_terminal_graph_state_v2(
            &snapshot,
            &trace,
            &attempt_metadata,
            completed_at,
            cancelled_at,
            cancel_reason.as_deref(),
        )?;
        validate_goal_state_v2(
            &snapshot,
            &trace,
            &attempt_metadata,
            &workflow_memory,
            &goal_state,
        )?;
        let goal_rework_target_attempt_ids = goal_state.goal_rework_target_attempt_ids(&trace);
        let reactivated_goal_rework_target_attempt_ids =
            goal_state.reactivated_goal_rework_target_attempt_ids(&trace);
        validate_workflow_memory_v2(
            &snapshot,
            &trace,
            &counters,
            &attempt_metadata,
            &workflow_memory,
            &goal_rework_target_attempt_ids,
            &reactivated_goal_rework_target_attempt_ids,
        )?;
        Ok(Self {
            workspace_revision,
            task_title,
            snapshot,
            trace,
            counters,
            attempt_metadata,
            workflow_memory,
            goal_state,
            created_at,
            completed_at,
            cancelled_at,
            cancel_reason,
        })
    }

    pub const fn workspace_revision(&self) -> Revision {
        self.workspace_revision
    }
    pub fn task_title(&self) -> &str {
        &self.task_title
    }
    pub fn snapshot(&self) -> &ProcedureSnapshotV2 {
        &self.snapshot
    }
    pub fn trace(&self) -> &SessionTraceV2 {
        &self.trace
    }
    pub fn counters(&self) -> &[GraphNodeCounterV2] {
        &self.counters
    }
    pub fn attempt_metadata(&self) -> &[AttemptMetadataV2] {
        &self.attempt_metadata
    }
    pub fn workflow_memory(&self) -> &WorkflowMemoryStateV2 {
        &self.workflow_memory
    }
    pub fn goal_state(&self) -> &GoalStateV2 {
        &self.goal_state
    }
    pub fn selected_evidence_readback(
        &self,
        attempt_id: &AttemptId,
    ) -> Result<Vec<EvidenceReadbackV2>, ProcedureStateErrorV2> {
        self.workflow_memory
            .selected_readback(&self.trace, attempt_id)
    }

    /// One item selected by a currently resolved, declared evidence reference of the consumer
    /// attempt, or the exact reason it cannot be read.
    pub fn selected_evidence_item(
        &self,
        consumer_attempt_id: &AttemptId,
        source_node: &GraphNodeId,
        item_id: &ItemId,
    ) -> Result<Result<SelectedEvidenceItemV2, EvidenceLookupErrorV2>, ProcedureStateErrorV2> {
        self.workflow_memory.selected_evidence_item(
            &self.trace,
            consumer_attempt_id,
            source_node,
            item_id,
        )
    }
    pub const fn created_at(&self) -> UnixMillis {
        self.created_at
    }
    pub const fn completed_at(&self) -> Option<UnixMillis> {
        self.completed_at
    }
    pub const fn cancelled_at(&self) -> Option<UnixMillis> {
        self.cancelled_at
    }
    pub fn cancel_reason(&self) -> Option<&str> {
        self.cancel_reason.as_deref()
    }

    pub fn mutate_active_item_v2(
        &self,
        expected_attempt_id: &AttemptId,
        item_id: &ItemId,
        expected_item_revision: Revision,
        mutation: ActiveItemMutationV2,
        now: UnixMillis,
    ) -> Result<GraphItemMutationOutcomeV2, GraphMutationErrorV2> {
        let memory = self.workflow_memory.mutate_active_item_v2(
            &self.snapshot,
            &self.trace,
            expected_attempt_id,
            item_id,
            expected_item_revision,
            mutation,
            now,
        )?;
        if !memory.changed {
            return Ok(GraphItemMutationOutcomeV2 {
                state: self.clone(),
                changed: false,
                item_revision: memory.item_revision,
                value_digest: memory.value_digest,
            });
        }
        let workspace_revision = self
            .workspace_revision
            .checked_next()
            .map_err(GraphMutationErrorV2::Domain)?;
        let trace = SessionTraceV2::from_parts(
            self.trace.session_id().clone(),
            self.trace.lifecycle(),
            self.trace
                .revision()
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.trace.attempts().to_vec(),
        )
        .map_err(GraphMutationErrorV2::Domain)?;
        let state = Self::new_with_goal_state(
            workspace_revision,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            memory.memory,
            self.goal_state.clone(),
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?;
        Ok(GraphItemMutationOutcomeV2 {
            state,
            changed: true,
            item_revision: memory.item_revision,
            value_digest: memory.value_digest,
        })
    }

    pub fn mutate_active_items_v2(
        &self,
        expected_attempt_id: &AttemptId,
        mutations: &[ActiveItemMutationRequestV2],
        now: UnixMillis,
    ) -> Result<GraphItemsMutationOutcomeV2, GraphMutationErrorV2> {
        let memory = self.workflow_memory.mutate_active_items_v2(
            &self.snapshot,
            &self.trace,
            expected_attempt_id,
            mutations,
            now,
        )?;
        let changed = memory.outcomes.iter().any(|outcome| outcome.changed);
        let items = memory
            .outcomes
            .into_iter()
            .map(|outcome| GraphItemMutationEntryV2 {
                item_id: outcome.item_id,
                changed: outcome.changed,
                item_revision: outcome.item_revision,
                value_digest: outcome.value_digest,
            })
            .collect();
        if !changed {
            return Ok(GraphItemsMutationOutcomeV2 {
                state: self.clone(),
                changed: false,
                items,
            });
        }
        let workspace_revision = self
            .workspace_revision
            .checked_next()
            .map_err(GraphMutationErrorV2::Domain)?;
        let trace = SessionTraceV2::from_parts(
            self.trace.session_id().clone(),
            self.trace.lifecycle(),
            self.trace
                .revision()
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.trace.attempts().to_vec(),
        )
        .map_err(GraphMutationErrorV2::Domain)?;
        let state = Self::new_with_goal_state(
            workspace_revision,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            memory.memory,
            self.goal_state.clone(),
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?;
        Ok(GraphItemsMutationOutcomeV2 {
            state,
            changed: true,
            items,
        })
    }

    pub fn block_active_attempt_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        blocker_id: BlockerId,
        reason: impl Into<String>,
        now: UnixMillis,
    ) -> Result<GraphBlockOutcomeV2, GraphMutationErrorV2> {
        let active =
            self.require_running_active_v2(expected_session_revision, expected_attempt_id)?;
        let graph_node_id = active.graph_node_id().clone();
        let memory = self.workflow_memory.block_active_attempt_v2(
            expected_attempt_id,
            blocker_id.clone(),
            reason.into(),
            now,
        )?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            SessionTraceV2::from_parts(
                self.trace.session_id().clone(),
                self.trace.lifecycle(),
                self.trace
                    .revision()
                    .checked_next()
                    .map_err(GraphMutationErrorV2::Domain)?,
                self.trace.attempts().to_vec(),
            )
            .map_err(GraphMutationErrorV2::Domain)?,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            memory,
            self.goal_state.clone(),
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?;
        Ok(GraphBlockOutcomeV2 {
            state,
            graph_node_id,
            attempt_id: expected_attempt_id.clone(),
            blocker_id,
        })
    }

    pub fn unblock_active_attempt_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        blocker_id: Option<&BlockerId>,
        unblock_all: bool,
        now: UnixMillis,
    ) -> Result<GraphUnblockOutcomeV2, GraphMutationErrorV2> {
        let active =
            self.require_running_active_v2(expected_session_revision, expected_attempt_id)?;
        let graph_node_id = active.graph_node_id().clone();
        let (memory, blocker_ids) = self.workflow_memory.unblock_active_attempt_v2(
            expected_attempt_id,
            blocker_id,
            unblock_all,
            now,
        )?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            SessionTraceV2::from_parts(
                self.trace.session_id().clone(),
                self.trace.lifecycle(),
                self.trace
                    .revision()
                    .checked_next()
                    .map_err(GraphMutationErrorV2::Domain)?,
                self.trace.attempts().to_vec(),
            )
            .map_err(GraphMutationErrorV2::Domain)?,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            memory,
            self.goal_state.clone(),
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?;
        Ok(GraphUnblockOutcomeV2 {
            state,
            graph_node_id,
            attempt_id: expected_attempt_id.clone(),
            blocker_ids,
        })
    }

    pub fn cancel_active_session_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        reason: ReasonV2,
        now: UnixMillis,
    ) -> Result<GraphCancelOutcomeV2, GraphMutationErrorV2> {
        let active =
            self.require_running_active_v2(expected_session_revision, expected_attempt_id)?;
        let graph_node_id = active.graph_node_id().clone();
        let mut trace = self.trace.clone();
        trace
            .cancel(expected_attempt_id)
            .map_err(GraphMutationErrorV2::Domain)?;
        let mut metadata = self.attempt_metadata.clone();
        let item = metadata
            .iter_mut()
            .find(|item| item.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        *item = AttemptMetadataV2::new(
            expected_attempt_id.clone(),
            item.started_at(),
            Some(now),
            Some(reason.as_str().to_owned()),
        )?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            self.counters.clone(),
            metadata,
            self.workflow_memory.clone(),
            self.goal_state.clone(),
            self.created_at,
            None,
            Some(now),
            Some(reason.into_inner()),
        )?;
        Ok(GraphCancelOutcomeV2 {
            state,
            graph_node_id,
            attempt_id: expected_attempt_id.clone(),
        })
    }

    fn require_running_active_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
    ) -> Result<&SessionAttemptV2, GraphMutationErrorV2> {
        if self.trace.lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning);
        }
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        let active =
            self.trace
                .active_attempt()
                .ok_or_else(|| GraphMutationErrorV2::AttemptNotCurrent {
                    expected: expected_attempt_id.clone(),
                    actual: None,
                })?;
        if active.attempt_id() != expected_attempt_id {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: expected_attempt_id.clone(),
                actual: Some(active.attempt_id().clone()),
            });
        }
        Ok(active)
    }

    pub fn complete_active_action_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        fresh_attempt_id: Option<AttemptId>,
        now: UnixMillis,
    ) -> Result<GraphActionCompletionOutcomeV2, GraphMutationErrorV2> {
        if self.trace.lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning);
        }
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        let active =
            self.trace
                .active_attempt()
                .ok_or_else(|| GraphMutationErrorV2::AttemptNotCurrent {
                    expected: expected_attempt_id.clone(),
                    actual: None,
                })?;
        if active.attempt_id() != expected_attempt_id {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: expected_attempt_id.clone(),
                actual: Some(active.attempt_id().clone()),
            });
        }
        let node = self
            .snapshot
            .graph_node(active.graph_node_id())
            .ok_or_else(|| invalid("active Procedure v2 graph node is absent"))?;
        if node.node_kind() != NodeKindV2::Action {
            return Err(GraphMutationErrorV2::GraphNodeTypeMismatch {
                graph_node_id: active.graph_node_id().clone(),
                actual: node.node_kind(),
            });
        }
        let placement: Value = serde_json::from_str(node.canonical_placement_json())
            .map_err(|_| invalid("active Procedure v2 placement is invalid"))?;
        let placement = placement
            .as_object()
            .ok_or_else(|| invalid("active Procedure v2 placement is invalid"))?;
        let target = placement
            .get("next")
            .and_then(Value::as_str)
            .map(|value| {
                GraphNodeId::new(value.to_owned())
                    .map_err(|_| invalid("Procedure v2 action target is invalid"))
            })
            .transpose()?;
        let terminal = placement.get("terminal").and_then(Value::as_bool) == Some(true);
        if target.is_some() == terminal {
            return Err(invalid("Procedure v2 action outcome is invalid").into());
        }
        if terminal && self.snapshot.goal_tracking() {
            let goal_revision = self
                .goal_state
                .current_revision()
                .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
            if self
                .goal_state
                .latest_fresh_assessment(&self.trace)
                .is_none()
            {
                return Err(GraphMutationErrorV2::FreshGoalAssessmentMissing { goal_revision });
            }
        }
        let active_metadata = self
            .attempt_metadata
            .iter()
            .find(|metadata| metadata.attempt_id() == active.attempt_id())
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        if now < active_metadata.started_at() {
            return Err(invalid("Procedure v2 completion timestamp regressed").into());
        }

        let from_graph_node_id = active.graph_node_id().clone();
        let from_attempt_id = active.attempt_id().clone();
        let mut trace = self.trace.clone();
        let to_attempt_id = match (&target, fresh_attempt_id) {
            (Some(target), Some(fresh_attempt_id)) => {
                trace.advance(
                    expected_attempt_id,
                    AdvanceTerminalV2::Completed,
                    target.clone(),
                    fresh_attempt_id.clone(),
                    self.goal_state.current_revision(),
                )?;
                Some(fresh_attempt_id)
            }
            (None, None) => {
                trace.finish(expected_attempt_id, AdvanceTerminalV2::Completed)?;
                None
            }
            _ => {
                return Err(
                    invalid("Procedure v2 action fresh-attempt identity is invalid").into(),
                );
            }
        };
        let workflow_memory = self.workflow_memory.complete_action_successor_v2(
            &self.snapshot,
            &self.trace,
            &trace,
            now,
        )?;
        let mut metadata = self.attempt_metadata.clone();
        let active_index = metadata
            .iter()
            .position(|candidate| candidate.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        metadata[active_index] = AttemptMetadataV2::new(
            expected_attempt_id.clone(),
            metadata[active_index].started_at(),
            Some(now),
            None,
        )?;
        if let Some(to_attempt_id) = &to_attempt_id {
            metadata.push(AttemptMetadataV2::new(
                to_attempt_id.clone(),
                now,
                None,
                None,
            )?);
        }
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                Ok(GraphNodeCounterV2::new(
                    counter.graph_node_id().clone(),
                    counter
                        .attempt_count()
                        .checked_add(u64::from(target.as_ref() == Some(counter.graph_node_id())))
                        .ok_or_else(|| invalid("Procedure v2 action attempt counter overflowed"))?,
                    counter.rework_traversal_count(),
                ))
            })
            .collect::<Result<Vec<_>, GraphMutationErrorV2>>()?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            counters,
            metadata,
            workflow_memory,
            self.goal_state.clone(),
            self.created_at,
            terminal.then_some(now),
            None,
            None,
        )?;
        Ok(GraphActionCompletionOutcomeV2 {
            state,
            from_graph_node_id,
            from_attempt_id,
            to_graph_node_id: target,
            to_attempt_id,
        })
    }

    pub fn retry_active_attempt_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        fresh_attempt_id: AttemptId,
        reason: ReasonV2,
        now: UnixMillis,
    ) -> Result<GraphRetryOutcomeV2, GraphMutationErrorV2> {
        if self.trace.lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning);
        }
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        let active =
            self.trace
                .active_attempt()
                .ok_or_else(|| GraphMutationErrorV2::AttemptNotCurrent {
                    expected: expected_attempt_id.clone(),
                    actual: None,
                })?;
        if active.attempt_id() != expected_attempt_id {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: expected_attempt_id.clone(),
                actual: Some(active.attempt_id().clone()),
            });
        }
        let active_metadata = self
            .attempt_metadata
            .iter()
            .find(|metadata| metadata.attempt_id() == active.attempt_id())
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        if now < active_metadata.started_at() {
            return Err(invalid("Procedure v2 retry timestamp regressed").into());
        }

        let graph_node_id = active.graph_node_id().clone();
        let from_attempt_id = active.attempt_id().clone();
        let to_attempt_id = fresh_attempt_id.clone();
        let mut trace = self.trace.clone();
        trace.retry(
            expected_attempt_id,
            fresh_attempt_id,
            self.goal_state.current_revision(),
        )?;
        let workflow_memory =
            self.workflow_memory
                .retry_successor_v2(&self.snapshot, &trace, now)?;
        let mut metadata = self.attempt_metadata.clone();
        let active_index = metadata
            .iter()
            .position(|candidate| candidate.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        metadata[active_index] = AttemptMetadataV2::new(
            expected_attempt_id.clone(),
            metadata[active_index].started_at(),
            Some(now),
            Some(reason.into_inner()),
        )?;
        metadata.push(AttemptMetadataV2::new(
            to_attempt_id.clone(),
            now,
            None,
            None,
        )?);
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                Ok(GraphNodeCounterV2::new(
                    counter.graph_node_id().clone(),
                    counter
                        .attempt_count()
                        .checked_add(u64::from(counter.graph_node_id() == &graph_node_id))
                        .ok_or_else(|| invalid("Procedure v2 retry attempt counter overflowed"))?,
                    counter.rework_traversal_count(),
                ))
            })
            .collect::<Result<Vec<_>, GraphMutationErrorV2>>()?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            counters,
            metadata,
            workflow_memory,
            self.goal_state.clone(),
            self.created_at,
            None,
            None,
            None,
        )?;
        Ok(GraphRetryOutcomeV2 {
            state,
            graph_node_id,
            from_attempt_id,
            to_attempt_id,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decide_active_route_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        selected_option: OptionId,
        fresh_attempt_id: AttemptId,
        reason: Option<ReasonV2>,
        actor: Option<ActorAttributionV2>,
        now: UnixMillis,
    ) -> Result<GraphDecisionOutcomeV2, GraphMutationErrorV2> {
        let active =
            self.require_running_active_v2(expected_session_revision, expected_attempt_id)?;
        let active_metadata = self
            .attempt_metadata
            .iter()
            .find(|metadata| metadata.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        if now < active_metadata.started_at() {
            return Err(invalid("Procedure v2 decision timestamp regressed").into());
        }
        if self
            .snapshot
            .graph_node(active.graph_node_id())
            .is_some_and(|node| node.goal_assessment())
        {
            return Err(
                GraphMutationErrorV2::GoalAssessmentDecisionRequiresAssessment {
                    graph_node_id: active.graph_node_id().clone(),
                },
            );
        }
        self.decide_active_route_with_goal_revision_v2(
            expected_session_revision,
            expected_attempt_id,
            selected_option,
            fresh_attempt_id,
            None,
            reason,
            actor,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decide_active_route_with_goal_revision_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        selected_option: OptionId,
        fresh_attempt_id: AttemptId,
        expected_goal_revision: Option<GoalRevisionNumberV2>,
        reason: Option<ReasonV2>,
        actor: Option<ActorAttributionV2>,
        now: UnixMillis,
    ) -> Result<GraphDecisionOutcomeV2, GraphMutationErrorV2> {
        let active =
            self.require_running_active_v2(expected_session_revision, expected_attempt_id)?;
        let active_metadata = self
            .attempt_metadata
            .iter()
            .find(|metadata| metadata.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        if now < active_metadata.started_at() {
            return Err(invalid("Procedure v2 decision timestamp regressed").into());
        }
        let from_graph_node_id = active.graph_node_id().clone();
        let from_attempt_id = active.attempt_id().clone();
        let is_goal_assessment = self
            .snapshot
            .graph_node(active.graph_node_id())
            .is_some_and(|node| node.goal_assessment());
        if is_goal_assessment && expected_goal_revision.is_none() {
            return Err(invalid(
                "Procedure v2 goal-assessment decision requires a goal revision fence",
            )
            .into());
        }
        if let Some(expected_goal_revision) = expected_goal_revision {
            let current_goal_revision = self
                .goal_state
                .current_revision()
                .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
            if expected_goal_revision != current_goal_revision {
                return Err(GraphMutationErrorV2::GoalRevisionStale {
                    expected: expected_goal_revision,
                    actual: current_goal_revision,
                });
            }
            let Some(actual_goal_revision) = active.goal_revision() else {
                return Err(invalid(
                    "fenced Procedure v2 decision attempt lacks its goal revision binding",
                )
                .into());
            };
            if actual_goal_revision != expected_goal_revision {
                return Err(GraphMutationErrorV2::GoalRevisionStale {
                    expected: expected_goal_revision,
                    actual: actual_goal_revision,
                });
            }
        }
        let goal_assessment_state = if is_goal_assessment {
            let goal_revision = self
                .goal_state
                .current_revision()
                .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
            debug_assert_eq!(active.goal_revision(), Some(goal_revision));
            let revision = self
                .goal_state
                .revisions()
                .iter()
                .find(|revision| revision.revision() == goal_revision)
                .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
            let attempt_state = self
                .goal_state
                .attempt_assessments()
                .iter()
                .find(|assessment| assessment.attempt_id() == active.attempt_id());
            let missing_criterion_ids = revision
                .criteria()
                .criteria()
                .iter()
                .filter(|criterion| {
                    !attempt_state.is_some_and(|assessment| {
                        assessment
                            .results()
                            .iter()
                            .any(|state| state.result().criterion_id() == criterion.id())
                    })
                })
                .map(|criterion| criterion.id().clone())
                .collect::<Vec<_>>();
            if !missing_criterion_ids.is_empty() {
                return Err(GraphMutationErrorV2::CriterionResultMissing {
                    missing_criterion_ids,
                });
            }
            let attempt_state = attempt_state
                .ok_or_else(|| invalid("complete Procedure v2 goal assessment is absent"))?;
            let outcome = if attempt_state.results()[0].result().mode()
                == crate::CriterionAssessmentModeV2::Applicability
            {
                GoalOutcome::Superseded
            } else if attempt_state
                .results()
                .iter()
                .all(|state| state.result().status() == crate::CriterionStatusV2::Satisfied)
            {
                GoalOutcome::Achieved
            } else {
                GoalOutcome::NotAchieved
            };
            Some((goal_revision, attempt_state, outcome))
        } else {
            None
        };
        let transition = self.workflow_memory.decide_active_route_v2(
            &self.snapshot,
            &self.trace,
            expected_attempt_id,
            selected_option,
            fresh_attempt_id,
            self.goal_state.current_revision(),
            reason,
            actor,
            goal_assessment_state.map(|(_, _, outcome)| outcome),
            now,
        )?;
        let to_graph_node_id = transition.decision.route_target().clone();
        let to_attempt_id = transition
            .trace
            .active_attempt()
            .ok_or_else(|| invalid("fresh Procedure v2 decision target is absent"))?
            .attempt_id()
            .clone();
        let mut metadata = self.attempt_metadata.clone();
        let active_index = metadata
            .iter()
            .position(|candidate| candidate.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        metadata[active_index] = AttemptMetadataV2::new(
            expected_attempt_id.clone(),
            metadata[active_index].started_at(),
            Some(now),
            None,
        )?;
        metadata.push(AttemptMetadataV2::new(
            to_attempt_id.clone(),
            now,
            None,
            None,
        )?);
        let declared_rework = transition.declared_rework.clone();
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                let is_target = counter.graph_node_id() == &to_graph_node_id;
                Ok(GraphNodeCounterV2::new(
                    counter.graph_node_id().clone(),
                    counter
                        .attempt_count()
                        .checked_add(u64::from(is_target))
                        .ok_or_else(|| {
                            invalid("Procedure v2 decision attempt counter overflowed")
                        })?,
                    counter
                        .rework_traversal_count()
                        .checked_add(u64::from(is_target && declared_rework.is_some()))
                        .ok_or_else(|| {
                            invalid("Procedure v2 decision rework counter overflowed")
                        })?,
                ))
            })
            .collect::<Result<Vec<_>, GraphMutationErrorV2>>()?;
        let decision = transition.decision.clone();
        let goal_assessment = goal_assessment_state
            .map(|(goal_revision, attempt_state, outcome)| {
                GoalAssessmentRecordV2::new(
                    goal_revision,
                    outcome,
                    attempt_state
                        .results()
                        .iter()
                        .map(|state| state.result().clone())
                        .collect(),
                    decision.evidence().clone(),
                    decision.actor().cloned(),
                    decision.attempt_id().clone(),
                    decision.graph_node_id().clone(),
                    decision.trace(),
                    decision.recorded_at(),
                )
                .map_err(GraphMutationErrorV2::Domain)
            })
            .transpose()?;
        let goal_state = if let Some(assessment) = &goal_assessment {
            let mut assessments = self.goal_state.assessments().to_vec();
            assessments.push(assessment.clone());
            GoalStateV2::new(
                self.goal_state.current_revision(),
                self.goal_state.revisions().to_vec(),
                self.goal_state.attempt_assessments().to_vec(),
                assessments,
            )?
        } else {
            self.goal_state.clone()
        };
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            transition.trace,
            counters,
            metadata,
            transition.memory,
            goal_state,
            self.created_at,
            None,
            None,
            None,
        )?;
        Ok(GraphDecisionOutcomeV2 {
            state,
            decision,
            goal_assessment,
            declared_rework,
            from_graph_node_id,
            from_attempt_id,
            to_graph_node_id,
            to_attempt_id,
        })
    }

    pub fn bind_initial_goal_at_start_v2(
        &self,
        statement: GoalStatementV2,
        criteria: GoalDefinitionV2,
        actor: Option<ActorAttributionV2>,
        now: UnixMillis,
    ) -> Result<GraphGoalDefinitionOutcomeV2, GraphMutationErrorV2> {
        if !self.snapshot.goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled);
        }
        if let Some(goal_revision) = self.goal_state.current_revision() {
            return Err(GraphMutationErrorV2::SessionGoalAlreadyDefined { goal_revision });
        }
        let active = self
            .trace
            .active_attempt()
            .ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let mut trace = self.trace.clone();
        trace.bind_initial_goal_revision_at_start(active.attempt_id())?;
        let revision = GoalRevisionRecordV2::new(
            GoalRevisionNumberV2::FIRST,
            None,
            statement,
            criteria,
            None,
            None,
            false,
            actor,
            active.trace(),
            now,
        )?;
        let goal_state = GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![revision.clone()],
            Vec::new(),
            Vec::new(),
        )?;
        let state = Self::new_with_goal_state(
            self.workspace_revision,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            self.workflow_memory.clone(),
            goal_state,
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?;
        Ok(GraphGoalDefinitionOutcomeV2 { state, revision })
    }

    pub fn define_goal_v2(
        &self,
        expected_session_revision: Revision,
        statement: GoalStatementV2,
        criteria: GoalDefinitionV2,
        actor: Option<ActorAttributionV2>,
        now: UnixMillis,
    ) -> Result<GraphGoalDefinitionOutcomeV2, GraphMutationErrorV2> {
        if !self.snapshot.goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled);
        }
        if let Some(goal_revision) = self.goal_state.current_revision() {
            return Err(GraphMutationErrorV2::SessionGoalAlreadyDefined { goal_revision });
        }
        if self.trace.lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning);
        }
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        let active = self
            .trace
            .active_attempt()
            .ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let mut trace = self.trace.clone();
        trace.bind_initial_goal_revision(active.attempt_id())?;
        let revision = GoalRevisionRecordV2::new(
            GoalRevisionNumberV2::FIRST,
            None,
            statement,
            criteria,
            None,
            None,
            false,
            actor,
            active.trace(),
            now,
        )?;
        let goal_state = GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![revision.clone()],
            Vec::new(),
            Vec::new(),
        )?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            self.workflow_memory.clone(),
            goal_state,
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?;
        Ok(GraphGoalDefinitionOutcomeV2 { state, revision })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn assess_goal_criterion_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        expected_goal_revision: GoalRevisionNumberV2,
        result: CriterionAssessmentResultV2,
        actor: Option<ActorAttributionV2>,
        now: UnixMillis,
    ) -> Result<GraphCriterionAssessmentOutcomeV2, GraphMutationErrorV2> {
        if !self.snapshot.goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled);
        }
        let active =
            self.require_running_active_v2(expected_session_revision, expected_attempt_id)?;
        let current_goal_revision = self
            .goal_state
            .current_revision()
            .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
        if current_goal_revision != expected_goal_revision {
            return Err(GraphMutationErrorV2::GoalRevisionStale {
                expected: expected_goal_revision,
                actual: current_goal_revision,
            });
        }
        if active.goal_revision() != Some(current_goal_revision) {
            return Err(GraphMutationErrorV2::GoalRevisionStale {
                expected: expected_goal_revision,
                actual: active.goal_revision().unwrap_or(current_goal_revision),
            });
        }
        let node = self
            .snapshot
            .graph_node(active.graph_node_id())
            .ok_or_else(|| invalid("active Procedure v2 graph node is absent"))?;
        if node.node_kind() != NodeKindV2::Decision {
            return Err(GraphMutationErrorV2::GraphNodeTypeMismatch {
                graph_node_id: active.graph_node_id().clone(),
                actual: node.node_kind(),
            });
        }
        if !node.goal_assessment() {
            return Err(GraphMutationErrorV2::GoalAssessmentDecisionRequired {
                graph_node_id: active.graph_node_id().clone(),
            });
        }

        let revision = self
            .goal_state
            .revisions()
            .iter()
            .find(|revision| revision.revision() == current_goal_revision)
            .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
        let criterion_order = revision
            .criteria()
            .criteria()
            .iter()
            .enumerate()
            .map(|(index, criterion)| (criterion.id(), index))
            .collect::<BTreeMap<_, _>>();
        if !criterion_order.contains_key(result.criterion_id()) {
            return Err(GraphMutationErrorV2::CriterionNotFound {
                criterion_id: result.criterion_id().clone(),
            });
        }

        let active_memory = self
            .workflow_memory
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == active.attempt_id())
            .ok_or_else(|| invalid("active Procedure v2 workflow memory is absent"))?;
        let existing = self
            .goal_state
            .attempt_assessments()
            .iter()
            .find(|assessment| assessment.attempt_id() == active.attempt_id());
        if existing.is_some_and(|assessment| {
            assessment
                .results()
                .iter()
                .any(|state| state.result().criterion_id() == result.criterion_id())
        }) {
            return Err(GraphMutationErrorV2::CriterionResultAlreadyRecorded {
                criterion_id: result.criterion_id().clone(),
            });
        }
        if let Some(expected_mode) = existing
            .and_then(|assessment| assessment.results().first())
            .map(|state| state.result().mode())
            && expected_mode != result.mode()
        {
            return Err(GraphMutationErrorV2::CriterionModeMixed {
                criterion_id: result.criterion_id().clone(),
                expected_mode,
                actual_status: result.status(),
            });
        }
        if result.citations().iter().collect::<BTreeSet<_>>().len() != result.citations().len() {
            return Err(GraphMutationErrorV2::CriterionCitationInvalid {
                criterion_id: result.criterion_id().clone(),
                citation: result.citations()[0].clone(),
            });
        }
        for citation in result.citations() {
            let valid = match citation {
                CriterionCitationV2::Evidence(source_graph_node_id) => active_memory
                    .evidence()
                    .iter()
                    .find(|reference| reference.resolution().source_node() == source_graph_node_id)
                    .and_then(|reference| match reference.resolution() {
                        crate::ResolvedEvidenceReferenceV2::Resolved(snapshot) => {
                            Some(snapshot.source_attempt_id())
                        }
                        crate::ResolvedEvidenceReferenceV2::Skipped(_)
                        | crate::ResolvedEvidenceReferenceV2::Unresolved { .. } => None,
                    })
                    .is_some_and(|source_attempt_id| {
                        self.trace.attempts().iter().any(|source| {
                            source.attempt_id() == source_attempt_id
                                && source.validity() == crate::AttemptValidityV2::Valid
                        })
                    }),
                CriterionCitationV2::Item(item_id) => active_memory
                    .item_slots()
                    .iter()
                    .any(|slot| slot.item_id() == item_id && slot.value().is_some()),
            };
            if !valid {
                return Err(GraphMutationErrorV2::CriterionCitationInvalid {
                    criterion_id: result.criterion_id().clone(),
                    citation: citation.clone(),
                });
            }
        }

        let assessment = CriterionAssessmentStateV2::new(result, actor, now);
        let mut attempt_assessments = self.goal_state.attempt_assessments().to_vec();
        if let Some(index) = attempt_assessments
            .iter()
            .position(|value| value.attempt_id() == active.attempt_id())
        {
            let mut results = attempt_assessments[index].results().to_vec();
            results.push(assessment.clone());
            results.sort_by_key(|state| criterion_order[state.result().criterion_id()]);
            attempt_assessments[index] = AttemptCriterionAssessmentStateV2::new(
                active.attempt_id().clone(),
                current_goal_revision,
                results,
            )?;
        } else {
            attempt_assessments.push(AttemptCriterionAssessmentStateV2::new(
                active.attempt_id().clone(),
                current_goal_revision,
                vec![assessment.clone()],
            )?);
        }
        let complete = attempt_assessments
            .iter()
            .find(|value| value.attempt_id() == active.attempt_id())
            .is_some_and(|value| value.results().len() == criterion_order.len());
        let determined_outcome = complete.then(|| {
            let results = attempt_assessments
                .iter()
                .find(|value| value.attempt_id() == active.attempt_id())
                .expect("complete assessment owner exists")
                .results();
            if results[0].result().mode() == crate::CriterionAssessmentModeV2::Applicability {
                GoalOutcome::Superseded
            } else if results
                .iter()
                .all(|state| state.result().status() == crate::CriterionStatusV2::Satisfied)
            {
                GoalOutcome::Achieved
            } else {
                GoalOutcome::NotAchieved
            }
        });
        let goal_state = GoalStateV2::new(
            Some(current_goal_revision),
            self.goal_state.revisions().to_vec(),
            attempt_assessments,
            self.goal_state.assessments().to_vec(),
        )?;
        let trace = SessionTraceV2::from_parts(
            self.trace.session_id().clone(),
            self.trace.lifecycle(),
            self.trace
                .revision()
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.trace.attempts().to_vec(),
        )?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            self.counters.clone(),
            self.attempt_metadata.clone(),
            self.workflow_memory.clone(),
            goal_state,
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.clone(),
        )?;
        Ok(GraphCriterionAssessmentOutcomeV2 {
            state,
            assessment,
            graph_node_id: active.graph_node_id().clone(),
            attempt_id: active.attempt_id().clone(),
            goal_revision: current_goal_revision,
            complete,
            determined_outcome,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn revise_goal_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: Option<&AttemptId>,
        expected_goal_revision: GoalRevisionNumberV2,
        statement: GoalStatementV2,
        criteria: GoalDefinitionV2,
        target_graph_node_id: GraphNodeId,
        fresh_attempt_id: AttemptId,
        reason: GoalRevisionReasonV2,
        actor: Option<ActorAttributionV2>,
        reactivate: bool,
        now: UnixMillis,
    ) -> Result<GraphGoalRevisionOutcomeV2, GraphMutationErrorV2> {
        if !self.snapshot.goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled);
        }
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        let current = self
            .goal_state
            .current_revision()
            .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
        if current != expected_goal_revision {
            return Err(GraphMutationErrorV2::GoalRevisionStale {
                expected: expected_goal_revision,
                actual: current,
            });
        }
        match self.trace.lifecycle() {
            SessionLifecycle::Prepared => return Err(GraphMutationErrorV2::SessionNotRunning),
            SessionLifecycle::Cancelled => return Err(GraphMutationErrorV2::SessionCancelled),
            SessionLifecycle::Completed if !reactivate => {
                return Err(GraphMutationErrorV2::ReactivationFlagRequired);
            }
            SessionLifecycle::Completed => {
                if let Some(expected) = expected_attempt_id {
                    return Err(GraphMutationErrorV2::AttemptNotCurrent {
                        expected: expected.clone(),
                        actual: None,
                    });
                }
            }
            SessionLifecycle::Running => {
                let active = self
                    .trace
                    .active_attempt()
                    .ok_or(GraphMutationErrorV2::SessionNotRunning)?;
                let expected =
                    expected_attempt_id.ok_or_else(|| GraphMutationErrorV2::AttemptNotCurrent {
                        expected: active.attempt_id().clone(),
                        actual: Some(active.attempt_id().clone()),
                    })?;
                if active.attempt_id() != expected {
                    return Err(GraphMutationErrorV2::AttemptNotCurrent {
                        expected: expected.clone(),
                        actual: Some(active.attempt_id().clone()),
                    });
                }
            }
        }
        validate_goal_revision_target_v2(&self.snapshot, &target_graph_node_id)?;
        if !self.trace.attempts().iter().any(|attempt| {
            attempt.graph_node_id() == &target_graph_node_id
                && attempt.validity() == crate::AttemptValidityV2::Valid
        }) {
            return Err(GraphMutationErrorV2::ManualReworkTargetNotOnTrace {
                target_graph_node_id,
            });
        }
        let next_revision = GoalRevisionNumberV2::new(
            current
                .get()
                .checked_add(1)
                .ok_or_else(|| invalid("Procedure v2 goal revision overflowed"))?,
        );
        let transition = self.workflow_memory.goal_rework_v2(
            &self.snapshot,
            &self.trace,
            expected_attempt_id,
            target_graph_node_id.clone(),
            fresh_attempt_id.clone(),
            next_revision,
            now,
        )?;
        let target = transition
            .trace
            .active_attempt()
            .ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let revision = GoalRevisionRecordV2::new(
            next_revision,
            Some(current),
            statement,
            criteria,
            Some(reason.clone()),
            Some(target_graph_node_id.clone()),
            self.trace.lifecycle() == SessionLifecycle::Completed,
            actor,
            target.trace(),
            now,
        )?;
        let mut revisions = self.goal_state.revisions().to_vec();
        revisions.push(revision.clone());
        let goal_state = GoalStateV2::new(
            Some(next_revision),
            revisions,
            self.goal_state.attempt_assessments().to_vec(),
            self.goal_state.assessments().to_vec(),
        )?;
        let mut metadata = self.attempt_metadata.clone();
        if self.trace.lifecycle() == SessionLifecycle::Running {
            let source = self
                .trace
                .active_attempt()
                .expect("running trace checked above");
            let source_index = metadata
                .iter()
                .position(|candidate| candidate.attempt_id() == source.attempt_id())
                .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
            metadata[source_index] = AttemptMetadataV2::new(
                source.attempt_id().clone(),
                metadata[source_index].started_at(),
                Some(now),
                Some(reason.as_str().to_owned()),
            )?;
        }
        metadata.push(AttemptMetadataV2::new(
            fresh_attempt_id.clone(),
            now,
            None,
            None,
        )?);
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                let is_target = counter.graph_node_id() == &target_graph_node_id;
                Ok(GraphNodeCounterV2::new(
                    counter.graph_node_id().clone(),
                    counter
                        .attempt_count()
                        .checked_add(u64::from(is_target))
                        .ok_or_else(|| invalid("Procedure v2 goal attempt counter overflowed"))?,
                    counter
                        .rework_traversal_count()
                        .checked_add(u64::from(is_target))
                        .ok_or_else(|| invalid("Procedure v2 goal rework counter overflowed"))?,
                ))
            })
            .collect::<Result<Vec<_>, GraphMutationErrorV2>>()?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            transition.trace,
            counters,
            metadata,
            transition.memory,
            goal_state,
            self.created_at,
            None,
            None,
            None,
        )?;
        Ok(GraphGoalRevisionOutcomeV2 {
            state,
            revision,
            target_attempt_id: fresh_attempt_id,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn manual_rework_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: Option<&AttemptId>,
        target_graph_node_id: GraphNodeId,
        fresh_attempt_id: AttemptId,
        reason: ReasonV2,
        actor: Option<ActorAttributionV2>,
        now: UnixMillis,
    ) -> Result<GraphManualReworkOutcomeV2, GraphMutationErrorV2> {
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        let source = match self.trace.lifecycle() {
            SessionLifecycle::Prepared => return Err(GraphMutationErrorV2::SessionNotRunning),
            SessionLifecycle::Cancelled => return Err(GraphMutationErrorV2::SessionCancelled),
            SessionLifecycle::Running => {
                let active = self
                    .trace
                    .active_attempt()
                    .ok_or_else(|| invalid("running Procedure v2 session has no active attempt"))?;
                let expected = expected_attempt_id.ok_or_else(|| {
                    invalid("running Procedure v2 manual rework requires an attempt fence")
                })?;
                if active.attempt_id() != expected {
                    return Err(GraphMutationErrorV2::AttemptNotCurrent {
                        expected: expected.clone(),
                        actual: Some(active.attempt_id().clone()),
                    });
                }
                active
            }
            SessionLifecycle::Completed => {
                if let Some(expected) = expected_attempt_id {
                    return Err(GraphMutationErrorV2::AttemptNotCurrent {
                        expected: expected.clone(),
                        actual: None,
                    });
                }
                self.trace
                    .attempts()
                    .last()
                    .ok_or_else(|| invalid("completed Procedure v2 session has no attempt"))?
            }
        };
        let source_metadata = self
            .attempt_metadata
            .iter()
            .find(|metadata| metadata.attempt_id() == source.attempt_id())
            .ok_or_else(|| invalid("Procedure v2 manual rework source metadata is absent"))?;
        if now
            < source_metadata
                .ended_at()
                .unwrap_or(source_metadata.started_at())
        {
            return Err(invalid("Procedure v2 manual rework timestamp regressed").into());
        }
        let terminal_reason = reason.clone();
        let transition = self.workflow_memory.manual_rework_v2(
            &self.snapshot,
            &self.trace,
            expected_attempt_id,
            target_graph_node_id,
            fresh_attempt_id,
            self.goal_state.current_revision(),
            reason,
            actor,
            now,
        )?;
        let to_graph_node_id = transition.record.to_node().clone();
        let to_attempt_id = transition.record.target_attempt_id().clone();
        let mut metadata = self.attempt_metadata.clone();
        if self.trace.lifecycle() == SessionLifecycle::Running {
            let source_index = metadata
                .iter()
                .position(|candidate| candidate.attempt_id() == source.attempt_id())
                .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
            metadata[source_index] = AttemptMetadataV2::new(
                source.attempt_id().clone(),
                metadata[source_index].started_at(),
                Some(now),
                Some(terminal_reason.as_str().to_owned()),
            )?;
        }
        metadata.push(AttemptMetadataV2::new(to_attempt_id, now, None, None)?);
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                let is_target = counter.graph_node_id() == &to_graph_node_id;
                Ok(GraphNodeCounterV2::new(
                    counter.graph_node_id().clone(),
                    counter
                        .attempt_count()
                        .checked_add(u64::from(is_target))
                        .ok_or_else(|| {
                            invalid("Procedure v2 manual rework attempt counter overflowed")
                        })?,
                    counter
                        .rework_traversal_count()
                        .checked_add(u64::from(is_target))
                        .ok_or_else(|| invalid("Procedure v2 manual rework counter overflowed"))?,
                ))
            })
            .collect::<Result<Vec<_>, GraphMutationErrorV2>>()?;
        let record = transition.record;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            transition.trace,
            counters,
            metadata,
            transition.memory,
            self.goal_state.clone(),
            self.created_at,
            None,
            None,
            None,
        )?;
        Ok(GraphManualReworkOutcomeV2 { state, record })
    }

    pub fn skip_active_action_v2(
        &self,
        expected_session_revision: Revision,
        expected_attempt_id: &AttemptId,
        fresh_attempt_id: Option<AttemptId>,
        reason: Option<ReasonV2>,
        now: UnixMillis,
    ) -> Result<GraphActionSkipOutcomeV2, GraphMutationErrorV2> {
        if self.trace.lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning);
        }
        if self.trace.revision() != expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: expected_session_revision,
                actual: self.trace.revision(),
            });
        }
        let active =
            self.trace
                .active_attempt()
                .ok_or_else(|| GraphMutationErrorV2::AttemptNotCurrent {
                    expected: expected_attempt_id.clone(),
                    actual: None,
                })?;
        if active.attempt_id() != expected_attempt_id {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: expected_attempt_id.clone(),
                actual: Some(active.attempt_id().clone()),
            });
        }
        let node = self
            .snapshot
            .graph_node(active.graph_node_id())
            .ok_or_else(|| invalid("active Procedure v2 graph node is absent"))?;
        if node.node_kind() != NodeKindV2::Action {
            return Err(GraphMutationErrorV2::GraphNodeTypeMismatch {
                graph_node_id: active.graph_node_id().clone(),
                actual: node.node_kind(),
            });
        }
        let placement: Value = serde_json::from_str(node.canonical_placement_json())
            .map_err(|_| invalid("active Procedure v2 placement is invalid"))?;
        let placement = placement
            .as_object()
            .ok_or_else(|| invalid("active Procedure v2 placement is invalid"))?;
        let skip = placement
            .get("skip")
            .and_then(Value::as_object)
            .filter(|skip| skip.get("allowed").and_then(Value::as_bool) == Some(true))
            .ok_or_else(|| GraphMutationErrorV2::SkipNotAllowed {
                graph_node_id: active.graph_node_id().clone(),
            })?;
        if skip.get("reason_required").and_then(Value::as_bool) == Some(true) && reason.is_none() {
            return Err(GraphMutationErrorV2::SkipReasonRequired {
                graph_node_id: active.graph_node_id().clone(),
            });
        }
        let target = placement
            .get("next")
            .and_then(Value::as_str)
            .map(|value| {
                GraphNodeId::new(value.to_owned())
                    .map_err(|_| invalid("Procedure v2 action target is invalid"))
            })
            .transpose()?;
        let terminal = placement.get("terminal").and_then(Value::as_bool) == Some(true);
        if target.is_some() == terminal {
            return Err(invalid("Procedure v2 action outcome is invalid").into());
        }
        if terminal && self.snapshot.goal_tracking() {
            let goal_revision = self
                .goal_state
                .current_revision()
                .ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
            if self
                .goal_state
                .latest_fresh_assessment(&self.trace)
                .is_none()
            {
                return Err(GraphMutationErrorV2::FreshGoalAssessmentMissing { goal_revision });
            }
        }
        let active_metadata = self
            .attempt_metadata
            .iter()
            .find(|metadata| metadata.attempt_id() == active.attempt_id())
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        if now < active_metadata.started_at() {
            return Err(invalid("Procedure v2 skip timestamp regressed").into());
        }

        let from_graph_node_id = active.graph_node_id().clone();
        let from_attempt_id = active.attempt_id().clone();
        let mut trace = self.trace.clone();
        let to_attempt_id = match (&target, fresh_attempt_id) {
            (Some(target), Some(fresh_attempt_id)) => {
                trace.advance(
                    expected_attempt_id,
                    AdvanceTerminalV2::Skipped,
                    target.clone(),
                    fresh_attempt_id.clone(),
                    self.goal_state.current_revision(),
                )?;
                Some(fresh_attempt_id)
            }
            (None, None) => {
                trace.finish(expected_attempt_id, AdvanceTerminalV2::Skipped)?;
                None
            }
            _ => return Err(invalid("Procedure v2 skip fresh-attempt identity is invalid").into()),
        };
        let workflow_memory = self.workflow_memory.skip_action_successor_v2(
            &self.snapshot,
            &self.trace,
            &trace,
            now,
        )?;
        let mut metadata = self.attempt_metadata.clone();
        let active_index = metadata
            .iter()
            .position(|candidate| candidate.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 attempt metadata is absent"))?;
        metadata[active_index] = AttemptMetadataV2::new(
            expected_attempt_id.clone(),
            metadata[active_index].started_at(),
            Some(now),
            reason.map(ReasonV2::into_inner),
        )?;
        if let Some(to_attempt_id) = &to_attempt_id {
            metadata.push(AttemptMetadataV2::new(
                to_attempt_id.clone(),
                now,
                None,
                None,
            )?);
        }
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                Ok(GraphNodeCounterV2::new(
                    counter.graph_node_id().clone(),
                    counter
                        .attempt_count()
                        .checked_add(u64::from(target.as_ref() == Some(counter.graph_node_id())))
                        .ok_or_else(|| invalid("Procedure v2 skip attempt counter overflowed"))?,
                    counter.rework_traversal_count(),
                ))
            })
            .collect::<Result<Vec<_>, GraphMutationErrorV2>>()?;
        let state = Self::new_with_goal_state(
            self.workspace_revision
                .checked_next()
                .map_err(GraphMutationErrorV2::Domain)?,
            self.task_title.clone(),
            self.snapshot.clone(),
            trace,
            counters,
            metadata,
            workflow_memory,
            self.goal_state.clone(),
            self.created_at,
            terminal.then_some(now),
            None,
            None,
        )?;
        Ok(GraphActionSkipOutcomeV2 {
            state,
            from_graph_node_id,
            from_attempt_id,
            to_graph_node_id: target,
            to_attempt_id,
        })
    }
}

fn validate_session_metadata(
    lifecycle: SessionLifecycle,
    created_at: UnixMillis,
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<&str>,
) -> Result<(), ProcedureStateErrorV2> {
    let cancel_reason_valid = cancel_reason.is_some_and(|reason| {
        !reason.trim().is_empty() && reason.chars().count() <= MAX_TERMINAL_REASON_CHARACTERS_V2
    });
    let valid = match lifecycle {
        SessionLifecycle::Prepared | SessionLifecycle::Running => {
            completed_at.is_none() && cancelled_at.is_none() && cancel_reason.is_none()
        }
        SessionLifecycle::Completed => {
            completed_at.is_some_and(|ended| ended >= created_at)
                && cancelled_at.is_none()
                && cancel_reason.is_none()
        }
        SessionLifecycle::Cancelled => {
            completed_at.is_none()
                && cancelled_at.is_some_and(|ended| ended >= created_at)
                && cancel_reason_valid
        }
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(
            "Procedure v2 session lifecycle metadata is inconsistent",
        ))
    }
}

fn validate_terminal_graph_state_v2(
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<&str>,
) -> Result<(), ProcedureStateErrorV2> {
    let Some(last) = trace.attempts().last() else {
        return Ok(());
    };
    let last_metadata = metadata
        .iter()
        .find(|item| item.attempt_id() == last.attempt_id())
        .ok_or_else(|| invalid("Procedure v2 terminal attempt metadata is absent"))?;
    let valid = match trace.lifecycle() {
        SessionLifecycle::Prepared => false,
        SessionLifecycle::Running => true,
        SessionLifecycle::Completed => {
            snapshot
                .graph_node(last.graph_node_id())
                .is_some_and(|node| {
                    if node.node_kind() != NodeKindV2::Action {
                        return false;
                    }
                    let Ok(placement) =
                        serde_json::from_str::<Value>(node.canonical_placement_json())
                    else {
                        return false;
                    };
                    placement.get("terminal").and_then(Value::as_bool) == Some(true)
                })
                && completed_at == last_metadata.ended_at()
                && cancelled_at.is_none()
                && cancel_reason.is_none()
        }
        SessionLifecycle::Cancelled => {
            cancelled_at == last_metadata.ended_at()
                && cancel_reason == last_metadata.terminal_reason()
                && completed_at.is_none()
        }
    };
    valid
        .then_some(())
        .ok_or_else(|| invalid("Procedure v2 terminal graph metadata is inconsistent"))
}

fn validate_graph_state_members(
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    counters: &[GraphNodeCounterV2],
    metadata: &[AttemptMetadataV2],
) -> Result<(), ProcedureStateErrorV2> {
    let mut counters_by_node = BTreeMap::new();
    for counter in counters {
        if snapshot.graph_node(counter.graph_node_id()).is_none()
            || counters_by_node
                .insert(counter.graph_node_id().clone(), counter)
                .is_some()
        {
            return Err(invalid(
                "Procedure v2 graph counters do not match the snapshot",
            ));
        }
    }
    if counters_by_node.len() != snapshot.graph_nodes().len() {
        return Err(invalid("Procedure v2 requires one counter per graph node"));
    }

    if metadata.len() != trace.attempts().len() {
        return Err(invalid("Procedure v2 attempt metadata is incomplete"));
    }
    let mut attempt_counts: BTreeMap<&GraphNodeId, u64> = BTreeMap::new();
    for (attempt, metadata) in trace.attempts().iter().zip(metadata) {
        if attempt.attempt_id() != metadata.attempt_id()
            || snapshot.graph_node(attempt.graph_node_id()).is_none()
        {
            return Err(invalid("Procedure v2 attempt identity is inconsistent"));
        }
        validate_attempt_metadata(attempt, metadata)?;
        *attempt_counts.entry(attempt.graph_node_id()).or_default() += 1;
    }
    for node in snapshot.graph_nodes() {
        let expected = attempt_counts
            .get(node.graph_node_id())
            .copied()
            .unwrap_or(0);
        if counters_by_node[node.graph_node_id()].attempt_count() != expected {
            return Err(invalid(
                "Procedure v2 attempt counter disagrees with the trace",
            ));
        }
    }
    Ok(())
}

fn validate_attempt_metadata(
    attempt: &SessionAttemptV2,
    metadata: &AttemptMetadataV2,
) -> Result<(), ProcedureStateErrorV2> {
    let ordered_end = metadata
        .ended_at()
        .is_some_and(|ended_at| ended_at >= metadata.started_at());
    let valid = match attempt.lifecycle() {
        AttemptLifecycle::Active => {
            metadata.ended_at().is_none() && metadata.terminal_reason().is_none()
        }
        AttemptLifecycle::Completed | AttemptLifecycle::Skipped => ordered_end,
        AttemptLifecycle::Abandoned => ordered_end && metadata.terminal_reason().is_some(),
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(
            "Procedure v2 attempt lifecycle metadata is inconsistent",
        ))
    }
}
