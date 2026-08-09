//! Authoritative v1 normalized session-row persistence and hydration.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};

use podway_core::{
    ArtifactValueV1, AttemptId, AttemptInputV1, AttemptLifecycle, AttemptV1, BlockerId,
    BlockerInputV1, BlockerState, BlockerV1, CanonicalProcedureJsonV1,
    CanonicalProcedureSnapshotInputV1, ItemSlotInputV1, ItemSlotV1, ItemTypeV1, ItemValueV1,
    ProcedureSnapshotId, ProcedureSnapshotV1, ProcedureSourceKindV1, ProcedureSourceLabelV1,
    Revision, SessionAggregateInputV1, SessionAggregateV1, SessionId, SessionLifecycle,
    SessionState, Sha256Digest, StageId, StageProgressState, StageProgressV1, UnixMillis,
    WorkspaceState,
};

use crate::{RusqliteErrorContextV1, StoreErrorV1, StoreRecordKindV1, map_rusqlite_error_v1};

pub(crate) fn load_workspace_state(
    transaction: &Transaction<'_>,
    workspace_id: podway_core::WorkspaceId,
) -> Result<WorkspaceState, StoreErrorV1> {
    let session = load_current_session(transaction)?;
    let revision = session
        .as_ref()
        .map_or(Revision::ZERO, SessionAggregateV1::revision);
    let session = session
        .as_ref()
        .map(|aggregate| {
            SessionState::new(
                aggregate.session_id().clone(),
                aggregate.lifecycle(),
                aggregate.revision(),
                aggregate.active_stage_id().cloned(),
                aggregate.active_attempt_id().cloned(),
            )
        })
        .transpose()
        .map_err(|_| corrupt(StoreRecordKindV1::Session))?;
    WorkspaceState::new(workspace_id, revision, session)
        .map_err(|_| corrupt(StoreRecordKindV1::Workspace))
}

pub(crate) fn load_current_session(
    transaction: &Connection,
) -> Result<Option<SessionAggregateV1>, StoreErrorV1> {
    let session = transaction
        .query_row(
            "SELECT session_id, task_title, procedure_snapshot_id, lifecycle, session_revision, \
             active_stage_id, active_attempt_id, created_at_ms, completed_at_ms, cancelled_at_ms, \
             cancel_reason FROM task_sessions WHERE singleton = 1",
            [],
            |row| {
                Ok(SessionRow {
                    session_id: row.get(0)?,
                    task_title: row.get(1)?,
                    snapshot_id: row.get(2)?,
                    lifecycle: row.get(3)?,
                    revision: row.get(4)?,
                    active_stage_id: row.get(5)?,
                    active_attempt_id: row.get(6)?,
                    created_at: row.get(7)?,
                    completed_at: row.get(8)?,
                    cancelled_at: row.get(9)?,
                    cancel_reason: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|error| storage(error, StoreRecordKindV1::Session))?;
    let Some(session) = session else {
        return Ok(None);
    };

    let session_id = session_id(session.session_id)?;
    let snapshot = load_snapshot(transaction, &session.snapshot_id)?;
    let progress = load_progress(transaction, &session_id)?;
    let attempts = load_attempts(transaction, &session_id, &snapshot)?;
    let aggregate = SessionAggregateV1::new(SessionAggregateInputV1 {
        session_id,
        task_title: session.task_title,
        snapshot,
        lifecycle: session_lifecycle(&session.lifecycle)?,
        revision: revision(session.revision, StoreRecordKindV1::Session)?,
        stage_progress: progress,
        attempts,
        active_stage_id: session.active_stage_id.map(stage_id).transpose()?,
        active_attempt_id: session.active_attempt_id.map(attempt_id).transpose()?,
        created_at: timestamp(session.created_at, StoreRecordKindV1::Session)?,
        completed_at: session
            .completed_at
            .map(|value| timestamp(value, StoreRecordKindV1::Session))
            .transpose()?,
        cancelled_at: session
            .cancelled_at
            .map(|value| timestamp(value, StoreRecordKindV1::Session))
            .transpose()?,
        cancel_reason: session.cancel_reason,
    })
    .map_err(|_| corrupt(StoreRecordKindV1::Session))?;
    Ok(Some(aggregate))
}

pub(crate) fn replace_current_session(
    transaction: &Transaction<'_>,
    aggregate: &SessionAggregateV1,
) -> Result<(), StoreErrorV1> {
    persist_snapshot(transaction, aggregate.snapshot())?;
    transaction
        .execute("DELETE FROM task_sessions WHERE singleton = 1", [])
        .map_err(|error| storage(error, StoreRecordKindV1::Session))?;
    let (completed_at, cancelled_at, cancel_reason) = match aggregate.lifecycle() {
        SessionLifecycle::Running => (None, None, None),
        SessionLifecycle::Completed => (
            Some(sqlite_u64(
                aggregate
                    .completed_at()
                    .ok_or_else(|| corrupt(StoreRecordKindV1::Session))?
                    .get(),
                "session completion timestamp",
            )?),
            None,
            None,
        ),
        SessionLifecycle::Cancelled => (
            None,
            Some(sqlite_u64(
                aggregate
                    .cancelled_at()
                    .ok_or_else(|| corrupt(StoreRecordKindV1::Session))?
                    .get(),
                "session cancellation timestamp",
            )?),
            aggregate.cancel_reason(),
        ),
    };
    transaction.execute(
        "INSERT INTO task_sessions (singleton, session_id, task_title, procedure_snapshot_id, lifecycle, \
         session_revision, active_stage_id, active_attempt_id, created_at_ms, completed_at_ms, \
         cancelled_at_ms, cancel_reason) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            aggregate.session_id().as_str(),
            aggregate.task_title(),
            aggregate.snapshot().snapshot_id().as_str(),
            session_lifecycle_name(aggregate.lifecycle()),
            sqlite_u64(aggregate.revision().get(), "session revision")?,
            aggregate.active_stage_id().map(StageId::as_str),
            aggregate.active_attempt_id().map(AttemptId::as_str),
            sqlite_u64(aggregate.created_at().get(), "session creation timestamp")?,
            completed_at,
            cancelled_at,
            cancel_reason,
        ],
    ).map_err(|error| storage(error, StoreRecordKindV1::Session))?;

    for progress in aggregate.stage_progress() {
        transaction
            .execute(
                "INSERT INTO stage_progress (session_id, stage_id, stage_index, progress_state, \
             latest_attempt_number, latest_attempt_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    aggregate.session_id().as_str(),
                    progress.stage_id().as_str(),
                    sqlite_usize(progress.stage_index(), "session stage index")?,
                    progress_state_name(progress.state()),
                    i64::from(progress.latest_attempt_number()),
                    progress.latest_attempt_id().map(AttemptId::as_str),
                ],
            )
            .map_err(|error| storage(error, StoreRecordKindV1::Session))?;
    }
    for attempt in aggregate.attempts() {
        transaction.execute(
            "INSERT INTO attempts (attempt_id, session_id, stage_id, attempt_number, lifecycle, \
             started_at_ms, ended_at_ms, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                attempt.attempt_id().as_str(),
                attempt.session_id().as_str(),
                attempt.stage_id().as_str(),
                i64::from(attempt.number()),
                attempt_lifecycle_name(attempt.lifecycle()),
                sqlite_u64(attempt.started_at().get(), "attempt start timestamp")?,
                attempt
                    .ended_at()
                    .map(|value| sqlite_u64(value.get(), "attempt end timestamp"))
                    .transpose()?,
                attempt.reason(),
            ],
        ).map_err(|error| storage(error, StoreRecordKindV1::Attempt))?;
        for slot in attempt.item_slots() {
            let (created_at, updated_at) = match (slot.created_at(), slot.updated_at()) {
                (Some(created_at), Some(updated_at)) => (created_at, updated_at),
                (None, None) if slot.revision() == Revision::ZERO && slot.value().is_none() => {
                    (attempt.started_at(), attempt.started_at())
                }
                _ => return Err(corrupt(StoreRecordKindV1::Item)),
            };
            transaction.execute(
                "INSERT INTO item_slots (attempt_id, item_id, item_type, item_revision, value_json, \
                 created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    slot.attempt_id().as_str(),
                    slot.item_id().as_str(),
                    item_type_name(slot.item_type()),
                    sqlite_u64(slot.revision().get(), "item revision")?,
                    slot.value().map(encode_item_value).transpose()?,
                    sqlite_u64(created_at.get(), "item creation timestamp")?,
                    sqlite_u64(updated_at.get(), "item update timestamp")?,
                ],
            ).map_err(|error| storage(error, StoreRecordKindV1::Item))?;
        }
        for blocker in attempt.blockers() {
            transaction.execute(
                "INSERT INTO blockers (blocker_id, attempt_id, reason, state, created_at_ms, resolved_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    blocker.blocker_id().as_str(),
                    blocker.attempt_id().as_str(),
                    blocker.reason(),
                    blocker_state_name(blocker.state()),
                    sqlite_u64(blocker.created_at().get(), "blocker creation timestamp")?,
                    blocker
                        .resolved_at()
                        .map(|value| sqlite_u64(value.get(), "blocker resolution timestamp"))
                        .transpose()?,
                ],
            ).map_err(|error| storage(error, StoreRecordKindV1::Blocker))?;
        }
    }
    Ok(())
}

fn load_snapshot(
    transaction: &Connection,
    snapshot_id: &str,
) -> Result<ProcedureSnapshotV1, StoreErrorV1> {
    let snapshot = transaction
        .query_row(
            "SELECT snapshot_id, schema_id, procedure_id, procedure_version, name, digest, canonical_json, \
             source_kind, source_label, created_at_ms FROM procedure_snapshots WHERE snapshot_id = ?1",
            [snapshot_id],
            |row| Ok(SnapshotRow {
                snapshot_id: row.get(0)?, schema_id: row.get(1)?, procedure_id: row.get(2)?,
                procedure_version: row.get(3)?, name: row.get(4)?, digest: row.get(5)?,
                canonical_json: row.get(6)?, source_kind: row.get(7)?, source_label: row.get(8)?,
                created_at: row.get(9)?,
            }),
        )
        .optional()
        .map_err(|error| storage(error, StoreRecordKindV1::Snapshot))?
        .ok_or_else(|| corrupt(StoreRecordKindV1::Snapshot))?;
    let source_kind = ProcedureSourceKindV1::from_row_value(&snapshot.source_kind)
        .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?;
    ProcedureSnapshotV1::from_canonical_json(CanonicalProcedureSnapshotInputV1 {
        snapshot_id: ProcedureSnapshotId::new(snapshot.snapshot_id)
            .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        schema_id: snapshot.schema_id,
        procedure_id: snapshot.procedure_id,
        procedure_version: snapshot.procedure_version,
        name: snapshot.name,
        source_label: ProcedureSourceLabelV1::from_row(source_kind, snapshot.source_label)
            .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        canonical_json: CanonicalProcedureJsonV1::new(snapshot.canonical_json)
            .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        digest: Sha256Digest::new(snapshot.digest)
            .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?,
        created_at: timestamp(snapshot.created_at, StoreRecordKindV1::Snapshot)?,
    })
    .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))
}

pub(crate) fn persist_snapshot(
    transaction: &Transaction<'_>,
    snapshot: &ProcedureSnapshotV1,
) -> Result<(), StoreErrorV1> {
    let found: Option<String> = transaction
        .query_row(
            "SELECT snapshot_id FROM procedure_snapshots WHERE snapshot_id = ?1",
            [snapshot.snapshot_id().as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error, StoreRecordKindV1::Snapshot))?;
    if found.is_some() {
        let hydrated = load_snapshot(transaction, snapshot.snapshot_id().as_str())?;
        if hydrated != *snapshot {
            return Err(corrupt(StoreRecordKindV1::Snapshot));
        }
        return Ok(());
    }
    transaction.execute(
        "INSERT INTO procedure_snapshots (snapshot_id, schema_id, procedure_id, procedure_version, name, \
         digest, canonical_json, source_kind, source_label, created_at_ms) \
         VALUES (?1, 'podway.procedure/v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            snapshot.snapshot_id().as_str(),
            snapshot.procedure_id(),
            snapshot.procedure_version(),
            snapshot.name(),
            snapshot.digest().as_str(),
            snapshot.canonical_json().as_str(),
            snapshot.source_label().kind().as_str(),
            snapshot.source_label().label(),
            sqlite_u64(snapshot.created_at().get(), "snapshot creation timestamp")?,
        ],
    ).map_err(|error| storage(error, StoreRecordKindV1::Snapshot))?;
    Ok(())
}

fn load_progress(
    transaction: &Connection,
    session_id: &SessionId,
) -> Result<Vec<StageProgressV1>, StoreErrorV1> {
    let mut statement = transaction.prepare(
        "SELECT stage_id, stage_index, progress_state, latest_attempt_number, latest_attempt_id \
         FROM stage_progress WHERE session_id = ?1 ORDER BY stage_index",
    ).map_err(|error| storage(error, StoreRecordKindV1::Session))?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|error| storage(error, StoreRecordKindV1::Session))?;
    rows.map(|row| {
        let (stage, index, state, number, latest) =
            row.map_err(|error| storage(error, StoreRecordKindV1::Session))?;
        StageProgressV1::new(
            stage_id(stage)?,
            usize::try_from(index).map_err(|_| corrupt(StoreRecordKindV1::Session))?,
            progress_state(&state)?,
            u32::try_from(number).map_err(|_| corrupt(StoreRecordKindV1::Session))?,
            latest.map(attempt_id).transpose()?,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Session))
    })
    .collect()
}

fn load_attempts(
    transaction: &Connection,
    session_id: &SessionId,
    snapshot: &ProcedureSnapshotV1,
) -> Result<Vec<AttemptV1>, StoreErrorV1> {
    let mut statement = transaction.prepare(
        "SELECT a.attempt_id, a.stage_id, a.attempt_number, a.lifecycle, a.started_at_ms, a.ended_at_ms, a.reason \
         FROM attempts a JOIN stage_progress p ON p.session_id = a.session_id AND p.stage_id = a.stage_id \
         WHERE a.session_id = ?1 ORDER BY p.stage_index, a.attempt_number",
    ).map_err(|error| storage(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok(AttemptRow {
                attempt_id: row.get(0)?,
                stage_id: row.get(1)?,
                number: row.get(2)?,
                lifecycle: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                reason: row.get(6)?,
            })
        })
        .map_err(|error| storage(error, StoreRecordKindV1::Attempt))?;
    rows.map(|row| {
        let row = row.map_err(|error| storage(error, StoreRecordKindV1::Attempt))?;
        let id = attempt_id(row.attempt_id)?;
        let stage = stage_id(row.stage_id)?;
        let specification = snapshot
            .stage(&stage)
            .ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?;
        let started_at = timestamp(row.started_at, StoreRecordKindV1::Attempt)?;
        let slots = load_slots(transaction, &id, specification, started_at)?;
        let blockers = load_blockers(transaction, &id)?;
        AttemptV1::new(AttemptInputV1 {
            attempt_id: id,
            session_id: session_id.clone(),
            stage: specification,
            number: u32::try_from(row.number).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            lifecycle: attempt_lifecycle(&row.lifecycle)?,
            started_at,
            ended_at: row
                .ended_at
                .map(|value| timestamp(value, StoreRecordKindV1::Attempt))
                .transpose()?,
            reason: row.reason,
            item_slots: slots,
            blockers,
        })
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    })
    .collect()
}

fn load_slots(
    transaction: &Connection,
    attempt_id: &AttemptId,
    stage: &podway_core::StageSpecV1,
    started_at: UnixMillis,
) -> Result<Vec<ItemSlotV1>, StoreErrorV1> {
    let mut statement = transaction.prepare("SELECT item_id, item_type, item_revision, value_json, created_at_ms, updated_at_ms FROM item_slots WHERE attempt_id = ?1 ORDER BY item_id").map_err(|error| storage(error, StoreRecordKindV1::Item))?;
    let rows = statement
        .query_map([attempt_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| storage(error, StoreRecordKindV1::Item))?;
    let mut loaded = std::collections::BTreeMap::new();
    for row in rows {
        let (item, item_type, item_revision, value, created_at, updated_at) =
            row.map_err(|error| storage(error, StoreRecordKindV1::Item))?;
        let item_id =
            podway_core::ItemId::new(item).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
        let specification = stage
            .item(&item_id)
            .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
        if item_type_name(specification.item_type()) != item_type {
            return Err(corrupt(StoreRecordKindV1::Item));
        }
        let revision = revision(item_revision, StoreRecordKindV1::Item)?;
        let (value, created_at, updated_at) = if revision == Revision::ZERO && value.is_none() {
            if timestamp(created_at, StoreRecordKindV1::Item)? != started_at
                || timestamp(updated_at, StoreRecordKindV1::Item)? != started_at
            {
                return Err(corrupt(StoreRecordKindV1::Item));
            }
            (None, None, None)
        } else {
            (
                value
                    .map(|encoded| decode_item_value(&encoded, specification))
                    .transpose()?,
                Some(timestamp(created_at, StoreRecordKindV1::Item)?),
                Some(timestamp(updated_at, StoreRecordKindV1::Item)?),
            )
        };
        let slot = ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: attempt_id.clone(),
            specification,
            revision,
            value,
            created_at,
            updated_at,
        })
        .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
        if loaded.insert(item_id, slot).is_some() {
            return Err(corrupt(StoreRecordKindV1::Item));
        }
    }
    stage
        .items()
        .iter()
        .map(|specification| {
            loaded
                .remove(specification.id())
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
        })
        .collect()
}

fn load_blockers(
    transaction: &Connection,
    attempt_id: &AttemptId,
) -> Result<Vec<BlockerV1>, StoreErrorV1> {
    let mut statement = transaction.prepare("SELECT blocker_id, reason, state, created_at_ms, resolved_at_ms FROM blockers WHERE attempt_id = ?1 ORDER BY created_at_ms, blocker_id").map_err(|error| storage(error, StoreRecordKindV1::Blocker))?;
    let rows = statement
        .query_map([attempt_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|error| storage(error, StoreRecordKindV1::Blocker))?;
    rows.map(|row| {
        let (id, reason, state, created, resolved) =
            row.map_err(|error| storage(error, StoreRecordKindV1::Blocker))?;
        BlockerV1::new(BlockerInputV1 {
            blocker_id: BlockerId::new(id).map_err(|_| corrupt(StoreRecordKindV1::Blocker))?,
            attempt_id: attempt_id.clone(),
            reason,
            state: blocker_state(&state)?,
            created_at: timestamp(created, StoreRecordKindV1::Blocker)?,
            resolved_at: resolved
                .map(|value| timestamp(value, StoreRecordKindV1::Blocker))
                .transpose()?,
        })
        .map_err(|_| corrupt(StoreRecordKindV1::Blocker))
    })
    .collect()
}

fn encode_item_value(value: &ItemValueV1) -> Result<String, StoreErrorV1> {
    let encoded = match value.value_type() {
        podway_core::ItemValueTypeV1::Confirm => json!({"kind":"confirm"}),
        podway_core::ItemValueTypeV1::Text => {
            json!({"kind":"text","value":value.as_text().ok_or_else(|| corrupt(StoreRecordKindV1::Item))?})
        }
        podway_core::ItemValueTypeV1::Choice => {
            json!({"kind":"choice","value":value.as_choice().ok_or_else(|| corrupt(StoreRecordKindV1::Item))?})
        }
        podway_core::ItemValueTypeV1::Integer => {
            json!({"kind":"integer","value":value.as_integer().ok_or_else(|| corrupt(StoreRecordKindV1::Item))?})
        }
        podway_core::ItemValueTypeV1::List => {
            json!({"kind":"list","value":value.as_list().ok_or_else(|| corrupt(StoreRecordKindV1::Item))?})
        }
        podway_core::ItemValueTypeV1::Artifact => {
            let artifact = value
                .as_artifact()
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
            json!({"kind":"artifact","location_kind": match artifact.location_kind() { podway_core::ArtifactLocationKindV1::LocalPath => "local_path", podway_core::ArtifactLocationKindV1::ExternalReference => "external_reference" }, "location":artifact.location(), "digest":artifact.digest().as_str(), "size_bytes":artifact.size_bytes(), "media_type":artifact.media_type()})
        }
    };
    serde_json::to_string(&encoded).map_err(|_| corrupt(StoreRecordKindV1::Item))
}

fn decode_item_value(
    encoded: &str,
    specification: &podway_core::ItemSpecV1,
) -> Result<ItemValueV1, StoreErrorV1> {
    let value: Value =
        serde_json::from_str(encoded).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
    let object = value
        .as_object()
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
    let expected = item_type_name(specification.item_type());
    if kind != expected {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    let value = match kind {
        "confirm" if has_exact_keys(object, &["kind"]) => ItemValueV1::confirm(),
        "text" if has_exact_keys(object, &["kind", "value"]) => {
            ItemValueV1::text(required_string(object, "value")?)
        }
        "choice" if has_exact_keys(object, &["kind", "value"]) => {
            ItemValueV1::choice(required_string(object, "value")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?
        }
        "integer" if has_exact_keys(object, &["kind", "value"]) => ItemValueV1::integer(
            object
                .get("value")
                .and_then(Value::as_i64)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?,
        ),
        "list" if has_exact_keys(object, &["kind", "value"]) => ItemValueV1::list(
            object
                .get("value")
                .and_then(Value::as_array)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
        "artifact"
            if has_exact_keys(
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
            let digest = Sha256Digest::new(required_string(object, "digest")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
            let size = object
                .get("size_bytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
            let location = required_string(object, "location")?;
            let media_type = required_string(object, "media_type")?;
            match object.get("location_kind").and_then(Value::as_str) {
                Some("local_path") => ItemValueV1::artifact(
                    ArtifactValueV1::local_path(location, digest, size, media_type)
                        .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                ),
                Some("external_reference") => ItemValueV1::artifact(
                    ArtifactValueV1::external_reference(location, digest, size, media_type)
                        .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
                ),
                _ => return Err(corrupt(StoreRecordKindV1::Item)),
            }
        }
        _ => return Err(corrupt(StoreRecordKindV1::Item)),
    };
    if encode_item_value(&value)? != encoded {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    Ok(value)
}

fn has_exact_keys(object: &serde_json::Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<String, StoreErrorV1> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
}
fn session_id(value: String) -> Result<SessionId, StoreErrorV1> {
    SessionId::new(value).map_err(|_| corrupt(StoreRecordKindV1::Session))
}
fn attempt_id(value: String) -> Result<AttemptId, StoreErrorV1> {
    AttemptId::new(value).map_err(|_| corrupt(StoreRecordKindV1::Attempt))
}
fn stage_id(value: String) -> Result<StageId, StoreErrorV1> {
    StageId::new(value).map_err(|_| corrupt(StoreRecordKindV1::Session))
}
fn timestamp(value: i64, kind: StoreRecordKindV1) -> Result<UnixMillis, StoreErrorV1> {
    u64::try_from(value)
        .map(UnixMillis::new)
        .map_err(|_| corrupt(kind))
}
fn revision(value: i64, kind: StoreRecordKindV1) -> Result<Revision, StoreErrorV1> {
    u64::try_from(value)
        .map(Revision::new)
        .map_err(|_| corrupt(kind))
}
fn sqlite_u64(value: u64, field: &'static str) -> Result<i64, StoreErrorV1> {
    i64::try_from(value).map_err(|_| {
        StoreErrorV1::InvalidStateV1(crate::StoreValueErrorV1::IntegerOutOfRange { field })
    })
}
fn sqlite_usize(value: usize, field: &'static str) -> Result<i64, StoreErrorV1> {
    i64::try_from(value).map_err(|_| {
        StoreErrorV1::InvalidStateV1(crate::StoreValueErrorV1::IntegerOutOfRange { field })
    })
}
fn corrupt(record: StoreRecordKindV1) -> StoreErrorV1 {
    StoreErrorV1::CorruptStateV1 { record }
}
fn storage(error: rusqlite::Error, record: StoreRecordKindV1) -> StoreErrorV1 {
    map_rusqlite_error_v1(error, RusqliteErrorContextV1::Record(record))
}

fn session_lifecycle(value: &str) -> Result<SessionLifecycle, StoreErrorV1> {
    match value {
        "running" => Ok(SessionLifecycle::Running),
        "completed" => Ok(SessionLifecycle::Completed),
        "cancelled" => Ok(SessionLifecycle::Cancelled),
        _ => Err(corrupt(StoreRecordKindV1::Session)),
    }
}
fn session_lifecycle_name(value: SessionLifecycle) -> &'static str {
    match value {
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}
fn progress_state(value: &str) -> Result<StageProgressState, StoreErrorV1> {
    match value {
        "pending" => Ok(StageProgressState::Pending),
        "current" => Ok(StageProgressState::Current),
        "done" => Ok(StageProgressState::Done),
        "skipped" => Ok(StageProgressState::Skipped),
        "redo" => Ok(StageProgressState::Redo),
        "abandoned" => Ok(StageProgressState::Abandoned),
        _ => Err(corrupt(StoreRecordKindV1::Session)),
    }
}
fn progress_state_name(value: StageProgressState) -> &'static str {
    match value {
        StageProgressState::Pending => "pending",
        StageProgressState::Current => "current",
        StageProgressState::Done => "done",
        StageProgressState::Skipped => "skipped",
        StageProgressState::Redo => "redo",
        StageProgressState::Abandoned => "abandoned",
    }
}
fn attempt_lifecycle(value: &str) -> Result<AttemptLifecycle, StoreErrorV1> {
    match value {
        "active" => Ok(AttemptLifecycle::Active),
        "completed" => Ok(AttemptLifecycle::Completed),
        "skipped" => Ok(AttemptLifecycle::Skipped),
        "abandoned" => Ok(AttemptLifecycle::Abandoned),
        _ => Err(corrupt(StoreRecordKindV1::Attempt)),
    }
}
fn attempt_lifecycle_name(value: AttemptLifecycle) -> &'static str {
    match value {
        AttemptLifecycle::Active => "active",
        AttemptLifecycle::Completed => "completed",
        AttemptLifecycle::Skipped => "skipped",
        AttemptLifecycle::Abandoned => "abandoned",
    }
}
fn blocker_state(value: &str) -> Result<BlockerState, StoreErrorV1> {
    match value {
        "open" => Ok(BlockerState::Open),
        "resolved" => Ok(BlockerState::Resolved),
        _ => Err(corrupt(StoreRecordKindV1::Blocker)),
    }
}
fn blocker_state_name(value: BlockerState) -> &'static str {
    match value {
        BlockerState::Open => "open",
        BlockerState::Resolved => "resolved",
    }
}
fn item_type_name(value: ItemTypeV1) -> &'static str {
    match value {
        ItemTypeV1::Confirm => "confirm",
        ItemTypeV1::Text => "text",
        ItemTypeV1::Choice => "choice",
        ItemTypeV1::Integer => "integer",
        ItemTypeV1::List => "list",
        ItemTypeV1::Artifact => "artifact",
    }
}

struct SessionRow {
    session_id: String,
    task_title: String,
    snapshot_id: String,
    lifecycle: String,
    revision: i64,
    active_stage_id: Option<String>,
    active_attempt_id: Option<String>,
    created_at: i64,
    completed_at: Option<i64>,
    cancelled_at: Option<i64>,
    cancel_reason: Option<String>,
}
struct SnapshotRow {
    snapshot_id: String,
    schema_id: String,
    procedure_id: String,
    procedure_version: String,
    name: String,
    digest: String,
    canonical_json: String,
    source_kind: String,
    source_label: String,
    created_at: i64,
}
struct AttemptRow {
    attempt_id: String,
    stage_id: String,
    number: i64,
    lifecycle: String,
    started_at: i64,
    ended_at: Option<i64>,
    reason: Option<String>,
}
