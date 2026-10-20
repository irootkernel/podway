//! Procedure v2 attempt-local workflow-memory persistence.

use std::cell::LazyCell;
use std::collections::BTreeMap;
use std::str::FromStr;

use podway_core::{
    ActorAttributionV2, ArtifactValueV1, AttemptId, AttemptNumberV2, BlockerId, BlockerState,
    CheckResultExecutorV2, CheckResultInputBasisV2, CheckResultOutcomeV2, CheckResultValueV2,
    DecisionRecordInputV2, DecisionRecordV2, EvidenceReferenceSnapshotV2, GoalRevisionNumberV2,
    GraphNodeId, ItemId, ItemSpecV2, ItemTypeV1, NodeDefinitionId, OperationId, OptionId,
    ProcedureSnapshotId, ReasonV2, RecordedItemValueV2, ResolvedEvidenceReferenceV2,
    ResolvedEvidenceSetV2, Revision, ReworkKindV2, ReworkRecordInputV2, ReworkRecordV2,
    SessionAttemptV2, SessionId, SessionTraceV2, Sha256Digest, TraceSequenceV2, TransitionEffectV2,
    UnixMillis, canonicalize_json_v1, verify_canonical_json_v1,
};
use rusqlite::fallible_streaming_iterator::FallibleStreamingIterator;
use rusqlite::{Connection, Rows, Statement, Transaction, params};
use serde_json::Value;

use crate::query_accounting::{
    accounted_delivered_row, accounted_prepare, accounted_query_map_prepared,
    accounted_query_prepared, accounted_query_row,
};
use crate::read_family::{
    CHUNK_PAIRS, CHUNK_SQL, ChunkStep, FamilyCursor, ReadFamily, arm_chunk, chunk_step,
    collect_owner,
};
use crate::v2_state::{AttemptMetadataV2, ProcedureSnapshotV2};
use crate::{
    RusqliteErrorContextV1, StoreErrorV1, StoreRecordKindV1, StoreValueErrorV1,
    map_rusqlite_error_v1,
};
pub use podway_core::workflow_v2::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, AttemptWorkflowMemoryV2, BlockerStateV2,
    EvidenceLookupErrorV2, EvidenceReadbackV2, EvidenceResolutionStateV2, GraphMutationErrorV2,
    ItemSlotStateV2, MAX_ACTIVE_ITEM_MUTATIONS_V2, MAX_OPEN_BLOCKERS_V2, SelectedEvidenceItemV2,
    WorkflowMemoryStateV2, canonical_recorded_items_json_v2, recorded_items_digest_v2,
    recorded_value_digest_v2, recorded_value_encoded_size_v2,
};
use podway_core::workflow_v2::{item_value_json_v2, parse_item_type};

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

fn item_type_text(value: ItemTypeV1) -> &'static str {
    match value {
        ItemTypeV1::Confirm => "confirm",
        ItemTypeV1::Text => "text",
        ItemTypeV1::Choice => "choice",
        ItemTypeV1::Integer => "integer",
        ItemTypeV1::List => "list",
        ItemTypeV1::Artifact => "artifact",
        ItemTypeV1::CheckResult => "check_result",
    }
}

fn blocker_state_text(value: BlockerState) -> &'static str {
    match value {
        BlockerState::Open => "open",
        BlockerState::Resolved => "resolved",
    }
}

fn encode_item_value_v2(value: &RecordedItemValueV2) -> Result<String, StoreErrorV1> {
    canonicalize_json_v1(
        &item_value_json_v2(value).map_err(|error| StoreErrorV1::InvalidStateV1(error.into()))?,
    )
    .map_err(|_| corrupt(StoreRecordKindV1::Item))
}

fn decode_item_value_v2(
    encoded: &str,
    expected: ItemTypeV1,
) -> Result<RecordedItemValueV2, StoreErrorV1> {
    let value: Value =
        serde_json::from_str(encoded).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
    let object = value
        .as_object()
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
    if parse_item_type(kind).map_err(|_| corrupt(StoreRecordKindV1::Item))? != expected {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    let decoded = match kind {
        "confirm" if exact_keys(object, &["kind"]) => RecordedItemValueV2::confirm(),
        "text" if exact_keys(object, &["kind", "value"]) => {
            RecordedItemValueV2::text(required_json_text(object, "value")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?
        }
        "choice" if exact_keys(object, &["kind", "value"]) => {
            RecordedItemValueV2::choice(required_json_text(object, "value")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?
        }
        "integer" if exact_keys(object, &["kind", "value"]) => RecordedItemValueV2::integer(
            object
                .get("value")
                .and_then(Value::as_i64)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?,
        ),
        "list" if exact_keys(object, &["kind", "value"]) => RecordedItemValueV2::list(
            object
                .get("value")
                .and_then(Value::as_array)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
        "artifact"
            if exact_keys(
                object,
                &[
                    "kind",
                    "location_kind",
                    "location",
                    "digest",
                    "size_bytes",
                    "media_type",
                ],
            ) =>
        {
            let location = required_json_text(object, "location")?;
            let digest = Sha256Digest::new(required_json_text(object, "digest")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
            let size = object
                .get("size_bytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
            let media_type = required_json_text(object, "media_type")?;
            let artifact = match object.get("location_kind").and_then(Value::as_str) {
                Some("local_path") => {
                    ArtifactValueV1::local_path(location, digest, size, media_type)
                }
                Some("external_reference") => {
                    ArtifactValueV1::external_reference(location, digest, size, media_type)
                }
                _ => return Err(corrupt(StoreRecordKindV1::Item)),
            }
            .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
            RecordedItemValueV2::artifact(artifact)
        }
        "check_result"
            if exact_keys(
                object,
                &[
                    "kind",
                    "operation_id",
                    "operation_digest",
                    "input_basis",
                    "executor",
                    "outcome",
                    "summary",
                    "output_digest",
                ],
            ) =>
        {
            let input_basis = required_json_object(object, "input_basis")?;
            if !exact_keys(input_basis, &["descriptor", "digest"]) {
                return Err(corrupt(StoreRecordKindV1::Item));
            }
            let executor = required_json_object(object, "executor")?;
            if !exact_keys(executor, &["name", "version"]) {
                return Err(corrupt(StoreRecordKindV1::Item));
            }
            let value = CheckResultValueV2::new(
                OperationId::new(required_json_text(object, "operation_id")?)
                    .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                Sha256Digest::new(required_json_text(object, "operation_digest")?)
                    .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                CheckResultInputBasisV2::new(
                    required_json_text(input_basis, "descriptor")?,
                    Sha256Digest::new(required_json_text(input_basis, "digest")?)
                        .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                )
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                CheckResultExecutorV2::new(
                    required_json_text(executor, "name")?,
                    required_json_text(executor, "version")?,
                )
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                CheckResultOutcomeV2::from_str(&required_json_text(object, "outcome")?)
                    .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                required_json_text(object, "summary")?,
                Sha256Digest::new(required_json_text(object, "output_digest")?)
                    .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
            )
            .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
            RecordedItemValueV2::check_result(value)
        }
        _ => return Err(corrupt(StoreRecordKindV1::Item)),
    };
    // Exact typed re-encoding checks both canonical bytes and normalization.
    // A preceding generic canonical check would parse and encode the same
    // bounded payload a second time on every cold read.
    if encode_item_value_v2(&decoded)? != encoded {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    Ok(decoded)
}

fn exact_keys(object: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn required_json_object<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<&'a serde_json::Map<String, Value>, StoreErrorV1> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
}

fn required_json_text(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<String, StoreErrorV1> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
}

pub(crate) fn insert_workflow_memory_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    memory: &WorkflowMemoryStateV2,
) -> Result<(), StoreErrorV1> {
    for attempt in memory.attempts() {
        insert_attempt_memory_v2(transaction, attempt)?;
    }
    for decision in memory.decisions() {
        insert_decision_v2(transaction, decision)?;
    }
    for rework in memory.reworks() {
        insert_rework_v2(transaction, session_id, rework)?;
    }
    Ok(())
}

pub(crate) fn replace_workflow_memory_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    previous: &WorkflowMemoryStateV2,
    next: &WorkflowMemoryStateV2,
) -> Result<(), StoreErrorV1> {
    for (old_attempt, new_attempt) in previous.attempts().iter().zip(next.attempts()) {
        for (old, new) in old_attempt
            .item_slots()
            .iter()
            .zip(new_attempt.item_slots())
        {
            if old == new {
                continue;
            }
            let changed = transaction.execute(
                "UPDATE v2_item_slots SET item_revision = ?1, value_json = ?2, updated_at_ms = ?3 \
                 WHERE attempt_id = ?4 AND item_id = ?5 AND item_type = ?6 AND item_revision = ?7 \
                 AND value_json IS ?8 AND created_at_ms = ?9 AND updated_at_ms = ?10",
                params![
                    sqlite_u64(new.revision().get(), "Procedure v2 item revision")?,
                    new.value().map(encode_item_value_v2).transpose()?,
                    sqlite_u64(new.updated_at().get(), "Procedure v2 item update timestamp")?,
                    old.attempt_id().as_str(), old.item_id().as_str(), item_type_text(old.item_type()),
                    sqlite_u64(old.revision().get(), "Procedure v2 item revision")?,
                    old.value().map(encode_item_value_v2).transpose()?,
                    sqlite_u64(old.created_at().get(), "Procedure v2 item creation timestamp")?,
                    sqlite_u64(old.updated_at().get(), "Procedure v2 item update timestamp")?,
                ],
            ).map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
            if changed != 1 {
                return Err(corrupt(StoreRecordKindV1::Item));
            }
        }
        // Timestamp ties and clock regressions can insert a new blocker before
        // existing entries in canonical order. Persistence follows identity.
        for old in old_attempt.blockers() {
            let new = new_attempt
                .blockers()
                .iter()
                .find(|new| new.blocker_id() == old.blocker_id())
                .ok_or_else(|| corrupt(StoreRecordKindV1::Blocker))?;
            if old == new {
                continue;
            }
            let changed = transaction
                .execute(
                    "UPDATE v2_blockers SET state = ?1, resolved_at_ms = ?2 \
                 WHERE blocker_id = ?3 AND attempt_id = ?4 AND reason = ?5 AND state = ?6 \
                 AND created_at_ms = ?7 AND resolved_at_ms IS ?8",
                    params![
                        blocker_state_text(new.state()),
                        new.resolved_at()
                            .map(|value| sqlite_u64(
                                value.get(),
                                "Procedure v2 blocker resolution timestamp"
                            ))
                            .transpose()?,
                        old.blocker_id().as_str(),
                        old.attempt_id().as_str(),
                        old.reason(),
                        blocker_state_text(old.state()),
                        sqlite_u64(
                            old.created_at().get(),
                            "Procedure v2 blocker creation timestamp"
                        )?,
                        old.resolved_at()
                            .map(|value| sqlite_u64(
                                value.get(),
                                "Procedure v2 blocker resolution timestamp"
                            ))
                            .transpose()?,
                    ],
                )
                .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
            if changed != 1 {
                return Err(corrupt(StoreRecordKindV1::Blocker));
            }
        }
        for blocker in new_attempt.blockers().iter().filter(|new| {
            !old_attempt
                .blockers()
                .iter()
                .any(|old| old.blocker_id() == new.blocker_id())
        }) {
            insert_blocker_v2(transaction, blocker)?;
        }
    }
    for attempt in next.attempts().iter().skip(previous.attempts().len()) {
        insert_attempt_memory_v2(transaction, attempt)?;
    }
    for decision in next.decisions().iter().skip(previous.decisions().len()) {
        insert_decision_v2(transaction, decision)?;
    }
    for rework in next.reworks().iter().skip(previous.reworks().len()) {
        insert_rework_v2(transaction, session_id, rework)?;
    }
    Ok(())
}

fn insert_attempt_memory_v2(
    transaction: &Transaction<'_>,
    attempt: &AttemptWorkflowMemoryV2,
) -> Result<(), StoreErrorV1> {
    for slot in attempt.item_slots() {
        transaction.execute(
            "INSERT INTO v2_item_slots (attempt_id, item_id, item_type, item_revision, value_json, \
             created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                slot.attempt_id().as_str(), slot.item_id().as_str(), item_type_text(slot.item_type()),
                sqlite_u64(slot.revision().get(), "Procedure v2 item revision")?,
                slot.value().map(encode_item_value_v2).transpose()?,
                sqlite_u64(slot.created_at().get(), "Procedure v2 item creation timestamp")?,
                sqlite_u64(slot.updated_at().get(), "Procedure v2 item update timestamp")?,
            ],
        ).map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    }
    for blocker in attempt.blockers() {
        insert_blocker_v2(transaction, blocker)?;
    }
    for reference in attempt.evidence() {
        let selected = canonicalize_json_v1(
            &reference
                .selected_item_ids()
                .iter()
                .map(ItemId::as_str)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let (state, source_attempt_id, source_attempt_number, digest, resolved_at) =
            match reference.resolution() {
                ResolvedEvidenceReferenceV2::Resolved(snapshot) => (
                    "resolved",
                    Some(snapshot.source_attempt_id().as_str()),
                    Some(sqlite_u64(
                        snapshot.source_attempt_number().get(),
                        "Procedure v2 source attempt number",
                    )?),
                    Some(snapshot.items_digest().as_str()),
                    Some(sqlite_u64(
                        snapshot.resolved_at().get(),
                        "Procedure v2 evidence timestamp",
                    )?),
                ),
                ResolvedEvidenceReferenceV2::Skipped(snapshot) => (
                    "skipped",
                    Some(snapshot.source_attempt_id().as_str()),
                    Some(sqlite_u64(
                        snapshot.source_attempt_number().get(),
                        "Procedure v2 source attempt number",
                    )?),
                    Some(snapshot.items_digest().as_str()),
                    Some(sqlite_u64(
                        snapshot.resolved_at().get(),
                        "Procedure v2 evidence timestamp",
                    )?),
                ),
                ResolvedEvidenceReferenceV2::Unresolved { .. } => {
                    ("unresolved", None, None, None, None)
                }
            };
        transaction
            .execute(
                "INSERT INTO v2_resolved_evidence_references (attempt_id, source_graph_node_id, \
             reference_ordinal, required, selected_item_ids_json, state, source_attempt_id, \
             source_attempt_number, items_digest, resolved_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    attempt.attempt_id().as_str(),
                    reference.resolution().source_node().as_str(),
                    i64::from(reference.ordinal()),
                    i64::from(reference.required()),
                    selected,
                    state,
                    source_attempt_id,
                    source_attempt_number,
                    digest,
                    resolved_at
                ],
            )
            .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    }
    Ok(())
}

fn insert_blocker_v2(
    transaction: &Transaction<'_>,
    blocker: &BlockerStateV2,
) -> Result<(), StoreErrorV1> {
    transaction.execute(
        "INSERT INTO v2_blockers (blocker_id, attempt_id, reason, state, created_at_ms, resolved_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![blocker.blocker_id().as_str(), blocker.attempt_id().as_str(), blocker.reason(),
            blocker_state_text(blocker.state()),
            sqlite_u64(blocker.created_at().get(), "Procedure v2 blocker creation timestamp")?,
            blocker.resolved_at().map(|value| sqlite_u64(value.get(), "Procedure v2 blocker resolution timestamp")).transpose()?],
    ).map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
    Ok(())
}

fn insert_decision_v2(
    transaction: &Transaction<'_>,
    record: &DecisionRecordV2,
) -> Result<(), StoreErrorV1> {
    transaction.execute(
        "INSERT INTO v2_decision_records (attempt_id, session_id, trace_sequence, session_revision, \
         procedure_snapshot_id, procedure_digest, graph_node_id, node_definition_id, attempt_number, \
         goal_revision, selected_option_id, route_effect, route_target_graph_node_id, reason, actor, recorded_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![record.attempt_id().as_str(), record.session_id().as_str(),
            sqlite_u64(record.trace().get(), "Procedure v2 decision trace")?,
            sqlite_u64(record.session_revision().get(), "Procedure v2 decision session revision")?,
            record.procedure_snapshot_id().as_str(), record.procedure_digest().as_str(),
            record.graph_node_id().as_str(), record.node_definition_id().as_str(),
            sqlite_u64(record.attempt_number().get(), "Procedure v2 decision attempt number")?,
            record.goal_revision().map(|value| sqlite_u64(value.get(), "Procedure v2 decision goal revision")).transpose()?,
            record.selected_option().as_str(), record.route_effect().as_str(), record.route_target().as_str(),
            record.reason().as_str(), record.actor().map(ActorAttributionV2::as_str),
            sqlite_u64(record.recorded_at().get(), "Procedure v2 decision timestamp")?],
    ).map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    Ok(())
}

fn insert_rework_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    record: &ReworkRecordV2,
) -> Result<(), StoreErrorV1> {
    transaction
        .execute(
            "INSERT INTO v2_rework_records (session_id, trace_sequence, kind, from_graph_node_id, \
         to_graph_node_id, target_attempt_id, reason, reactivated, actor, recorded_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                session_id.as_str(),
                sqlite_u64(record.trace().get(), "Procedure v2 rework trace")?,
                record.kind().as_str(),
                record.from_node().as_str(),
                record.to_node().as_str(),
                record.target_attempt_id().as_str(),
                record.reason().as_str(),
                i64::from(record.reactivated()),
                record.actor().map(ActorAttributionV2::as_str),
                sqlite_u64(record.recorded_at().get(), "Procedure v2 rework timestamp")?
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    Ok(())
}

const SLOTS_BULK_SQL: &str = "SELECT item_id, item_type, item_revision, value_json IS NOT NULL, created_at_ms, updated_at_ms, value_json, a.trace_sequence \
     FROM v2_item_slots s JOIN v2_attempts a ON a.attempt_id=s.attempt_id \
     WHERE a.session_id=?1 ORDER BY a.trace_sequence, s.item_id";
/// READS-002 mixed-path session header acquisition: scalar routing columns
/// plus lossless raw identity bytes for one owner at a time; no payload
/// column is selected.
pub(crate) const SLOTS_HEADER_BULK_SQL: &str = "SELECT s.item_id, s.item_type, s.item_revision, typeof(value_json) != 'null', s.rowid, a.trace_sequence \
     FROM v2_item_slots s JOIN v2_attempts a ON a.attempt_id=s.attempt_id \
     WHERE a.session_id=?1 ORDER BY a.trace_sequence, s.item_id";
const BLOCKERS_BULK_SQL: &str = "SELECT blocker_id, reason, state, created_at_ms, resolved_at_ms, a.trace_sequence FROM v2_blockers b \
     JOIN v2_attempts a ON a.attempt_id=b.attempt_id WHERE a.session_id=?1 \
     ORDER BY a.trace_sequence, b.created_at_ms, b.blocker_id";
const EVIDENCE_BULK_SQL: &str = "SELECT source_graph_node_id, reference_ordinal, required, selected_item_ids_json, state, \
     source_attempt_id, source_attempt_number, items_digest, resolved_at_ms, a.trace_sequence \
     FROM v2_resolved_evidence_references e JOIN v2_attempts a ON a.attempt_id=e.attempt_id \
     WHERE a.session_id=?1 ORDER BY a.trace_sequence, e.reference_ordinal";

type SlotRow = (
    String,
    String,
    i64,
    bool,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

/// READS-002 compact mixed-routing records. Production types (test-support
/// reports their exact `size_of`); no test shadow is authoritative.
pub(crate) struct RowStamp {
    pub(crate) spec_idx: usize,
    pub(crate) revision: i64,
    pub(crate) has_value: bool,
    pub(crate) rowid: i64,
}

pub(crate) struct PairEntry {
    pub(crate) attempt_index: usize,
    pub(crate) rowid: i64,
}

impl Clone for PairEntry {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for PairEntry {}

/// One staged header row's lossless raw cells (item id + item type bytes
/// plus scalars), retained only for the single current discovery owner.
struct RawHeaderCell {
    id: Vec<u8>,
    item_type: Vec<u8>,
    revision: i64,
    has_value: bool,
    rowid: i64,
}

#[cfg(any(test, feature = "test-support"))]
impl RawHeaderCell {
    fn scratch_bytes(&self) -> usize {
        self.id.len() + self.item_type.len()
    }
}

/// Per-owner discovery result. Fully validated owners keep compact stamps
/// and a range into the global pair list (no second rowid buffer); the
/// first failing owner keeps its staged raw cells (one-attempt bound) plus
/// the stepping fault for exact-position replay; a TooBig-truncated owner
/// keeps whatever it staged and takes the original per-owner acquisition
/// path at its validation turn.
struct OwnerDiscovery {
    stamps: Vec<RowStamp>,
    raw: Option<Vec<RawHeaderCell>>,
    fault: Option<SteppingFault>,
    pair_range: (usize, usize),
    truncated: bool,
}

/// A collection-stage fault captured while stepping (never raised during
/// discovery). `HeaderConversion` stops all stepping at its row — no later
/// row or owner is acquired — and replays at that exact position; a
/// duplicate's payload converts before the insert failure replays it.
struct SteppingFault {
    error: StoreErrorV1,
    row: usize,
    duplicate: bool,
}

struct MixedDiscovery {
    owners: Vec<Option<OwnerDiscovery>>,
    pairs: Vec<PairEntry>,
}

/// One reconstruction-local borrowed index over cached attempts, keyed by
/// attempt identity. First-match insertion keeps the first attempt for any
/// duplicated id, the same result as a first-match linear scan in attempt
/// order; construction performs no validation of later rows and clones no
/// attempt body.
fn index_cached_attempts(
    attempts: &[AttemptWorkflowMemoryV2],
) -> BTreeMap<&AttemptId, &AttemptWorkflowMemoryV2> {
    let mut index = BTreeMap::new();
    for attempt in attempts {
        index.entry(attempt.attempt_id()).or_insert(attempt);
    }
    index
}

/// Session-wide header discovery for the mixed (prepared-cache) path:
/// stages one owner's raw cells at a time, capturing every header
/// column-conversion fault (storage class, scalar type, UTF-8 validity) at
/// its original row and stopping the scan at the first such fault or at a
/// deterministic TooBig on the boundary step (conservative current owner,
/// partial pairs truncated). Fully valid owners convert losslessly to
/// compact stamps — their raw cells die here — and queue changed/new pairs
/// in the global consumption order. Owners after a stop are `None` and use
/// the original per-owner acquisition at their turn.
fn discover_slot_owners<'a>(
    connection: &Connection,
    trace: &SessionTraceV2,
    cached_attempts: &BTreeMap<&'a AttemptId, &'a AttemptWorkflowMemoryV2>,
    snapshot: &ProcedureSnapshotV2,
) -> Result<MixedDiscovery, StoreErrorV1> {
    let attempt_count = trace.attempts().len();
    let mut owners: Vec<Option<OwnerDiscovery>> = Vec::with_capacity(attempt_count);
    let mut pairs: Vec<PairEntry> = Vec::new();
    if attempt_count == 0 {
        return Ok(MixedDiscovery { owners, pairs });
    }
    let mut statement = accounted_prepare(connection, SLOTS_HEADER_BULK_SQL)
        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    let mut rows = accounted_query_prepared(
        &mut statement,
        SLOTS_HEADER_BULK_SQL,
        [trace.session_id().as_str()],
    )
    .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    let mut pending: Option<i64> = None;
    let mut done = false;
    for index in 0..attempt_count {
        let attempt = &trace.attempts()[index];
        let owner_key = attempt.trace().get() as i64;
        let cached_memory = cached_attempts.get(attempt.attempt_id()).copied();
        let mut cells: Vec<RawHeaderCell> = Vec::new();
        let mut seen: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
        let mut fault: Option<SteppingFault> = None;
        let mut truncated = false;
        // Collect this owner's rows in item_id ASC delivery order. Every
        // header conversion fault — storage class, scalar type, UTF-8
        // validity, duplicate id — is captured at its row and stops the
        // whole scan there; nothing raises during discovery. The step
        // that peeks past the group boundary can instead fail
        // deterministically with TooBig on an oversized later row; the
        // failing step produced no Row and its physical owner is unknown,
        // so this conservative owner truncates (P4.3.1 A2).
        loop {
            if done {
                break;
            }
            if pending.is_none() {
                match rows.next() {
                    Ok(Some(row)) => {
                        accounted_delivered_row(SLOTS_HEADER_BULK_SQL, row);
                        pending = Some(
                            row.get::<_, i64>(5)
                                .map_err(|error| record_error(error, StoreRecordKindV1::Item))?,
                        );
                    }
                    Ok(None) => {
                        done = true;
                        break;
                    }
                    Err(error) => {
                        if matches!(&error, rusqlite::Error::SqliteFailure(code, _)
                            if code.code == rusqlite::ErrorCode::TooBig)
                        {
                            truncated = true;
                            break;
                        }
                        return Err(record_error(error, StoreRecordKindV1::Item));
                    }
                }
            }
            if pending != Some(owner_key) {
                break;
            }
            let row = rows.get().expect("pending header row");
            let cell = match stage_header_cell(row) {
                Ok(cell) => cell,
                Err(error) => {
                    fault = Some(SteppingFault {
                        error,
                        row: cells.len(),
                        duplicate: false,
                    });
                    break;
                }
            };
            let utf8_error = std::str::from_utf8(&cell.id)
                .map_err(|error| rusqlite::Error::Utf8Error(0, error))
                .and_then(|_| {
                    std::str::from_utf8(&cell.item_type)
                        .map_err(|error| rusqlite::Error::Utf8Error(1, error))
                })
                .err()
                .map(|error| record_error(error, StoreRecordKindV1::Item));
            if let Some(error) = utf8_error {
                fault = Some(SteppingFault {
                    error,
                    row: cells.len(),
                    duplicate: false,
                });
                break;
            }
            let duplicate = !seen.insert(cell.id.clone());
            #[cfg(any(test, feature = "test-support"))]
            crate::query_accounting::ledger::acquire_discovery_scratch(cell.scratch_bytes());
            cells.push(cell);
            if duplicate {
                fault = Some(SteppingFault {
                    // The duplicate insert failure replays naturally after
                    // this row's payload conversion; no synthetic error.
                    error: corrupt(StoreRecordKindV1::Item),
                    row: cells.len() - 1,
                    duplicate: true,
                });
                break;
            }
            pending = None;
        }
        let pair_start = pairs.len();
        if truncated {
            owners.push(Some(OwnerDiscovery {
                stamps: Vec::new(),
                raw: Some(cells),
                fault: None,
                pair_range: (pair_start, pair_start),
                truncated: true,
            }));
            break;
        }
        if let Some(stepping) = &fault {
            if !stepping.duplicate {
                // A header conversion fault at row r: a new owner still
                // converts every earlier row's payload first; a cached
                // owner has no point pairs at all.
                let prefix = if cached_memory.is_some() {
                    0
                } else {
                    stepping.row
                };
                for cell in &cells[..prefix] {
                    pairs.push(PairEntry {
                        attempt_index: index,
                        rowid: cell.rowid,
                    });
                }
            } else {
                // A duplicate row's payload pair is included, then later
                // rows stop; a cached owner emits no point pairs.
                let prefix = if cached_memory.is_some() {
                    0
                } else {
                    stepping.row + 1
                };
                for cell in &cells[..prefix] {
                    pairs.push(PairEntry {
                        attempt_index: index,
                        rowid: cell.rowid,
                    });
                }
            }
            owners.push(Some(OwnerDiscovery {
                stamps: Vec::new(),
                raw: Some(cells),
                fault,
                pair_range: (pair_start, pairs.len()),
                truncated: false,
            }));
            break;
        }
        let specification = snapshot
            .item_declarations_v2(attempt.graph_node_id())
            .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let outcome = classify_owner(&cells, specification, cached_memory);
        #[cfg(any(test, feature = "test-support"))]
        if !outcome.failing {
            for cell in &cells {
                crate::query_accounting::ledger::release_discovery_scratch(cell.scratch_bytes());
            }
        }
        let OwnerOutcome {
            failing,
            stamps,
            changed,
        } = outcome;
        // Pair emission: cached owners queue exactly their changed spec
        // positions (classify truncates the prefix at a spec fault and
        // keeps every discovered pair for a trailing leftover); new owners
        // queue every staged row in delivery order regardless of spec
        // faults, because their collection replays all rows first.
        let owner_rowids: Vec<i64> = if cached_memory.is_some() {
            changed
        } else {
            cells.iter().map(|cell| cell.rowid).collect()
        };
        for rowid in owner_rowids {
            pairs.push(PairEntry {
                attempt_index: index,
                rowid,
            });
        }
        let pair_end = pairs.len();
        owners.push(Some(OwnerDiscovery {
            stamps,
            raw: failing.then_some(cells),
            fault: None,
            pair_range: (pair_start, pair_end),
            truncated: false,
        }));
        if failing {
            break;
        }
    }
    while owners.len() < attempt_count {
        owners.push(None);
    }
    drop(rows);
    drop(statement);
    Ok(MixedDiscovery { owners, pairs })
}

/// Stages one header row's cells in the cold mapper's column order,
/// capturing the first column-conversion error (storage class or scalar
/// type) instead of raising it. Raw TEXT bytes stay lossless.
fn stage_header_cell(row: &rusqlite::Row<'_>) -> Result<RawHeaderCell, StoreErrorV1> {
    let convert = |error: rusqlite::Error| record_error(error, StoreRecordKindV1::Item);
    let id = match row
        .get_ref(0)
        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?
    {
        rusqlite::types::ValueRef::Text(bytes) => bytes.to_vec(),
        other => {
            return Err(convert(rusqlite::Error::InvalidColumnType(
                0,
                String::new(),
                other.data_type(),
            )));
        }
    };
    let item_type = match row
        .get_ref(1)
        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?
    {
        rusqlite::types::ValueRef::Text(bytes) => bytes.to_vec(),
        other => {
            return Err(convert(rusqlite::Error::InvalidColumnType(
                1,
                String::new(),
                other.data_type(),
            )));
        }
    };
    let revision = row.get::<_, i64>(2).map_err(convert)?;
    let has_value = row.get::<_, bool>(3).map_err(convert)?;
    let rowid = row.get::<_, i64>(4).map_err(convert)?;
    Ok(RawHeaderCell {
        id,
        item_type,
        revision,
        has_value,
        rowid,
    })
}

struct OwnerOutcome {
    failing: bool,
    stamps: Vec<RowStamp>,
    changed: Vec<i64>,
}

/// Specification walk for one fully collected owner (header cells already
/// proven convertible and unique by stepping): missing items, mismatched
/// types and negative revisions stop cached prefix emission; extra rows
/// are a trailing leftover fault; new owners queue every row regardless.
/// Spec faults themselves replay naturally at the owner's validation turn.
/// Stamps are emitted in row (item_id ASC) order with each row's spec
/// position inside, so a new owner replays its original collection order.
fn classify_owner(
    cells: &[RawHeaderCell],
    specification: &[ItemSpecV2],
    cached_memory: Option<&AttemptWorkflowMemoryV2>,
) -> OwnerOutcome {
    let cached = cached_memory.is_some();
    let mut by_id: std::collections::BTreeMap<&[u8], usize> = std::collections::BTreeMap::new();
    for (ordinal, cell) in cells.iter().enumerate() {
        by_id.insert(&cell.id[..], ordinal);
    }
    let mut spec_of_row = vec![usize::MAX; cells.len()];
    let mut changed: Vec<i64> = Vec::new();
    for (spec_idx, item) in specification.iter().enumerate() {
        let Some(&ordinal) = by_id.get(item.id().as_str().as_bytes()) else {
            return OwnerOutcome {
                failing: true,
                stamps: Vec::new(),
                changed,
            };
        };
        let cell = &cells[ordinal];
        if cell.item_type.as_slice() != item_type_text(item.item_type()).as_bytes()
            || cell.revision < 0
        {
            return OwnerOutcome {
                failing: true,
                stamps: Vec::new(),
                changed,
            };
        }
        if cached {
            let unchanged = cached_memory.is_some_and(|memory| {
                memory
                    .item_slots()
                    .iter()
                    .find(|slot| slot.item_id() == item.id())
                    .is_some_and(|slot| {
                        slot.item_type() == item.item_type()
                            && slot.revision().get() == cell.revision as u64
                            && slot.value().is_some() == cell.has_value
                    })
            });
            if !unchanged {
                changed.push(cell.rowid);
            }
        }
        spec_of_row[ordinal] = spec_idx;
    }
    let leftover = spec_of_row.contains(&usize::MAX);
    if leftover {
        return OwnerOutcome {
            failing: true,
            // A leftover fault fires after all spec items: every already
            // discovered changed pair stays queued.
            stamps: Vec::new(),
            changed,
        };
    }
    let stamps = cells
        .iter()
        .enumerate()
        .map(|(ordinal, cell)| RowStamp {
            spec_idx: spec_of_row[ordinal],
            revision: cell.revision,
            has_value: cell.has_value,
            rowid: cell.rowid,
        })
        .collect();
    OwnerOutcome {
        failing: false,
        stamps,
        changed,
    }
}

/// Result of one changed-pair payload fetch from the chunk stream.
enum PairFetch {
    /// Delivered payload triple; `acquire_raw` already booked on the value.
    Payload(Option<String>, i64, i64),
    /// The expected rowid was not delivered (mismatch or exhausted):
    /// fail closed at the requesting position.
    Missing,
    /// The stream is retired (TooBig) or has no pair for this position:
    /// fall back to the original point selector.
    Unavailable,
}

/// Payload source for changed items at spec positions. The mixed path
/// implements it over the caller-frame chunk locals; cold/original paths
/// use the always-unavailable source and the original point selector.
trait PairSource {
    fn fetch(&mut self, expected_rowid: i64, cached_order: bool)
    -> Result<PairFetch, StoreErrorV1>;
}

/// Borrows of the caller-frame chunk locals (cells iterator, live cursor,
/// consumption cursor, retirement flag) plus the global pair list and the
/// owning trace. No self-reference: every field borrows a frame local.
struct ChunkPairSource<'a, 'cells, 'conn, F>
where
    F: FnOnce() -> Result<Statement<'conn>, StoreErrorV1>,
{
    /// Index of the next chunk cell to arm; advances only when a chunk is
    /// armed, so an exhausted cursor never re-arms or skips a chunk.
    armed: &'a mut usize,
    iter: &'a mut std::slice::IterMut<'cells, LazyCell<Result<Statement<'conn>, StoreErrorV1>, F>>,
    rows: &'a mut Option<Rows<'cells>>,
    retired: &'a mut bool,
    cursor: &'a mut usize,
    pairs: &'a [PairEntry],
    trace: &'a SessionTraceV2,
}

impl<'conn, F> PairSource for ChunkPairSource<'_, '_, 'conn, F>
where
    F: FnOnce() -> Result<Statement<'conn>, StoreErrorV1>,
{
    fn fetch(
        &mut self,
        expected_rowid: i64,
        cached_order: bool,
    ) -> Result<PairFetch, StoreErrorV1> {
        if *self.retired {
            return Ok(PairFetch::Unavailable);
        }
        loop {
            if self.rows.is_none() {
                if *self.armed * CHUNK_PAIRS >= self.pairs.len() {
                    // Every chunk has been armed and exhausted without
                    // delivering this pair's identity: fail closed.
                    return Ok(PairFetch::Missing);
                }
                let chunk_index = *self.armed;
                let chunk_start = chunk_index * CHUNK_PAIRS;
                let chunk_end = self.pairs.len().min(chunk_start + CHUNK_PAIRS);
                if chunk_start >= self.pairs.len() {
                    return Ok(PairFetch::Missing);
                }
                // Borrowed identity binds: the trace's owned attempt UUID
                // bytes and each pair's rowid, NULL for tail padding. No
                // cloned Rust IDs per chunk.
                let mut binds: Vec<rusqlite::types::ToSqlOutput<'_>> =
                    Vec::with_capacity(CHUNK_PAIRS * 2);
                for pair in &self.pairs[chunk_start..chunk_end] {
                    binds.push(rusqlite::types::ToSqlOutput::Borrowed(
                        rusqlite::types::ValueRef::Text(
                            self.trace.attempts()[pair.attempt_index]
                                .attempt_id()
                                .as_str()
                                .as_bytes(),
                        ),
                    ));
                    binds.push(rusqlite::types::ToSqlOutput::Borrowed(
                        rusqlite::types::ValueRef::Integer(pair.rowid),
                    ));
                }
                for _ in chunk_end..chunk_start + CHUNK_PAIRS {
                    binds.push(rusqlite::types::ToSqlOutput::Borrowed(
                        rusqlite::types::ValueRef::Null,
                    ));
                    binds.push(rusqlite::types::ToSqlOutput::Borrowed(
                        rusqlite::types::ValueRef::Null,
                    ));
                }
                let armed = arm_chunk(self.iter, self.rows, &binds, |error| {
                    record_error(error, StoreRecordKindV1::Item)
                })?;
                if !armed {
                    return Ok(PairFetch::Missing);
                }
                *self.armed += 1;
            }
            match chunk_step(self.rows, |error| {
                record_error(error, StoreRecordKindV1::Item)
            })? {
                ChunkStep::Available => {
                    let row = self
                        .rows
                        .as_ref()
                        .and_then(rusqlite::Rows::get)
                        .expect("available chunk row");
                    let delivered: i64 = row
                        .get(3)
                        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                    if delivered != expected_rowid {
                        return Ok(PairFetch::Missing);
                    }
                    // Scalar rowid verification precedes every payload
                    // String conversion; the conversion order follows the
                    // owning mode's original mapper.
                    let (value, created_at, updated_at) = if cached_order {
                        let value = row
                            .get::<_, Option<String>>(0)
                            .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                        let created_at = row
                            .get::<_, i64>(1)
                            .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                        let updated_at = row
                            .get::<_, i64>(2)
                            .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(raw) = value.as_ref() {
                            crate::query_accounting::ledger::acquire_raw(raw.len());
                        }
                        (value, created_at, updated_at)
                    } else {
                        let created_at = row
                            .get::<_, i64>(1)
                            .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                        let updated_at = row
                            .get::<_, i64>(2)
                            .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                        let value = row
                            .get::<_, Option<String>>(0)
                            .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                        // NEW-collection payloads are booked once, when the
                        // row enters the collection map (insert_slot_row).
                        (value, created_at, updated_at)
                    };
                    *self.cursor += 1;
                    return Ok(PairFetch::Payload(value, created_at, updated_at));
                }
                ChunkStep::End => {
                    if *self.cursor < self.pairs.len().min(*self.armed * CHUNK_PAIRS) {
                        return Ok(PairFetch::Missing);
                    }
                    // Chunk exhausted. The fulfilled-pair cursor already
                    // sits at this chunk's boundary: every delivered row
                    // matched its expected pair in order, and any missing
                    // identity fired Missing before End was reachable.
                    *self.rows = None;
                }
                ChunkStep::Retired => {
                    *self.retired = true;
                    return Ok(PairFetch::Unavailable);
                }
            }
        }
    }
}

/// Builds the caller-frame lazy chunk statement cells: one per
/// ceil(M/128) chunk, each prepared on first use.
type ChunkStatementCell<'conn, F> = LazyCell<Result<Statement<'conn>, StoreErrorV1>, F>;

fn chunk_statement_cells<'conn>(
    connection: &'conn Connection,
    count: usize,
) -> Vec<ChunkStatementCell<'conn, impl FnOnce() -> Result<Statement<'conn>, StoreErrorV1>>> {
    (0..count)
        .map(|_| {
            LazyCell::new(|| {
                accounted_prepare(connection, CHUNK_SQL)
                    .map_err(|error| record_error(error, StoreRecordKindV1::Item))
            })
        })
        .collect()
}

pub(crate) fn load_workflow_memory_v2(
    connection: &Connection,
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
    cached: Option<&WorkflowMemoryStateV2>,
) -> Result<WorkflowMemoryStateV2, StoreErrorV1> {
    snapshot
        .validate_memory_declarations_v2()
        .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?;
    let mut slot_statement = LazyCell::new(|| {
        accounted_prepare(connection, SLOTS_BULK_SQL)
            .map_err(|error| record_error(error, StoreRecordKindV1::Item))
    });
    let mut blocker_statement = LazyCell::new(|| {
        accounted_prepare(connection, BLOCKERS_BULK_SQL)
            .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))
    });
    let mut evidence_statement = LazyCell::new(|| {
        accounted_prepare(connection, EVIDENCE_BULK_SQL)
            .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))
    });
    let mut slot_family = LazyCell::new(|| {
        let statement = LazyCell::force_mut(&mut slot_statement)
            .as_mut()
            .map_err(|error| error.clone())?;
        accounted_query_prepared(statement, SLOTS_BULK_SQL, [trace.session_id().as_str()])
            .map(|rows| {
                FamilyCursor::new(
                    rows,
                    SLOTS_BULK_SQL,
                    |row| row.get(7),
                    StoreRecordKindV1::Item,
                )
            })
            .map_err(|error| record_error(error, StoreRecordKindV1::Item))
    });
    let mut blocker_family = LazyCell::new(|| {
        let statement = LazyCell::force_mut(&mut blocker_statement)
            .as_mut()
            .map_err(|error| error.clone())?;
        accounted_query_prepared(statement, BLOCKERS_BULK_SQL, [trace.session_id().as_str()])
            .map(|rows| {
                FamilyCursor::new(
                    rows,
                    BLOCKERS_BULK_SQL,
                    |row| row.get(5),
                    StoreRecordKindV1::Blocker,
                )
            })
            .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))
    });
    let mut evidence_family = LazyCell::new(|| {
        let statement = LazyCell::force_mut(&mut evidence_statement)
            .as_mut()
            .map_err(|error| error.clone())?;
        accounted_query_prepared(statement, EVIDENCE_BULK_SQL, [trace.session_id().as_str()])
            .map(|rows| {
                FamilyCursor::new(
                    rows,
                    EVIDENCE_BULK_SQL,
                    |row| row.get(9),
                    StoreRecordKindV1::Attempt,
                )
            })
            .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))
    });
    // Mixed (prepared-cache) path: one session-wide header discovery scan
    // first, then global changed-only chunks consumed in trace order. The
    // caller-frame chunk locals stay separate (cells vector, disjoint-place
    // iterator, one live cursor, retirement flag, consumption cursor).
    // Any error after scratch first exists books its never-consumed
    // balance at unwind (no-op on success, where the balance is zero).
    #[cfg(any(test, feature = "test-support"))]
    struct DiscoveryScratchUnwind;
    #[cfg(any(test, feature = "test-support"))]
    impl Drop for DiscoveryScratchUnwind {
        fn drop(&mut self) {
            crate::query_accounting::ledger::abort_discovery_scratch();
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    let _discovery_unwind = DiscoveryScratchUnwind;
    // One shared borrowed index over the cached attempts: header discovery
    // and the per-trace-attempt loop look owners up by identity through the
    // same first-match map.
    let cached_attempt_index =
        index_cached_attempts(cached.map(WorkflowMemoryStateV2::attempts).unwrap_or(&[]));
    let discovered = match cached {
        Some(_) => discover_slot_owners(connection, trace, &cached_attempt_index, snapshot)?,
        None => MixedDiscovery {
            owners: Vec::new(),
            pairs: Vec::new(),
        },
    };
    // Fully decomposed so the owner slots and the global pair list borrow
    // independently across the validation loop.
    let MixedDiscovery {
        owners: mut owner_list,
        pairs: global_pairs,
    } = discovered;
    let mixed_pairs: &[PairEntry] = &global_pairs[..];
    let chunk_cell_count = global_pairs.len().div_ceil(CHUNK_PAIRS);
    let mut chunk_cells = chunk_statement_cells(connection, chunk_cell_count);
    let mut chunk_iter = chunk_cells.iter_mut();
    let mut chunk_rows: Option<Rows<'_>> = None;
    let mut chunk_retired = false;
    let mut chunk_armed = 0usize;
    let mut chunk_cursor = 0usize;
    let mut attempts = Vec::with_capacity(trace.attempts().len());

    let load_result = (|| -> Result<(), StoreErrorV1> {
        for (index, (attempt, attempt_metadata)) in
            trace.attempts().iter().zip(metadata).enumerate()
        {
            let specification = snapshot
                .item_declarations_v2(attempt.graph_node_id())
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
            let cached_attempt = cached_attempt_index.get(attempt.attempt_id()).copied();
            let owner = if index < owner_list.len() {
                owner_list[index].take()
            } else {
                None
            };
            let owner_pairs_slice = owner.as_ref().map_or(&[][..], |found| {
                &mixed_pairs[found.pair_range.0..found.pair_range.1]
            });
            let memory = match owner {
                Some(mut owner) => {
                    let mut source = ChunkPairSource {
                        iter: &mut chunk_iter,
                        rows: &mut chunk_rows,
                        retired: &mut chunk_retired,
                        armed: &mut chunk_armed,
                        cursor: &mut chunk_cursor,
                        pairs: mixed_pairs,
                        trace,
                    };
                    load_attempt_memory_mixed_v2(
                        connection,
                        attempt,
                        attempt_metadata,
                        specification,
                        cached_attempt,
                        &mut owner,
                        &mut source,
                        &mut blocker_family,
                        &mut evidence_family,
                        owner_pairs_slice,
                    )?
                }
                None => {
                    // Original per-owner acquisition: cold bulk families or
                    // the old per-attempt selectors, one validator either
                    // way. The slot family is never forced on this path.
                    load_attempt_memory_v2(
                        connection,
                        attempt,
                        attempt_metadata,
                        specification,
                        cached_attempt,
                        cached.is_none(),
                        (&mut slot_family, &mut blocker_family, &mut evidence_family),
                    )?
                }
            };
            attempts.push(memory);
        }
        Ok(())
    })();
    load_result?;
    let decisions = load_decisions_v2(connection, snapshot, trace, &attempts)?;
    let reworks = load_reworks_v2(connection, trace.session_id())?;
    WorkflowMemoryStateV2::new(attempts, decisions, reworks)
        .map_err(|_| corrupt(StoreRecordKindV1::Session))
}

/// The single spec-position validator shared by every acquisition mode:
/// consumes the loaded map in specification order, applies the unchanged
/// stamp filter with its pointer-reuse accounting, and acquires changed
/// payloads — from the live chunk stream first when one is supplied,
/// falling back to the original point selector. Payload conversion order
/// follows the owning mode's original mapper.
#[allow(clippy::too_many_arguments)]
fn validate_attempt_slots(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    specification: &[ItemSpecV2],
    cached: Option<&AttemptWorkflowMemoryV2>,
    loaded: &mut BTreeMap<String, SlotRow>,
    mut mixed_pairs: Option<(&mut dyn PairSource, &mut usize)>,
    owner_pairs: &[PairEntry],
) -> Result<Vec<ItemSlotStateV2>, StoreErrorV1> {
    let mut slots = Vec::with_capacity(specification.len());
    for item in specification {
        let (_, row_type, revision, has_value, created_at, updated_at, loaded_value) = loaded
            .remove(item.id().as_str())
            .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
        // READS-002 baseline metadata unit: the map key, the row id and the
        // row type String all die within this iteration; release their
        // identity bytes at the consumption point (upper bound of one
        // iteration). The removed key is byte-equal to the spec item id.
        // Rows never removed here stay booked until the owning slot scope
        // ends on the error path.
        #[cfg(any(test, feature = "test-support"))]
        crate::query_accounting::ledger::release_identity(
            item.id().as_str().len().saturating_mul(2) + row_type.len(),
        );
        let item_type = parse_item_type(&row_type).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
        if item_type != item.item_type() {
            return Err(corrupt(StoreRecordKindV1::Item));
        }
        let revision = Revision::new(persisted_u64(revision, StoreRecordKindV1::Item)?);
        if let Some(slot) = cached
            .and_then(|memory| {
                memory
                    .item_slots()
                    .iter()
                    .find(|slot| slot.item_id() == item.id())
            })
            .filter(|slot| {
                slot.item_type() == item_type
                    && slot.revision() == revision
                    && slot.value().is_some() == has_value
            })
        {
            // Compatible writers keep identity/creation metadata immutable and
            // advance the revision with any value/update-time change. Reading
            // only the header avoids loading unchanged overflow-page payloads.
            #[cfg(any(test, feature = "test-support"))]
            {
                crate::query_accounting::ledger::record_container_copy();
                if let Some(pointer) = crate::query_accounting::ledger::payload_pointer(slot) {
                    crate::query_accounting::ledger::record_reuse(pointer);
                }
            }
            slots.push(slot.clone());
            continue;
        }
        let (value, created_at, updated_at) = if cached.is_some() {
            let mut from_chunk = None;
            if let Some((source, pair_cursor)) = &mut mixed_pairs {
                let position = **pair_cursor;
                if position < owner_pairs.len() {
                    let expected = owner_pairs[position].rowid;
                    match source.fetch(expected, true)? {
                        PairFetch::Payload(value, created_at, updated_at) => {
                            **pair_cursor += 1;
                            from_chunk = Some((value, created_at, updated_at));
                        }
                        PairFetch::Missing => {
                            return Err(corrupt(StoreRecordKindV1::Item));
                        }
                        PairFetch::Unavailable => {
                            // Retired or no pair for this position: the
                            // original point selector takes over.
                        }
                    }
                }
            }
            match from_chunk {
                Some(triple) => triple,
                None => {
                    let reload_sql = "SELECT value_json, created_at_ms, updated_at_ms FROM v2_item_slots WHERE attempt_id=?1 AND item_id=?2";
                    let (value, created_at, updated_at) = accounted_query_row(
                        connection,
                        reload_sql,
                        params![attempt.attempt_id().as_str(), item.id().as_str()],
                        |row| {
                            Ok((
                                row.get::<_, Option<String>>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
                    #[cfg(any(test, feature = "test-support"))]
                    if let Some(raw) = value.as_ref() {
                        crate::query_accounting::ledger::acquire_raw(raw.len());
                    }
                    (value, created_at, updated_at)
                }
            }
        } else {
            (
                loaded_value,
                created_at.ok_or_else(|| corrupt(StoreRecordKindV1::Item))?,
                updated_at.ok_or_else(|| corrupt(StoreRecordKindV1::Item))?,
            )
        };
        // The consuming map closure preserves the original drop behavior: the
        // raw serialized payload is dropped when the decode consumes it.
        #[cfg(any(test, feature = "test-support"))]
        let reads001_raw_len = value.as_ref().map_or(0, |raw| raw.len());
        let decoded_value = value
            .map(|value| {
                let decoded = decode_item_value_v2(&value, item_type);
                #[cfg(any(test, feature = "test-support"))]
                {
                    // Count every decode attempt; account a successfully
                    // decoded payload's logical bytes only when one exists,
                    // while the raw string is still alive so the combined
                    // peak observes the true overlap. The consumed raw bytes
                    // are released on both outcomes.
                    crate::query_accounting::ledger::record_decode_attempt();
                    if decoded.is_ok() {
                        crate::query_accounting::ledger::record_decoded_bytes(reads001_raw_len);
                    }
                    crate::query_accounting::ledger::release_raw(reads001_raw_len);
                }
                decoded
            })
            .transpose()?;
        let slot = ItemSlotStateV2::new(
            attempt.attempt_id().clone(),
            item.id().clone(),
            item_type,
            revision,
            decoded_value,
            UnixMillis::new(persisted_u64(created_at, StoreRecordKindV1::Item)?),
            UnixMillis::new(persisted_u64(updated_at, StoreRecordKindV1::Item)?),
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
        #[cfg(any(test, feature = "test-support"))]
        if let Some(pointer) = crate::query_accounting::ledger::payload_pointer(&slot) {
            crate::query_accounting::ledger::register_payload_pointer(pointer, reads001_raw_len);
        }
        slots.push(slot);
    }
    if loaded.is_empty() {
        Ok(slots)
    } else {
        Err(corrupt(StoreRecordKindV1::Item))
    }
}

/// Mixed-path validation for one discovered owner, sharing the single
/// spec-position validator. Truncated owners release their retained
/// scratch at turn start and take the original per-owner acquisition;
/// failing owners replay their staged cells row by row (payload before
/// the deferred fault for new owners; header-only for cached owners);
/// valid owners materialize from compact stamps — cached owners
/// header-only with chunk payloads at spec positions, new owners
/// row-major with payloads during collection. A chunk TooBig retires the
/// stream: cached owners keep validated slots and point-reload the
/// remaining changed positions; a new owner drops its uncommitted raw
/// buffer and original-cold-collects the whole owner before any spec
/// check. Already-committed owners are never replayed.
#[allow(clippy::too_many_arguments)]
fn load_attempt_memory_mixed_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    metadata: &AttemptMetadataV2,
    specification: &[ItemSpecV2],
    cached: Option<&AttemptWorkflowMemoryV2>,
    owner: &mut OwnerDiscovery,
    source: &mut dyn PairSource,
    blocker_family: &mut impl ReadFamily<i64>,
    evidence_family: &mut impl ReadFamily<i64>,
    owner_pairs: &[PairEntry],
) -> Result<AttemptWorkflowMemoryV2, StoreErrorV1> {
    #[cfg(any(test, feature = "test-support"))]
    let _reads001_slot_scope = crate::query_accounting::RawSlotScope::enter();
    let mut loaded: BTreeMap<String, SlotRow> = BTreeMap::new();
    let mut pair_cursor = 0usize;
    let cold_sql = "SELECT item_id, item_type, item_revision, value_json IS NOT NULL, created_at_ms, updated_at_ms, value_json \
         FROM v2_item_slots WHERE attempt_id = ?1";
    let header_sql = "SELECT item_id, item_type, item_revision, typeof(value_json) != 'null', NULL, NULL, NULL \
         FROM v2_item_slots WHERE attempt_id = ?1";
    if owner.truncated {
        // The conservative TooBig owner: retained scratch dies here (real
        // drop at its turn), and the original per-owner selector acquires
        // this owner in its original order.
        drop_retained_scratch(owner);
        collect_owner(
            connection,
            if cached.is_some() {
                header_sql
            } else {
                cold_sql
            },
            [attempt.attempt_id().as_str()],
            StoreRecordKindV1::Item,
            |row| collect_slot_row(row, &mut loaded),
        )?;
    } else if let Some(mut cells) = owner.raw.take() {
        // Failing owner: exact-position replay. Earlier rows convert fully
        // (a new owner's payloads first); the fault row re-raises its
        // captured error; a cached owner emits no point pairs on a header
        // collection fault.
        let mut fault_row = usize::MAX;
        let mut duplicate = false;
        let fault_error = owner.fault.as_ref().map(|stepping| {
            fault_row = stepping.row;
            duplicate = stepping.duplicate;
            stepping.error.clone()
        });
        let spec_fault = fault_error.is_none();
        let replay_end = if fault_row == usize::MAX {
            cells.len()
        } else if duplicate {
            fault_row + 1
        } else {
            fault_row
        };
        for index in 0..replay_end {
            let cell = &mut cells[index];
            let (item_id, item_type) = materialize_header_cell(cell)?;
            if cached.is_some() {
                insert_slot_row(
                    &mut loaded,
                    (
                        item_id,
                        item_type,
                        cell.revision,
                        cell.has_value,
                        None,
                        None,
                        None,
                    ),
                )?;
            } else if pair_cursor < owner_pairs.len()
                && owner_pairs[pair_cursor].rowid == cell.rowid
            {
                match source.fetch(cell.rowid, false)? {
                    PairFetch::Payload(value, created_at, updated_at) => {
                        pair_cursor += 1;
                        insert_slot_row(
                            &mut loaded,
                            (
                                item_id,
                                item_type,
                                cell.revision,
                                cell.has_value,
                                Some(created_at),
                                Some(updated_at),
                                value,
                            ),
                        )?;
                    }
                    PairFetch::Missing => return Err(corrupt(StoreRecordKindV1::Item)),
                    PairFetch::Unavailable => {
                        // New-owner retirement mid-collection: drop the
                        // uncommitted raw buffer with real releases, then
                        // original-cold-collect the entire owner before
                        // any spec check.
                        clear_partial_loaded(&mut loaded);
                        #[cfg(any(test, feature = "test-support"))]
                        crate::query_accounting::ledger::release_identity(
                            item_id.len() + item_type.len(),
                        );
                        drop((item_id, item_type));
                        drop_header_cells(&mut cells);
                        owner.stamps.clear();
                        collect_owner(
                            connection,
                            cold_sql,
                            [attempt.attempt_id().as_str()],
                            StoreRecordKindV1::Item,
                            |row| collect_slot_row(row, &mut loaded),
                        )?;
                        pair_cursor = owner_pairs.len();
                        break;
                    }
                }
            } else {
                // A row without a queued pair sits past the prefix that
                // today's loader would ever reach.
                break;
            }
        }
        if let Some(error) = fault_error.filter(|_| !duplicate) {
            return Err(error);
        }
        // A duplicate's payload converted before its map insert; the
        // natural insert failure inside the replay loop has already
        // replayed that collection fault. Spec and leftover faults replay
        // naturally inside the spec-position validator below.
        let _ = spec_fault;
    } else {
        // Valid owner: materialize from compact stamps. Cached owners
        // carry header-only rows (payloads arrive at spec positions); new
        // owners replay row-major with their payloads during collection.
        if cached.is_some() {
            for stamp in &owner.stamps {
                let item = &specification[stamp.spec_idx];
                let item_id = item.id().as_str().to_owned();
                let item_type = item_type_text(item.item_type()).to_owned();
                #[cfg(any(test, feature = "test-support"))]
                crate::query_accounting::ledger::acquire_identity(item_id.len() + item_type.len());
                insert_slot_row(
                    &mut loaded,
                    (
                        item_id,
                        item_type,
                        stamp.revision,
                        stamp.has_value,
                        None,
                        None,
                        None,
                    ),
                )?;
            }
        } else {
            for stamp in &owner.stamps {
                let item = &specification[stamp.spec_idx];
                let item_id = item.id().as_str().to_owned();
                let item_type = item_type_text(item.item_type()).to_owned();
                #[cfg(any(test, feature = "test-support"))]
                crate::query_accounting::ledger::acquire_identity(item_id.len() + item_type.len());
                if pair_cursor < owner_pairs.len() && owner_pairs[pair_cursor].rowid == stamp.rowid
                {
                    match source.fetch(stamp.rowid, false)? {
                        PairFetch::Payload(value, created_at, updated_at) => {
                            pair_cursor += 1;
                            insert_slot_row(
                                &mut loaded,
                                (
                                    item_id,
                                    item_type,
                                    stamp.revision,
                                    stamp.has_value,
                                    Some(created_at),
                                    Some(updated_at),
                                    value,
                                ),
                            )?;
                        }
                        PairFetch::Missing => return Err(corrupt(StoreRecordKindV1::Item)),
                        PairFetch::Unavailable => {
                            clear_partial_loaded(&mut loaded);
                            #[cfg(any(test, feature = "test-support"))]
                            crate::query_accounting::ledger::release_identity(
                                item_id.len() + item_type.len(),
                            );
                            drop((item_id, item_type));
                            collect_owner(
                                connection,
                                cold_sql,
                                [attempt.attempt_id().as_str()],
                                StoreRecordKindV1::Item,
                                |row| collect_slot_row(row, &mut loaded),
                            )?;
                            pair_cursor = owner_pairs.len();
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }
    }
    let slots = validate_attempt_slots(
        connection,
        attempt,
        specification,
        cached,
        &mut loaded,
        Some((source, &mut pair_cursor)),
        owner_pairs,
    )?;
    let blockers = load_blockers_v2(connection, attempt, metadata, blocker_family)?;
    let evidence = load_evidence_v2(connection, attempt, evidence_family)?;
    let memory =
        AttemptWorkflowMemoryV2::new(attempt.attempt_id().clone(), slots, blockers, evidence)
            .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
    #[cfg(any(test, feature = "test-support"))]
    crate::query_accounting::ledger::commit_slot_scope();
    Ok(memory)
}

/// Physically drops one owner's retained discovery scratch at a real drop
/// point (its validation turn or an original-path reacquisition) and books
/// the release: the bytes leave memory exactly where the balance does.
fn drop_retained_scratch(owner: &mut OwnerDiscovery) {
    if let Some(mut cells) = owner.raw.take() {
        drop_header_cells(&mut cells);
    }
}

fn drop_header_cells(cells: &mut Vec<RawHeaderCell>) {
    let retained = std::mem::take(cells);
    #[cfg(any(test, feature = "test-support"))]
    for cell in &retained {
        crate::query_accounting::ledger::release_discovery_scratch(cell.scratch_bytes());
    }
    drop(retained);
}

/// Converts one replayed header cell's proven-valid bytes, booking identity
/// at materialization and releasing its retained scratch bytes.
fn materialize_header_cell(cell: &mut RawHeaderCell) -> Result<(String, String), StoreErrorV1> {
    let id_bytes = std::mem::take(&mut cell.id);
    let type_bytes = std::mem::take(&mut cell.item_type);
    #[cfg(any(test, feature = "test-support"))]
    let scratch_bytes = id_bytes.len() + type_bytes.len();
    let item_id = String::from_utf8(id_bytes).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
    let item_type = String::from_utf8(type_bytes).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
    #[cfg(any(test, feature = "test-support"))]
    {
        crate::query_accounting::ledger::acquire_identity(item_id.len() + item_type.len());
        crate::query_accounting::ledger::release_discovery_scratch(scratch_bytes);
    }
    Ok((item_id, item_type))
}

/// Releases a partial uncommitted collection's raw and identity bytes with
/// real release events (no counter subtraction) before an original-path
/// re-acquisition.
fn clear_partial_loaded(loaded: &mut BTreeMap<String, SlotRow>) {
    #[cfg(any(test, feature = "test-support"))]
    for (_, row) in loaded.iter() {
        if let Some(value) = &row.6 {
            crate::query_accounting::ledger::release_raw(value.len());
        }
        crate::query_accounting::ledger::release_identity(row.0.len() * 2 + row.1.len());
    }
    loaded.clear();
}

fn load_attempt_memory_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    metadata: &AttemptMetadataV2,
    specification: &[ItemSpecV2],
    cached: Option<&AttemptWorkflowMemoryV2>,
    bulk_slots: bool,
    families: (
        &mut impl ReadFamily<i64>,
        &mut impl ReadFamily<i64>,
        &mut impl ReadFamily<i64>,
    ),
) -> Result<AttemptWorkflowMemoryV2, StoreErrorV1> {
    let (slot_family, blocker_family, evidence_family) = families;
    #[cfg(any(test, feature = "test-support"))]
    let _reads001_slot_scope = crate::query_accounting::RawSlotScope::enter();
    let slots_sql = if cached.is_some() {
        "SELECT item_id, item_type, item_revision, typeof(value_json) != 'null', NULL, NULL, NULL \
         FROM v2_item_slots WHERE attempt_id = ?1"
    } else {
        "SELECT item_id, item_type, item_revision, value_json IS NOT NULL, created_at_ms, updated_at_ms, value_json \
         FROM v2_item_slots WHERE attempt_id = ?1"
    };
    let mut loaded = BTreeMap::new();
    let acquired = if bulk_slots {
        slot_family
            .collect(
                sqlite_u64(attempt.trace().get(), "Procedure v2 attempt trace")?,
                |row| collect_slot_row(row, &mut loaded),
            )?
            .is_some()
    } else {
        false
    };
    if !acquired {
        // No slot has been decoded yet. Retiring an unconfirmed collection
        // discards only raw/identity ownership; acquisition counts stay intact.
        #[cfg(any(test, feature = "test-support"))]
        for row in loaded.values() {
            if let Some(value) = &row.6 {
                crate::query_accounting::ledger::release_raw(value.len());
            }
            crate::query_accounting::ledger::release_identity(row.0.len() * 2 + row.1.len());
        }
        loaded.clear();
        collect_owner(
            connection,
            slots_sql,
            [attempt.attempt_id().as_str()],
            StoreRecordKindV1::Item,
            |row| collect_slot_row(row, &mut loaded),
        )?;
    }
    let slots = validate_attempt_slots(
        connection,
        attempt,
        specification,
        cached,
        &mut loaded,
        None,
        &[],
    )?;
    let blockers = load_blockers_v2(connection, attempt, metadata, blocker_family)?;
    let evidence = load_evidence_v2(connection, attempt, evidence_family)?;
    let memory =
        AttemptWorkflowMemoryV2::new(attempt.attempt_id().clone(), slots, blockers, evidence)
            .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
    #[cfg(any(test, feature = "test-support"))]
    crate::query_accounting::ledger::commit_slot_scope();
    Ok(memory)
}

fn collect_slot_row(
    row: &rusqlite::Row<'_>,
    loaded: &mut BTreeMap<String, SlotRow>,
) -> Result<(), StoreErrorV1> {
    let convert = || -> rusqlite::Result<SlotRow> {
        let item_id: String = row.get(0)?;
        let item_type: String = row.get(1)?;
        #[cfg(any(test, feature = "test-support"))]
        crate::query_accounting::ledger::acquire_identity(item_id.len() + item_type.len());
        Ok((
            item_id,
            item_type,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
        ))
    };
    insert_slot_row(
        loaded,
        convert().map_err(|error| record_error(error, StoreRecordKindV1::Item))?,
    )
}

fn insert_slot_row(
    loaded: &mut BTreeMap<String, SlotRow>,
    row: SlotRow,
) -> Result<(), StoreErrorV1> {
    #[cfg(any(test, feature = "test-support"))]
    {
        if let Some(raw) = &row.6 {
            crate::query_accounting::ledger::acquire_raw(raw.len());
        }
        crate::query_accounting::ledger::acquire_identity_copy(row.0.len());
    }
    if loaded.insert(row.0.clone(), row).is_some() {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    Ok(())
}

fn load_blockers_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    _metadata: &AttemptMetadataV2,
    family: &mut impl ReadFamily<i64>,
) -> Result<Vec<BlockerStateV2>, StoreErrorV1> {
    let blockers_sql = "SELECT blocker_id, reason, state, created_at_ms, resolved_at_ms FROM v2_blockers \
         WHERE attempt_id = ?1 ORDER BY created_at_ms, blocker_id";
    let read = |row: &rusqlite::Row<'_>| {
        let (id, reason, state, created, resolved) = (|| {
            Ok::<_, rusqlite::Error>((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })()
        .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
        BlockerStateV2::new(
            BlockerId::new(id).map_err(|_| corrupt(StoreRecordKindV1::Blocker))?,
            attempt.attempt_id().clone(),
            reason,
            match state.as_str() {
                "open" => BlockerState::Open,
                "resolved" => BlockerState::Resolved,
                _ => return Err(corrupt(StoreRecordKindV1::Blocker)),
            },
            UnixMillis::new(persisted_u64(created, StoreRecordKindV1::Blocker)?),
            resolved
                .map(|value| persisted_u64(value, StoreRecordKindV1::Blocker).map(UnixMillis::new))
                .transpose()?,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Blocker))
    };
    if let Some(values) = family.collect(
        sqlite_u64(attempt.trace().get(), "Procedure v2 attempt trace")?,
        &read,
    )? {
        Ok(values)
    } else {
        collect_owner(
            connection,
            blockers_sql,
            [attempt.attempt_id().as_str()],
            StoreRecordKindV1::Blocker,
            read,
        )
    }
}

fn load_evidence_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    family: &mut impl ReadFamily<i64>,
) -> Result<Vec<EvidenceResolutionStateV2>, StoreErrorV1> {
    type EvidenceRow = (
        String,
        i64,
        i64,
        String,
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
    );
    let evidence_sql = "SELECT source_graph_node_id, reference_ordinal, required, selected_item_ids_json, state, \
         source_attempt_id, source_attempt_number, items_digest, resolved_at_ms \
         FROM v2_resolved_evidence_references WHERE attempt_id = ?1 ORDER BY reference_ordinal";
    let read = |value: &rusqlite::Row<'_>| {
        let row: EvidenceRow = (|| {
            Ok::<_, rusqlite::Error>((
                value.get(0)?,
                value.get(1)?,
                value.get(2)?,
                value.get(3)?,
                value.get(4)?,
                value.get(5)?,
                value.get(6)?,
                value.get(7)?,
                value.get(8)?,
            ))
        })()
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        verify_canonical_json_v1(row.3.as_bytes())
            .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let selected: Vec<String> =
            serde_json::from_str(&row.3).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let selected = selected
            .into_iter()
            .map(|id| ItemId::new(id).map_err(|_| corrupt(StoreRecordKindV1::Attempt)))
            .collect::<Result<Vec<_>, _>>()?;
        let source_node =
            GraphNodeId::new(row.0).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let resolution = match row.4.as_str() {
            "unresolved"
                if row.5.is_none() && row.6.is_none() && row.7.is_none() && row.8.is_none() =>
            {
                ResolvedEvidenceReferenceV2::unresolved(source_node)
            }
            "resolved" | "skipped" => {
                let snapshot = EvidenceReferenceSnapshotV2::new(
                    source_node,
                    AttemptId::new(row.5.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?)
                        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
                    AttemptNumberV2::new(persisted_u64(
                        row.6.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?,
                        StoreRecordKindV1::Attempt,
                    )?),
                    Sha256Digest::new(row.7.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?)
                        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
                    UnixMillis::new(persisted_u64(
                        row.8.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?,
                        StoreRecordKindV1::Attempt,
                    )?),
                )
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
                if row.4 == "resolved" {
                    ResolvedEvidenceReferenceV2::resolved(snapshot)
                } else {
                    ResolvedEvidenceReferenceV2::skipped(snapshot)
                }
            }
            _ => return Err(corrupt(StoreRecordKindV1::Attempt)),
        };
        EvidenceResolutionStateV2::new(
            u32::try_from(row.1).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            match row.2 {
                0 => false,
                1 => true,
                _ => return Err(corrupt(StoreRecordKindV1::Attempt)),
            },
            selected,
            resolution,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    };
    if let Some(values) = family.collect(
        sqlite_u64(attempt.trace().get(), "Procedure v2 attempt trace")?,
        &read,
    )? {
        Ok(values)
    } else {
        collect_owner(
            connection,
            evidence_sql,
            [attempt.attempt_id().as_str()],
            StoreRecordKindV1::Attempt,
            read,
        )
    }
}

fn load_decisions_v2(
    connection: &Connection,
    _snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    attempts: &[AttemptWorkflowMemoryV2],
) -> Result<Vec<DecisionRecordV2>, StoreErrorV1> {
    type DecisionRow = (
        String,
        String,
        i64,
        i64,
        String,
        String,
        String,
        String,
        i64,
        Option<i64>,
        String,
        String,
        String,
        String,
        Option<String>,
        i64,
    );
    let memory_by_id: BTreeMap<_, _> = attempts
        .iter()
        .map(|memory| (memory.attempt_id(), memory))
        .collect();
    let decisions_sql = "SELECT attempt_id, session_id, trace_sequence, session_revision, procedure_snapshot_id, \
         procedure_digest, graph_node_id, node_definition_id, attempt_number, goal_revision, \
         selected_option_id, route_effect, route_target_graph_node_id, reason, actor, recorded_at_ms \
         FROM v2_decision_records WHERE session_id = ?1 ORDER BY trace_sequence";
    let mut statement = accounted_prepare(connection, decisions_sql)
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = accounted_query_map_prepared(
        &mut statement,
        decisions_sql,
        [trace.session_id().as_str()],
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
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
                row.get(14)?,
                row.get(15)?,
            ))
        },
    )
    .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    rows.map(|row| {
        let row: DecisionRow =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        let attempt_id = AttemptId::new(row.0).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let evidence = ResolvedEvidenceSetV2::new(
            memory_by_id
                .get(&attempt_id)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?
                .evidence()
                .iter()
                .map(|reference| reference.resolution().clone())
                .collect(),
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        DecisionRecordV2::new(DecisionRecordInputV2 {
            attempt_id,
            session_id: SessionId::new(row.1).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            trace: TraceSequenceV2::new(persisted_u64(row.2, StoreRecordKindV1::Attempt)?),
            session_revision: Revision::new(persisted_u64(row.3, StoreRecordKindV1::Attempt)?),
            procedure_snapshot_id: ProcedureSnapshotId::new(row.4)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            procedure_digest: Sha256Digest::new(row.5)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            graph_node_id: GraphNodeId::new(row.6)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            node_definition_id: NodeDefinitionId::new(row.7)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            attempt_number: AttemptNumberV2::new(persisted_u64(row.8, StoreRecordKindV1::Attempt)?),
            goal_revision: row
                .9
                .map(|value| {
                    persisted_u64(value, StoreRecordKindV1::Attempt).map(GoalRevisionNumberV2::new)
                })
                .transpose()?,
            selected_option: OptionId::new(row.10)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            route_effect: TransitionEffectV2::from_str(&row.11)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            route_target: GraphNodeId::new(row.12)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            reason: ReasonV2::new(row.13).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            evidence,
            actor: row
                .14
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            recorded_at: UnixMillis::new(persisted_u64(row.15, StoreRecordKindV1::Attempt)?),
        })
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    })
    .collect()
}

fn load_reworks_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Vec<ReworkRecordV2>, StoreErrorV1> {
    type ReworkRow = (
        i64,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        i64,
    );
    let reworks_sql = "SELECT trace_sequence, kind, from_graph_node_id, to_graph_node_id, target_attempt_id, \
         reason, reactivated, actor, recorded_at_ms FROM v2_rework_records \
         WHERE session_id = ?1 ORDER BY trace_sequence";
    let mut statement = accounted_prepare(connection, reworks_sql)
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows =
        accounted_query_map_prepared(&mut statement, reworks_sql, [session_id.as_str()], |row| {
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
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    rows.map(|row| {
        let row: ReworkRow =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        ReworkRecordV2::new(ReworkRecordInputV2 {
            trace: TraceSequenceV2::new(persisted_u64(row.0, StoreRecordKindV1::Attempt)?),
            kind: ReworkKindV2::from_str(&row.1)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            from_node: GraphNodeId::new(row.2).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            to_node: GraphNodeId::new(row.3).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            target_attempt_id: AttemptId::new(row.4)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            reason: ReasonV2::new(row.5).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            reactivated: match row.6 {
                0 => false,
                1 => true,
                _ => return Err(corrupt(StoreRecordKindV1::Attempt)),
            },
            actor: row
                .7
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            recorded_at: UnixMillis::new(persisted_u64(row.8, StoreRecordKindV1::Attempt)?),
        })
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    })
    .collect()
}

#[cfg(test)]
mod reads_tests {
    use super::*;

    #[test]
    fn duplicate_slot_acquires_payload_before_insert_refusal() {
        let accountant = crate::test_support::query_account::QueryAccountant::start();
        {
            let _scope = crate::query_accounting::RawSlotScope::enter();
            let mut rows = BTreeMap::new();
            let slot = |value: &str| {
                (
                    "item".to_owned(),
                    "text".to_owned(),
                    1,
                    true,
                    Some(1),
                    Some(1),
                    Some(value.to_owned()),
                )
            };
            insert_slot_row(&mut rows, slot("first")).unwrap();
            assert_eq!(
                insert_slot_row(&mut rows, slot("second")),
                Err(corrupt(StoreRecordKindV1::Item))
            );
            assert_eq!(rows["item"].6.as_deref(), Some("second"));
        }
        let report = accountant.finish();
        assert_eq!(report.retention.raw_window_peak, 11);
        assert_eq!(report.retention.raw_released_at_scope_end, 11);
    }

    #[test]
    fn state003_attempt_index_keeps_first_match_for_duplicate_ids() {
        let id = AttemptId::new("11111111-1111-1111-1111-111111111111").unwrap();
        let probe = id.clone();
        let first = AttemptWorkflowMemoryV2::new(id, Vec::new(), Vec::new(), Vec::new()).unwrap();
        let second =
            AttemptWorkflowMemoryV2::new(probe.clone(), Vec::new(), Vec::new(), Vec::new())
                .unwrap();
        let attempts = [first, second];
        let index = index_cached_attempts(&attempts[..]);
        assert_eq!(index.len(), 1);
        assert!(std::ptr::eq(
            index.get(&probe).copied().unwrap(),
            &attempts[0]
        ));
    }
}
