//! Focused read-only SQLite v1 integrity fixtures.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use podway_core::{DomainCommand, Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1};
use podway_store::codec::{
    PersistedTerminalReceiptV1, encode_command_v1, encode_persisted_terminal_receipt_v1,
};
use podway_store::schema::{inspect_integrity_v1, open_or_initialize_v1};
use podway_store::{
    ClaimedExecutionV1, DurableWorktreeIdentityV1, IntegrityModeV1, JobIdV1, JobReceiptV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, StoreErrorV1, StoreIntegrityCheckV1,
    ValidatedWorkspaceRootV1,
};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
const FAST_INTEGRITY_CHECKS: &[StoreIntegrityCheckV1] = &[
    StoreIntegrityCheckV1::SchemaVersion,
    StoreIntegrityCheckV1::RequiredSchemaObjects,
    StoreIntegrityCheckV1::MigrationChecksum,
    StoreIntegrityCheckV1::ConnectionPragmas,
    StoreIntegrityCheckV1::WorkspaceIdentity,
    StoreIntegrityCheckV1::SqliteQuickCheck,
    StoreIntegrityCheckV1::ForeignKeys,
    StoreIntegrityCheckV1::SnapshotDigest,
    StoreIntegrityCheckV1::ActiveAttempt,
    StoreIntegrityCheckV1::SessionCursor,
    StoreIntegrityCheckV1::JobQueue,
    StoreIntegrityCheckV1::InternalCodec,
    StoreIntegrityCheckV1::IdempotencyReceipt,
];

const DEEP_INTEGRITY_CHECKS: &[StoreIntegrityCheckV1] = &[
    StoreIntegrityCheckV1::SchemaVersion,
    StoreIntegrityCheckV1::RequiredSchemaObjects,
    StoreIntegrityCheckV1::MigrationChecksum,
    StoreIntegrityCheckV1::ConnectionPragmas,
    StoreIntegrityCheckV1::WorkspaceIdentity,
    StoreIntegrityCheckV1::SqliteQuickCheck,
    StoreIntegrityCheckV1::ForeignKeys,
    StoreIntegrityCheckV1::SnapshotDigest,
    StoreIntegrityCheckV1::ActiveAttempt,
    StoreIntegrityCheckV1::SessionCursor,
    StoreIntegrityCheckV1::JobQueue,
    StoreIntegrityCheckV1::InternalCodec,
    StoreIntegrityCheckV1::IdempotencyReceipt,
    StoreIntegrityCheckV1::SqliteDeepCheck,
];

fn digest(hex_digit: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", hex_digit.to_string().repeat(64)))
        .expect("fixture digest must be valid")
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001")
            .expect("fixture workspace ID must be valid"),
        digest('b'),
    )
}

fn other_identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('c'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001")
            .expect("fixture workspace ID must be valid"),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-integrity"))
        .expect("fixture root must be valid")
}

fn options() -> SqliteStoreOptionsV1 {
    SqliteStoreOptionsV1::new(8).expect("fixture options must be valid")
}

fn job_id() -> JobIdV1 {
    JobIdV1::new("00000000-0000-4000-8000-000000000002").expect("fixture job ID must be valid")
}
#[derive(Debug, Eq, PartialEq)]
struct DurableDatabaseFilesV1 {
    database: Vec<u8>,
    wal: Option<Vec<u8>>,
    shm: Option<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
struct LogicalTableContentsV1 {
    name: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Debug, Eq, PartialEq)]
struct LogicalDatabaseContentsV1 {
    tables: Vec<LogicalTableContentsV1>,
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}

fn read_optional_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn snapshot_durable_database_files(
    path: &Path,
) -> Result<DurableDatabaseFilesV1, Box<dyn std::error::Error>> {
    Ok(DurableDatabaseFilesV1 {
        database: fs::read(path)?,
        wal: read_optional_bytes(&sidecar_path(path, "-wal"))?,
        shm: read_optional_bytes(&sidecar_path(path, "-shm"))?,
    })
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn logical_contents_from_durable_snapshot(
    snapshot: &DurableDatabaseFilesV1,
) -> Result<LogicalDatabaseContentsV1, Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let path = temporary.path().join("snapshot.sqlite3");
    fs::write(&path, &snapshot.database)?;
    if let Some(wal) = &snapshot.wal {
        fs::write(sidecar_path(&path, "-wal"), wal)?;
    }
    if let Some(shm) = &snapshot.shm {
        fs::write(sidecar_path(&path, "-shm"), shm)?;
    }

    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tables = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut contents = Vec::with_capacity(tables.len());
    for name in tables {
        let quoted_name = quote_identifier(&name);
        let columns = {
            let mut statement = connection.prepare(&format!("PRAGMA table_info({quoted_name})"))?;
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
        };
        assert!(
            !columns.is_empty(),
            "persisted table {name} must expose its complete column set"
        );

        let projection = columns
            .iter()
            .map(|column| format!("quote({})", quote_identifier(column)))
            .collect::<Vec<_>>()
            .join(", ");
        let ordering = columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = connection.prepare(&format!(
            "SELECT {projection} FROM {quoted_name} ORDER BY {ordering}"
        ))?;
        let mut cursor = statement.query([])?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next()? {
            let mut values: Vec<String> = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                values.push(row.get(index)?);
            }
            rows.push(values);
        }
        contents.push(LogicalTableContentsV1 {
            name,
            columns,
            rows,
        });
    }

    Ok(LogicalDatabaseContentsV1 { tables: contents })
}

fn snapshot_json() -> Result<String, Box<dyn std::error::Error>> {
    Ok(canonicalize_json_v1(&serde_json::json!({
        "schema": "podway.procedure/v1",
        "id": "integrity-fixture",
        "version": "1",
        "name": "Integrity fixture",
        "stages": [{
            "id": "first",
            "title": "First",
            "instructions": [],
            "items": [{
                "type": "confirm",
                "id": "done",
                "prompt": "Done",
                "required": false
            }]
        }],
        "rework": {"allow_return_to": "any_previous"}
    }))?)
}

fn insert_snapshot(
    connection: &Connection,
    canonical_json: &str,
    digest: &Sha256Digest,
) -> Result<(), Box<dyn std::error::Error>> {
    connection.execute(
        "INSERT INTO procedure_snapshots (snapshot_id, schema_id, procedure_id, procedure_version, name, \
         digest, canonical_json, source_kind, source_label, created_at_ms) VALUES \
         ('00000000-0000-4000-8000-000000000004', 'podway.procedure/v1', 'integrity-fixture', '1', \
         'Integrity fixture', ?1, ?2, 'preset', 'integrity-fixture', 1)",
        params![digest.as_str(), canonical_json],
    )?;
    Ok(())
}

fn initialize(temporary: &TempDir) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = temporary.path().join("state.sqlite3");
    let connection = open_or_initialize_v1(
        &path,
        &root(),
        &identity(),
        &options(),
        UnixMillis::new(100),
    )?;
    drop(connection);
    Ok(path)
}

fn inspect(path: &Path) -> Result<podway_store::IntegrityReportV1, StoreErrorV1> {
    inspect_with_mode(path, IntegrityModeV1::Fast)
}

fn inspect_with_mode(
    path: &Path,
    mode: IntegrityModeV1,
) -> Result<podway_store::IntegrityReportV1, StoreErrorV1> {
    inspect_integrity_v1(path, &identity(), &options(), mode, UnixMillis::new(200))
}
fn assert_exact_passed_checks(
    report: &podway_store::IntegrityReportV1,
    expected: &[StoreIntegrityCheckV1],
) {
    assert_eq!(report.checks().len(), expected.len());
    assert!(report.checks().iter().all(|result| result.passed()));
    let actual = report
        .checks()
        .iter()
        .map(|result| result.check().clone())
        .collect::<Vec<_>>();
    assert_eq!(actual.as_slice(), expected);
}

fn assert_integrity_error(
    path: &Path,
    check: StoreIntegrityCheckV1,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_integrity_error_without_mutation(path, IntegrityModeV1::Fast, check)
}

fn assert_integrity_error_without_mutation(
    path: &Path,
    mode: IntegrityModeV1,
    check: StoreIntegrityCheckV1,
) -> Result<(), Box<dyn std::error::Error>> {
    let durable_before = snapshot_durable_database_files(path)?;
    assert!(matches!(
        inspect_with_mode(path, mode),
        Err(StoreErrorV1::StorageIntegrityV1 { check: actual }) if actual == check
    ));
    let durable_after = snapshot_durable_database_files(path)?;
    assert_eq!(durable_after, durable_before);
    Ok(())
}

fn insert_live_job(
    connection: &Connection,
    request: &str,
    idempotency_digest: &Sha256Digest,
) -> Result<(), Box<dyn std::error::Error>> {
    let job_id = job_id();
    let request_digest = digest('d');
    connection.execute(
        "INSERT INTO jobs (job_id, workspace_sequence, idempotency_key, request_digest, command_name, \
         canonical_request_json, state, submitted_at_ms) \
         VALUES (?1, 1, 'request-key', ?2, 'workspace.initialize', ?3, 'queued', 1)",
        params![job_id.as_str(), request_digest.as_str(), request],
    )?;
    connection.execute(
        "INSERT INTO idempotency_records (idempotency_key, request_digest, job_id, scope_kind, \
         scope_session_id, created_at_ms, updated_at_ms) \
         VALUES ('request-key', ?1, ?2, 'workspace', NULL, 1, 1)",
        params![idempotency_digest.as_str(), job_id.as_str()],
    )?;
    connection.execute(
        "UPDATE workspace_state SET next_workspace_sequence = 1 WHERE singleton = 1",
        [],
    )?;
    Ok(())
}

fn insert_orphan_terminal_record(
    connection: &Connection,
    idempotency_key: &str,
    job_id: &JobIdV1,
    request_digest: &Sha256Digest,
    terminal: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    connection.execute(
        "INSERT INTO idempotency_records (idempotency_key, request_digest, job_id, scope_kind, \
         scope_session_id, terminal_response_json, created_at_ms, updated_at_ms) VALUES \
         (?1, ?2, ?3, 'workspace', NULL, ?4, 1, 1)",
        params![
            idempotency_key,
            request_digest.as_str(),
            job_id.as_str(),
            terminal
        ],
    )?;
    Ok(())
}
const FIXTURE_SNAPSHOT_ID: &str = "00000000-0000-4000-8000-000000000004";
const FIXTURE_SESSION_ID: &str = "00000000-0000-4000-8000-000000000003";
const FIRST_ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000005";
const SECOND_ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000006";
const NON_UNIQUE_ACTIVE_ATTEMPT_INDEX_SQL: &str =
    "CREATE INDEX ux_attempts_one_active ON attempts(session_id) WHERE lifecycle = 'active'";

fn insert_running_session(connection: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    let canonical_json = snapshot_json()?;
    let snapshot_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json.as_bytes())
    ))?;
    insert_snapshot(connection, &canonical_json, &snapshot_digest)?;
    connection.execute(
        "INSERT INTO task_sessions (singleton, session_id, task_title, procedure_snapshot_id, lifecycle, \
         session_revision, active_stage_id, active_attempt_id, created_at_ms, completed_at_ms, \
         cancelled_at_ms, cancel_reason) VALUES \
         (1, ?1, 'Integrity task', ?2, 'running', 1, 'first', ?3, 1, NULL, NULL, NULL)",
        params![FIXTURE_SESSION_ID, FIXTURE_SNAPSHOT_ID, FIRST_ATTEMPT_ID],
    )?;
    connection.execute(
        "INSERT INTO stage_progress (session_id, stage_id, stage_index, progress_state, \
         latest_attempt_number, latest_attempt_id) VALUES (?1, 'first', 0, 'current', 1, ?2)",
        params![FIXTURE_SESSION_ID, FIRST_ATTEMPT_ID],
    )?;
    connection.execute(
        "INSERT INTO attempts (attempt_id, session_id, stage_id, attempt_number, lifecycle, \
         started_at_ms) VALUES (?1, ?2, 'first', 1, 'active', 1)",
        params![FIRST_ATTEMPT_ID, FIXTURE_SESSION_ID],
    )?;
    connection.execute(
        "INSERT INTO item_slots (attempt_id, item_id, item_type, item_revision, value_json, \
         created_at_ms, updated_at_ms) VALUES (?1, 'done', 'confirm', 0, NULL, 1, 1)",
        [FIRST_ATTEMPT_ID],
    )?;
    Ok(())
}

fn insert_second_active_attempt(connection: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    connection.execute(
        "INSERT INTO attempts (attempt_id, session_id, stage_id, attempt_number, lifecycle, \
         started_at_ms) VALUES (?1, ?2, 'first', 2, 'active', 2)",
        params![SECOND_ATTEMPT_ID, FIXTURE_SESSION_ID],
    )?;
    connection.execute(
        "UPDATE stage_progress SET latest_attempt_number = 2, latest_attempt_id = ?1 \
         WHERE session_id = ?2 AND stage_id = 'first'",
        params![SECOND_ATTEMPT_ID, FIXTURE_SESSION_ID],
    )?;
    connection.execute(
        "UPDATE task_sessions SET active_attempt_id = ?1 WHERE singleton = 1",
        [SECOND_ATTEMPT_ID],
    )?;
    Ok(())
}

fn replace_active_attempt_index_sql(
    path: &Path,
    replacement: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let connection = Connection::open(path)?;
    let original: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = 'ux_attempts_one_active'",
        [],
        |row| row.get(0),
    )?;
    let schema_version: i64 =
        connection.query_row("PRAGMA schema_version", [], |row| row.get(0))?;
    connection.pragma_update(None, "writable_schema", "ON")?;
    assert_eq!(
        connection.execute(
            "UPDATE sqlite_schema SET sql = ?1 \
             WHERE type = 'index' AND name = 'ux_attempts_one_active'",
            [replacement],
        )?,
        1
    );
    connection.pragma_update(None, "writable_schema", "OFF")?;
    connection.pragma_update(
        None,
        "schema_version",
        schema_version
            .checked_add(1)
            .expect("fixture schema version must not overflow"),
    )?;
    Ok(original)
}

fn checkpoint_wal(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

fn database_page_metadata(
    path: &Path,
    object_name: &str,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let root_page: i64 = connection.query_row(
        "SELECT rootpage FROM sqlite_schema WHERE name = ?1",
        [object_name],
        |row| row.get(0),
    )?;
    let page_size: i64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let root_page = usize::try_from(root_page)?;
    let page_size = usize::try_from(page_size)?;
    assert!(
        root_page > 0,
        "fixture object {object_name} must have a root page"
    );
    assert!(
        page_size > 0,
        "fixture database must have a positive page size"
    );
    Ok((root_page, page_size))
}

fn btree_header_offset(page_number: usize, page_size: usize) -> usize {
    let page_offset = page_number
        .checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .expect("fixture page number and size must not overflow");
    if page_number == 1 {
        page_offset + 100
    } else {
        page_offset
    }
}

fn read_big_endian_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(
        bytes
            .get(offset..offset + 2)
            .expect("fixture database page must contain a u16")
            .try_into()
            .expect("fixture u16 must have two bytes"),
    )
}

fn read_sqlite_varint(bytes: &[u8], offset: usize) -> (u64, usize) {
    let mut value = 0_u64;
    for index in 0..8 {
        let byte = *bytes
            .get(offset + index)
            .expect("fixture database page must contain a SQLite varint");
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return (value, index + 1);
        }
    }
    let byte = *bytes
        .get(offset + 8)
        .expect("fixture database page must contain a nine-byte SQLite varint");
    ((value << 8) | u64::from(byte), 9)
}

fn corrupt_empty_table_cell_count(
    path: &Path,
    table_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    checkpoint_wal(path)?;
    let (page_number, page_size) = database_page_metadata(path, table_name)?;
    let mut bytes = fs::read(path)?;
    let header_offset = btree_header_offset(page_number, page_size);
    assert_eq!(bytes[header_offset], 0x0d);
    assert_eq!(read_big_endian_u16(&bytes, header_offset + 3), 0);
    bytes[header_offset + 3..header_offset + 5].copy_from_slice(&1_u16.to_be_bytes());
    fs::write(path, bytes)?;
    Ok(())
}

fn insert_operational_journal_row(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::open(path)?;
    connection.execute(
        "INSERT INTO operational_journal (recorded_at_ms, level, event_name, summary) \
         VALUES (42, 'info', 'deep-check-fixture', 'deep-check-fixture')",
        [],
    )?;
    Ok(())
}

fn corrupt_operational_journal_index_consistency(
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    checkpoint_wal(path)?;

    let (page_number, page_size) = database_page_metadata(path, "operational_journal")?;
    let mut bytes = fs::read(path)?;
    let header_offset = btree_header_offset(page_number, page_size);
    assert_eq!(bytes[header_offset], 0x0d);
    assert_eq!(read_big_endian_u16(&bytes, header_offset + 3), 1);

    let cell_offset =
        (page_number - 1) * page_size + usize::from(read_big_endian_u16(&bytes, header_offset + 8));
    let (payload_size, payload_size_width) = read_sqlite_varint(&bytes, cell_offset);
    let (_, rowid_width) = read_sqlite_varint(&bytes, cell_offset + payload_size_width);
    let record_offset = cell_offset + payload_size_width + rowid_width;
    assert!(
        record_offset + usize::try_from(payload_size)? <= page_number * page_size,
        "fixture operational journal record must fit in its root page"
    );

    let (header_size, header_size_width) = read_sqlite_varint(&bytes, record_offset);
    let header_size = usize::try_from(header_size)?;
    let header_end = record_offset + header_size;
    let mut serial_offset = record_offset + header_size_width;
    let mut serial_types = Vec::new();
    while serial_offset < header_end {
        let (serial_type, width) = read_sqlite_varint(&bytes, serial_offset);
        serial_types.push(serial_type);
        serial_offset += width;
    }
    assert_eq!(serial_offset, header_end);
    assert_eq!(serial_types, vec![0, 1, 21, 49, 0, 0, 49, 0]);

    let content_offset = record_offset + header_size;
    assert_eq!(bytes[content_offset], 42);
    bytes[content_offset] = 43;
    fs::write(path, bytes)?;
    Ok(())
}

#[test]
fn deep_inspection_reports_all_checks_without_mutating_durable_or_logical_contents()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let path = initialize(&temporary)?;
    let durable_before = snapshot_durable_database_files(&path)?;
    let logical_before = logical_contents_from_durable_snapshot(&durable_before)?;

    let report = inspect_integrity_v1(
        &path,
        &identity(),
        &options(),
        IntegrityModeV1::Deep,
        UnixMillis::new(200),
    )?;

    assert_eq!(report.mode(), IntegrityModeV1::Deep);
    assert_eq!(report.checked_at(), UnixMillis::new(200));
    assert_exact_passed_checks(&report, DEEP_INTEGRITY_CHECKS);

    let durable_after = snapshot_durable_database_files(&path)?;
    let logical_after = logical_contents_from_durable_snapshot(&durable_after)?;
    assert_eq!(durable_after, durable_before);
    assert_eq!(logical_after, logical_before);
    Ok(())
}
#[test]
fn fast_inspection_reports_exact_check_taxonomy() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let path = initialize(&temporary)?;

    let report = inspect(&path)?;

    assert_eq!(report.mode(), IntegrityModeV1::Fast);
    assert_eq!(report.checked_at(), UnixMillis::new(200));
    assert_exact_passed_checks(&report, FAST_INTEGRITY_CHECKS);
    Ok(())
}

#[test]
fn inspection_rejects_altered_or_extra_schema_migration_and_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let altered = TempDir::new()?;
    let altered_path = initialize(&altered)?;
    let connection = Connection::open(&altered_path)?;
    connection.execute_batch(
        "DROP INDEX ix_jobs_state_sequence; \
         CREATE INDEX ix_jobs_state_sequence ON jobs(workspace_sequence);",
    )?;
    drop(connection);
    assert_integrity_error(&altered_path, StoreIntegrityCheckV1::RequiredSchemaObjects)?;

    let extra = TempDir::new()?;
    let extra_path = initialize(&extra)?;
    Connection::open(&extra_path)?
        .execute_batch("CREATE TABLE unexpected (value INTEGER) STRICT;")?;
    assert_integrity_error(&extra_path, StoreIntegrityCheckV1::RequiredSchemaObjects)?;

    let schema_version = TempDir::new()?;
    let schema_version_path = initialize(&schema_version)?;
    Connection::open(&schema_version_path)?.pragma_update(None, "user_version", 0)?;
    assert_integrity_error(&schema_version_path, StoreIntegrityCheckV1::SchemaVersion)?;

    let pragmas = TempDir::new()?;
    let pragma_path = initialize(&pragmas)?;
    Connection::open(&pragma_path)?.pragma_update(None, "journal_mode", "DELETE")?;
    assert_integrity_error(&pragma_path, StoreIntegrityCheckV1::ConnectionPragmas)?;
    let migration = TempDir::new()?;
    let migration_path = initialize(&migration)?;
    Connection::open(&migration_path)?.execute(
        "UPDATE schema_migrations SET checksum = 'sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
        [],
    )?;
    assert_integrity_error(&migration_path, StoreIntegrityCheckV1::MigrationChecksum)?;

    let identity_mismatch = TempDir::new()?;
    let identity_path = initialize(&identity_mismatch)?;
    let durable_before_identity_check = snapshot_durable_database_files(&identity_path)?;
    assert!(matches!(
        inspect_integrity_v1(
            &identity_path,
            &other_identity(),
            &options(),
            IntegrityModeV1::Fast,
            UnixMillis::new(200),
        ),
        Err(StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::WorkspaceIdentity
        })
    ));
    let durable_after_identity_check = snapshot_durable_database_files(&identity_path)?;
    assert_eq!(durable_after_identity_check, durable_before_identity_check);
    Ok(())
}

#[test]
fn inspection_rejects_foreign_keys_snapshots_and_queue_invariants()
-> Result<(), Box<dyn std::error::Error>> {
    let foreign_keys = TempDir::new()?;
    let foreign_key_path = initialize(&foreign_keys)?;
    let connection = Connection::open(&foreign_key_path)?;
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    connection.execute(
        "INSERT INTO task_sessions (singleton, session_id, task_title, procedure_snapshot_id, lifecycle, \
         session_revision, active_stage_id, active_attempt_id, created_at_ms, completed_at_ms, \
         cancelled_at_ms, cancel_reason) VALUES \
         (1, '00000000-0000-4000-8000-000000000003', 'task', 'missing-snapshot', 'completed', 1, \
         NULL, NULL, 1, 2, NULL, NULL)",
        [],
    )?;
    drop(connection);
    assert_integrity_error(&foreign_key_path, StoreIntegrityCheckV1::ForeignKeys)?;

    let snapshots = TempDir::new()?;
    let snapshot_path = initialize(&snapshots)?;
    let canonical_json = snapshot_json()?;
    let correct_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json.as_bytes())
    ))?;
    let incorrect_digest = digest('f');
    assert_ne!(correct_digest, incorrect_digest);
    let connection = Connection::open(&snapshot_path)?;
    insert_snapshot(&connection, &canonical_json, &correct_digest)?;
    drop(connection);
    assert!(inspect(&snapshot_path).is_ok());

    Connection::open(&snapshot_path)?.execute(
        "UPDATE procedure_snapshots SET digest = ?1 WHERE snapshot_id = \
         '00000000-0000-4000-8000-000000000004'",
        [incorrect_digest.as_str()],
    )?;
    assert_integrity_error(&snapshot_path, StoreIntegrityCheckV1::SnapshotDigest)?;

    let queue = TempDir::new()?;
    let queue_path = initialize(&queue)?;
    let connection = Connection::open(&queue_path)?;
    connection.execute(
        "INSERT INTO jobs (job_id, workspace_sequence, idempotency_key, request_digest, command_name, \
         canonical_request_json, state, submitted_at_ms) VALUES \
         ('00000000-0000-4000-8000-000000000002', 1, 'queue-key', \
         'sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd', \
         'workspace.initialize', '{}', 'queued', 1)",
        [],
    )?;
    connection.execute("UPDATE workspace_state SET next_workspace_sequence = 0", [])?;
    drop(connection);
    assert_integrity_error(&queue_path, StoreIntegrityCheckV1::JobQueue)?;
    Ok(())
}

#[test]
fn inspection_rejects_malformed_codecs_idempotency_and_invalid_orphan_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    let codecs = TempDir::new()?;
    let codec_path = initialize(&codecs)?;
    let connection = Connection::open(&codec_path)?;
    insert_live_job(&connection, "{}", &digest('d'))?;
    drop(connection);
    assert_integrity_error(&codec_path, StoreIntegrityCheckV1::InternalCodec)?;

    let idempotency = TempDir::new()?;
    let idempotency_path = initialize(&idempotency)?;
    let preconditions = RevisionAttemptItemPreconditionsV1::new(None, None, None, None)?;
    let canonical_request = encode_command_v1(&ClaimedExecutionV1::new(
        DomainCommand::WorkspaceInitialize,
        preconditions,
    ))?;
    let connection = Connection::open(&idempotency_path)?;
    insert_live_job(&connection, &canonical_request, &digest('e'))?;
    drop(connection);
    assert_integrity_error(&idempotency_path, StoreIntegrityCheckV1::IdempotencyReceipt)?;

    let orphan = TempDir::new()?;
    let orphan_path = initialize(&orphan)?;
    let orphan_job = job_id();
    let orphan_digest = digest('d');
    let terminal = encode_persisted_terminal_receipt_v1(&PersistedTerminalReceiptV1::cancelled(
        JobReceiptV1::new(1, orphan_job.clone(), orphan_digest.clone()),
    ))?;
    let connection = Connection::open(&orphan_path)?;
    connection.execute("UPDATE workspace_state SET next_workspace_sequence = 1", [])?;
    insert_orphan_terminal_record(
        &connection,
        "in-range-orphan-key",
        &orphan_job,
        &orphan_digest,
        &terminal,
    )?;
    drop(connection);
    assert!(inspect(&orphan_path).is_ok());

    let zero_orphan = TempDir::new()?;
    let zero_orphan_path = initialize(&zero_orphan)?;
    let zero_terminal = format!(
        r#"{{"job":{{"identity_sequence":0,"job_id":"{}","request_digest":"{}"}},"result":{{"kind":"cancelled"}},"schema":"podway.store-terminal/v1"}}"#,
        orphan_job.as_str(),
        orphan_digest.as_str(),
    );
    let connection = Connection::open(&zero_orphan_path)?;
    connection.execute("UPDATE workspace_state SET next_workspace_sequence = 1", [])?;
    insert_orphan_terminal_record(
        &connection,
        "zero-orphan-key",
        &orphan_job,
        &orphan_digest,
        &zero_terminal,
    )?;
    drop(connection);
    assert_integrity_error(&zero_orphan_path, StoreIntegrityCheckV1::IdempotencyReceipt)?;

    let future_orphan = TempDir::new()?;
    let future_orphan_path = initialize(&future_orphan)?;
    let future_terminal =
        encode_persisted_terminal_receipt_v1(&PersistedTerminalReceiptV1::cancelled(
            JobReceiptV1::new(2, orphan_job.clone(), orphan_digest.clone()),
        ))?;
    let connection = Connection::open(&future_orphan_path)?;
    connection.execute("UPDATE workspace_state SET next_workspace_sequence = 1", [])?;
    insert_orphan_terminal_record(
        &connection,
        "future-orphan-key",
        &orphan_job,
        &orphan_digest,
        &future_terminal,
    )?;
    drop(connection);
    assert_integrity_error(
        &future_orphan_path,
        StoreIntegrityCheckV1::IdempotencyReceipt,
    )?;
    Ok(())
}
#[test]
fn fast_inspection_rejects_physical_table_damage_as_sqlite_quick_check()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let path = initialize(&temporary)?;
    let clean_report = inspect_with_mode(&path, IntegrityModeV1::Fast)?;
    assert_exact_passed_checks(&clean_report, FAST_INTEGRITY_CHECKS);

    corrupt_empty_table_cell_count(&path, "operational_journal")?;

    assert_integrity_error_without_mutation(
        &path,
        IntegrityModeV1::Fast,
        StoreIntegrityCheckV1::SqliteQuickCheck,
    )?;
    Ok(())
}

#[test]
fn fast_inspection_rejects_duplicate_active_attempts_with_exact_classification()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let path = initialize(&temporary)?;
    let connection = Connection::open(&path)?;
    insert_running_session(&connection)?;
    drop(connection);
    let clean_report = inspect_with_mode(&path, IntegrityModeV1::Fast)?;
    assert_exact_passed_checks(&clean_report, FAST_INTEGRITY_CHECKS);

    let original_sql =
        replace_active_attempt_index_sql(&path, NON_UNIQUE_ACTIVE_ATTEMPT_INDEX_SQL)?;
    let connection = Connection::open(&path)?;
    insert_second_active_attempt(&connection)?;
    drop(connection);
    assert_eq!(
        replace_active_attempt_index_sql(&path, &original_sql)?,
        NON_UNIQUE_ACTIVE_ATTEMPT_INDEX_SQL
    );

    assert_integrity_error_without_mutation(
        &path,
        IntegrityModeV1::Fast,
        StoreIntegrityCheckV1::ActiveAttempt,
    )?;
    Ok(())
}

#[test]
fn fast_inspection_rejects_a_dangling_active_attempt_cursor_with_exact_classification()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let path = initialize(&temporary)?;
    let connection = Connection::open(&path)?;
    insert_running_session(&connection)?;
    drop(connection);
    let clean_report = inspect_with_mode(&path, IntegrityModeV1::Fast)?;
    assert_exact_passed_checks(&clean_report, FAST_INTEGRITY_CHECKS);

    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE task_sessions SET active_attempt_id = ?1 WHERE singleton = 1",
        [SECOND_ATTEMPT_ID],
    )?;
    drop(connection);

    assert_integrity_error_without_mutation(
        &path,
        IntegrityModeV1::Fast,
        StoreIntegrityCheckV1::SessionCursor,
    )?;
    Ok(())
}

#[test]
fn deep_inspection_rejects_an_index_table_mismatch_after_fast_inspection_passes()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let path = initialize(&temporary)?;
    insert_operational_journal_row(&path)?;
    let clean_fast_report = inspect_with_mode(&path, IntegrityModeV1::Fast)?;
    assert_exact_passed_checks(&clean_fast_report, FAST_INTEGRITY_CHECKS);

    corrupt_operational_journal_index_consistency(&path)?;

    let durable_before_fast_inspection = snapshot_durable_database_files(&path)?;
    let fast_report = inspect_with_mode(&path, IntegrityModeV1::Fast)?;
    assert_exact_passed_checks(&fast_report, FAST_INTEGRITY_CHECKS);
    assert_eq!(
        snapshot_durable_database_files(&path)?,
        durable_before_fast_inspection
    );
    assert_integrity_error_without_mutation(
        &path,
        IntegrityModeV1::Deep,
        StoreIntegrityCheckV1::SqliteDeepCheck,
    )?;
    Ok(())
}
