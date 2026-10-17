//! Procedure v2 attempt-local workflow-memory persistence.

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
use rusqlite::{Connection, Transaction, params};
use serde_json::Value;

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
    let mut attempts = Vec::with_capacity(trace.attempts().len());
    for (attempt, attempt_metadata) in trace.attempts().iter().zip(metadata) {
        attempts.push(load_attempt_memory_v2(
            connection,
            attempt,
            attempt_metadata,
            snapshot
                .item_declarations_v2(attempt.graph_node_id())
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            cached.and_then(|memory| {
                memory
                    .attempts()
                    .iter()
                    .find(|memory| memory.attempt_id() == attempt.attempt_id())
            }),
        )?);
    }
    let decisions = load_decisions_v2(connection, snapshot, trace, &attempts)?;
    let reworks = load_reworks_v2(connection, trace.session_id())?;
    WorkflowMemoryStateV2::new(attempts, decisions, reworks)
        .map_err(|_| corrupt(StoreRecordKindV1::Session))
}

fn load_attempt_memory_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    metadata: &AttemptMetadataV2,
    specification: &[ItemSpecV2],
    cached: Option<&AttemptWorkflowMemoryV2>,
) -> Result<AttemptWorkflowMemoryV2, StoreErrorV1> {
    type SlotRow = (
        String,
        String,
        i64,
        bool,
        Option<i64>,
        Option<i64>,
        Option<String>,
    );
    let mut statement = connection
        .prepare(if cached.is_some() {
            "SELECT item_id, item_type, item_revision, typeof(value_json) != 'null', NULL, NULL, NULL \
             FROM v2_item_slots WHERE attempt_id = ?1"
        } else {
            "SELECT item_id, item_type, item_revision, value_json IS NOT NULL, created_at_ms, updated_at_ms, value_json \
             FROM v2_item_slots WHERE attempt_id = ?1"
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    let rows = statement
        .query_map([attempt.attempt_id().as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    let mut loaded = BTreeMap::new();
    for row in rows {
        let row: SlotRow = row.map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
        if loaded.insert(row.0.clone(), row).is_some() {
            return Err(corrupt(StoreRecordKindV1::Item));
        }
    }
    let mut slots = Vec::with_capacity(specification.len());
    for item in specification {
        let (_, item_type, revision, has_value, created_at, updated_at, loaded_value) = loaded
            .remove(item.id().as_str())
            .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
        let item_type =
            parse_item_type(&item_type).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
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
            slots.push(slot.clone());
            continue;
        }
        let (value, created_at, updated_at) = if cached.is_some() {
            let (value, created_at, updated_at) = connection
                .query_row(
                    "SELECT value_json, created_at_ms, updated_at_ms FROM v2_item_slots WHERE attempt_id=?1 AND item_id=?2",
                    params![attempt.attempt_id().as_str(), item.id().as_str()],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
                )
                .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
            (value, created_at, updated_at)
        } else {
            (
                loaded_value,
                created_at.ok_or_else(|| corrupt(StoreRecordKindV1::Item))?,
                updated_at.ok_or_else(|| corrupt(StoreRecordKindV1::Item))?,
            )
        };
        slots.push(
            ItemSlotStateV2::new(
                attempt.attempt_id().clone(),
                item.id().clone(),
                item_type,
                revision,
                value
                    .map(|value| decode_item_value_v2(&value, item_type))
                    .transpose()?,
                UnixMillis::new(persisted_u64(created_at, StoreRecordKindV1::Item)?),
                UnixMillis::new(persisted_u64(updated_at, StoreRecordKindV1::Item)?),
            )
            .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
        );
    }
    if !loaded.is_empty() {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    let blockers = load_blockers_v2(connection, attempt, metadata)?;
    let evidence = load_evidence_v2(connection, attempt)?;
    AttemptWorkflowMemoryV2::new(attempt.attempt_id().clone(), slots, blockers, evidence)
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
}

fn load_blockers_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    _metadata: &AttemptMetadataV2,
) -> Result<Vec<BlockerStateV2>, StoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT blocker_id, reason, state, created_at_ms, resolved_at_ms FROM v2_blockers \
         WHERE attempt_id = ?1 ORDER BY created_at_ms, blocker_id",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
    let rows = statement
        .query_map([attempt.attempt_id().as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
    rows.map(|row| {
        let (id, reason, state, created, resolved) =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
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
    })
    .collect()
}

fn load_evidence_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
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
    let mut statement = connection.prepare(
        "SELECT source_graph_node_id, reference_ordinal, required, selected_item_ids_json, state, \
         source_attempt_id, source_attempt_number, items_digest, resolved_at_ms \
         FROM v2_resolved_evidence_references WHERE attempt_id = ?1 ORDER BY reference_ordinal",
    ).map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([attempt.attempt_id().as_str()], |row| {
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
        let row: EvidenceRow =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
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
    })
    .collect()
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
    let mut statement = connection.prepare(
        "SELECT attempt_id, session_id, trace_sequence, session_revision, procedure_snapshot_id, \
         procedure_digest, graph_node_id, node_definition_id, attempt_number, goal_revision, \
         selected_option_id, route_effect, route_target_graph_node_id, reason, actor, recorded_at_ms \
         FROM v2_decision_records WHERE session_id = ?1 ORDER BY trace_sequence",
    ).map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([trace.session_id().as_str()], |row| {
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
        })
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
    let mut statement = connection
        .prepare(
            "SELECT trace_sequence, kind, from_graph_node_id, to_graph_node_id, target_attempt_id, \
         reason, reactivated, actor, recorded_at_ms FROM v2_rework_records \
         WHERE session_id = ?1 ORDER BY trace_sequence",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
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
