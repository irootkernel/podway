//! Procedure v2 graph, cursor, counter, and attempt persistence.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use podway_core::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptNumberV2, CanonicalProcedureJsonV1,
    GoalRevisionNumberV2, GraphNodeId, ProcedureSnapshotId, ProcedureSourceKindV1,
    ProcedureSourceLabelV1, Revision, SessionAttemptV2, SessionId, SessionLifecycle,
    SessionTraceV2, Sha256Digest, TerminalDispositionKindV2, TerminalDispositionV2,
    TraceSequenceV2, UnixMillis, WorkspaceId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::query_accounting::{
    accounted_prepare, accounted_query_map_prepared, accounted_query_row,
};
use crate::v2_goal::{insert_goal_state_v2, load_goal_state_v2, replace_goal_state_v2};
use crate::v2_memory::{
    insert_workflow_memory_v2, load_workflow_memory_v2, replace_workflow_memory_v2,
};
use podway_core::goal_state_v2::validate_goal_state_successor_v2;
pub use podway_core::graph_v2::*;
use podway_core::workflow_v2::{WorkflowGoalTransitionV2, validate_workflow_memory_successor_v2};

use crate::{
    DurableWorktreeIdentityV1, EpochMillisV1, RusqliteErrorContextV1, StoreErrorV1,
    StoreInvariantV1, StoreRecordKindV1, StoreValueErrorV1, map_rusqlite_error_v1,
};

/// One loader admission: the admitted graph plus the terminal dispositions
/// its loader stage already validated. Store-internal; reuse is valid only
/// inside the snapshot that admitted it.
pub(crate) struct AdmittedGraphStateV2 {
    pub(crate) graph: GraphSessionStateV2,
    pub(crate) dispositions: Vec<TerminalDispositionV2>,
}

const PROCEDURE_SCHEMA_V2: &str = "podway.procedure/v2";
const PROCEDURE_SCHEMA_DOCUMENT_V2: &str =
    include_str!("../../../assets/schemas/procedure-v2.schema.json");
const DELEGATION_COMPONENTS_SCHEMA_V1: &str =
    include_str!("../../../assets/schemas/delegation-components-v1.schema.json");
/// Newest-first archive list window. Matches ADR-0034 / protocol `SESSION_ARCHIVE_LIST_WINDOW_V2`.
pub const MAX_ARCHIVED_SESSION_LIST_WINDOW_V2: usize = 256;
const MAX_ARCHIVE_SLOT_V2: i64 = u32::MAX as i64;
const ARCHIVE_SUMMARY_COLUMNS_V2: &str = "session_id, task_title, lifecycle, session_revision, \
     archive_slot, archived_at_ms, archived_workspace_revision";
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

fn schema_matches_v2(root: &Value, schema: &Value, instance: &Value) -> bool {
    if let Some(boolean) = schema.as_bool() {
        return boolean;
    }
    let Some(schema) = schema.as_object() else {
        return false;
    };
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let external = reference.strip_prefix("urn:podway:schema:delegation-components:v1#");
        static DELEGATION_SCHEMA: OnceLock<Option<Value>> = OnceLock::new();
        let reference_root = if external.is_some() {
            let Some(schema) = DELEGATION_SCHEMA
                .get_or_init(|| serde_json::from_str(DELEGATION_COMPONENTS_SCHEMA_V1).ok())
                .as_ref()
            else {
                return false;
            };
            schema
        } else {
            root
        };
        let Some(target) = external
            .or_else(|| reference.strip_prefix('#'))
            .and_then(|pointer| reference_root.pointer(pointer))
        else {
            return false;
        };
        if !schema_matches_v2(reference_root, target, instance) {
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
        || schema
            .get("x-podway-max-utf8-bytes")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| text.len() > maximum as usize)
    {
        return false;
    }
    match schema.get("pattern").and_then(Value::as_str) {
        None => true,
        Some("^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$") => identifier_pattern_matches_v2(text),
        Some("^[^\\x00-\\x1f\\x7f-\\x9f]+$") => {
            !text.chars().any(|character| character.is_control())
        }
        Some("^[\\x21-\\x7e]{1,128}$") => {
            !text.is_empty()
                && text.len() <= 128
                && text.bytes().all(|byte| (33..=126).contains(&byte))
        }
        Some("^[a-z0-9][a-z0-9!#$&^_.+-]*/[a-z0-9][a-z0-9!#$&^_.+-]*$") => {
            media_type_pattern_matches_v2(text)
        }
        Some("^sha256:[0-9a-f]{64}$") => sha256_digest_pattern_matches_v2(text),
        Some(_) => false,
    }
}

fn sha256_digest_pattern_matches_v2(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
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

/// Independently admits canonical persisted shape before constructing pure state.
pub fn admit_procedure_snapshot_v2(
    snapshot_id: ProcedureSnapshotId,
    canonical_json: CanonicalProcedureJsonV1,
    digest: Sha256Digest,
    source: ProcedureSourceLabelV1,
    created_at: UnixMillis,
) -> Result<ProcedureSnapshotV2, StoreValueErrorV1> {
    let document = podway_core::CanonicalProcedureDocumentV2::new(canonical_json, digest)
        .map_err(StoreValueErrorV1::from)?;
    validate_snapshot_shape_v2(document.as_value())?;
    ProcedureSnapshotV2::from_admitted_document(snapshot_id, document, source, created_at)
        .map_err(StoreValueErrorV1::from)
}

fn validate_snapshot_shape_v2(document: &Value) -> Result<(), StoreValueErrorV1> {
    let schema: Value = serde_json::from_str(PROCEDURE_SCHEMA_DOCUMENT_V2)
        .map_err(|_| invalid("canonical Procedure v2 schema is unavailable"))?;
    if !schema_matches_v2(&schema, &schema, document) {
        return Err(invalid(
            "Procedure v2 snapshot does not satisfy the canonical schema",
        ));
    }
    Ok(())
}

/// One coherent committed observation of Procedure v2 state and its workspace queue.
///
/// The Store constructs this view from one SQLite read transaction so a daemon read never pairs
/// a graph cursor from one committed point with queue or sequence facts from another.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphWorkspaceViewV2 {
    identity: DurableWorktreeIdentityV1,
    graph_state: Option<GraphSessionStateV2>,
    delegation_receipts: crate::v2_delegation::DelegationReceiptSnapshotV1,
    current_terminal_disposition: bool,
    latest_workspace_sequence: u64,
    observed_at: EpochMillisV1,
}

impl GraphWorkspaceViewV2 {
    pub fn new(
        identity: DurableWorktreeIdentityV1,
        graph_state: Option<GraphSessionStateV2>,
        latest_workspace_sequence: u64,
        observed_at: EpochMillisV1,
    ) -> Self {
        Self {
            identity,
            graph_state,
            delegation_receipts: crate::v2_delegation::DelegationReceiptSnapshotV1::empty(),
            current_terminal_disposition: false,
            latest_workspace_sequence,
            observed_at,
        }
    }

    pub fn with_current_terminal_disposition(mut self, current_terminal_disposition: bool) -> Self {
        self.current_terminal_disposition = current_terminal_disposition;
        self
    }

    pub fn with_delegation_receipts(
        mut self,
        receipts: crate::v2_delegation::DelegationReceiptSnapshotV1,
    ) -> Self {
        self.delegation_receipts = receipts;
        self
    }

    pub fn delegation_receipts(&self) -> &crate::v2_delegation::DelegationReceiptSnapshotV1 {
        &self.delegation_receipts
    }

    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    pub fn graph_state(&self) -> Option<&GraphSessionStateV2> {
        self.graph_state.as_ref()
    }

    pub const fn current_terminal_disposition(&self) -> bool {
        self.current_terminal_disposition
    }

    pub const fn latest_workspace_sequence(&self) -> u64 {
        self.latest_workspace_sequence
    }

    pub const fn observed_at(&self) -> EpochMillisV1 {
        self.observed_at
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionArchiveSummaryV2 {
    session_id: SessionId,
    task_title: String,
    lifecycle: SessionLifecycle,
    session_revision: Revision,
    archive_slot: u32,
    archived_at: UnixMillis,
    archived_workspace_revision: Revision,
}

impl SessionArchiveSummaryV2 {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn task_title(&self) -> &str {
        &self.task_title
    }

    pub fn lifecycle(&self) -> SessionLifecycle {
        self.lifecycle
    }

    pub fn session_revision(&self) -> Revision {
        self.session_revision
    }

    pub fn archive_slot(&self) -> u32 {
        self.archive_slot
    }

    pub fn archived_at(&self) -> UnixMillis {
        self.archived_at
    }

    pub fn archived_workspace_revision(&self) -> Revision {
        self.archived_workspace_revision
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionArchiveListPageV2 {
    count: u64,
    sessions: Vec<SessionArchiveSummaryV2>,
    oldest: Option<SessionArchiveSummaryV2>,
}

impl SessionArchiveListPageV2 {
    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn sessions(&self) -> &[SessionArchiveSummaryV2] {
        &self.sessions
    }

    pub fn oldest(&self) -> Option<&SessionArchiveSummaryV2> {
        self.oldest.as_ref()
    }
}

pub(crate) fn create_graph_session_transaction_v2(
    transaction: &Transaction<'_>,
    state: &GraphSessionStateV2,
) -> Result<(), StoreErrorV1> {
    let current_v2: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM v2_task_sessions WHERE activity = 'current'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let v2_workspace: i64 = transaction
        .query_row("SELECT COUNT(*) FROM v2_workspace_state", [], |row| {
            row.get(0)
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    if current_v2 != 0 || v2_workspace != 0 {
        return Err(invalid_store(
            "a workspace may contain only one current task",
        ));
    }

    // Core construction proves domain consistency, not persisted schema shape.
    // Independently admit newly supplied bytes before writing any graph rows.
    let document = serde_json::from_str(state.snapshot().canonical_json().as_str())
        .map_err(|_| invalid_store("Procedure v2 snapshot JSON is not a document"))?;
    validate_snapshot_shape_v2(&document).map_err(StoreErrorV1::InvalidStateV1)?;
    insert_snapshot_v2(transaction, state.snapshot())?;
    insert_session_row_v2(transaction, state)?;
    transaction
        .execute(
            "INSERT INTO v2_workspace_state (singleton, current_session_id, workspace_revision) \
             VALUES (1, ?1, ?2)",
            params![
                state.trace().session_id().as_str(),
                sqlite_u64(
                    state.workspace_revision().get(),
                    "Procedure v2 workspace revision",
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
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
    insert_goal_state_v2(
        transaction,
        state.trace().session_id(),
        state.workflow_memory(),
        state.goal_state(),
    )?;
    Ok(())
}

/// Persistence reuses the state loaded by its private transaction and writes only changed attempts.
pub(crate) fn replace_direct_graph_session_transaction_v2(
    transaction: &Transaction<'_>,
    previous: &GraphSessionStateV2,
    expected_workspace_revision: Revision,
    expected_session_revision: Revision,
    next: &GraphSessionStateV2,
    delegation_accept: bool,
) -> Result<(), StoreErrorV1> {
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
    let goal_transition = validate_goal_state_successor_v2(
        previous.trace(),
        previous.workflow_memory(),
        previous.goal_state(),
        next.trace(),
        next.workflow_memory(),
        next.goal_state(),
    )
    .map_err(|error| StoreErrorV1::InvalidStateV1(error.into()))?;
    validate_successor_v2(
        previous,
        next,
        expected_workspace_revision,
        expected_session_revision,
        &goal_transition,
    )
    .map_err(StoreErrorV1::InvalidStateV1)?;
    if !delegation_accept
        && previous.trace().active_attempt().is_some_and(|active| {
            next.trace().attempts().iter().any(|attempt| {
                attempt.attempt_id() == active.attempt_id()
                    && attempt.lifecycle() == AttemptLifecycle::Completed
            })
        })
    {
        let workspace_uuid: String = transaction
            .query_row(
                "SELECT workspace_uuid FROM workspace_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| corrupt(StoreRecordKindV1::Workspace))?;
        let workspace_uuid =
            WorkspaceId::new(workspace_uuid).map_err(|_| corrupt(StoreRecordKindV1::Workspace))?;
        if !matches!(
            crate::v2_delegation::completion_readiness_connection_v1(
                transaction,
                previous,
                &workspace_uuid,
            )?,
            crate::v2_delegation::DelegationReadinessV1::Direct
                | crate::v2_delegation::DelegationReadinessV1::ReceiptCurrent
        ) {
            return Err(invalid_store("delegation receipt is missing or stale"));
        }
    }
    if delegation_accept {
        let active = previous
            .trace()
            .active_attempt()
            .ok_or_else(|| invalid_store("delegation acceptance has no active attempt"))?;
        if previous
            .accept_delegation_successor_v1(active.attempt_id())
            .ok()
            .as_ref()
            != Some(next)
        {
            return Err(invalid_store("delegation acceptance successor is invalid"));
        }
    }
    if !delegation_accept
        && (previous.workflow_memory() != next.workflow_memory()
            || previous.goal_state() == next.goal_state()
            || !matches!(&goal_transition, WorkflowGoalTransitionV2::None))
    {
        validate_workflow_memory_successor_v2(
            previous.snapshot(),
            previous.trace(),
            previous.workflow_memory(),
            next.trace(),
            next.workflow_memory(),
            goal_transition,
        )
        .map_err(|error| StoreErrorV1::InvalidStateV1(error.into()))?;
    }

    for ((old_attempt, old_metadata), (new_attempt, new_metadata)) in previous
        .trace()
        .attempts()
        .iter()
        .zip(previous.attempt_metadata())
        .zip(next.trace().attempts().iter().zip(next.attempt_metadata()))
    {
        if old_attempt == new_attempt && old_metadata == new_metadata {
            continue;
        }
        let changed = transaction
            .execute(
                "UPDATE v2_attempts SET lifecycle = ?1, validity = ?2, ended_at_ms = ?3, \
                 terminal_reason = ?4, goal_revision = ?5 WHERE attempt_id = ?6 AND lifecycle = ?7 \
                 AND validity = ?8 AND ended_at_ms IS ?9 AND terminal_reason IS ?10 \
                 AND goal_revision IS ?11",
                params![
                    attempt_lifecycle_text(new_attempt.lifecycle()),
                    validity_text(new_attempt.validity()),
                    optional_sqlite_time(new_metadata.ended_at(), "Procedure v2 attempt end")?,
                    new_metadata.terminal_reason(),
                    new_attempt
                        .goal_revision()
                        .map(|value| sqlite_u64(value.get(), "Procedure v2 attempt goal revision"))
                        .transpose()?,
                    old_attempt.attempt_id().as_str(),
                    attempt_lifecycle_text(old_attempt.lifecycle()),
                    validity_text(old_attempt.validity()),
                    optional_sqlite_time(old_metadata.ended_at(), "Procedure v2 attempt end")?,
                    old_metadata.terminal_reason(),
                    old_attempt
                        .goal_revision()
                        .map(|value| sqlite_u64(value.get(), "Procedure v2 attempt goal revision"))
                        .transpose()?,
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
    for (previous_counter, counter) in previous.counters().iter().zip(next.counters()) {
        if previous_counter == counter {
            continue;
        }
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
    replace_goal_state_v2(
        transaction,
        next.trace().session_id(),
        previous.goal_state(),
        next.workflow_memory(),
        next.goal_state(),
    )?;
    update_session_row_v2(transaction, previous, next)?;
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

pub(crate) fn clear_graph_session_transaction_v2(
    transaction: &Transaction<'_>,
    previous: Option<&GraphSessionStateV2>,
    expected_workspace_revision: Revision,
    expected_session_revision: Revision,
) -> Result<(), StoreErrorV1> {
    let previous =
        previous.ok_or_else(|| invalid_store("no current Procedure v2 graph session exists"))?;
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

    let session_id = previous.trace().session_id().as_str();
    let snapshot_id = previous.snapshot().snapshot_id().as_str();

    // These cross-attempt and cross-goal references use RESTRICT so that ordinary state changes
    // cannot erase their sources. Reset removes both sides, but must remove the referencing rows
    // before deleting the session and letting its remaining session-scoped rows cascade.
    transaction
        .execute(
            "DELETE FROM v2_resolved_evidence_references WHERE attempt_id IN \
             (SELECT attempt_id FROM v2_attempts WHERE session_id = ?1)",
            [session_id],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    transaction
        .execute(
            "DELETE FROM v2_goal_assessments WHERE session_id = ?1",
            [session_id],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    transaction
        .execute(
            "DELETE FROM v2_criterion_assessment_results WHERE session_id = ?1",
            [session_id],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;

    let deleted_workspace = transaction
        .execute(
            "DELETE FROM v2_workspace_state WHERE singleton = 1 AND workspace_revision = ?1",
            [sqlite_u64(
                expected_workspace_revision.get(),
                "Procedure v2 workspace revision",
            )?],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    if deleted_workspace != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }

    let deleted_session = transaction
        .execute(
            "DELETE FROM v2_task_sessions WHERE activity = 'current' AND session_id = ?1 \
             AND session_revision = ?2",
            params![
                session_id,
                sqlite_u64(
                    expected_session_revision.get(),
                    "Procedure v2 session revision"
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    if deleted_session != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }

    let deleted_snapshot = transaction
        .execute(
            "DELETE FROM v2_procedure_snapshots WHERE snapshot_id = ?1",
            [snapshot_id],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    if deleted_snapshot != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }
    if load_graph_session_connection_v2(transaction)?.is_some() {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }
    Ok(())
}

pub(crate) fn archive_current_session_transaction_v2(
    transaction: &Transaction<'_>,
    current: Option<&GraphSessionStateV2>,
    expected_workspace_revision: Revision,
    expected_session_revision: Revision,
    now: EpochMillisV1,
) -> Result<SessionArchiveSummaryV2, StoreErrorV1> {
    let current =
        current.ok_or_else(|| invalid_store("no current Procedure v2 graph session exists"))?;
    if current.workspace_revision() != expected_workspace_revision {
        return Err(StoreErrorV1::PreconditionConflictV1 {
            expected: Some(expected_workspace_revision),
            actual: Some(current.workspace_revision()),
        });
    }
    if current.trace().revision() != expected_session_revision {
        return Err(StoreErrorV1::PreconditionConflictV1 {
            expected: Some(expected_session_revision),
            actual: Some(current.trace().revision()),
        });
    }
    let dispositions = load_terminal_dispositions_connection_v2(transaction, current)?;
    let current_disposition =
        terminal_disposition_is_current_v2(&dispositions, current.trace().revision());
    if !matches!(
        current.trace().lifecycle(),
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    ) || !current_disposition
    {
        return Err(StoreErrorV1::SessionArchiveNotEligibleV1 {
            lifecycle: current.trace().lifecycle(),
            current_terminal_disposition: current_disposition,
        });
    }

    let archive_slot = next_unused_archive_slot_v2(transaction)?;

    let changed = transaction
        .execute(
            "UPDATE v2_task_sessions SET activity = 'inactive', archive_slot = ?1, \
             archived_at_ms = ?2, archived_workspace_revision = ?3 \
             WHERE activity = 'current' AND session_id = ?4 AND session_revision = ?5",
            params![
                i64::from(archive_slot),
                sqlite_u64(now.get(), "Procedure v2 archive timestamp")?,
                sqlite_u64(
                    current.workspace_revision().get(),
                    "Procedure v2 archived workspace revision",
                )?,
                current.trace().session_id().as_str(),
                sqlite_u64(
                    current.trace().revision().get(),
                    "Procedure v2 session revision",
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let deleted_workspace = transaction
        .execute(
            "DELETE FROM v2_workspace_state WHERE singleton = 1 \
             AND current_session_id = ?1 AND workspace_revision = ?2",
            params![
                current.trace().session_id().as_str(),
                sqlite_u64(
                    current.workspace_revision().get(),
                    "Procedure v2 workspace revision",
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    if changed != 1 || deleted_workspace != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }
    Ok(SessionArchiveSummaryV2 {
        session_id: current.trace().session_id().clone(),
        task_title: current.task_title().to_owned(),
        lifecycle: current.trace().lifecycle(),
        session_revision: current.trace().revision(),
        archive_slot,
        archived_at: UnixMillis::new(now.get()),
        archived_workspace_revision: current.workspace_revision(),
    })
}

fn next_unused_archive_slot_v2(transaction: &Transaction<'_>) -> Result<u32, StoreErrorV1> {
    let slot: Option<i64> = transaction
        .query_row(
            "SELECT MIN(candidate) FROM ( \
             SELECT 1 AS candidate \
             UNION ALL \
             SELECT archive_slot + 1 FROM v2_task_sessions \
             WHERE activity = 'inactive' AND archive_slot < ?1 \
             ) WHERE candidate NOT IN ( \
             SELECT archive_slot FROM v2_task_sessions WHERE activity = 'inactive' \
             )",
            params![MAX_ARCHIVE_SLOT_V2],
            |row| row.get(0),
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let slot = slot.ok_or(StoreErrorV1::InternalInvariantViolationV1 {
        invariant: StoreInvariantV1::ProcedureV2GraphState,
    })?;
    u32::try_from(slot).map_err(|_| StoreErrorV1::InternalInvariantViolationV1 {
        invariant: StoreInvariantV1::ProcedureV2GraphState,
    })
}

fn session_archive_summary_from_parts_v2(
    session_id: String,
    task_title: String,
    lifecycle: String,
    revision: i64,
    slot: i64,
    archived_at: i64,
    workspace_revision: i64,
) -> Result<SessionArchiveSummaryV2, StoreErrorV1> {
    Ok(SessionArchiveSummaryV2 {
        session_id: SessionId::new(session_id).map_err(|_| corrupt(StoreRecordKindV1::Session))?,
        task_title,
        lifecycle: parse_session_lifecycle_v2(&lifecycle)?,
        session_revision: Revision::new(persisted_u64(revision, StoreRecordKindV1::Session)?),
        archive_slot: u32::try_from(slot).map_err(|_| corrupt(StoreRecordKindV1::Session))?,
        archived_at: persisted_time_v2(archived_at, StoreRecordKindV1::Session)?,
        archived_workspace_revision: Revision::new(persisted_u64(
            workspace_revision,
            StoreRecordKindV1::Workspace,
        )?),
    })
}

fn query_archive_summary_row_v2(
    connection: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Option<SessionArchiveSummaryV2>, StoreErrorV1> {
    let row = accounted_query_row(connection, sql, params, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })
    .optional()
    .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    row.map(
        |(session_id, task_title, lifecycle, revision, slot, archived_at, workspace_revision)| {
            session_archive_summary_from_parts_v2(
                session_id,
                task_title,
                lifecycle,
                revision,
                slot,
                archived_at,
                workspace_revision,
            )
        },
    )
    .transpose()
}

pub(crate) fn list_archived_sessions_connection_v2(
    connection: &Connection,
    window: usize,
) -> Result<SessionArchiveListPageV2, StoreErrorV1> {
    if window == 0 || window > MAX_ARCHIVED_SESSION_LIST_WINDOW_V2 {
        return Err(StoreErrorV1::InvalidStateV1(
            StoreValueErrorV1::IntegerOutOfRange {
                field: "archive_list_window",
            },
        ));
    }
    let count_i64: i64 = accounted_query_row(
        connection,
        "SELECT COUNT(*) FROM v2_task_sessions WHERE activity = 'inactive'",
        [],
        |row| row.get(0),
    )
    .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let count = persisted_u64(count_i64, StoreRecordKindV1::Session)?;
    let limit = i64::try_from(window).map_err(|_| {
        StoreErrorV1::InvalidStateV1(StoreValueErrorV1::IntegerOutOfRange {
            field: "archive_list_window",
        })
    })?;
    let page_sql = format!(
        "SELECT {ARCHIVE_SUMMARY_COLUMNS_V2} FROM v2_task_sessions \
         WHERE activity = 'inactive' \
         ORDER BY archived_at_ms DESC, session_id DESC LIMIT ?1"
    );
    let mut statement = accounted_prepare(connection, &page_sql)
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let page_parameters = params![limit];
    let rows = accounted_query_map_prepared(&mut statement, &page_sql, page_parameters, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })
    .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let sessions = rows
        .map(|row| {
            let (
                session_id,
                task_title,
                lifecycle,
                revision,
                slot,
                archived_at,
                workspace_revision,
            ) = row.map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
            session_archive_summary_from_parts_v2(
                session_id,
                task_title,
                lifecycle,
                revision,
                slot,
                archived_at,
                workspace_revision,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let oldest = if count == 0 {
        None
    } else {
        query_archive_summary_row_v2(
            connection,
            &format!(
                "SELECT {ARCHIVE_SUMMARY_COLUMNS_V2} FROM v2_task_sessions \
                 WHERE activity = 'inactive' \
                 ORDER BY archived_at_ms ASC, session_id ASC LIMIT 1"
            ),
            [],
        )?
    };
    Ok(SessionArchiveListPageV2 {
        count,
        sessions,
        oldest,
    })
}

pub(crate) fn archived_session_summary_connection_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Option<SessionArchiveSummaryV2>, StoreErrorV1> {
    query_archive_summary_row_v2(
        connection,
        &format!(
            "SELECT {ARCHIVE_SUMMARY_COLUMNS_V2} FROM v2_task_sessions \
             WHERE activity = 'inactive' AND session_id = ?1"
        ),
        [session_id.as_str()],
    )
}

pub(crate) fn load_archived_session_connection_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Option<GraphSessionStateV2>, StoreErrorV1> {
    load_archived_session_admission_connection_v2(connection, session_id)
        .map(|admission| admission.map(|admitted| admitted.graph))
}

pub(crate) fn load_archived_session_admission_connection_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Option<AdmittedGraphStateV2>, StoreErrorV1> {
    let row = accounted_query_row(
        connection,
        "SELECT archived_workspace_revision, session_id, task_title, procedure_snapshot_id, \
         lifecycle, session_revision, latest_trace_sequence, active_graph_node_id, \
         active_attempt_id, active_trace_sequence, goal_tracking, current_goal_revision, \
         created_at_ms, completed_at_ms, cancelled_at_ms, cancel_reason \
         FROM v2_task_sessions WHERE activity = 'inactive' AND session_id = ?1",
        [session_id.as_str()],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                PersistedSessionV2 {
                    session_id: row.get(1)?,
                    task_title: row.get(2)?,
                    snapshot_id: row.get(3)?,
                    lifecycle: row.get(4)?,
                    session_revision: row.get(5)?,
                    latest_trace: row.get(6)?,
                    active_node: row.get(7)?,
                    active_attempt: row.get(8)?,
                    active_trace: row.get(9)?,
                    goal_tracking: row.get(10)?,
                    current_goal_revision: row.get(11)?,
                    created_at: row.get(12)?,
                    completed_at: row.get(13)?,
                    cancelled_at: row.get(14)?,
                    cancel_reason: row.get(15)?,
                },
            ))
        },
    );
    match row {
        Ok((workspace_revision, persisted)) => load_present_graph_session_with_cache_v2(
            connection,
            workspace_revision,
            persisted,
            None,
        )
        .map(Some),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(record_error(error, StoreRecordKindV1::Session)),
    }
}

pub(crate) fn purge_archived_session_transaction_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    expected_session_revision: Revision,
) -> Result<SessionArchiveSummaryV2, StoreErrorV1> {
    let summary =
        archived_session_summary_connection_v2(transaction, session_id)?.ok_or_else(|| {
            StoreErrorV1::SessionArchiveNotFoundV1 {
                session_id: session_id.clone(),
            }
        })?;
    if summary.session_revision() != expected_session_revision {
        return Err(StoreErrorV1::PreconditionConflictV1 {
            expected: Some(expected_session_revision),
            actual: Some(summary.session_revision()),
        });
    }
    let snapshot_id: String = transaction
        .query_row(
            "SELECT procedure_snapshot_id FROM v2_task_sessions \
             WHERE activity = 'inactive' AND session_id = ?1",
            [session_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    transaction
        .execute(
            "DELETE FROM v2_resolved_evidence_references WHERE attempt_id IN \
             (SELECT attempt_id FROM v2_attempts WHERE session_id = ?1)",
            [session_id.as_str()],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    transaction
        .execute(
            "DELETE FROM v2_goal_assessments WHERE session_id = ?1",
            [session_id.as_str()],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    transaction
        .execute(
            "DELETE FROM v2_criterion_assessment_results WHERE session_id = ?1",
            [session_id.as_str()],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let deleted = transaction
        .execute(
            "DELETE FROM v2_task_sessions WHERE activity = 'inactive' \
             AND session_id = ?1 AND session_revision = ?2",
            params![
                session_id.as_str(),
                sqlite_u64(
                    expected_session_revision.get(),
                    "Procedure v2 session revision",
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let deleted_snapshot = transaction
        .execute(
            "DELETE FROM v2_procedure_snapshots WHERE snapshot_id = ?1",
            [snapshot_id],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    if deleted != 1 || deleted_snapshot != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }
    Ok(summary)
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
    load_graph_session_admission_connection_v2(connection)
        .map(|admission| admission.map(|admitted| admitted.graph))
}

pub(crate) fn load_graph_session_admission_connection_v2(
    connection: &Connection,
) -> Result<Option<AdmittedGraphStateV2>, StoreErrorV1> {
    load_graph_session_admission_with_cache_v2(connection, None)
}

pub(crate) fn load_graph_session_with_cache_v2(
    connection: &Connection,
    cached: Option<&GraphSessionStateV2>,
) -> Result<Option<GraphSessionStateV2>, StoreErrorV1> {
    load_graph_session_admission_with_cache_v2(connection, cached)
        .map(|admission| admission.map(|admitted| admitted.graph))
}

fn load_graph_session_admission_with_cache_v2(
    connection: &Connection,
    cached: Option<&GraphSessionStateV2>,
) -> Result<Option<AdmittedGraphStateV2>, StoreErrorV1> {
    let current_count: i64 = accounted_query_row(
        connection,
        "SELECT COUNT(*) FROM v2_task_sessions WHERE activity = 'current'",
        [],
        |row| row.get(0),
    )
    .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let workspace_count: i64 = accounted_query_row(
        connection,
        "SELECT COUNT(*) FROM v2_workspace_state",
        [],
        |row| row.get(0),
    )
    .map_err(|error| record_error(error, StoreRecordKindV1::Workspace))?;
    if current_count == 0 && workspace_count == 0 {
        return Ok(None);
    }
    if current_count != 1 || workspace_count != 1 {
        return Err(corrupt(StoreRecordKindV1::Session));
    }
    let (workspace_revision, session) = accounted_query_row(
        connection,
        "SELECT w.workspace_revision, s.session_id, s.task_title, s.procedure_snapshot_id, \
             s.lifecycle, s.session_revision, \
             latest_trace_sequence, active_graph_node_id, active_attempt_id, active_trace_sequence, \
             goal_tracking, current_goal_revision, created_at_ms, completed_at_ms, cancelled_at_ms, \
             cancel_reason FROM v2_workspace_state w JOIN v2_task_sessions s \
             ON s.session_id = w.current_session_id \
             WHERE w.singleton = 1 AND s.activity = 'current'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    PersistedSessionV2 {
                        session_id: row.get(1)?,
                        task_title: row.get(2)?,
                        snapshot_id: row.get(3)?,
                        lifecycle: row.get(4)?,
                        session_revision: row.get(5)?,
                        latest_trace: row.get(6)?,
                        active_node: row.get(7)?,
                        active_attempt: row.get(8)?,
                        active_trace: row.get(9)?,
                        goal_tracking: row.get(10)?,
                        current_goal_revision: row.get(11)?,
                        created_at: row.get(12)?,
                        completed_at: row.get(13)?,
                        cancelled_at: row.get(14)?,
                        cancel_reason: row.get(15)?,
                    },
                ))
            },
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    load_present_graph_session_with_cache_v2(connection, workspace_revision, session, cached)
        .map(Some)
}

fn load_present_graph_session_with_cache_v2(
    connection: &Connection,
    workspace_revision: i64,
    persisted: PersistedSessionV2,
    cached: Option<&GraphSessionStateV2>,
) -> Result<AdmittedGraphStateV2, StoreErrorV1> {
    // One guard per whole reconstruction: committed slot payloads stay
    // counted as live only when this function actually returns a graph.
    #[cfg(any(test, feature = "test-support"))]
    let _reads001_graph_scope = crate::query_accounting::GraphLoadScope::enter();
    let cached = cached.filter(|state| {
        state.trace().session_id().as_str() == persisted.session_id
            && state.snapshot().snapshot_id().as_str() == persisted.snapshot_id
    });
    let snapshot = if let Some(cached) = cached {
        let digest: String = accounted_query_row(
            connection,
            "SELECT digest FROM v2_procedure_snapshots WHERE snapshot_id=?1",
            [&persisted.snapshot_id],
            |row| row.get(0),
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
        if digest != cached.snapshot().digest().as_str() {
            return Err(corrupt(StoreRecordKindV1::Snapshot));
        }
        cached.snapshot().clone()
    } else {
        load_snapshot_v2(connection, &persisted.snapshot_id)?
    };
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
    let workflow_memory = load_workflow_memory_v2(
        connection,
        &snapshot,
        &trace,
        &metadata,
        cached.map(GraphSessionStateV2::workflow_memory),
    )?;
    let current_goal_revision = persisted
        .current_goal_revision
        .map(|value| {
            persisted_u64(value, StoreRecordKindV1::Session).map(GoalRevisionNumberV2::new)
        })
        .transpose()?;
    let goal_state = load_goal_state_v2(
        connection,
        trace.session_id(),
        current_goal_revision,
        &workflow_memory,
    )?;
    let state = GraphSessionStateV2::new_with_goal_state(
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
        goal_state,
        persisted_time_v2(persisted.created_at, StoreRecordKindV1::Session)?,
        optional_persisted_time_v2(persisted.completed_at, StoreRecordKindV1::Session)?,
        optional_persisted_time_v2(persisted.cancelled_at, StoreRecordKindV1::Session)?,
        persisted.cancel_reason,
    )
    .map_err(|_| corrupt(StoreRecordKindV1::Session))?;
    let schema_version: i64 =
        accounted_query_row(connection, "PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let dispositions = if schema_version >= 5 {
        load_terminal_dispositions_connection_v2(connection, &state)?
    } else {
        Vec::new()
    };

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
    #[cfg(any(test, feature = "test-support"))]
    _reads001_graph_scope.commit();
    Ok(AdmittedGraphStateV2 {
        graph: state,
        dispositions,
    })
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
    let row: SnapshotRow = accounted_query_row(
        connection,
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
    let source_label: String = accounted_query_row(
        connection,
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
    let snapshot = crate::admit_procedure_snapshot_v2(
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
    let nodes_sql = "SELECT graph_node_id, node_definition_id, placement_index, node_type, \
             goal_assessment, canonical_placement_json FROM v2_graph_nodes \
             WHERE snapshot_id = ?1 ORDER BY placement_index";
    let mut statement = accounted_prepare(connection, nodes_sql)
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    let rows = accounted_query_map_prepared(
        &mut statement,
        nodes_sql,
        [snapshot.snapshot_id().as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )
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
    let attempts_sql = "SELECT attempt_id, snapshot_id, graph_node_id, node_definition_id, attempt_number, \
             trace_sequence, lifecycle, validity, goal_revision FROM v2_attempts \
             WHERE session_id = ?1 ORDER BY trace_sequence";
    let mut statement = accounted_prepare(connection, attempts_sql)
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows =
        accounted_query_map_prepared(&mut statement, attempts_sql, [session_id.as_str()], |row| {
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
    let metadata_sql = "SELECT attempt_id, started_at_ms, ended_at_ms, terminal_reason FROM v2_attempts \
             WHERE session_id = ?1 ORDER BY trace_sequence";
    let mut statement = accounted_prepare(connection, metadata_sql)
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows =
        accounted_query_map_prepared(&mut statement, metadata_sql, [session_id.as_str()], |row| {
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
    let counters_sql = "SELECT c.snapshot_id, c.graph_node_id, c.attempt_count, c.rework_traversal_count \
             FROM v2_graph_node_counters c JOIN v2_graph_nodes n \
             ON n.snapshot_id = c.snapshot_id AND n.graph_node_id = c.graph_node_id \
             WHERE c.session_id = ?1 ORDER BY n.placement_index";
    let mut statement = accounted_prepare(connection, counters_sql)
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let rows =
        accounted_query_map_prepared(&mut statement, counters_sql, [session_id.as_str()], |row| {
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
) -> Result<Option<AdmittedGraphStateV2>, StoreErrorV1> {
    let orphan_snapshot_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM v2_procedure_snapshots p \
             LEFT JOIN v2_task_sessions s ON s.procedure_snapshot_id = p.snapshot_id \
             WHERE s.session_id IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Snapshot))?;
    if orphan_snapshot_count != 0 {
        return Err(corrupt(StoreRecordKindV1::Snapshot));
    }
    let admission = load_graph_session_admission_connection_v2(connection)?;
    let invalid_inactive: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM v2_task_sessions \
             WHERE activity = 'inactive' \
             AND (archive_slot IS NULL OR archive_slot < 1 \
                  OR archive_slot > ?1 \
                  OR lifecycle NOT IN ('completed', 'cancelled'))",
            params![MAX_ARCHIVE_SLOT_V2],
            |row| row.get(0),
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    if invalid_inactive != 0 {
        return Err(corrupt(StoreRecordKindV1::Session));
    }
    let duplicate_slot = connection
        .query_row(
            "SELECT 1 FROM v2_task_sessions WHERE activity = 'inactive' \
             GROUP BY archive_slot HAVING COUNT(*) > 1 LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    if duplicate_slot.is_some() {
        return Err(corrupt(StoreRecordKindV1::Session));
    }
    crate::v2_delegation::verify_receipts_connection_v1(connection)?;
    Ok(admission)
}

pub(crate) fn load_terminal_dispositions_connection_v2(
    connection: &Connection,
    state: &GraphSessionStateV2,
) -> Result<Vec<TerminalDispositionV2>, StoreErrorV1> {
    type PersistedDispositionV2 = (
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    );
    let schema_version: i64 =
        accounted_query_row(connection, "PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    let persisted = if schema_version >= 8 {
        let dispositions_v8_sql = "SELECT session_id, terminal_session_revision, kind, summary, stable_reference, \
                 reason, successor_session_id, actor, recorded_at_ms FROM v2_terminal_dispositions \
                 WHERE session_id = ?1 ORDER BY terminal_session_revision";
        let mut statement = accounted_prepare(connection, dispositions_v8_sql)
            .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
        accounted_query_map_prepared(
            &mut statement,
            dispositions_v8_sql,
            [state.trace().session_id().as_str()],
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
                ))
            },
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?
        .collect::<Result<Vec<PersistedDispositionV2>, _>>()
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?
    } else {
        let dispositions_legacy_sql = "SELECT session_id, terminal_session_revision, kind, summary, stable_reference, \
                 reason, actor, recorded_at_ms FROM v2_terminal_dispositions \
                 WHERE session_id = ?1 ORDER BY terminal_session_revision";
        let mut statement = accounted_prepare(connection, dispositions_legacy_sql)
            .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
        accounted_query_map_prepared(
            &mut statement,
            dispositions_legacy_sql,
            [state.trace().session_id().as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    None,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?
        .collect::<Result<Vec<PersistedDispositionV2>, _>>()
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?
    };
    let mut dispositions = Vec::with_capacity(persisted.len());
    for (session_id, revision, kind, summary, reference, reason, successor, actor, recorded_at) in
        persisted
    {
        let session_id =
            SessionId::new(session_id).map_err(|_| corrupt(StoreRecordKindV1::Session))?;
        let revision = Revision::new(persisted_u64(revision, StoreRecordKindV1::Session)?);
        let actor = actor
            .map(ActorAttributionV2::new)
            .transpose()
            .map_err(|_| corrupt(StoreRecordKindV1::Session))?;
        let recorded_at = persisted_time_v2(recorded_at, StoreRecordKindV1::Session)?;
        let disposition = match kind.as_str() {
            "handed_off" => TerminalDispositionV2::handed_off(
                session_id,
                revision,
                summary.ok_or_else(|| corrupt(StoreRecordKindV1::Session))?,
                reference.ok_or_else(|| corrupt(StoreRecordKindV1::Session))?,
                actor,
                recorded_at,
            ),
            "not_required" => TerminalDispositionV2::not_required(
                session_id,
                revision,
                reason.ok_or_else(|| corrupt(StoreRecordKindV1::Session))?,
                actor,
                recorded_at,
            ),
            "superseded" => TerminalDispositionV2::superseded(
                session_id,
                revision,
                reason.ok_or_else(|| corrupt(StoreRecordKindV1::Session))?,
                SessionId::new(successor.ok_or_else(|| corrupt(StoreRecordKindV1::Session))?)
                    .map_err(|_| corrupt(StoreRecordKindV1::Session))?,
                actor,
                recorded_at,
            ),
            _ => return Err(corrupt(StoreRecordKindV1::Session)),
        }
        .map_err(|_| corrupt(StoreRecordKindV1::Session))?;
        dispositions.push(disposition);
    }
    validate_terminal_disposition_history_v2(state, &dispositions)?;
    Ok(dispositions)
}

fn validate_terminal_disposition_history_v2(
    state: &GraphSessionStateV2,
    dispositions: &[TerminalDispositionV2],
) -> Result<(), StoreErrorV1> {
    if state.trace().lifecycle() == SessionLifecycle::Prepared && !dispositions.is_empty() {
        return Err(corrupt(StoreRecordKindV1::Session));
    }
    let mut prior = Revision::ZERO;
    for disposition in dispositions {
        if disposition.session_id() != state.trace().session_id()
            || disposition.terminal_session_revision() <= prior
            || disposition.terminal_session_revision() > state.trace().revision()
            || disposition.recorded_at() < state.created_at()
            || (disposition.terminal_session_revision() == state.trace().revision()
                && !matches!(
                    state.trace().lifecycle(),
                    SessionLifecycle::Completed | SessionLifecycle::Cancelled
                ))
        {
            return Err(corrupt(StoreRecordKindV1::Session));
        }
        prior = disposition.terminal_session_revision();
    }
    Ok(())
}

pub(crate) fn record_terminal_disposition_transaction_v2(
    transaction: &Transaction<'_>,
    disposition: &TerminalDispositionV2,
) -> Result<(), StoreErrorV1> {
    let state = load_graph_session_connection_v2(transaction)?
        .ok_or_else(|| invalid_store("no current Procedure v2 graph session exists"))?;
    if disposition.session_id() != state.trace().session_id()
        || disposition.terminal_session_revision() != state.trace().revision()
    {
        return Err(invalid_store("terminal disposition identity is stale"));
    }
    if !matches!(
        state.trace().lifecycle(),
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    ) {
        return Err(invalid_store(
            "terminal disposition requires a terminal session",
        ));
    }
    if disposition.recorded_at() < state.created_at() {
        return Err(invalid_store("terminal disposition timestamp regressed"));
    }
    let dispositions = load_terminal_dispositions_connection_v2(transaction, &state)?;
    if terminal_disposition_is_current_v2(&dispositions, disposition.terminal_session_revision()) {
        return Err(StoreErrorV1::TerminalDispositionAlreadyRecordedV1 {
            session_revision: disposition.terminal_session_revision(),
        });
    }
    let (kind, summary, reference, reason, successor_session_id) = match disposition.kind() {
        TerminalDispositionKindV2::HandedOff => (
            "handed_off",
            disposition.summary(),
            disposition.stable_reference(),
            None,
            None,
        ),
        TerminalDispositionKindV2::NotRequired => {
            ("not_required", None, None, disposition.reason(), None)
        }
        TerminalDispositionKindV2::Superseded => (
            "superseded",
            None,
            None,
            disposition.reason(),
            disposition.successor_session_id().map(SessionId::as_str),
        ),
    };
    let inserted = transaction
        .execute(
            "INSERT INTO v2_terminal_dispositions (session_id, terminal_session_revision, kind, \
             summary, stable_reference, reason, successor_session_id, actor, recorded_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                disposition.session_id().as_str(),
                sqlite_u64(
                    disposition.terminal_session_revision().get(),
                    "terminal disposition session revision",
                )?,
                kind,
                summary,
                reference,
                reason,
                successor_session_id,
                disposition.actor().map(ActorAttributionV2::as_str),
                sqlite_u64(
                    disposition.recorded_at().get(),
                    "terminal disposition timestamp",
                )?,
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Session))?;
    if inserted != 1 {
        return Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::ProcedureV2GraphState,
        });
    }
    Ok(())
}

pub(crate) fn terminal_disposition_is_current_v2(
    dispositions: &[TerminalDispositionV2],
    session_revision: Revision,
) -> bool {
    dispositions
        .last()
        .is_some_and(|disposition| disposition.terminal_session_revision() == session_revision)
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
        "prepared" => Ok(SessionLifecycle::Prepared),
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
    goal_transition: &WorkflowGoalTransitionV2<'_>,
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
    let goal_reactivation = previous.trace().lifecycle() == SessionLifecycle::Completed
        && next.trace().lifecycle() == SessionLifecycle::Running
        && matches!(goal_transition, WorkflowGoalTransitionV2::Rework { .. });
    let prepared_begin = previous.trace().lifecycle() == SessionLifecycle::Prepared
        && next.trace().lifecycle() == SessionLifecycle::Running
        && previous.trace().attempts().is_empty()
        && next.trace().attempts().len() == 1
        && next.trace().attempts().first().is_some_and(|attempt| {
            attempt.graph_node_id() == previous.snapshot().entry_graph_node_id()
                && attempt.number() == AttemptNumberV2::FIRST
                && attempt.trace() == TraceSequenceV2::FIRST
        });
    if (previous.trace().lifecycle() != SessionLifecycle::Running
        && !manual_reactivation
        && !goal_reactivation
        && !prepared_begin)
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
        let initial_goal_binding = matches!(
            goal_transition,
            WorkflowGoalTransitionV2::InitialBinding { attempt_id }
                if *attempt_id == old_attempt.attempt_id()
                    && old_attempt.lifecycle() == AttemptLifecycle::Active
                    && new_attempt.lifecycle() == AttemptLifecycle::Active
                    && old_attempt.goal_revision().is_none()
                    && new_attempt.goal_revision() == Some(GoalRevisionNumberV2::FIRST)
        );
        if old_attempt.attempt_id() != new_attempt.attempt_id()
            || old_attempt.graph_node_id() != new_attempt.graph_node_id()
            || old_attempt.number() != new_attempt.number()
            || old_attempt.trace() != new_attempt.trace()
            || (old_attempt.goal_revision() != new_attempt.goal_revision() && !initial_goal_binding)
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
            && ((!initial_goal_binding && old_attempt != new_attempt)
                || old_attempt.lifecycle() != new_attempt.lifecycle()
                || old_attempt.validity() != new_attempt.validity()
                || old_metadata != new_metadata)
        {
            return Err(invalid(
                "Procedure v2 cursor-stable mutation changed the active attempt",
            ));
        }
    }
    let cursor_stable = previous.trace().attempts().len() == next.trace().attempts().len()
        && previous.trace().active_attempt().is_some_and(|old| {
            next.trace()
                .active_attempt()
                .is_some_and(|new| new.attempt_id() == old.attempt_id())
        });
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
    if previous.trace().lifecycle() == SessionLifecycle::Running
        && next.trace().lifecycle() == SessionLifecycle::Cancelled
        && previous.counters() != next.counters()
    {
        return Err(invalid("Procedure v2 cancellation changed graph counters"));
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
    let rework_target =
        if let WorkflowGoalTransitionV2::Rework { target_attempt_id } = goal_transition {
            Some(
                next.trace()
                    .attempts()
                    .iter()
                    .find(|attempt| attempt.attempt_id() == *target_attempt_id)
                    .ok_or_else(|| invalid("Procedure v2 goal rework counter target is absent"))?
                    .graph_node_id(),
            )
        } else if next.workflow_memory().reworks().len()
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
            "INSERT INTO v2_task_sessions (session_id, activity, task_title, procedure_snapshot_id, \
             lifecycle, session_revision, latest_trace_sequence, active_graph_node_id, \
             active_attempt_id, active_trace_sequence, goal_tracking, current_goal_revision, \
             created_at_ms, completed_at_ms, cancelled_at_ms, cancel_reason) \
             VALUES (?1, 'current', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                state
                    .goal_state()
                    .current_revision()
                    .map(|value| sqlite_u64(value.get(), "Procedure v2 current goal revision"))
                    .transpose()?,
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
             active_trace_sequence = ?6, current_goal_revision = ?7, completed_at_ms = ?8, \
             cancelled_at_ms = ?9, cancel_reason = ?10 WHERE activity = 'current' AND session_id = ?11 \
             AND session_revision = ?12 AND current_goal_revision IS ?13",
            params![
                session_lifecycle_text(next.trace().lifecycle()),
                sqlite_u64(
                    next.trace().revision().get(),
                    "Procedure v2 session revision"
                )?,
                sqlite_u64(
                    latest_trace(next.trace()).get(),
                    "Procedure v2 trace sequence"
                )?,
                active_node,
                active_attempt,
                active_trace,
                next.goal_state()
                    .current_revision()
                    .map(|value| sqlite_u64(value.get(), "Procedure v2 current goal revision"))
                    .transpose()?,
                optional_sqlite_time(next.completed_at(), "Procedure v2 completion timestamp")?,
                optional_sqlite_time(next.cancelled_at(), "Procedure v2 cancellation timestamp")?,
                next.cancel_reason(),
                next.trace().session_id().as_str(),
                sqlite_u64(
                    previous.trace().revision().get(),
                    "Procedure v2 session revision",
                )?,
                previous
                    .goal_state()
                    .current_revision()
                    .map(|value| sqlite_u64(value.get(), "Procedure v2 current goal revision"))
                    .transpose()?,
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                attempt
                    .goal_revision()
                    .map(|value| sqlite_u64(value.get(), "Procedure v2 attempt goal revision"))
                    .transpose()?,
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
        SessionLifecycle::Prepared => "prepared",
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
