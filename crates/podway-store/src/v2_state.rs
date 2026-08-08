//! Procedure v2 graph, cursor, counter, and attempt persistence.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use podway_core::{
    AttemptId, AttemptLifecycle, AttemptNumberV2, CanonicalProcedureJsonV1, GoalRevisionNumberV2,
    GraphNodeId, NodeDefinitionId, NodeKindV2, ProcedureSnapshotId, ProcedureSourceKindV1,
    ProcedureSourceLabelV1, Revision, SessionAttemptV2, SessionId, SessionLifecycle,
    SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis, canonicalize_json_v1,
    verify_canonical_json_v1,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::v2_memory::{
    EvidenceReadbackV2, WorkflowMemoryStateV2, insert_workflow_memory_v2, load_workflow_memory_v2,
    replace_workflow_memory_v2, validate_workflow_memory_successor_v2, validate_workflow_memory_v2,
};
use crate::{
    DurableWorktreeIdentityV1, RusqliteErrorContextV1, StoreErrorV1, StoreInvariantV1,
    StoreRecordKindV1, StoreValueErrorV1, map_rusqlite_error_v1,
};

const PROCEDURE_SCHEMA_V2: &str = "podway.procedure/v2";
const PROCEDURE_SCHEMA_DOCUMENT_V2: &str =
    include_str!("../../../assets/schemas/procedure-v2.schema.json");
const MAX_GRAPH_NODES_V2: usize = 64;
const MAX_TASK_TITLE_CHARACTERS_V2: usize = 500;
const MAX_TERMINAL_REASON_CHARACTERS_V2: usize = 4_000;
type ActiveCursorColumnsV2<'a> = (Option<&'a str>, Option<&'a str>, Option<i64>);

fn invalid(reason: &'static str) -> StoreValueErrorV1 {
    StoreValueErrorV1::InvalidProcedureV2State { reason }
}

fn invalid_store(reason: &'static str) -> StoreErrorV1 {
    StoreErrorV1::InvalidStateV1(invalid(reason))
}

fn corrupt(record: StoreRecordKindV1) -> StoreErrorV1 {
    StoreErrorV1::CorruptStateV1 { record }
}

fn record_error(error: rusqlite::Error, record: StoreRecordKindV1) -> StoreErrorV1 {
    map_rusqlite_error_v1(error, RusqliteErrorContextV1::Record(record))
}

fn sqlite_u64(value: u64, field: &'static str) -> Result<i64, StoreErrorV1> {
    i64::try_from(value)
        .map_err(|_| StoreErrorV1::InvalidStateV1(StoreValueErrorV1::IntegerOutOfRange { field }))
}

fn persisted_u64(value: i64, record: StoreRecordKindV1) -> Result<u64, StoreErrorV1> {
    u64::try_from(value).map_err(|_| corrupt(record))
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, StoreValueErrorV1> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("canonical Procedure v2 metadata is incomplete"))
}

fn schema_matches_v2(root: &Value, schema: &Value, instance: &Value) -> bool {
    if let Some(boolean) = schema.as_bool() {
        return boolean;
    }
    let Some(schema) = schema.as_object() else {
        return false;
    };
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let Some(target) = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
        else {
            return false;
        };
        if !schema_matches_v2(root, target, instance) {
            return false;
        }
    }
    if let Some(expected) = schema.get("const")
        && expected != instance
    {
        return false;
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(instance)
    {
        return false;
    }
    if let Some(expected_type) = schema.get("type").and_then(Value::as_str)
        && !schema_type_matches_v2(expected_type, instance)
    {
        return false;
    }
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array)
        && !all_of
            .iter()
            .all(|branch| schema_matches_v2(root, branch, instance))
    {
        return false;
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array)
        && one_of
            .iter()
            .filter(|branch| schema_matches_v2(root, branch, instance))
            .count()
            != 1
    {
        return false;
    }
    if let Some(excluded) = schema.get("not")
        && schema_matches_v2(root, excluded, instance)
    {
        return false;
    }
    if let Some(text) = instance.as_str()
        && !schema_string_matches_v2(schema, text)
    {
        return false;
    }
    if let Some(items) = instance.as_array()
        && !schema_array_matches_v2(root, schema, items)
    {
        return false;
    }
    if let Some(object) = instance.as_object()
        && !schema_object_matches_v2(root, schema, object)
    {
        return false;
    }
    if instance.is_number() && !schema_number_matches_v2(schema, instance) {
        return false;
    }
    true
}

fn schema_type_matches_v2(expected: &str, instance: &Value) -> bool {
    match expected {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance
            .as_number()
            .is_some_and(|number| number.is_i64() || number.is_u64()),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => false,
    }
}

fn schema_string_matches_v2(schema: &serde_json::Map<String, Value>, text: &str) -> bool {
    let length = text.chars().count();
    if schema
        .get("minLength")
        .and_then(Value::as_u64)
        .is_some_and(|minimum| length < minimum as usize)
        || schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| length > maximum as usize)
    {
        return false;
    }
    match schema.get("pattern").and_then(Value::as_str) {
        None => true,
        Some("^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$") => identifier_pattern_matches_v2(text),
        Some("^[a-z0-9][a-z0-9!#$&^_.+-]*/[a-z0-9][a-z0-9!#$&^_.+-]*$") => {
            media_type_pattern_matches_v2(text)
        }
        Some(_) => false,
    }
}

fn identifier_pattern_matches_v2(text: &str) -> bool {
    let mut segments = text.split('-');
    let Some(first) = segments.next() else {
        return false;
    };
    if first.is_empty()
        || !first.as_bytes()[0].is_ascii_lowercase()
        || !first
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return false;
    }
    segments.all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

fn media_type_pattern_matches_v2(text: &str) -> bool {
    let mut parts = text.split('/');
    let Some(category) = parts.next() else {
        return false;
    };
    let Some(subtype) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && media_type_component_matches_v2(category)
        && media_type_component_matches_v2(subtype)
}

fn media_type_component_matches_v2(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
}

fn schema_array_matches_v2(
    root: &Value,
    schema: &serde_json::Map<String, Value>,
    items: &[Value],
) -> bool {
    if schema
        .get("minItems")
        .and_then(Value::as_u64)
        .is_some_and(|minimum| items.len() < minimum as usize)
        || schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| items.len() > maximum as usize)
    {
        return false;
    }
    if schema.get("uniqueItems") == Some(&Value::Bool(true))
        && items
            .iter()
            .enumerate()
            .any(|(index, item)| items[..index].contains(item))
    {
        return false;
    }
    schema.get("items").is_none_or(|item_schema| {
        items
            .iter()
            .all(|item| schema_matches_v2(root, item_schema, item))
    })
}

fn schema_object_matches_v2(
    root: &Value,
    schema: &serde_json::Map<String, Value>,
    object: &serde_json::Map<String, Value>,
) -> bool {
    if schema
        .get("minProperties")
        .and_then(Value::as_u64)
        .is_some_and(|minimum| object.len() < minimum as usize)
        || schema
            .get("maxProperties")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| object.len() > maximum as usize)
    {
        return false;
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array)
        && required.iter().any(|field| {
            field
                .as_str()
                .is_none_or(|field| !object.contains_key(field))
        })
    {
        return false;
    }
    if let Some(property_names) = schema.get("propertyNames")
        && object
            .keys()
            .any(|key| !schema_matches_v2(root, property_names, &Value::String(key.clone())))
    {
        return false;
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    for (key, value) in object {
        if let Some(property_schema) = properties.and_then(|properties| properties.get(key)) {
            if !schema_matches_v2(root, property_schema, value) {
                return false;
            }
            continue;
        }
        match schema.get("additionalProperties") {
            Some(Value::Bool(false)) => return false,
            Some(additional) if !schema_matches_v2(root, additional, value) => return false,
            _ => {}
        }
    }
    true
}

fn schema_number_matches_v2(schema: &serde_json::Map<String, Value>, instance: &Value) -> bool {
    let Some(number) = json_integer_v2(instance) else {
        return false;
    };
    !schema
        .get("minimum")
        .and_then(json_integer_v2)
        .is_some_and(|minimum| number < minimum)
        && !schema
            .get("maximum")
            .and_then(json_integer_v2)
            .is_some_and(|maximum| number > maximum)
}

fn json_integer_v2(value: &Value) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
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
    created_at: UnixMillis,
}

impl ProcedureSnapshotV2 {
    /// Rehydrates an admitted snapshot and verifies its exact canonical bytes, canonical schema,
    /// digest, and relational graph projection. Procedure source parsing, closed-reference
    /// validation, and graph vetting remain configuration-owned admission steps.
    pub fn new(
        snapshot_id: ProcedureSnapshotId,
        canonical_json: CanonicalProcedureJsonV1,
        digest: Sha256Digest,
        source: ProcedureSourceLabelV1,
        created_at: UnixMillis,
    ) -> Result<Self, StoreValueErrorV1> {
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
        let schema: Value = serde_json::from_str(PROCEDURE_SCHEMA_DOCUMENT_V2)
            .map_err(|_| invalid("canonical Procedure v2 schema is unavailable"))?;
        if !schema_matches_v2(&schema, &schema, &document) {
            return Err(invalid(
                "Procedure v2 snapshot does not satisfy the canonical schema",
            ));
        }
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
        if placements.is_empty() || placements.len() > MAX_GRAPH_NODES_V2 {
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

    fn graph_node(&self, id: &GraphNodeId) -> Option<&GraphNodeSnapshotV2> {
        self.graph_nodes
            .iter()
            .find(|node| node.graph_node_id() == id)
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
    ) -> Result<Self, StoreValueErrorV1> {
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
    created_at: UnixMillis,
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<String>,
}

impl GraphSessionStateV2 {
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
    ) -> Result<Self, StoreValueErrorV1> {
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
    ) -> Result<Self, StoreValueErrorV1> {
        let task_title = task_title.into();
        if task_title.trim().is_empty() || task_title.chars().count() > MAX_TASK_TITLE_CHARACTERS_V2
        {
            return Err(invalid("Procedure v2 task title is invalid"));
        }
        if workspace_revision == Revision::ZERO || trace.revision() == Revision::ZERO {
            return Err(invalid("Procedure v2 persisted revisions must be nonzero"));
        }
        if trace
            .attempts()
            .iter()
            .any(|attempt| attempt.goal_revision().is_some())
        {
            return Err(invalid(
                "Procedure v2 goal state is not part of graph persistence",
            ));
        }
        validate_session_metadata(
            trace.lifecycle(),
            created_at,
            completed_at,
            cancelled_at,
            cancel_reason.as_deref(),
        )?;
        validate_graph_state_members(&snapshot, &trace, &counters, &attempt_metadata)?;
        validate_workflow_memory_v2(&snapshot, &trace, &attempt_metadata, &workflow_memory)?;
        Ok(Self {
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
    pub fn selected_evidence_readback(
        &self,
        attempt_id: &AttemptId,
    ) -> Result<Vec<EvidenceReadbackV2>, StoreValueErrorV1> {
        self.workflow_memory
            .selected_readback(&self.trace, attempt_id)
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
}

fn validate_session_metadata(
    lifecycle: SessionLifecycle,
    created_at: UnixMillis,
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<&str>,
) -> Result<(), StoreValueErrorV1> {
    let cancel_reason_valid = cancel_reason.is_some_and(|reason| {
        !reason.trim().is_empty() && reason.chars().count() <= MAX_TERMINAL_REASON_CHARACTERS_V2
    });
    let valid = match lifecycle {
        SessionLifecycle::Running => {
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

fn validate_graph_state_members(
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    counters: &[GraphNodeCounterV2],
    metadata: &[AttemptMetadataV2],
) -> Result<(), StoreValueErrorV1> {
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
) -> Result<(), StoreValueErrorV1> {
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

/// Additive Store boundary for Procedure v2 graph/action state.
pub trait StoreGraphStateContractV2: Send + Sync {
    fn create_graph_session_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
        state: GraphSessionStateV2,
    ) -> Result<(), StoreErrorV1>;

    fn replace_graph_session_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
        expected_workspace_revision: Revision,
        expected_session_revision: Revision,
        state: GraphSessionStateV2,
    ) -> Result<(), StoreErrorV1>;

    fn read_graph_session_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<Option<GraphSessionStateV2>, StoreErrorV1>;
}

impl<Store> StoreGraphStateContractV2 for Arc<Store>
where
    Store: StoreGraphStateContractV2 + ?Sized,
{
    fn create_graph_session_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
        state: GraphSessionStateV2,
    ) -> Result<(), StoreErrorV1> {
        (**self).create_graph_session_v2(identity, state)
    }

    fn replace_graph_session_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
        expected_workspace_revision: Revision,
        expected_session_revision: Revision,
        state: GraphSessionStateV2,
    ) -> Result<(), StoreErrorV1> {
        (**self).replace_graph_session_v2(
            identity,
            expected_workspace_revision,
            expected_session_revision,
            state,
        )
    }

    fn read_graph_session_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<Option<GraphSessionStateV2>, StoreErrorV1> {
        (**self).read_graph_session_v2(identity)
    }
}

pub(crate) fn create_graph_session_transaction_v2(
    transaction: &Transaction<'_>,
    state: &GraphSessionStateV2,
) -> Result<(), StoreErrorV1> {
    let current_v1: i64 = transaction
        .query_row("SELECT COUNT(*) FROM task_sessions", [], |row| row.get(0))
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let current_v2: i64 = transaction
        .query_row("SELECT COUNT(*) FROM v2_task_sessions", [], |row| {
            row.get(0)
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let v2_workspace: i64 = transaction
        .query_row("SELECT COUNT(*) FROM v2_workspace_state", [], |row| {
            row.get(0)
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    let v2_snapshots: i64 = transaction
        .query_row("SELECT COUNT(*) FROM v2_procedure_snapshots", [], |row| {
            row.get(0)
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    if current_v1 != 0 || current_v2 != 0 || v2_workspace != 0 || v2_snapshots != 0 {
        return Err(invalid_store(
            "a workspace may contain only one current v1 or v2 task",
        ));
    }

    insert_snapshot_v2(transaction, state.snapshot())?;
    transaction
        .execute(
            "INSERT INTO v2_workspace_state (singleton, workspace_revision) VALUES (1, ?1)",
            [sqlite_u64(
                state.workspace_revision().get(),
                "Procedure v2 workspace revision",
            )?],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    insert_session_row_v2(transaction, state)?;
    insert_counters_v2(transaction, state)?;
    for (attempt, metadata) in state
        .trace()
        .attempts()
        .iter()
        .zip(state.attempt_metadata())
    {
        insert_attempt_v2(transaction, state, attempt, metadata)?;
    }
    insert_workflow_memory_v2(
        transaction,
        state.trace().session_id(),
        state.workflow_memory(),
    )?;
    Ok(())
}

pub(crate) fn replace_graph_session_transaction_v2(
    transaction: &Transaction<'_>,
    expected_workspace_revision: Revision,
    expected_session_revision: Revision,
    next: &GraphSessionStateV2,
) -> Result<(), StoreErrorV1> {
    let previous = load_graph_session_connection_v2(transaction)?
        .ok_or_else(|| invalid_store("no current Procedure v2 graph session exists"))?;
    if previous.workspace_revision() != expected_workspace_revision {
        return Err(StoreErrorV1::PreconditionConflictV1 {
            expected: Some(expected_workspace_revision),
            actual: Some(previous.workspace_revision()),
        });
    }
    if previous.trace().revision() != expected_session_revision {
        return Err(StoreErrorV1::PreconditionConflictV1 {
            expected: Some(expected_session_revision),
            actual: Some(previous.trace().revision()),
        });
    }
    validate_successor_v2(
        &previous,
        next,
        expected_workspace_revision,
        expected_session_revision,
    )
    .map_err(StoreErrorV1::InvalidStateV1)?;
    validate_workflow_memory_successor_v2(
        previous.snapshot(),
        previous.trace(),
        previous.workflow_memory(),
        next.trace(),
        next.workflow_memory(),
    )
    .map_err(StoreErrorV1::InvalidStateV1)?;

    for ((old_attempt, old_metadata), (new_attempt, new_metadata)) in previous
        .trace()
        .attempts()
        .iter()
        .zip(previous.attempt_metadata())
        .zip(next.trace().attempts().iter().zip(next.attempt_metadata()))
    {
        let changed = transaction
            .execute(
                "UPDATE v2_attempts SET lifecycle = ?1, validity = ?2, ended_at_ms = ?3, \
                 terminal_reason = ?4 WHERE attempt_id = ?5 AND lifecycle = ?6 AND validity = ?7 \
                 AND ended_at_ms IS ?8 AND terminal_reason IS ?9",
                params![
                    attempt_lifecycle_text(new_attempt.lifecycle()),
                    validity_text(new_attempt.validity()),
                    optional_sqlite_time(new_metadata.ended_at(), "Procedure v2 attempt end")?,
                    new_metadata.terminal_reason(),
                    old_attempt.attempt_id().as_str(),
                    attempt_lifecycle_text(old_attempt.lifecycle()),
                    validity_text(old_attempt.validity()),
                    optional_sqlite_time(old_metadata.ended_at(), "Procedure v2 attempt end")?,
                    old_metadata.terminal_reason(),
                ],
            )
            .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        if changed != 1 {
            return Err(StoreErrorV1::InternalInvariantViolationV1 {
                invariant: StoreInvariantV1::ProcedureV2GraphState,
            });
        }
    }
    for (attempt, metadata) in next
        .trace()
        .attempts()
        .iter()
        .zip(next.attempt_metadata())
        .skip(previous.trace().attempts().len())
    {
        insert_attempt_v2(transaction, next, attempt, metadata)?;
    }
    for counter in next.counters() {
        let changed = transaction
            .execute(
                "UPDATE v2_graph_node_counters SET attempt_count = ?1, rework_traversal_count = ?2 \
                 WHERE session_id = ?3 AND graph_node_id = ?4",
                params![
                    sqlite_u64(counter.attempt_count(), "Procedure v2 attempt count")?,
                    sqlite_u64(
                        counter.rework_traversal_count(),
                        "Procedure v2 rework traversal count",
                    )?,
                    next.trace().session_id().as_str(),
                    counter.graph_node_id().as_str(),
                ],
            )
            .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
        if changed != 1 {
            return Err(StoreErrorV1::InternalInvariantViolationV1 {
                invariant: StoreInvariantV1::ProcedureV2GraphState,
            });
        }
    }
    replace_workflow_memory_v2(
        transaction,
        next.trace().session_id(),
        previous.workflow_memory(),
        next.workflow_memory(),
    )?;
    update_session_row_v2(transaction, &previous, next)?;
    let changed = transaction
        .execute(
            "UPDATE v2_workspace_state SET workspace_revision = ?1 \
             WHERE singleton = 1 AND workspace_revision = ?2",
            params![
                sqlite_u64(
                    next.workspace_revision().get(),
                    "Procedure v2 workspace revision",
                )?,
                sqlite_u64(
                    expected_workspace_revision.get(),
                    "Procedure v2 workspace revision",
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    if changed != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }
    Ok(())
}

#[derive(Debug)]
struct PersistedSessionV2 {
    session_id: String,
    task_title: String,
    snapshot_id: String,
    lifecycle: String,
    session_revision: i64,
    latest_trace: i64,
    active_node: Option<String>,
    active_attempt: Option<String>,
    active_trace: Option<i64>,
    goal_tracking: i64,
    current_goal_revision: Option<i64>,
    created_at: i64,
    completed_at: Option<i64>,
    cancelled_at: Option<i64>,
    cancel_reason: Option<String>,
}

pub(crate) fn load_graph_session_connection_v2(
    connection: &Connection,
) -> Result<Option<GraphSessionStateV2>, StoreErrorV1> {
    let workspace_revision = connection
        .query_row(
            "SELECT workspace_revision FROM v2_workspace_state WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    let session = connection
        .query_row(
            "SELECT session_id, task_title, procedure_snapshot_id, lifecycle, session_revision, \
             latest_trace_sequence, active_graph_node_id, active_attempt_id, active_trace_sequence, \
             goal_tracking, current_goal_revision, created_at_ms, completed_at_ms, cancelled_at_ms, \
             cancel_reason FROM v2_task_sessions WHERE singleton = 1",
            [],
            |row| {
                Ok(PersistedSessionV2 {
                    session_id: row.get(0)?,
                    task_title: row.get(1)?,
                    snapshot_id: row.get(2)?,
                    lifecycle: row.get(3)?,
                    session_revision: row.get(4)?,
                    latest_trace: row.get(5)?,
                    active_node: row.get(6)?,
                    active_attempt: row.get(7)?,
                    active_trace: row.get(8)?,
                    goal_tracking: row.get(9)?,
                    current_goal_revision: row.get(10)?,
                    created_at: row.get(11)?,
                    completed_at: row.get(12)?,
                    cancelled_at: row.get(13)?,
                    cancel_reason: row.get(14)?,
                })
            },
        )
        .optional()
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;

    match (workspace_revision, session) {
        (None, None) => {
            let orphan_count: i64 = connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM v2_procedure_snapshots) + \
                     (SELECT COUNT(*) FROM v2_graph_nodes) + \
                     (SELECT COUNT(*) FROM v2_graph_node_counters) + \
                     (SELECT COUNT(*) FROM v2_attempts) + \
                     (SELECT COUNT(*) FROM v2_item_slots) + \
                     (SELECT COUNT(*) FROM v2_blockers) + \
                     (SELECT COUNT(*) FROM v2_resolved_evidence_references) + \
                     (SELECT COUNT(*) FROM v2_decision_records) + \
                     (SELECT COUNT(*) FROM v2_rework_records)",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
            if orphan_count == 0 {
                return Ok(None);
            }
            Err(corrupt(StoreRecordKindV1::Session))
        }
        (Some(workspace_revision), Some(session)) => {
            load_present_graph_session_v2(connection, workspace_revision, session).map(Some)
        }
        _ => Err(corrupt(StoreRecordKindV1::Session)),
    }
}

fn load_present_graph_session_v2(
    connection: &Connection,
    workspace_revision: i64,
    persisted: PersistedSessionV2,
) -> Result<GraphSessionStateV2, StoreErrorV1> {
    let current_v1: i64 = connection
        .query_row("SELECT COUNT(*) FROM task_sessions", [], |row| row.get(0))
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let snapshot_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_procedure_snapshots", [], |row| {
            row.get(0)
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    if current_v1 != 0 || snapshot_count != 1 || persisted.current_goal_revision.is_some() {
        return Err(corrupt(StoreRecordKindV1::Session));
    }
    let snapshot = load_snapshot_v2(connection, &persisted.snapshot_id)?;
    if persisted.goal_tracking != i64::from(snapshot.goal_tracking()) {
        return Err(corrupt(StoreRecordKindV1::Session));
    }

    let session_id = SessionId::new(persisted.session_id.clone())
        .map_err(|_| corrupt(StoreRecordKindV1::Session))?;
    let lifecycle = parse_session_lifecycle_v2(&persisted.lifecycle)?;
    let attempts = load_attempts_v2(connection, &session_id, &snapshot)?;
    let trace = SessionTraceV2::from_parts(
        session_id,
        lifecycle,
        Revision::new(persisted_u64(
            persisted.session_revision,
            StoreRecordKindV1::Session,
        )?),
        attempts,
    )
    .map_err(|_| corrupt(StoreRecordKindV1::Session))?;
    let counters = load_counters_v2(connection, trace.session_id(), &snapshot)?;
    let metadata = load_attempt_metadata_v2(connection, trace.session_id())?;
    let workflow_memory = load_workflow_memory_v2(connection, &snapshot, &trace, &metadata)?;
    let state = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(persisted_u64(
            workspace_revision,
            StoreRecordKindV1::Workspace,
        )?),
        persisted.task_title,
        snapshot,
        trace,
        counters,
        metadata,
        workflow_memory,
        persisted_time_v2(persisted.created_at, StoreRecordKindV1::Session)?,
        optional_persisted_time_v2(persisted.completed_at, StoreRecordKindV1::Session)?,
        optional_persisted_time_v2(persisted.cancelled_at, StoreRecordKindV1::Session)?,
        persisted.cancel_reason,
    )
    .map_err(|_| corrupt(StoreRecordKindV1::Session))?;

    let expected_cursor = active_cursor_values(state.trace())?;
    let persisted_cursor = (
        persisted.active_node.as_deref(),
        persisted.active_attempt.as_deref(),
        persisted.active_trace,
    );
    if persisted.latest_trace
        != sqlite_u64(
            latest_trace(state.trace()).get(),
            "Procedure v2 trace sequence",
        )?
        || expected_cursor != persisted_cursor
    {
        return Err(corrupt(StoreRecordKindV1::Session));
    }
    Ok(state)
}

fn load_snapshot_v2(
    connection: &Connection,
    expected_snapshot_id: &str,
) -> Result<ProcedureSnapshotV2, StoreErrorV1> {
    type SnapshotRow = (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
    );
    let row: SnapshotRow = connection
        .query_row(
            "SELECT schema_id, procedure_id, procedure_version, name, purpose, digest, \
             canonical_json, source_kind, goal_tracking, created_at_ms \
             FROM v2_procedure_snapshots WHERE snapshot_id = ?1",
            [expected_snapshot_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    let source_label: String = connection
        .query_row(
            "SELECT source_label FROM v2_procedure_snapshots WHERE snapshot_id = ?1",
            [expected_snapshot_id],
            |row| row.get(0),
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    if row.0 != PROCEDURE_SCHEMA_V2 {
        return Err(corrupt(StoreRecordKindV1::Snapshot));
    }
    let source_kind = ProcedureSourceKindV1::from_row_value(&row.7)
        .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?;
    let snapshot = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new(expected_snapshot_id.to_owned())
            .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        CanonicalProcedureJsonV1::new(row.6).map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        Sha256Digest::new(row.5).map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        ProcedureSourceLabelV1::from_row(source_kind, source_label)
            .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        persisted_time_v2(row.9, StoreRecordKindV1::Snapshot)?,
    )
    .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?;
    if snapshot.procedure_id() != row.1
        || snapshot.procedure_version() != row.2
        || snapshot.name() != row.3
        || snapshot.purpose() != row.4
        || i64::from(snapshot.goal_tracking()) != row.8
    {
        return Err(corrupt(StoreRecordKindV1::Snapshot));
    }
    verify_snapshot_nodes_v2(connection, &snapshot)?;
    Ok(snapshot)
}

fn verify_snapshot_nodes_v2(
    connection: &Connection,
    snapshot: &ProcedureSnapshotV2,
) -> Result<(), StoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT graph_node_id, node_definition_id, placement_index, node_type, \
             goal_assessment, canonical_placement_json FROM v2_graph_nodes \
             WHERE snapshot_id = ?1 ORDER BY placement_index",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    let rows = statement
        .query_map([snapshot.snapshot_id().as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    let persisted = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    if persisted.len() != snapshot.graph_nodes().len() {
        return Err(corrupt(StoreRecordKindV1::Snapshot));
    }
    for (row, node) in persisted.iter().zip(snapshot.graph_nodes()) {
        if row.0 != node.graph_node_id().as_str()
            || row.1 != node.node_definition_id().as_str()
            || row.2 != i64::from(node.placement_index())
            || row.3 != node.node_kind().as_str()
            || row.4 != i64::from(node.goal_assessment())
            || row.5 != node.canonical_placement_json()
        {
            return Err(corrupt(StoreRecordKindV1::Snapshot));
        }
    }
    Ok(())
}

fn load_attempts_v2(
    connection: &Connection,
    session_id: &SessionId,
    snapshot: &ProcedureSnapshotV2,
) -> Result<Vec<SessionAttemptV2>, StoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT attempt_id, snapshot_id, graph_node_id, node_definition_id, attempt_number, \
             trace_sequence, lifecycle, validity, goal_revision FROM v2_attempts \
             WHERE session_id = ?1 ORDER BY trace_sequence",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let mut attempts = Vec::new();
    for row in rows {
        let row = row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        let graph_node_id =
            GraphNodeId::new(row.2).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let node = snapshot
            .graph_node(&graph_node_id)
            .ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?;
        if row.1 != snapshot.snapshot_id().as_str() || row.3 != node.node_definition_id().as_str() {
            return Err(corrupt(StoreRecordKindV1::Attempt));
        }
        attempts.push(
            SessionAttemptV2::new(
                AttemptId::new(row.0).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
                graph_node_id,
                AttemptNumberV2::new(persisted_u64(row.4, StoreRecordKindV1::Attempt)?),
                TraceSequenceV2::new(persisted_u64(row.5, StoreRecordKindV1::Attempt)?),
                parse_attempt_lifecycle_v2(&row.6)?,
                parse_validity_v2(&row.7)?,
                row.8
                    .map(|value| {
                        persisted_u64(value, StoreRecordKindV1::Attempt)
                            .map(GoalRevisionNumberV2::new)
                    })
                    .transpose()?,
            )
            .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
        );
    }
    Ok(attempts)
}

fn load_attempt_metadata_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Vec<AttemptMetadataV2>, StoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT attempt_id, started_at_ms, ended_at_ms, terminal_reason FROM v2_attempts \
             WHERE session_id = ?1 ORDER BY trace_sequence",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    rows.map(|row| {
        let row = row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        AttemptMetadataV2::new(
            AttemptId::new(row.0).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            persisted_time_v2(row.1, StoreRecordKindV1::Attempt)?,
            optional_persisted_time_v2(row.2, StoreRecordKindV1::Attempt)?,
            row.3,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    })
    .collect()
}

fn load_counters_v2(
    connection: &Connection,
    session_id: &SessionId,
    snapshot: &ProcedureSnapshotV2,
) -> Result<Vec<GraphNodeCounterV2>, StoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT c.snapshot_id, c.graph_node_id, c.attempt_count, c.rework_traversal_count \
             FROM v2_graph_node_counters c JOIN v2_graph_nodes n \
             ON n.snapshot_id = c.snapshot_id AND n.graph_node_id = c.graph_node_id \
             WHERE c.session_id = ?1 ORDER BY n.placement_index",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    rows.map(|row| {
        let row = row.map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
        if row.0 != snapshot.snapshot_id().as_str() {
            return Err(corrupt(StoreRecordKindV1::Session));
        }
        Ok(GraphNodeCounterV2::new(
            GraphNodeId::new(row.1).map_err(|_| corrupt(StoreRecordKindV1::Session))?,
            persisted_u64(row.2, StoreRecordKindV1::Session)?,
            persisted_u64(row.3, StoreRecordKindV1::Session)?,
        ))
    })
    .collect()
}

pub(crate) fn verify_v2_graph_state_connection_v2(
    connection: &Connection,
) -> Result<(), StoreErrorV1> {
    load_graph_session_connection_v2(connection).map(|_| ())
}

fn persisted_time_v2(value: i64, record: StoreRecordKindV1) -> Result<UnixMillis, StoreErrorV1> {
    persisted_u64(value, record).map(UnixMillis::new)
}

fn optional_persisted_time_v2(
    value: Option<i64>,
    record: StoreRecordKindV1,
) -> Result<Option<UnixMillis>, StoreErrorV1> {
    value
        .map(|value| persisted_time_v2(value, record))
        .transpose()
}

fn parse_session_lifecycle_v2(value: &str) -> Result<SessionLifecycle, StoreErrorV1> {
    match value {
        "running" => Ok(SessionLifecycle::Running),
        "completed" => Ok(SessionLifecycle::Completed),
        "cancelled" => Ok(SessionLifecycle::Cancelled),
        _ => Err(corrupt(StoreRecordKindV1::Session)),
    }
}

fn parse_attempt_lifecycle_v2(value: &str) -> Result<AttemptLifecycle, StoreErrorV1> {
    match value {
        "active" => Ok(AttemptLifecycle::Active),
        "completed" => Ok(AttemptLifecycle::Completed),
        "skipped" => Ok(AttemptLifecycle::Skipped),
        "abandoned" => Ok(AttemptLifecycle::Abandoned),
        _ => Err(corrupt(StoreRecordKindV1::Attempt)),
    }
}

fn parse_validity_v2(value: &str) -> Result<podway_core::AttemptValidityV2, StoreErrorV1> {
    match value {
        "valid" => Ok(podway_core::AttemptValidityV2::Valid),
        "stale" => Ok(podway_core::AttemptValidityV2::Stale),
        _ => Err(corrupt(StoreRecordKindV1::Attempt)),
    }
}

fn validate_successor_v2(
    previous: &GraphSessionStateV2,
    next: &GraphSessionStateV2,
    expected_workspace_revision: Revision,
    expected_session_revision: Revision,
) -> Result<(), StoreValueErrorV1> {
    if previous.trace().session_id() != next.trace().session_id()
        || previous.snapshot() != next.snapshot()
        || previous.task_title() != next.task_title()
        || previous.created_at() != next.created_at()
    {
        return Err(invalid("Procedure v2 immutable session identity changed"));
    }
    let required_workspace_revision = expected_workspace_revision
        .checked_next()
        .map_err(|_| invalid("Procedure v2 workspace revision overflowed"))?;
    let required_session_revision = expected_session_revision
        .checked_next()
        .map_err(|_| invalid("Procedure v2 session revision overflowed"))?;
    if next.workspace_revision() != required_workspace_revision
        || next.trace().revision() != required_session_revision
    {
        return Err(invalid(
            "Procedure v2 successor revisions must advance exactly once",
        ));
    }
    let manual_reactivation = previous.trace().lifecycle() == SessionLifecycle::Completed
        && next.trace().lifecycle() == SessionLifecycle::Running
        && next.trace().attempts().len() == previous.trace().attempts().len() + 1
        && next.workflow_memory().reworks().len() == previous.workflow_memory().reworks().len() + 1
        && next
            .workflow_memory()
            .reworks()
            .last()
            .is_some_and(|record| {
                record.kind() == podway_core::ReworkKindV2::Manual
                    && record.reactivated()
                    && next
                        .trace()
                        .attempts()
                        .last()
                        .is_some_and(|attempt| attempt.attempt_id() == record.target_attempt_id())
            });
    if (previous.trace().lifecycle() != SessionLifecycle::Running && !manual_reactivation)
        || next.trace().attempts().len() < previous.trace().attempts().len()
        || next.trace().attempts().len() > previous.trace().attempts().len() + 1
    {
        return Err(invalid("Procedure v2 trace successor shape is invalid"));
    }

    for ((old_attempt, old_metadata), (new_attempt, new_metadata)) in previous
        .trace()
        .attempts()
        .iter()
        .zip(previous.attempt_metadata())
        .zip(next.trace().attempts().iter().zip(next.attempt_metadata()))
    {
        if old_attempt.attempt_id() != new_attempt.attempt_id()
            || old_attempt.graph_node_id() != new_attempt.graph_node_id()
            || old_attempt.number() != new_attempt.number()
            || old_attempt.trace() != new_attempt.trace()
            || old_attempt.goal_revision() != new_attempt.goal_revision()
            || old_metadata.attempt_id() != new_metadata.attempt_id()
            || old_metadata.started_at() != new_metadata.started_at()
        {
            return Err(invalid("Procedure v2 trace history identity changed"));
        }
        if old_attempt.validity() == podway_core::AttemptValidityV2::Stale
            && new_attempt.validity() != podway_core::AttemptValidityV2::Stale
        {
            return Err(invalid("Procedure v2 stale validity cannot become valid"));
        }
        if old_attempt.lifecycle() != AttemptLifecycle::Active
            && (old_attempt.lifecycle() != new_attempt.lifecycle()
                || old_metadata.ended_at() != new_metadata.ended_at()
                || old_metadata.terminal_reason() != new_metadata.terminal_reason())
        {
            return Err(invalid(
                "Procedure v2 terminal attempt history is immutable",
            ));
        }
        if old_attempt.lifecycle() == AttemptLifecycle::Active
            && new_attempt.lifecycle() == AttemptLifecycle::Active
            && (old_attempt != new_attempt || old_metadata != new_metadata)
        {
            return Err(invalid(
                "Procedure v2 cursor-stable mutation changed the active attempt",
            ));
        }
    }
    let cursor_stable = previous.trace().attempts().len() == next.trace().attempts().len()
        && previous
            .trace()
            .active_attempt()
            .is_some_and(|old| next.trace().active_attempt() == Some(old));
    if cursor_stable
        && (next.trace().lifecycle() != SessionLifecycle::Running
            || previous.counters() != next.counters()
            || previous.completed_at() != next.completed_at()
            || previous.cancelled_at() != next.cancelled_at()
            || previous.cancel_reason() != next.cancel_reason())
    {
        return Err(invalid(
            "Procedure v2 cursor-stable mutation changed graph state",
        ));
    }
    if next.trace().attempts().len() == previous.trace().attempts().len() + 1 {
        validate_trace_invalidation_successor_v2(previous.trace(), next.trace())?;
    }
    let next_counters: BTreeMap<_, _> = next
        .counters()
        .iter()
        .map(|counter| (counter.graph_node_id(), counter))
        .collect();
    for old in previous.counters() {
        let new = next_counters
            .get(old.graph_node_id())
            .ok_or_else(|| invalid("Procedure v2 counters must be monotonic"))?;
        if new.attempt_count() < old.attempt_count()
            || new.rework_traversal_count() < old.rework_traversal_count()
        {
            return Err(invalid("Procedure v2 counters must be monotonic"));
        }
    }
    let rework_target = if next.workflow_memory().reworks().len()
        == previous.workflow_memory().reworks().len() + 1
    {
        Some(
            next.workflow_memory()
                .reworks()
                .last()
                .ok_or_else(|| invalid("Procedure v2 rework counter target is absent"))?
                .to_node(),
        )
    } else {
        None
    };
    for old in previous.counters() {
        let new = next_counters[old.graph_node_id()];
        let expected = if rework_target == Some(old.graph_node_id()) {
            old.rework_traversal_count()
                .checked_add(1)
                .ok_or_else(|| invalid("Procedure v2 rework counter overflowed"))?
        } else {
            old.rework_traversal_count()
        };
        if new.rework_traversal_count() != expected {
            return Err(invalid(
                "Procedure v2 rework counters do not match the traversal",
            ));
        }
    }
    Ok(())
}

fn validate_trace_invalidation_successor_v2(
    previous: &SessionTraceV2,
    next: &SessionTraceV2,
) -> Result<(), StoreValueErrorV1> {
    let fresh = next
        .attempts()
        .last()
        .ok_or_else(|| invalid("Procedure v2 fresh attempt is absent"))?;
    let prior_target = previous.attempts().iter().find(|attempt| {
        attempt.graph_node_id() == fresh.graph_node_id()
            && attempt.validity() == podway_core::AttemptValidityV2::Valid
    });
    for (old, new) in previous.attempts().iter().zip(next.attempts()) {
        let expected_validity = prior_target.map_or(old.validity(), |target| {
            if old.validity() == podway_core::AttemptValidityV2::Valid
                && old.trace() >= target.trace()
            {
                podway_core::AttemptValidityV2::Stale
            } else {
                old.validity()
            }
        });
        if new.validity() != expected_validity {
            return Err(invalid(
                "Procedure v2 successor does not apply conservative suffix invalidation",
            ));
        }
    }
    Ok(())
}

fn insert_snapshot_v2(
    transaction: &Transaction<'_>,
    snapshot: &ProcedureSnapshotV2,
) -> Result<(), StoreErrorV1> {
    transaction
        .execute(
            "INSERT INTO v2_procedure_snapshots (snapshot_id, schema_id, procedure_id, \
             procedure_version, name, purpose, digest, canonical_json, source_kind, source_label, \
             goal_tracking, created_at_ms) VALUES (?1, 'podway.procedure/v2', ?2, ?3, ?4, ?5, \
             ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                snapshot.snapshot_id().as_str(),
                snapshot.procedure_id(),
                snapshot.procedure_version(),
                snapshot.name(),
                snapshot.purpose(),
                snapshot.digest().as_str(),
                snapshot.canonical_json().as_str(),
                snapshot.source().kind().as_str(),
                snapshot.source().label(),
                i64::from(snapshot.goal_tracking()),
                sqlite_u64(
                    snapshot.created_at().get(),
                    "Procedure v2 snapshot timestamp"
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    for node in snapshot.graph_nodes() {
        transaction
            .execute(
                "INSERT INTO v2_graph_nodes (snapshot_id, graph_node_id, node_definition_id, \
                 placement_index, node_type, goal_assessment, canonical_placement_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    snapshot.snapshot_id().as_str(),
                    node.graph_node_id().as_str(),
                    node.node_definition_id().as_str(),
                    i64::from(node.placement_index()),
                    node.node_kind().as_str(),
                    i64::from(node.goal_assessment()),
                    node.canonical_placement_json(),
                ],
            )
            .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    }
    Ok(())
}

fn insert_session_row_v2(
    transaction: &Transaction<'_>,
    state: &GraphSessionStateV2,
) -> Result<(), StoreErrorV1> {
    let (active_node, active_attempt, active_trace) = active_cursor_values(state.trace())?;
    transaction
        .execute(
            "INSERT INTO v2_task_sessions (singleton, session_id, task_title, procedure_snapshot_id, \
             lifecycle, session_revision, latest_trace_sequence, active_graph_node_id, \
             active_attempt_id, active_trace_sequence, goal_tracking, current_goal_revision, \
             created_at_ms, completed_at_ms, cancelled_at_ms, cancel_reason) \
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?12, ?13, ?14)",
            params![
                state.trace().session_id().as_str(),
                state.task_title(),
                state.snapshot().snapshot_id().as_str(),
                session_lifecycle_text(state.trace().lifecycle()),
                sqlite_u64(state.trace().revision().get(), "Procedure v2 session revision")?,
                sqlite_u64(latest_trace(state.trace()).get(), "Procedure v2 trace sequence")?,
                active_node,
                active_attempt,
                active_trace,
                i64::from(state.snapshot().goal_tracking()),
                sqlite_u64(state.created_at().get(), "Procedure v2 session timestamp")?,
                optional_sqlite_time(state.completed_at(), "Procedure v2 completion timestamp")?,
                optional_sqlite_time(state.cancelled_at(), "Procedure v2 cancellation timestamp")?,
                state.cancel_reason(),
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    Ok(())
}

fn update_session_row_v2(
    transaction: &Transaction<'_>,
    previous: &GraphSessionStateV2,
    next: &GraphSessionStateV2,
) -> Result<(), StoreErrorV1> {
    let (active_node, active_attempt, active_trace) = active_cursor_values(next.trace())?;
    let changed = transaction
        .execute(
            "UPDATE v2_task_sessions SET lifecycle = ?1, session_revision = ?2, \
             latest_trace_sequence = ?3, active_graph_node_id = ?4, active_attempt_id = ?5, \
             active_trace_sequence = ?6, completed_at_ms = ?7, cancelled_at_ms = ?8, \
             cancel_reason = ?9 WHERE singleton = 1 AND session_id = ?10 AND session_revision = ?11",
            params![
                session_lifecycle_text(next.trace().lifecycle()),
                sqlite_u64(next.trace().revision().get(), "Procedure v2 session revision")?,
                sqlite_u64(latest_trace(next.trace()).get(), "Procedure v2 trace sequence")?,
                active_node,
                active_attempt,
                active_trace,
                optional_sqlite_time(next.completed_at(), "Procedure v2 completion timestamp")?,
                optional_sqlite_time(next.cancelled_at(), "Procedure v2 cancellation timestamp")?,
                next.cancel_reason(),
                next.trace().session_id().as_str(),
                sqlite_u64(
                    previous.trace().revision().get(),
                    "Procedure v2 session revision",
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    if changed != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }
    Ok(())
}

fn insert_counters_v2(
    transaction: &Transaction<'_>,
    state: &GraphSessionStateV2,
) -> Result<(), StoreErrorV1> {
    for counter in state.counters() {
        transaction
            .execute(
                "INSERT INTO v2_graph_node_counters (session_id, snapshot_id, graph_node_id, \
                 attempt_count, rework_traversal_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    state.trace().session_id().as_str(),
                    state.snapshot().snapshot_id().as_str(),
                    counter.graph_node_id().as_str(),
                    sqlite_u64(counter.attempt_count(), "Procedure v2 attempt count")?,
                    sqlite_u64(
                        counter.rework_traversal_count(),
                        "Procedure v2 rework traversal count",
                    )?,
                ],
            )
            .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    }
    Ok(())
}

fn insert_attempt_v2(
    transaction: &Transaction<'_>,
    state: &GraphSessionStateV2,
    attempt: &SessionAttemptV2,
    metadata: &AttemptMetadataV2,
) -> Result<(), StoreErrorV1> {
    let graph_node = state
        .snapshot()
        .graph_node(attempt.graph_node_id())
        .ok_or_else(|| invalid_store("Procedure v2 attempt graph node is absent"))?;
    transaction
        .execute(
            "INSERT INTO v2_attempts (attempt_id, session_id, snapshot_id, graph_node_id, \
             node_definition_id, attempt_number, trace_sequence, lifecycle, validity, \
             goal_revision, started_at_ms, ended_at_ms, terminal_reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, ?12)",
            params![
                attempt.attempt_id().as_str(),
                state.trace().session_id().as_str(),
                state.snapshot().snapshot_id().as_str(),
                attempt.graph_node_id().as_str(),
                graph_node.node_definition_id().as_str(),
                sqlite_u64(attempt.number().get(), "Procedure v2 attempt number")?,
                sqlite_u64(attempt.trace().get(), "Procedure v2 trace sequence")?,
                attempt_lifecycle_text(attempt.lifecycle()),
                validity_text(attempt.validity()),
                sqlite_u64(metadata.started_at().get(), "Procedure v2 attempt start")?,
                optional_sqlite_time(metadata.ended_at(), "Procedure v2 attempt end")?,
                metadata.terminal_reason(),
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    Ok(())
}

fn active_cursor_values(trace: &SessionTraceV2) -> Result<ActiveCursorColumnsV2<'_>, StoreErrorV1> {
    trace
        .active_attempt()
        .map_or(Ok((None, None, None)), |attempt| {
            Ok((
                Some(attempt.graph_node_id().as_str()),
                Some(attempt.attempt_id().as_str()),
                Some(sqlite_u64(
                    attempt.trace().get(),
                    "Procedure v2 active trace sequence",
                )?),
            ))
        })
}

fn latest_trace(trace: &SessionTraceV2) -> TraceSequenceV2 {
    trace
        .attempts()
        .last()
        .map_or(TraceSequenceV2::ZERO, SessionAttemptV2::trace)
}

fn optional_sqlite_time(
    value: Option<UnixMillis>,
    field: &'static str,
) -> Result<Option<i64>, StoreErrorV1> {
    value
        .map(|value| sqlite_u64(value.get(), field))
        .transpose()
}

fn session_lifecycle_text(lifecycle: SessionLifecycle) -> &'static str {
    match lifecycle {
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}

fn attempt_lifecycle_text(lifecycle: AttemptLifecycle) -> &'static str {
    match lifecycle {
        AttemptLifecycle::Active => "active",
        AttemptLifecycle::Completed => "completed",
        AttemptLifecycle::Skipped => "skipped",
        AttemptLifecycle::Abandoned => "abandoned",
    }
}

fn validity_text(validity: podway_core::AttemptValidityV2) -> &'static str {
    match validity {
        podway_core::AttemptValidityV2::Valid => "valid",
        podway_core::AttemptValidityV2::Stale => "stale",
    }
}
