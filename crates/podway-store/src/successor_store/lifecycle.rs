//! Durable session metadata and terminal disposition on an admitted successor writer.

use super::{
    CandidateStoreError, Result, bounded_text, corrupt, current_session_header, decode_row, encode,
};
use crate::{StoreErrorV1, direct_store::DirectStoreErrorV1};
use podway_core::{
    ActorAttributionV2, DomainError, Revision, SessionId, SessionLifecycle,
    TerminalDispositionKindV2, TerminalDispositionV2, UnixMillis,
};
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};

const MAX_TASK_TITLE_SCALARS: usize = 500;
const MAX_DISPOSITION_BYTES: usize = 65_536;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorSessionMetadata {
    pub task_title: String,
    pub archived: Option<SuccessorArchiveMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorArchiveMetadata {
    pub slot: u32,
    pub at: UnixMillis,
    pub workspace_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorArchiveSummary {
    pub session: SessionId,
    pub task_title: String,
    pub revision: Revision,
    pub lifecycle: SessionLifecycle,
    pub archive: SuccessorArchiveMetadata,
}

pub struct SuccessorArchivePage {
    pub count: u64,
    pub sessions: Vec<SuccessorArchiveSummary>,
    pub oldest: Option<SuccessorArchiveSummary>,
}

pub(crate) fn create_session_metadata(
    transaction: &Transaction<'_>,
    session: &SessionId,
    title: &str,
) -> Result<()> {
    if title.trim().is_empty() || title.chars().count() > MAX_TASK_TITLE_SCALARS {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "task title must be nonblank and contain at most 500 characters",
        }
        .into());
    }
    transaction.execute(
        "INSERT INTO successor_session_metadata (session_id, task_title) VALUES (?1, ?2)",
        params![session.as_str(), title],
    )?;
    Ok(())
}

pub(crate) fn session_metadata(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<SuccessorSessionMetadata>> {
    let mut statement = connection.prepare(
        "SELECT s.archived, m.task_title, m.archive_slot, m.archived_at_ms, m.archived_sequence
         FROM successor_sessions s LEFT JOIN successor_session_metadata m USING (session_id)
         WHERE s.session_id = ?1",
    )?;
    let mut rows = statement.query([session.as_str()])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let title = bounded_text(row, 1, MAX_TASK_TITLE_SCALARS * 4)?;
    if title.trim().is_empty() || title.chars().count() > MAX_TASK_TITLE_SCALARS {
        return Err(corrupt("retained task title differs from its contract"));
    }
    let archived = match (
        row.get::<_, bool>(0)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ) {
        (false, None, None, None) => None,
        (true, Some(slot), Some(at), Some(workspace_sequence))
            if slot != 0 && workspace_sequence != 0 =>
        {
            Some(SuccessorArchiveMetadata {
                slot,
                at: UnixMillis::new(at),
                workspace_sequence,
            })
        }
        _ => {
            return Err(corrupt(
                "retained archive metadata differs from its session",
            ));
        }
    };
    Ok(Some(SuccessorSessionMetadata {
        task_title: title.to_owned(),
        archived,
    }))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DispositionRecord {
    reason: Disposition,
    actor: Option<String>,
    at: UnixMillis,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Disposition {
    HandedOff {
        summary: String,
        reference: String,
    },
    NotRequired {
        reason: String,
    },
    Superseded {
        reason: String,
        successor: SessionId,
    },
}

pub(crate) fn terminal_disposition(
    connection: &Connection,
    session: &SessionId,
    revision: Revision,
) -> Result<Option<TerminalDispositionV2>> {
    let mut statement = connection.prepare(
        "SELECT record_json, record_digest FROM successor_terminal_dispositions
         WHERE session_id = ?1 AND terminal_revision = ?2",
    )?;
    let mut rows = statement.query(params![session.as_str(), sqlite_value(revision.get())?])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let record: DispositionRecord = decode_row(row, 0, MAX_DISPOSITION_BYTES)?;
    let actor = record.actor.map(ActorAttributionV2::new).transpose()?;
    let (session, at) = (session.clone(), record.at);
    Ok(Some(match record.reason {
        Disposition::HandedOff { summary, reference } => {
            TerminalDispositionV2::handed_off(session, revision, summary, reference, actor, at)?
        }
        Disposition::NotRequired { reason } => {
            TerminalDispositionV2::not_required(session, revision, reason, actor, at)?
        }
        Disposition::Superseded { reason, successor } => {
            TerminalDispositionV2::superseded(session, revision, reason, successor, actor, at)?
        }
    }))
}

pub(crate) fn record_disposition(
    transaction: &Transaction<'_>,
    disposition: &TerminalDispositionV2,
    now: UnixMillis,
    replacement: Option<&SessionId>,
) -> Result<()> {
    let session = disposition.session_id();
    let revision = disposition.terminal_session_revision();
    let header = require_current(transaction, session, revision)?;
    if !matches!(
        header.lifecycle,
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    ) || disposition.recorded_at() != now
        || now < header.updated_at
    {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "disposition requires an exact current terminal revision and its current time",
        }
        .into());
    }
    if terminal_disposition(transaction, session, revision)?.is_some() {
        return Err(StoreErrorV1::TerminalDispositionAlreadyRecordedV1 {
            session_revision: revision,
        }
        .into());
    }
    let reason = match disposition.kind() {
        TerminalDispositionKindV2::HandedOff => Disposition::HandedOff {
            summary: disposition
                .summary()
                .expect("typed handoff has a summary")
                .into(),
            reference: disposition
                .stable_reference()
                .expect("typed handoff has a reference")
                .into(),
        },
        TerminalDispositionKindV2::NotRequired => Disposition::NotRequired {
            reason: disposition
                .reason()
                .expect("typed disposition has a reason")
                .into(),
        },
        TerminalDispositionKindV2::Superseded => {
            let successor = replacement
                .filter(|id| Some(*id) == disposition.successor_session_id())
                .ok_or(DirectStoreErrorV1::InvalidInput {
                    reason: "supersession requires its exact successor in the same transaction",
                })?;
            Disposition::Superseded {
                reason: disposition
                    .reason()
                    .expect("typed supersession has a reason")
                    .into(),
                successor: successor.clone(),
            }
        }
    };
    let (record, digest) = encode(&DispositionRecord {
        reason,
        actor: disposition.actor().map(|actor| actor.as_str().into()),
        at: now,
    })?;
    transaction.execute(
        "INSERT INTO successor_terminal_dispositions VALUES (?1, ?2, ?3, ?4)",
        params![
            session.as_str(),
            sqlite_value(revision.get())?,
            record,
            digest.as_str()
        ],
    )?;
    Ok(())
}

pub(crate) fn archive_session(
    transaction: &Transaction<'_>,
    session: &SessionId,
    revision: Revision,
    now: UnixMillis,
    sequence: u64,
) -> Result<SuccessorArchiveMetadata> {
    let header = require_current(transaction, session, revision)?;
    let disposition = terminal_disposition(transaction, session, revision)?;
    let terminal = matches!(
        header.lifecycle,
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    );
    if !terminal
        || disposition.as_ref().is_none_or(|disposition| {
            disposition.recorded_at() < header.updated_at || disposition.recorded_at() > now
        })
    {
        return Err(StoreErrorV1::SessionArchiveNotEligibleV1 {
            lifecycle: header.lifecycle,
            current_terminal_disposition: disposition.is_some(),
        }
        .into());
    }
    let slot: Option<u32> = transaction.query_row(
        "SELECT MIN(candidate) FROM (
             SELECT 1 AS candidate UNION ALL
             SELECT archive_slot + 1 FROM successor_session_metadata WHERE archive_slot < ?1
         ) WHERE candidate NOT IN (
             SELECT archive_slot FROM successor_session_metadata WHERE archive_slot IS NOT NULL
         )",
        [i64::from(u32::MAX)],
        |row| row.get(0),
    )?;
    let slot = slot.ok_or_else(|| corrupt("no session archive slot is available"))?;
    let session_changed = transaction.execute(
        "UPDATE successor_sessions SET archived = 1 WHERE session_id = ?1 AND archived = 0",
        [session.as_str()],
    )?;
    let metadata_changed = transaction.execute(
        "UPDATE successor_session_metadata SET archive_slot = ?2, archived_at_ms = ?3,
             archived_sequence = ?4 WHERE session_id = ?1 AND archive_slot IS NULL",
        params![
            session.as_str(),
            slot,
            sqlite_value(now.get())?,
            sqlite_value(sequence)?
        ],
    )?;
    if session_changed != 1 || metadata_changed != 1 {
        return Err(corrupt(
            "archive did not update the exact current session and metadata",
        ));
    }
    Ok(SuccessorArchiveMetadata {
        slot,
        at: now,
        workspace_sequence: sequence,
    })
}

pub(crate) fn delete_current_session(
    transaction: &Transaction<'_>,
    session: &SessionId,
    revision: Revision,
) -> Result<()> {
    require_current(transaction, session, revision)?;
    let deleted = transaction.execute(
        "DELETE FROM successor_sessions WHERE session_id = ?1 AND archived = 0",
        [session.as_str()],
    )?;
    if deleted != 1 {
        return Err(corrupt("delete did not remove the exact current session"));
    }
    transaction.execute(
        "DELETE FROM mutation_outcomes WHERE scope_kind = 'session' AND scope_session_id = ?1",
        [session.as_str()],
    )?;
    delete_unreferenced_definitions(transaction)
}

pub(crate) fn purge_archive(
    transaction: &Transaction<'_>,
    session: &SessionId,
    expected: Revision,
) -> Result<()> {
    let summary = archive_summary(transaction, session)?.ok_or_else(|| {
        StoreErrorV1::SessionArchiveNotFoundV1 {
            session_id: session.clone(),
        }
    })?;
    if summary.revision != expected {
        return Err(crate::direct_store::DirectStoreErrorV1::from(
            podway_core::GraphMutationErrorV2::SessionRevisionConflict {
                expected,
                actual: summary.revision,
            },
        )
        .into());
    }
    if transaction.execute(
        "DELETE FROM successor_sessions WHERE session_id = ?1 AND archived = 1",
        [session.as_str()],
    )? != 1
    {
        return Err(StoreErrorV1::SessionArchiveNotFoundV1 {
            session_id: session.clone(),
        }
        .into());
    }
    // Workspace outcome lookup continues to recover the original committed
    // bytes under normal retention, even when its historical content is purged.
    delete_unreferenced_definitions(transaction)
}

fn archive_summary(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<SuccessorArchiveSummary>> {
    let mut headers = connection.prepare(
        "SELECT base_digest, record_json, record_digest FROM successor_sessions WHERE session_id = ?1 AND archived = 1",
    )?;
    let mut headers = headers.query([session.as_str()])?;
    let Some(row) = headers.next()? else {
        return Ok(None);
    };
    let header: super::Session = decode_row(row, 1, super::MAX_SESSION_JSON_BYTES)?;
    let metadata = session_metadata(connection, session)?
        .ok_or_else(|| corrupt("archived successor session metadata is absent"))?;
    let archive = metadata
        .archived
        .ok_or_else(|| corrupt("successor archive slot is absent"))?;
    if &header.id != session
        || header.base_digest.as_str() != bounded_text(row, 0, super::DIGEST_BYTES)?
        || !matches!(
            header.lifecycle,
            SessionLifecycle::Completed | SessionLifecycle::Cancelled
        )
        || terminal_disposition(connection, session, header.revision)?.is_none_or(|record| {
            record.recorded_at() < header.updated_at || record.recorded_at() > archive.at
        })
    {
        return Err(corrupt(
            "retained archive summary differs from its terminal session",
        ));
    }
    Ok(Some(SuccessorArchiveSummary {
        session: session.clone(),
        task_title: metadata.task_title,
        revision: header.revision,
        lifecycle: header.lifecycle,
        archive,
    }))
}

pub(crate) fn list_archives(connection: &Connection, limit: usize) -> Result<SuccessorArchivePage> {
    if !(1..=crate::MAX_ARCHIVED_SESSION_LIST_WINDOW_V2).contains(&limit) {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "invalid successor archive list window",
        }
        .into());
    }
    let count: u64 = connection.query_row(
        "SELECT COUNT(*) FROM successor_sessions WHERE archived = 1",
        [],
        |row| row.get(0),
    )?;
    let mut window = connection.prepare(
        "SELECT session_id FROM successor_session_metadata WHERE archive_slot IS NOT NULL
         ORDER BY archived_at_ms DESC, session_id DESC LIMIT ?1",
    )?;
    let mut window = window.query([limit as i64])?;
    let mut sessions = Vec::new();
    while let Some(row) = window.next()? {
        let id = SessionId::new(bounded_text(row, 0, super::UUID_BYTES)?)?;
        sessions.push(
            archive_summary(connection, &id)?
                .ok_or_else(|| corrupt("listed successor archive is absent"))?,
        );
    }
    if sessions.len() as u64 != count.min(limit as u64) {
        return Err(corrupt(
            "successor archive headers and metadata have different counts",
        ));
    }
    let oldest = if count > limit as u64 {
        let mut oldest = connection.prepare(
            "SELECT session_id FROM successor_session_metadata WHERE archive_slot IS NOT NULL
             ORDER BY archived_at_ms, session_id LIMIT 1",
        )?;
        let oldest = oldest.query_row([], |row| row.get::<_, String>(0))?;
        archive_summary(connection, &SessionId::new(oldest)?)?
    } else {
        sessions.last().cloned()
    };
    Ok(SuccessorArchivePage {
        count,
        sessions,
        oldest,
    })
}

fn delete_unreferenced_definitions(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute(
        "DELETE FROM successor_definitions WHERE
         NOT EXISTS (SELECT 1 FROM successor_sessions WHERE base_digest = digest)
         AND NOT EXISTS (SELECT 1 FROM successor_activations WHERE definition_digest = digest)",
        [],
    )?;
    Ok(())
}

pub(crate) fn verify_integrity(
    connection: &Connection,
    admit: impl Fn(&str) -> Result<super::CandidateDefinition>,
) -> Result<Option<(super::ExecutionSessionV3, super::CandidateSources)>> {
    let current = super::load_current_execution(connection, &admit)?;
    let mut archives = connection.prepare(
        "SELECT session_id FROM successor_sessions WHERE archived = 1 ORDER BY session_id",
    )?;
    let mut archives = archives.query([])?;
    while let Some(row) = archives.next()? {
        let id = SessionId::new(bounded_text(row, 0, 36)?)?;
        super::load_archived_execution(connection, &id, &admit)?
            .ok_or_else(|| corrupt("invalid retained archive metadata"))?;
    }
    Ok(current)
}

fn require_current(
    connection: &Connection,
    session: &SessionId,
    revision: Revision,
) -> Result<super::Session> {
    let header = current_session_header(connection)?;
    let header = match header {
        Some(header) if &header.id == session => header,
        other => {
            return Err(DomainError::SessionIdentityMismatch {
                expected: session.clone(),
                actual: other.map(|header| header.id),
            }
            .into());
        }
    };
    if header.revision != revision {
        return Err(DomainError::PreconditionFailed {
            expected: revision,
            actual: header.revision,
        }
        .into());
    }
    Ok(header)
}

fn sqlite_value(value: u64) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| CandidateStoreError::Corrupt("integer exceeds successor Store range"))
}
