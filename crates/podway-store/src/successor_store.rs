//! Candidate Store persistence for isolated execution-group integration fixtures.
//! Transaction callers own commit and success-only outcome publication.

mod delegation;
mod lifecycle;
pub(crate) use delegation::{read_receipt, record_receipt, verify_receipts};
mod native;
mod records;
mod transaction;
pub use lifecycle::{
    SuccessorArchiveMetadata, SuccessorArchivePage, SuccessorArchiveSummary,
    SuccessorSessionMetadata,
};
pub(crate) use lifecycle::{
    archive_session, create_session_metadata, delete_current_session, list_archives, purge_archive,
    record_disposition, session_metadata, terminal_disposition, verify_integrity,
};
pub use native::{
    NativeSuccessorStore, SuccessorMutationStore, SuccessorProgressVerifier, SuccessorReadStore,
};
pub use transaction::*;

#[cfg(feature = "test-support")]
use crate::direct_store::{EXECUTION_GROUP_STORE_DDL_V1, EXECUTION_GROUP_STORE_GENERATION_V1};
use crate::{StoreErrorV1, direct_store::MAX_EXECUTION_GROUP_ATTEMPT_ROW_BYTES_V1};
use podway_core::execution_groups::*;
use podway_core::{
    DomainError, MAX_PROCEDURE_DOCUMENT_BYTES, Revision, SessionId, SessionLifecycle, Sha256Digest,
    UnixMillis, canonicalize_json_v1, verify_canonical_json_v1,
};
#[cfg(feature = "test-support")]
use rusqlite::OpenFlags;
#[cfg(feature = "test-support")]
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Params, Row, Transaction, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[cfg(feature = "test-support")]
use std::path::Path;

#[cfg(feature = "test-support")]
const CANDIDATE_DDL: &str = EXECUTION_GROUP_STORE_DDL_V1;
#[cfg(feature = "test-support")]
const CANDIDATE_GENERATION: u32 = EXECUTION_GROUP_STORE_GENERATION_V1;
#[cfg(feature = "test-support")]
const CANDIDATE_WRITER: &str = "execution-groups-candidate";
// Match the canonical v16 row bounds. The activation budget covers 128 ×
// 2,000 rationale scalars at six JSON bytes each, their retained references,
// and the bounded plan items, closure feedback, and activation metadata.
const MAX_SESSION_JSON_BYTES: usize = 1_048_576;
const MAX_ACTIVATION_JSON_BYTES: usize = 2_097_152;
const MAX_ATTEMPT_JSON_BYTES: usize = MAX_EXECUTION_GROUP_ATTEMPT_ROW_BYTES_V1;
const MAX_GOAL_JSON_BYTES: usize = 1_048_576;
#[cfg(feature = "test-support")]
const SQLITE_RECORD_METADATA_HEADROOM: i32 = 16_384;
const UUID_BYTES: usize = 36;
const DIGEST_BYTES: usize = 71;

#[derive(Debug)]
pub enum CandidateStoreError {
    Sql(rusqlite::Error),
    Io(std::io::Error),
    Item(StoreErrorV1),
    Direct(crate::direct_store::DirectStoreErrorV1),
    Domain(DomainError),
    ArchivedSession,
    ArchiveRequiresTerminalSession,
    ArchiveRevisionMismatch {
        expected: Revision,
        actual: Revision,
    },
    Corrupt(&'static str),
}

impl From<rusqlite::Error> for CandidateStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}
impl From<std::io::Error> for CandidateStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<crate::direct_store::DirectStoreErrorV1> for CandidateStoreError {
    fn from(error: crate::direct_store::DirectStoreErrorV1) -> Self {
        Self::Direct(error)
    }
}
impl From<StoreErrorV1> for CandidateStoreError {
    fn from(error: StoreErrorV1) -> Self {
        Self::Item(error)
    }
}
impl From<DomainError> for CandidateStoreError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}
impl std::fmt::Display for CandidateStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for CandidateStoreError {}

type Result<T> = std::result::Result<T, CandidateStoreError>;
fn corrupt(reason: &'static str) -> CandidateStoreError {
    CandidateStoreError::Corrupt(reason)
}

/// Typed definition produced by the fixture caller's admission callback.
pub enum CandidateDefinition {
    Base(ProcedureDefinitionV3),
    Group(ExecutionGroupDefinitionV1),
}

/// Canonical admitted bytes indexed by immutable digest.
pub type CandidateSources = BTreeMap<Sha256Digest, String>;

/// Create a new candidate file exclusively inside a caller-owned fixture directory.
#[cfg(feature = "test-support")]
pub fn create_candidate_fixture(path: &Path) -> Result<Connection> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut connection = open_candidate(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let transaction = connection.transaction()?;
    transaction.execute_batch(CANDIDATE_DDL)?;
    transaction.execute(
        "INSERT INTO writer_contract VALUES (1, ?1, ?2, 0)",
        params![digest(CANDIDATE_DDL).as_str(), CANDIDATE_WRITER],
    )?;
    transaction.commit()?;
    Ok(connection)
}

/// Read an isolated fixture through an explicitly read-only connection and validate its identity.
#[cfg(feature = "test-support")]
pub fn reopen_candidate_fixture(path: &Path) -> Result<Connection> {
    let connection = open_candidate(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    admit_writer(&connection)?;
    Ok(connection)
}

#[cfg(feature = "test-support")]
fn open_candidate(path: &Path, flags: OpenFlags) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
    // Bound SQLite row materialization as well as the individual Rust field reads.
    connection.set_limit(
        Limit::SQLITE_LIMIT_LENGTH,
        MAX_ATTEMPT_JSON_BYTES as i32 + SQLITE_RECORD_METADATA_HEADROOM,
    )?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;")?;
    if flags.contains(OpenFlags::SQLITE_OPEN_READ_WRITE) {
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")?;
    }
    Ok(connection)
}

#[cfg(feature = "test-support")]
fn admit_writer(connection: &Connection) -> Result<()> {
    let generation: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if generation != CANDIDATE_GENERATION {
        return Err(corrupt("unsupported candidate store generation"));
    }
    let foreign_keys: bool =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    if !foreign_keys {
        return Err(corrupt("candidate connection requires foreign keys"));
    }
    let mut statement = connection.prepare(
        "SELECT contract_manifest_digest, generation_id FROM writer_contract WHERE singleton = 1",
    )?;
    let mut rows = statement.query([])?;
    let row = rows
        .next()?
        .ok_or_else(|| corrupt("candidate writer identity differs"))?;
    if bounded_text(row, 0, DIGEST_BYTES)? != digest(CANDIDATE_DDL).as_str()
        || bounded_text(row, 1, 64)? != CANDIDATE_WRITER
    {
        return Err(corrupt("candidate writer identity differs"));
    }
    crate::direct_store::verify_candidate_schema(connection, CANDIDATE_DDL)
        .map_err(|_| corrupt("candidate schema inventory differs"))?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    id: SessionId,
    base_digest: Sha256Digest,
    selected: Option<GroupExecutionIdV1>,
    initial_goal: Option<records::Goal>,
    initial_goal_actor: Option<String>,
    #[serde(with = "records::Lifecycle")]
    lifecycle: SessionLifecycle,
    revision: Revision,
    created_at: UnixMillis,
    updated_at: UnixMillis,
}

/// Store one retained successor inside the caller's transaction. The operation changes only
/// affected rows. Definitions already retained under a digest must have identical bytes.
#[cfg(feature = "test-support")]
pub fn persist_candidate_execution(
    transaction: &Transaction<'_>,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
) -> Result<()> {
    admit_writer(transaction)?;
    persist_execution(transaction, state, sources)
}

pub(crate) fn persist_execution(
    transaction: &Transaction<'_>,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
) -> Result<()> {
    let fence = state.fence();
    let session_id = fence.session.as_str();
    if transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM successor_sessions WHERE session_id = ?1 AND archived = 1)",
        [session_id],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(CandidateStoreError::ArchivedSession);
    }
    let has_trailing_history: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM successor_attempts WHERE session_id = ?1 AND ordinal > ?2)
         OR EXISTS(SELECT 1 FROM successor_activations WHERE session_id = ?1 AND ordinal > ?3)
         OR EXISTS(SELECT 1 FROM successor_goal_revisions WHERE session_id = ?1 AND ordinal > ?4)",
        params![
            session_id,
            state.attempt_history().len(),
            state.group_history().len(),
            state.goal_revision_history().len() + 1
        ],
        |row| row.get(0),
    )?;
    if has_trailing_history {
        return Err(corrupt("retained execution cannot lose history"));
    }
    persist_definition(transaction, sources, state.base_digest(), "base")?;
    let header = Session {
        id: fence.session.clone(),
        base_digest: state.base_digest().clone(),
        selected: state
            .selected()
            .map(|entry| entry.identity().execution().clone()),
        initial_goal: state.initial_goal().map(Into::into),
        initial_goal_actor: state
            .initial_goal_actor()
            .map(|value| value.as_str().to_owned()),
        lifecycle: state.lifecycle(),
        revision: fence.revision,
        created_at: state.created_at(),
        updated_at: state.updated_at(),
    };
    let (record, checksum) = encode(&header)?;
    let previous = prior_record::<Session>(
        transaction,
        "SELECT record_json, record_digest FROM successor_sessions WHERE session_id = ?1",
        [session_id],
        MAX_SESSION_JSON_BYTES,
    )?;
    let mut exact_revision = false;
    if let Some((previous, digest)) = previous {
        exact_revision = previous.revision == fence.revision;
        if previous.id != fence.session
            || previous.base_digest != *state.base_digest()
            || previous.created_at != state.created_at()
            || previous.revision > fence.revision
            || (exact_revision && digest != checksum.as_str())
        {
            return Err(corrupt("conflicting retained session identity or revision"));
        }
    }
    let changed = transaction.execute(
        "INSERT INTO successor_sessions (session_id, base_digest, record_json, record_digest)
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (session_id) DO UPDATE SET record_json = excluded.record_json, \
         record_digest = excluded.record_digest WHERE record_digest != excluded.record_digest AND base_digest = excluded.base_digest",
        params![session_id, state.base_digest().as_str(), record, checksum.as_str()],
    )?;
    require_exact_if_unchanged(
        transaction,
        changed,
        exact_revision,
        "SELECT base_digest IS ?2 AND record_json IS ?3 AND record_digest IS ?4
         FROM successor_sessions WHERE session_id = ?1",
        params![
            session_id,
            state.base_digest().as_str(),
            record,
            checksum.as_str()
        ],
    )?;
    for (ordinal, execution) in state.group_history().iter().enumerate() {
        let identity = execution.identity();
        persist_definition(transaction, sources, identity.definition_digest(), "group")?;
        let (record, checksum) = encode(&records::Activation::from(execution))?;
        let previous = prior_record::<records::Activation>(
            transaction,
            "SELECT record_json, record_digest FROM successor_activations
             WHERE session_id = ?1 AND ordinal = ?2",
            params![session_id, ordinal + 1],
            MAX_ACTIVATION_JSON_BYTES,
        )?;
        if let Some((previous, _)) = previous {
            let previous = previous.admit()?;
            if &previous.identity != identity
                || previous
                    .closure
                    .as_ref()
                    .is_some_and(|closure| Some(closure) != execution.closure())
                || (previous.outcome != GroupExecutionOutcomeV1::Running
                    && previous.outcome != execution.outcome())
                || previous
                    .terminal_attempt
                    .as_ref()
                    .is_some_and(|attempt| Some(attempt) != execution.terminal_attempt())
            {
                return Err(corrupt(
                    "conflicting retained activation identity or outcome",
                ));
            }
        }
        let arguments = params![
            session_id,
            ordinal + 1,
            identity.execution().as_str(),
            identity.definition_digest().as_str(),
            identity.plan().design_attempt().as_str(),
            identity.predecessor().map(GroupExecutionIdV1::as_str),
            record,
            checksum.as_str()
        ];
        let changed = transaction.execute(
            "INSERT INTO successor_activations VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT (session_id, ordinal) DO UPDATE SET record_json = excluded.record_json, \
             record_digest = excluded.record_digest WHERE record_digest != excluded.record_digest \
             AND execution_id = excluded.execution_id AND definition_digest = excluded.definition_digest \
             AND design_attempt_id = excluded.design_attempt_id AND predecessor_id IS excluded.predecessor_id",
            arguments,
        )?;
        require_exact_if_unchanged(
            transaction, changed, exact_revision,
            "SELECT execution_id IS ?3 AND definition_digest IS ?4 AND design_attempt_id IS ?5 AND predecessor_id IS ?6
             AND record_json IS ?7 AND record_digest IS ?8 FROM successor_activations WHERE session_id = ?1 AND ordinal = ?2",
            arguments,
        )?;
    }
    for (ordinal, attempt) in state.attempt_history().iter().enumerate() {
        let (group, definition) = scope_columns(attempt.target());
        let (record, checksum) = encode(&records::Attempt::capture(attempt)?)?;
        let previous = prior_record::<records::Attempt>(
            transaction,
            "SELECT record_json, record_digest FROM successor_attempts
             WHERE session_id = ?1 AND ordinal = ?2 AND record_digest IS NOT ?3",
            params![session_id, ordinal + 1, checksum.as_str()],
            MAX_ATTEMPT_JSON_BYTES,
        )?;
        if let Some((previous, _)) = previous {
            previous.require_preserved_on_write(attempt)?;
        }
        let arguments = params![
            session_id,
            ordinal + 1,
            attempt.target().attempt().as_str(),
            group,
            definition,
            record,
            checksum.as_str()
        ];
        let changed = transaction.execute(
            "INSERT INTO successor_attempts VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT (session_id, ordinal) DO UPDATE SET record_json = excluded.record_json, \
             record_digest = excluded.record_digest WHERE record_digest != excluded.record_digest \
             AND attempt_id = excluded.attempt_id AND execution_id IS excluded.execution_id \
             AND definition_digest IS excluded.definition_digest",
            arguments,
        )?;
        require_exact_if_unchanged(
            transaction, changed, exact_revision,
            "SELECT attempt_id IS ?3 AND execution_id IS ?4 AND definition_digest IS ?5
             AND record_json IS ?6 AND record_digest IS ?7 FROM successor_attempts WHERE session_id = ?1 AND ordinal = ?2",
            arguments,
        )?;
    }
    for goal in state.goal_revision_history() {
        let (record, checksum) = encode(&records::GoalChange::from(goal))?;
        let arguments = params![
            session_id,
            goal.revision.get(),
            goal.source.attempt().as_str(),
            goal.successor.attempt().as_str(),
            record,
            checksum.as_str()
        ];
        let changed = transaction.execute(
            "INSERT INTO successor_goal_revisions VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT (session_id, ordinal) DO NOTHING",
            arguments,
        )?;
        require_exact_if_unchanged(
            transaction, changed, exact_revision,
            "SELECT source_attempt_id IS ?3 AND successor_attempt_id IS ?4 AND record_json IS ?5 AND record_digest IS ?6
             FROM successor_goal_revisions WHERE session_id = ?1 AND ordinal = ?2",
            arguments,
        )?;
    }
    Ok(())
}

fn require_exact_if_unchanged(
    transaction: &Transaction<'_>,
    changed: usize,
    exact_revision: bool,
    query: &str,
    arguments: &[&dyn rusqlite::ToSql],
) -> Result<()> {
    if (exact_revision && changed != 0)
        || (changed == 0
            && !transaction.query_row(query, arguments, |row| row.get::<_, bool>(0))?)
    {
        return Err(corrupt("conflicting retained row"));
    }
    Ok(())
}

fn persist_definition(
    transaction: &Transaction<'_>,
    sources: &CandidateSources,
    identity: &Sha256Digest,
    kind: &str,
) -> Result<()> {
    let source = sources
        .get(identity)
        .ok_or_else(|| corrupt("missing canonical definition bytes"))?;
    if source.len() > MAX_PROCEDURE_DOCUMENT_BYTES
        || digest(source) != *identity
        || verify_canonical_json_v1(source.as_bytes()).is_err()
    {
        return Err(corrupt(
            "definition bytes do not match their digest, canonical shape, or bounds",
        ));
    }
    transaction.execute(
        "INSERT INTO successor_definitions (digest, kind, document) VALUES (?1, ?2, ?3) ON CONFLICT (digest) DO NOTHING",
        params![identity.as_str(), kind, source],
    )?;
    let matches: bool = transaction.query_row(
        "SELECT kind = ?2 AND document = ?3 FROM successor_definitions WHERE digest = ?1",
        params![identity.as_str(), kind, source],
        |row| row.get(0),
    )?;
    if !matches {
        return Err(corrupt("immutable definition bytes differ"));
    }
    Ok(())
}

/// Reconstruct and independently admit a retained execution within a single caller snapshot.
///
/// The caller must fully admit the canonical definition bytes and return the corresponding typed
/// definition. The candidate integration harness supplies Config's successor parser and validator.
#[cfg(feature = "test-support")]
pub fn load_candidate_execution(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    admit_definition: impl Fn(&str) -> Result<CandidateDefinition>,
) -> Result<(ExecutionSessionV3, CandidateSources)> {
    load_candidate_content(transaction, session_id, admit_definition, None)
}

#[cfg(feature = "test-support")]
fn load_candidate_content(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    admit_definition: impl Fn(&str) -> Result<CandidateDefinition>,
    checksums: Option<&mut Sha256>,
) -> Result<(ExecutionSessionV3, CandidateSources)> {
    admit_writer(transaction)?;
    load_execution(transaction, session_id, admit_definition, checksums)
}

fn current_session_header(connection: &Connection) -> Result<Option<Session>> {
    let mut statement = connection.prepare(
        "SELECT session_id, record_json, record_digest FROM successor_sessions WHERE archived = 0",
    )?;
    let mut rows = statement.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let header: Session = decode_row(row, 1, MAX_SESSION_JSON_BYTES)?;
    if bounded_text(row, 0, UUID_BYTES)? != header.id.as_str() {
        return Err(corrupt("current session identity differs"));
    }
    Ok(Some(header))
}

pub(crate) fn current_execution(connection: &Connection) -> Result<Option<(SessionId, Revision)>> {
    Ok(current_session_header(connection)?.map(|header| (header.id, header.revision)))
}

pub(crate) fn load_current_execution(
    connection: &Connection,
    admit: impl Fn(&str) -> Result<CandidateDefinition>,
) -> Result<Option<(ExecutionSessionV3, CandidateSources)>> {
    let Some((id, _)) = current_execution(connection)? else {
        return Ok(None);
    };
    session_metadata(connection, &id)?
        .ok_or_else(|| corrupt("current session metadata is absent"))?;
    let (state, sources) = load_execution(connection, &id, admit, None)?;
    require_settled_goal_assessments(&state)?;
    verify_receipts(connection, &state)?;
    Ok(Some((state, sources)))
}

pub(crate) fn load_archived_execution(
    connection: &Connection,
    session: &SessionId,
    admit: impl Fn(&str) -> Result<CandidateDefinition>,
) -> Result<Option<(ExecutionSessionV3, CandidateSources)>> {
    let Some(metadata) = session_metadata(connection, session)? else {
        return Ok(None);
    };
    let Some(archive) = metadata.archived else {
        return Ok(None);
    };
    let (state, sources) = load_execution(connection, session, admit, None)?;
    require_settled_goal_assessments(&state)?;
    verify_receipts(connection, &state)?;
    check_archived_state(connection, &state, &archive)?;
    Ok(Some((state, sources)))
}

pub(crate) fn require_settled_goal_assessments(state: &ExecutionSessionV3) -> Result<()> {
    if state.attempt_history().iter().any(|attempt| {
        attempt.criterion_assessments().is_none()
            && attempt
                .decision()
                .is_some_and(|decision| decision.goal_assessment.is_some())
    }) {
        return Err(corrupt(
            "goal decision has no retained criterion assessment set",
        ));
    }
    Ok(())
}

pub(crate) fn check_archived_state(
    connection: &Connection,
    state: &ExecutionSessionV3,
    archive: &SuccessorArchiveMetadata,
) -> Result<()> {
    let fence = state.fence();
    let disposition = terminal_disposition(connection, &fence.session, fence.revision)?;
    if !matches!(
        state.lifecycle(),
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    ) || disposition.is_none_or(|disposition| {
        disposition.recorded_at() < state.updated_at() || disposition.recorded_at() > archive.at
    }) {
        return Err(corrupt("archived session and its disposition disagree"));
    }
    Ok(())
}

fn load_execution(
    transaction: &Connection,
    session_id: &SessionId,
    admit_definition: impl Fn(&str) -> Result<CandidateDefinition>,
    checksums: Option<&mut Sha256>,
) -> Result<(ExecutionSessionV3, CandidateSources)> {
    let mut history = HistoryChecksums(checksums);
    let mut header_query = transaction.prepare(
        "SELECT base_digest, record_json, record_digest FROM successor_sessions WHERE session_id = ?1",
    )?;
    let mut header_rows = header_query.query([session_id.as_str()])?;
    let header_row = header_rows
        .next()?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let header: Session = history.decode(header_row, 1, MAX_SESSION_JSON_BYTES, b's')?;
    if &header.id != session_id
        || header.base_digest.as_str() != bounded_text(header_row, 0, DIGEST_BYTES)?
    {
        return Err(corrupt("retained session identity differs"));
    }
    let mut sources = BTreeMap::new();
    let CandidateDefinition::Base(base) = load_definition(
        transaction,
        &header.base_digest,
        "base",
        &mut sources,
        &admit_definition,
    )?
    else {
        return Err(corrupt("base definition has the wrong schema"));
    };
    let mut definitions = BTreeMap::new();
    let mut executions = Vec::new();
    let mut statement = transaction.prepare(
        "SELECT ordinal, execution_id, definition_digest, design_attempt_id, predecessor_id, record_json, record_digest \
         FROM successor_activations WHERE session_id = ?1 ORDER BY ordinal",
    )?;
    let mut rows = statement.query([session_id.as_str()])?;
    while let Some(row) = rows.next()? {
        let ordinal: usize = row.get(0)?;
        if ordinal > MAX_GROUP_EXECUTIONS_V1 || ordinal != executions.len() + 1 {
            return Err(corrupt("retained activation ordinal differs"));
        }
        let value = history
            .decode::<records::Activation>(row, 5, MAX_ACTIVATION_JSON_BYTES, b'g')?
            .admit()?;
        let identity = &value.identity;
        if bounded_text(row, 1, UUID_BYTES)? != identity.execution().as_str()
            || bounded_text(row, 2, DIGEST_BYTES)? != identity.definition_digest().as_str()
            || bounded_text(row, 3, UUID_BYTES)? != identity.plan().design_attempt().as_str()
            || optional_bounded_text(row, 4, UUID_BYTES)?
                != identity.predecessor().map(GroupExecutionIdV1::as_str)
        {
            return Err(corrupt("retained activation row identity differs"));
        }
        let definition_digest = identity.definition_digest();
        if let std::collections::btree_map::Entry::Vacant(entry) =
            definitions.entry(definition_digest.clone())
        {
            let CandidateDefinition::Group(definition) = load_definition(
                transaction,
                definition_digest,
                "group",
                &mut sources,
                &admit_definition,
            )?
            else {
                return Err(corrupt("group definition has the wrong schema"));
            };
            entry.insert(definition);
        }
        executions.push(value);
    }
    let attempts = load_attempts(transaction, session_id, &mut history)?;
    let goal_revisions = load_goals(transaction, session_id, &mut history)?;
    let state = ExecutionSessionV3::from_retained(RetainedExecutionV3 {
        session: header.id,
        base,
        base_digest: header.base_digest,
        definitions,
        executions,
        selected: header.selected,
        attempts,
        initial_goal: header.initial_goal.map(records::Goal::admit).transpose()?,
        initial_goal_actor: header
            .initial_goal_actor
            .map(podway_core::ActorAttributionV2::new)
            .transpose()?,
        goal_revisions,
        lifecycle: header.lifecycle,
        revision: header.revision,
        created_at: header.created_at,
        updated_at: header.updated_at,
    })?;
    Ok((state, sources))
}

fn load_definition(
    transaction: &Connection,
    identity: &Sha256Digest,
    expected: &str,
    sources: &mut CandidateSources,
    admit: &impl Fn(&str) -> Result<CandidateDefinition>,
) -> Result<CandidateDefinition> {
    let mut statement = transaction
        .prepare("SELECT kind, document FROM successor_definitions WHERE digest = ?1")?;
    let mut rows = statement.query([identity.as_str()])?;
    let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let kind = bounded_text(row, 0, 5)?;
    let source = bounded_text(row, 1, MAX_PROCEDURE_DOCUMENT_BYTES)?;
    if kind != expected
        || digest(source) != *identity
        || verify_canonical_json_v1(source.as_bytes()).is_err()
    {
        return Err(corrupt(
            "definition kind, digest, canonical shape, or size differs",
        ));
    }
    let definition = admit(source)?;
    sources.insert(identity.clone(), source.to_owned());
    Ok(definition)
}

fn load_attempts(
    transaction: &Connection,
    session_id: &SessionId,
    history: &mut HistoryChecksums<'_>,
) -> Result<Vec<RetainedAttemptV3>> {
    let mut values = Vec::new();
    let mut statement = transaction.prepare(
        "SELECT ordinal, attempt_id, execution_id, definition_digest, record_json, record_digest \
         FROM successor_attempts WHERE session_id = ?1 ORDER BY ordinal",
    )?;
    let mut rows = statement.query([session_id.as_str()])?;
    while let Some(row) = rows.next()? {
        if values.len() >= MAX_SESSION_ATTEMPTS_V1 || row.get::<_, usize>(0)? != values.len() + 1 {
            return Err(corrupt("retained attempt trace ordinal differs"));
        }
        let value = history
            .decode::<records::Attempt>(row, 4, MAX_ATTEMPT_JSON_BYTES, b'a')?
            .admit()?;
        let (group, definition) = scope_columns(&value.target);
        if bounded_text(row, 1, UUID_BYTES)? != value.target.attempt().as_str()
            || optional_bounded_text(row, 2, UUID_BYTES)? != group
            || optional_bounded_text(row, 3, DIGEST_BYTES)? != definition
        {
            return Err(corrupt("retained attempt row identity differs"));
        }
        values.push(value);
    }
    Ok(values)
}

fn load_goals(
    transaction: &Connection,
    session_id: &SessionId,
    history: &mut HistoryChecksums<'_>,
) -> Result<Vec<GroupGoalRevisionV1>> {
    let mut values = Vec::new();
    let mut statement = transaction.prepare(
        "SELECT ordinal, source_attempt_id, successor_attempt_id, record_json, record_digest \
         FROM successor_goal_revisions WHERE session_id = ?1 ORDER BY ordinal",
    )?;
    let mut rows = statement.query([session_id.as_str()])?;
    while let Some(row) = rows.next()? {
        let ordinal: usize = row.get(0)?;
        if ordinal > MAX_SESSION_ATTEMPTS_V1 || ordinal != values.len() + 2 {
            return Err(corrupt("retained goal revision ordinal differs"));
        }
        let value = history
            .decode::<records::GoalChange>(row, 3, MAX_GOAL_JSON_BYTES, b'r')?
            .admit()?;
        if ordinal as u64 != value.revision.get()
            || bounded_text(row, 1, UUID_BYTES)? != value.source.attempt().as_str()
            || bounded_text(row, 2, UUID_BYTES)? != value.successor.attempt().as_str()
        {
            return Err(corrupt("retained goal revision row identity differs"));
        }
        values.push(value);
    }
    Ok(values)
}

fn bounded_text<'row>(row: &'row Row<'_>, index: usize, maximum: usize) -> Result<&'row str> {
    let ValueRef::Text(bytes) = row.get_ref(index)? else {
        return Err(corrupt("retained field must be text"));
    };
    if bytes.len() > maximum {
        return Err(corrupt("retained text exceeds its byte bound"));
    }
    std::str::from_utf8(bytes).map_err(|_| corrupt("retained text is not UTF-8"))
}

fn optional_bounded_text<'row>(
    row: &'row Row<'_>,
    index: usize,
    maximum: usize,
) -> Result<Option<&'row str>> {
    if row.get_ref(index)? == ValueRef::Null {
        return Ok(None);
    }
    bounded_text(row, index, maximum).map(Some)
}

fn decode_row<T: DeserializeOwned + Serialize>(
    row: &Row<'_>,
    record_column: usize,
    maximum: usize,
) -> Result<T> {
    decode(
        bounded_text(row, record_column, maximum)?,
        bounded_text(row, record_column + 1, DIGEST_BYTES)?,
    )
}

struct HistoryChecksums<'a>(Option<&'a mut Sha256>);

impl HistoryChecksums<'_> {
    fn decode<T: DeserializeOwned + Serialize>(
        &mut self,
        row: &Row<'_>,
        column: usize,
        maximum: usize,
        kind: u8,
    ) -> Result<T> {
        let record = decode_row(row, column, maximum)?;
        if let Some(checksums) = self.0.as_deref_mut() {
            // The verified checksums are fixed-width; the kind separates ordered row runs.
            checksums.update([kind]);
            checksums.update(bounded_text(row, column + 1, DIGEST_BYTES)?.as_bytes());
        }
        Ok(record)
    }
}

fn prior_record<T: DeserializeOwned + Serialize>(
    transaction: &Transaction<'_>,
    sql: &str,
    arguments: impl Params,
    maximum: usize,
) -> Result<Option<(T, String)>> {
    let mut statement = transaction.prepare(sql)?;
    let mut rows = statement.query(arguments)?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let record = decode_row(row, 0, maximum)?;
    Ok(Some((
        record,
        bounded_text(row, 1, DIGEST_BYTES)?.to_owned(),
    )))
}

fn scope_columns(target: &ScopedAttemptV1) -> (Option<&str>, Option<&str>) {
    match target.scope() {
        ExecutionScopeV1::Base => (None, None),
        ExecutionScopeV1::Group {
            group_execution_id,
            definition_digest,
        } => (
            Some(group_execution_id.as_str()),
            Some(definition_digest.as_str()),
        ),
    }
}

fn digest(value: &str) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(value.as_bytes())))
        .expect("hex encoded SHA-256 is a valid digest")
}

fn encode(value: &impl Serialize) -> Result<(String, Sha256Digest)> {
    let payload =
        canonicalize_json_v1(value).map_err(|_| corrupt("unencodable retained record"))?;
    let checksum = digest(&payload);
    Ok((payload, checksum))
}

fn decode<T: DeserializeOwned + Serialize>(value: &str, checksum: &str) -> Result<T> {
    if digest(value).as_str() != checksum {
        return Err(corrupt("retained record checksum differs"));
    }
    let record: T = serde_json::from_str(value).map_err(|_| corrupt("invalid retained record"))?;
    if encode(&record)?.0 != value {
        return Err(corrupt("retained record is not canonical"));
    }
    Ok(record)
}
