//! SQLite connection setup and schema initialization.

#[cfg(unix)]
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use podway_core::Sha256Digest;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::codec::{
    PersistedDomainResultV1, PersistedTerminalResultV1, decode_command_v1,
    decode_terminal_receipt_v1, encode_command_v1, encode_persisted_terminal_receipt_v1,
    normalize_terminal_receipt_for_schema_v4_v1, validate_persisted_terminal_result_for_command_v1,
};
use crate::v2_state::verify_v2_graph_state_connection_v2;
use crate::{
    DurableWorktreeIdentityV1, EpochMillisV1, IdempotencyKeyV1, IntegrityCheckResultV1,
    IntegrityModeV1, IntegrityReportV1, JobIdV1, MAX_SQLITE_BUSY_TIMEOUT_MS_V1,
    RusqliteErrorContextV1, SqliteStoreOptionsV1, StoreErrorV1, StoreFailpointV1,
    StoreIntegrityCheckV1, StoreUnavailableReasonV1, ValidatedWorkspaceRootV1,
    command_is_session_scoped_v1, command_name_v1, map_rusqlite_error_v1,
};
const TEMPORARY_OWNERSHIP_MARKER_HEADER_V1: &str = "podway-store temporary ownership v2\n";

pub(crate) fn write_temporary_ownership_marker_v1(
    marker: &mut File,
    temporary: &File,
) -> Result<(), StoreErrorV1> {
    #[cfg(unix)]
    {
        let metadata = temporary.metadata().map_err(storage_io_error)?;
        let record = format!(
            "{TEMPORARY_OWNERSHIP_MARKER_HEADER_V1}device={}\ninode={}\n",
            metadata.dev(),
            metadata.ino()
        );
        marker
            .write_all(record.as_bytes())
            .and_then(|()| marker.sync_all())
            .map_err(storage_io_error)
    }
    #[cfg(not(unix))]
    {
        let _ = temporary;
        marker
            .write_all(TEMPORARY_OWNERSHIP_MARKER_HEADER_V1.as_bytes())
            .and_then(|()| marker.sync_all())
            .map_err(storage_io_error)
    }
}

pub const SQLITE_SCHEMA_VERSION_V1: u32 = 1;
pub const SQLITE_SCHEMA_VERSION_V2: u32 = 2;
pub const SQLITE_SCHEMA_VERSION_V3: u32 = 3;
pub const SQLITE_SCHEMA_VERSION_V4: u32 = 4;
pub const SQLITE_SCHEMA_VERSION_V5: u32 = 5;
pub const SQLITE_SCHEMA_VERSION_CURRENT: u32 = SQLITE_SCHEMA_VERSION_V5;
pub const SQLITE_INITIAL_MIGRATION_NAME_V1: &str = "schema-0-uninitialized";
pub const SQLITE_RESPONSE_CONTEXT_MIGRATION_NAME_V2: &str = "schema-1-response-context";
pub const SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3: &str = "schema-2-procedure-v2-state";
pub const SQLITE_V2_ONLY_MIGRATION_NAME_V4: &str = "schema-3-procedure-v2-only";
pub const SQLITE_PREPARED_LIFECYCLE_MIGRATION_NAME_V5: &str = "schema-4-prepared-session-lifecycle";

const SQLITE_V1_DDL: &str = include_str!("../../../assets/specifications/sqlite-v1.sql");
const SQLITE_V2_DDL: &str = include_str!("../../../assets/specifications/sqlite-v2.sql");
const SQLITE_V3_DDL: &str = include_str!("../../../assets/specifications/sqlite-v3.sql");
const SQLITE_V4_DDL: &str = include_str!("../../../assets/specifications/sqlite-v4.sql");
const SQLITE_V5_DDL: &str = include_str!("../../../assets/specifications/sqlite-v5.sql");
const CONNECTION_PRAGMA_PREAMBLE_V1: &str = concat!(
    "PRAGMA foreign_keys = ON;\n",
    "PRAGMA journal_mode = WAL;\n",
    "PRAGMA synchronous = FULL;\n",
    "PRAGMA busy_timeout = 5000;\n",
    "PRAGMA trusted_schema = OFF;\n\n",
);
const USER_VERSION_SUFFIX_V1: &str = "PRAGMA user_version = 1;\n";
const USER_VERSION_SUFFIX_V2: &str = "PRAGMA user_version = 2;\n";
const USER_VERSION_SUFFIX_V3: &str = "PRAGMA user_version = 3;\n";
const USER_VERSION_SUFFIX_V4: &str = "PRAGMA user_version = 4;\n";
const USER_VERSION_SUFFIX_V5: &str = "PRAGMA user_version = 5;\n";

/// The exact immutable bytes of the canonical v1 migration.
pub fn sqlite_v1_ddl() -> &'static str {
    SQLITE_V1_DDL
}

/// SHA-256 of the exact immutable bytes returned by [`sqlite_v1_ddl`].
pub fn sqlite_v1_ddl_checksum() -> String {
    let digest = Sha256::digest(SQLITE_V1_DDL.as_bytes());
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut checksum = String::with_capacity("sha256:".len() + digest.len() * 2);
    checksum.push_str("sha256:");
    for byte in digest {
        checksum.push(char::from(HEX[usize::from(byte >> 4)]));
        checksum.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    checksum
}

pub fn sqlite_v2_ddl() -> &'static str {
    SQLITE_V2_DDL
}

pub fn sqlite_v2_ddl_checksum() -> String {
    let digest = Sha256::digest(SQLITE_V2_DDL.as_bytes());
    format!("sha256:{digest:x}")
}

pub fn sqlite_v3_ddl() -> &'static str {
    SQLITE_V3_DDL
}

pub fn sqlite_v3_ddl_checksum() -> String {
    let digest = Sha256::digest(SQLITE_V3_DDL.as_bytes());
    format!("sha256:{digest:x}")
}

pub fn sqlite_v4_ddl() -> &'static str {
    SQLITE_V4_DDL
}

pub fn sqlite_v4_ddl_checksum() -> String {
    let digest = Sha256::digest(SQLITE_V4_DDL.as_bytes());
    format!("sha256:{digest:x}")
}

pub fn sqlite_v5_ddl() -> &'static str {
    SQLITE_V5_DDL
}

pub fn sqlite_v5_ddl_checksum() -> String {
    let digest = Sha256::digest(SQLITE_V5_DDL.as_bytes());
    format!("sha256:{digest:x}")
}

/// Opens a SQLite database only after the current schema, required pragmas, and durable identity verify.
///
/// A version-zero database is initialized only when it has no user schema objects. The validated
/// root is updated in a separate short write transaction after the immutable identity matches.
pub fn open_or_initialize_v1(
    path: impl AsRef<Path>,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<Connection, StoreErrorV1> {
    open_or_initialize_with_temporary_cleanup_arm_v1(
        path.as_ref(),
        root,
        identity,
        options,
        now,
        None,
    )
}

pub(crate) fn open_or_initialize_with_temporary_cleanup_arm_v1(
    path: &Path,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
    temporary_cleanup_armed: Option<&mut bool>,
) -> Result<Connection, StoreErrorV1> {
    let path = canonical_database_path_v1(path)?;
    recover_interrupted_publication_v1(&path)?;
    prepare_database_path_for_write_open_v1(&path)?;
    let mut connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_error)?;
    validate_existing_database_path_v1(&path)?;
    apply_connection_pragmas_v1(&connection, options.busy_timeout_ms())?;

    match read_user_version_v1(&connection)? {
        0 => {
            if user_schema_object_count_v1(&connection)? != 0 {
                return Err(integrity_error(
                    StoreIntegrityCheckV1::RequiredSchemaObjects,
                ));
            }
            if let Some(
                failpoint @ (StoreFailpointV1::SchemaAfterPragmas
                | StoreFailpointV1::SchemaAfterPragmasAndTemporaryCleanup),
            ) = options.failpoint()
            {
                if let (StoreFailpointV1::SchemaAfterPragmasAndTemporaryCleanup, Some(armed)) =
                    (failpoint, temporary_cleanup_armed)
                {
                    *armed = true;
                }
                options.trigger_failpoint(failpoint)?;
            }
            run_schema_migration_v1(&mut connection, |connection| {
                initialize_empty_schema_v1(connection, root, identity, options, now)
            })?;
        }
        SQLITE_SCHEMA_VERSION_V1 => run_schema_migration_v1(&mut connection, |connection| {
            migrate_schema_v1_to_v5(connection, identity, options, now)
        })?,
        SQLITE_SCHEMA_VERSION_V2 => run_schema_migration_v1(&mut connection, |connection| {
            migrate_schema_v2_to_v5(connection, identity, options, now)
        })?,
        SQLITE_SCHEMA_VERSION_V3 => run_schema_migration_v1(&mut connection, |connection| {
            migrate_schema_v3_to_v5(connection, identity, options, now)
        })?,
        SQLITE_SCHEMA_VERSION_V4 => run_schema_migration_v1(&mut connection, |connection| {
            migrate_schema_v4_to_v5(connection, identity, options, now)
        })?,
        SQLITE_SCHEMA_VERSION_V5 => {}
        found if found > SQLITE_SCHEMA_VERSION_CURRENT => {
            return Err(StoreErrorV1::NewerStateV1 {
                found_schema_version: found,
                supported_schema_version: SQLITE_SCHEMA_VERSION_CURRENT,
            });
        }
        _ => return Err(integrity_error(StoreIntegrityCheckV1::SchemaVersion)),
    }

    let _report = verify_integrity_connection_v1(
        &mut connection,
        identity,
        options,
        IntegrityModeV1::Fast,
        now,
    )?;
    update_validated_root_v1(&mut connection, root, identity, now)?;
    if let Some(max_page_count) = options.max_page_count_for_test() {
        connection
            .execute_batch(&format!("PRAGMA max_page_count = {max_page_count};"))
            .map_err(storage_error)?;
    }
    validate_existing_database_path_v1(&path)?;
    Ok(connection)
}

/// Applies the required SQLite durability and safety pragmas and verifies their exact values.
pub fn apply_connection_pragmas_v1(
    connection: &Connection,
    busy_timeout_ms: u32,
) -> Result<(), StoreErrorV1> {
    if busy_timeout_ms == 0 || busy_timeout_ms > MAX_SQLITE_BUSY_TIMEOUT_MS_V1 {
        return Err(integrity_error(StoreIntegrityCheckV1::ConnectionPragmas));
    }

    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "busy_timeout", busy_timeout_ms)
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "trusted_schema", "OFF")
        .map_err(storage_error)?;

    verify_connection_pragmas_v1(connection, busy_timeout_ms)
}

fn run_schema_migration_v1<T>(
    connection: &mut Connection,
    operation: impl FnOnce(&mut Connection) -> Result<T, StoreErrorV1>,
) -> Result<T, StoreErrorV1> {
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .map_err(storage_error)?;
    let result = operation(connection);
    let restored = connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(storage_error);
    match result {
        Ok(value) => {
            restored?;
            Ok(value)
        }
        Err(error) => {
            let _ = restored;
            Err(error)
        }
    }
}

/// Verifies the pragma state required by the canonical SQLite schema.
pub fn verify_connection_pragmas_v1(
    connection: &Connection,
    busy_timeout_ms: u32,
) -> Result<(), StoreErrorV1> {
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(storage_error)?;
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(storage_error)?;
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(storage_error)?;
    let actual_busy_timeout: i64 = connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .map_err(storage_error)?;
    let trusted_schema: i64 = connection
        .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
        .map_err(storage_error)?;

    if foreign_keys != 1
        || !journal_mode.eq_ignore_ascii_case("wal")
        || synchronous != 2
        || actual_busy_timeout != i64::from(busy_timeout_ms)
        || trusted_schema != 0
    {
        return Err(integrity_error(StoreIntegrityCheckV1::ConnectionPragmas));
    }
    Ok(())
}

/// Verifies the frozen schema version, migration row/checksum, and exact DDL object set.
pub fn verify_schema_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    verify_schema_version_v1(connection)?;
    verify_exact_schema_objects_v1(connection)?;
    verify_migration_checksum_v1(connection)
}

pub(crate) fn verify_binding_inspection_schema_v1(
    connection: &Connection,
) -> Result<(), StoreErrorV1> {
    let version = read_user_version_v1(connection)?;
    match version {
        SQLITE_SCHEMA_VERSION_V1 | SQLITE_SCHEMA_VERSION_V2 | SQLITE_SCHEMA_VERSION_V3 => {
            verify_migration_predecessor_v1(connection, version)?;
            reject_legacy_procedure_state_v1(connection)
        }
        SQLITE_SCHEMA_VERSION_V4 => {
            verify_migration_predecessor_v1(connection, SQLITE_SCHEMA_VERSION_V4)?;
            verify_v2_graph_state_connection_v2(connection)
        }
        SQLITE_SCHEMA_VERSION_V5 => verify_schema_v1(connection),
        found if found > SQLITE_SCHEMA_VERSION_CURRENT => Err(StoreErrorV1::NewerStateV1 {
            found_schema_version: found,
            supported_schema_version: SQLITE_SCHEMA_VERSION_CURRENT,
        }),
        _ => Err(integrity_error(StoreIntegrityCheckV1::SchemaVersion)),
    }
}

pub(crate) fn verify_reset_binding_inspection_schema_v1(
    connection: &Connection,
) -> Result<(), StoreErrorV1> {
    let version = read_user_version_v1(connection)?;
    match version {
        SQLITE_SCHEMA_VERSION_V1 | SQLITE_SCHEMA_VERSION_V2 | SQLITE_SCHEMA_VERSION_V3 => {
            verify_migration_predecessor_v1(connection, version)
        }
        SQLITE_SCHEMA_VERSION_V4 => {
            verify_migration_predecessor_v1(connection, SQLITE_SCHEMA_VERSION_V4)?;
            verify_v2_graph_state_connection_v2(connection)
        }
        SQLITE_SCHEMA_VERSION_V5 => verify_schema_v1(connection),
        found if found > SQLITE_SCHEMA_VERSION_CURRENT => Err(StoreErrorV1::NewerStateV1 {
            found_schema_version: found,
            supported_schema_version: SQLITE_SCHEMA_VERSION_CURRENT,
        }),
        _ => Err(integrity_error(StoreIntegrityCheckV1::SchemaVersion)),
    }
}

pub(crate) fn verify_binding_inspection_identity_v1(
    connection: &mut Connection,
    expected_identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
) -> Result<(), StoreErrorV1> {
    if read_user_version_v1(connection)? == SQLITE_SCHEMA_VERSION_CURRENT {
        verify_inspection_integrity_connection_v1(
            connection,
            expected_identity,
            options,
            IntegrityModeV1::Fast,
            EpochMillisV1::new(0),
        )?;
    } else {
        verify_connection_pragmas_v1(connection, options.busy_timeout_ms())?;
        verify_workspace_identity_v1(connection, expected_identity)?;
    }
    Ok(())
}

/// Inspects an existing database through a disposable byte-for-byte clone.
///
/// Bound integrity inspection finalizes a proven Store-owned interrupted publication before cloning.
/// Copying the finalized database and its sidecars before opening SQLite keeps inspection physically
/// read-only with respect to the authoritative workspace files.
pub fn inspect_integrity_v1(
    path: impl AsRef<Path>,
    expected_identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    mode: IntegrityModeV1,
    checked_at: EpochMillisV1,
) -> Result<IntegrityReportV1, StoreErrorV1> {
    inspect_database_snapshot_v1(
        path.as_ref(),
        expected_identity,
        options,
        mode,
        checked_at,
        |_| Ok(()),
    )
    .map(|(report, ())| report)
}

pub(crate) fn inspect_database_snapshot_v1<T>(
    path: &Path,
    expected_identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    mode: IntegrityModeV1,
    checked_at: EpochMillisV1,
    inspect: impl FnOnce(&Connection) -> Result<T, StoreErrorV1>,
) -> Result<(IntegrityReportV1, T), StoreErrorV1> {
    let path = canonical_database_path_v1(path)?;
    recover_interrupted_publication_v1(&path)?;
    inspect_database_snapshot_unbound_v1(&path, options, |connection| {
        let report = verify_inspection_integrity_connection_v1(
            connection,
            expected_identity,
            options,
            mode,
            checked_at,
        )?;
        let inspected = inspect(connection)?;
        Ok((report, inspected))
    })
}

/// Opens only a disposable, validated byte-for-byte database snapshot for inspection.
///
/// Bound callers must finalize any interrupted publication before calling this helper. This helper
/// never recovers a publication or otherwise mutates authoritative database or sidecar paths. The
/// callback must derive any expected identity from the snapshot before asking it to validate that identity.
pub(crate) fn inspect_database_snapshot_unbound_v1<T>(
    path: &Path,
    options: &SqliteStoreOptionsV1,
    inspect: impl FnOnce(&mut Connection) -> Result<T, StoreErrorV1>,
) -> Result<T, StoreErrorV1> {
    let path = canonical_database_path_v1(path)?;
    validate_existing_database_path_v1(&path)?;
    let inspection_directory = TempDir::new().map_err(storage_io_error)?;
    let inspection_path = fs::canonicalize(inspection_directory.path())
        .map_err(storage_io_error)?
        .join("inspection.sqlite3");
    copy_database_for_inspection_v1(&path, &inspection_path)?;
    let mut connection = Connection::open_with_flags(
        &inspection_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_error)?;
    apply_inspection_pragmas_v1(&connection, options.busy_timeout_ms())?;
    inspect(&mut connection)
}
/// Verifies an already-configured disposable inspection connection without changing its pragmas.
pub(crate) fn verify_inspection_integrity_connection_v1(
    connection: &mut Connection,
    expected_identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    mode: IntegrityModeV1,
    checked_at: EpochMillisV1,
) -> Result<IntegrityReportV1, StoreErrorV1> {
    verify_integrity_connection_inner_v1(
        connection,
        expected_identity,
        options,
        mode,
        checked_at,
        true,
    )
}

/// Runs the startup integrity preflight before callers perform durable state mutations.
pub(crate) fn verify_integrity_connection_v1(
    connection: &mut Connection,
    expected_identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    mode: IntegrityModeV1,
    checked_at: EpochMillisV1,
) -> Result<IntegrityReportV1, StoreErrorV1> {
    apply_connection_pragmas_v1(connection, options.busy_timeout_ms())?;
    verify_integrity_connection_inner_v1(
        connection,
        expected_identity,
        options,
        mode,
        checked_at,
        true,
    )
}

fn verify_integrity_connection_inner_v1(
    connection: &Connection,
    expected_identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    mode: IntegrityModeV1,
    checked_at: EpochMillisV1,
    verify_connection_pragmas: bool,
) -> Result<IntegrityReportV1, StoreErrorV1> {
    let mut checks = Vec::new();

    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::SchemaVersion,
        verify_schema_version_v1(connection),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::RequiredSchemaObjects,
        verify_exact_schema_objects_v1(connection),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::MigrationChecksum,
        verify_migration_checksum_v1(connection),
    )?;
    if verify_connection_pragmas {
        record_integrity_check_v1(
            &mut checks,
            StoreIntegrityCheckV1::ConnectionPragmas,
            verify_connection_pragmas_v1(connection, options.busy_timeout_ms()),
        )?;
    }
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::WorkspaceIdentity,
        verify_workspace_identity_v1(connection, expected_identity),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::SqliteQuickCheck,
        verify_sqlite_check_v1(
            connection,
            "quick_check",
            StoreIntegrityCheckV1::SqliteQuickCheck,
        ),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::ForeignKeys,
        verify_foreign_keys_v1(connection),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::SessionCursor,
        verify_v2_graph_state_connection_v2(connection)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::SessionCursor)),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::JobQueue,
        verify_job_queue_v1(connection),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::InternalCodec,
        verify_job_codecs_v1(connection),
    )?;
    record_integrity_check_v1(
        &mut checks,
        StoreIntegrityCheckV1::IdempotencyReceipt,
        verify_idempotency_receipts_v1(connection),
    )?;
    if mode == IntegrityModeV1::Deep {
        record_integrity_check_v1(
            &mut checks,
            StoreIntegrityCheckV1::SqliteDeepCheck,
            verify_sqlite_check_v1(
                connection,
                "integrity_check",
                StoreIntegrityCheckV1::SqliteDeepCheck,
            ),
        )?;
    }

    Ok(IntegrityReportV1::new(mode, checked_at, checks))
}

fn record_integrity_check_v1(
    checks: &mut Vec<IntegrityCheckResultV1>,
    check: StoreIntegrityCheckV1,
    result: Result<(), StoreErrorV1>,
) -> Result<(), StoreErrorV1> {
    result?;
    checks.push(IntegrityCheckResultV1::new(check, true));
    Ok(())
}

/// Verifies the singleton identity and validates its lossless root encoding without mutating it.
pub fn verify_workspace_identity_v1(
    connection: &Connection,
    identity: &DurableWorktreeIdentityV1,
) -> Result<(), StoreErrorV1> {
    let workspace_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM workspace_state", [], |row| row.get(0))
        .map_err(storage_error)?;
    if workspace_count != 1 {
        return Err(integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity));
    }

    let row: (String, String, String, String) = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, last_validated_root \
             FROM workspace_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(storage_error)?;

    let stored_workspace = crate::WorkspaceUuidV1::new(row.0)
        .map_err(|_| integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity))?;
    let stored_common = crate::GitIdentityV1::new(row.1)
        .map_err(|_| integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity))?;
    let stored_worktree = crate::GitIdentityV1::new(row.2)
        .map_err(|_| integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity))?;
    ValidatedWorkspaceRootV1::from_encoded(row.3)
        .map_err(|_| integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity))?;

    if stored_workspace != *identity.workspace_uuid()
        || stored_common != *identity.common_dir_identity()
        || stored_worktree != *identity.worktree_admin_identity()
    {
        return Err(integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity));
    }
    Ok(())
}

fn apply_inspection_pragmas_v1(
    connection: &Connection,
    busy_timeout_ms: u32,
) -> Result<(), StoreErrorV1> {
    if busy_timeout_ms == 0 || busy_timeout_ms > MAX_SQLITE_BUSY_TIMEOUT_MS_V1 {
        return Err(integrity_error(StoreIntegrityCheckV1::ConnectionPragmas));
    }

    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "busy_timeout", busy_timeout_ms)
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "trusted_schema", "OFF")
        .map_err(storage_error)?;
    connection
        .pragma_update(None, "query_only", "ON")
        .map_err(storage_error)?;

    verify_connection_pragmas_v1(connection, busy_timeout_ms)
}

fn verify_schema_version_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    if read_user_version_v1(connection)? == SQLITE_SCHEMA_VERSION_CURRENT {
        Ok(())
    } else {
        Err(integrity_error(StoreIntegrityCheckV1::SchemaVersion))
    }
}

fn verify_migration_checksum_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    verify_migration_checksum_through_v1(connection, SQLITE_SCHEMA_VERSION_CURRENT)
}

fn verify_migration_checksum_through_v1(
    connection: &Connection,
    expected_version: u32,
) -> Result<(), StoreErrorV1> {
    let migration_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(storage_error)?;
    let migrations: Vec<(i64, String, String)> = {
        let mut statement = connection
            .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(storage_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)?
    };
    let expected = expected_migrations_through_v1(expected_version)?;
    if migration_count
        != i64::try_from(expected.len())
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::MigrationChecksum))?
        || migrations != expected
    {
        return Err(integrity_error(StoreIntegrityCheckV1::MigrationChecksum));
    }

    Ok(())
}

fn expected_migrations_through_v1(
    expected_version: u32,
) -> Result<Vec<(i64, String, String)>, StoreErrorV1> {
    let mut expected = vec![(
        i64::from(SQLITE_SCHEMA_VERSION_V1),
        SQLITE_INITIAL_MIGRATION_NAME_V1.to_owned(),
        sqlite_v1_ddl_checksum(),
    )];
    if expected_version >= SQLITE_SCHEMA_VERSION_V2 {
        expected.push((
            i64::from(SQLITE_SCHEMA_VERSION_V2),
            SQLITE_RESPONSE_CONTEXT_MIGRATION_NAME_V2.to_owned(),
            sqlite_v2_ddl_checksum(),
        ));
    }
    if expected_version >= SQLITE_SCHEMA_VERSION_V3 {
        expected.push((
            i64::from(SQLITE_SCHEMA_VERSION_V3),
            SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3.to_owned(),
            sqlite_v3_ddl_checksum(),
        ));
    }
    if expected_version >= SQLITE_SCHEMA_VERSION_V4 {
        expected.push((
            i64::from(SQLITE_SCHEMA_VERSION_V4),
            SQLITE_V2_ONLY_MIGRATION_NAME_V4.to_owned(),
            sqlite_v4_ddl_checksum(),
        ));
    }
    if expected_version >= SQLITE_SCHEMA_VERSION_V5 {
        expected.push((
            i64::from(SQLITE_SCHEMA_VERSION_V5),
            SQLITE_PREPARED_LIFECYCLE_MIGRATION_NAME_V5.to_owned(),
            sqlite_v5_ddl_checksum(),
        ));
    }
    if !(SQLITE_SCHEMA_VERSION_V1..=SQLITE_SCHEMA_VERSION_CURRENT).contains(&expected_version) {
        return Err(integrity_error(StoreIntegrityCheckV1::SchemaVersion));
    }
    Ok(expected)
}

#[derive(Debug, Eq, PartialEq)]
struct SchemaObjectV1 {
    object_type: String,
    name: String,
    table_name: String,
    sql: Option<String>,
}

fn verify_exact_schema_objects_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    verify_exact_schema_objects_through_v1(connection, SQLITE_SCHEMA_VERSION_CURRENT)
}

fn verify_exact_schema_objects_through_v1(
    connection: &Connection,
    expected_version: u32,
) -> Result<(), StoreErrorV1> {
    let expected = expected_schema_objects_through_v1(expected_version)?;
    let actual = schema_objects_v1(connection)?;
    if actual == expected {
        Ok(())
    } else {
        Err(integrity_error(
            StoreIntegrityCheckV1::RequiredSchemaObjects,
        ))
    }
}

fn expected_schema_objects_through_v1(
    expected_version: u32,
) -> Result<Vec<SchemaObjectV1>, StoreErrorV1> {
    let reference = Connection::open_in_memory().map_err(storage_error)?;
    reference
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(storage_error)?;
    reference
        .execute_batch(migration_schema_statements_v1()?)
        .map_err(storage_error)?;
    if expected_version >= SQLITE_SCHEMA_VERSION_V2 {
        reference
            .execute_batch(migration_schema_statements_v2()?)
            .map_err(storage_error)?;
    }
    if expected_version >= SQLITE_SCHEMA_VERSION_V3 {
        reference
            .execute_batch(migration_schema_statements_v3()?)
            .map_err(storage_error)?;
    }
    if expected_version >= SQLITE_SCHEMA_VERSION_V4 {
        reference
            .execute_batch(migration_schema_statements_v4()?)
            .map_err(storage_error)?;
    }
    if expected_version >= SQLITE_SCHEMA_VERSION_V5 {
        reference
            .pragma_update(None, "foreign_keys", "OFF")
            .map_err(storage_error)?;
        reference
            .execute_batch(migration_schema_statements_v5()?)
            .map_err(storage_error)?;
    }
    if !(SQLITE_SCHEMA_VERSION_V1..=SQLITE_SCHEMA_VERSION_CURRENT).contains(&expected_version) {
        return Err(integrity_error(StoreIntegrityCheckV1::SchemaVersion));
    }
    schema_objects_v1(&reference)
}

fn schema_objects_v1(connection: &Connection) -> Result<Vec<SchemaObjectV1>, StoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name, tbl_name, sql",
        )
        .map_err(storage_error)?;
    let mut rows = statement.query([]).map_err(storage_error)?;
    let mut objects = Vec::new();
    while let Some(row) = rows.next().map_err(storage_error)? {
        objects.push(SchemaObjectV1 {
            object_type: row.get(0).map_err(storage_error)?,
            name: row.get(1).map_err(storage_error)?,
            table_name: row.get(2).map_err(storage_error)?,
            sql: row.get(3).map_err(storage_error)?,
        });
    }
    Ok(objects)
}

fn verify_sqlite_check_v1(
    connection: &Connection,
    pragma: &str,
    check: StoreIntegrityCheckV1,
) -> Result<(), StoreErrorV1> {
    let mut statement = connection
        .prepare(&format!("PRAGMA {pragma}"))
        .map_err(storage_error)?;
    let mut rows = statement.query([]).map_err(storage_error)?;
    let Some(row) = rows.next().map_err(storage_error)? else {
        return Err(integrity_error(check));
    };
    let value: String = row.get(0).map_err(storage_error)?;
    if value != "ok" || rows.next().map_err(storage_error)?.is_some() {
        return Err(integrity_error(check));
    }
    Ok(())
}

fn verify_foreign_keys_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(storage_error)?;
    let mut rows = statement.query([]).map_err(storage_error)?;
    if rows.next().map_err(storage_error)?.is_some() {
        return Err(integrity_error(StoreIntegrityCheckV1::ForeignKeys));
    }
    Ok(())
}

fn verify_job_queue_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    let invalid_sequence: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE workspace_sequence < 1)",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let running_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE state = 'running'",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let invalid_claim_time: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE (state = 'queued' AND claimed_at_ms IS NOT NULL) \
             OR (state = 'running' AND claimed_at_ms IS NULL))",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let overtaken_queue: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs running JOIN jobs queued \
             ON queued.workspace_sequence < running.workspace_sequence \
             WHERE running.state = 'running' AND queued.state = 'queued')",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let invalid_state: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE state NOT IN \
             ('queued', 'running', 'succeeded', 'failed', 'cancelled'))",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let maximum_sequence: Option<i64> = connection
        .query_row("SELECT MAX(workspace_sequence) FROM jobs", [], |row| {
            row.get(0)
        })
        .map_err(storage_error)?;
    let next_sequence: i64 = connection
        .query_row(
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;

    if invalid_sequence != 0
        || running_count > 1
        || invalid_claim_time != 0
        || overtaken_queue != 0
        || invalid_state != 0
        || next_sequence < 0
        || maximum_sequence.unwrap_or(0) > next_sequence
    {
        return Err(integrity_error(StoreIntegrityCheckV1::JobQueue));
    }
    Ok(())
}

fn verify_job_codecs_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    let workspace_id: String = connection
        .query_row(
            "SELECT workspace_uuid FROM workspace_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let mut statement = connection
        .prepare(
            "SELECT job_id, workspace_sequence, request_digest, command_name, canonical_request_json, state, \
             session_id, submitted_at_ms, claimed_at_ms, finished_at_ms, terminal_response_json FROM jobs",
        )
        .map_err(storage_error)?;
    let mut rows = statement.query([]).map_err(storage_error)?;
    while let Some(row) = rows.next().map_err(storage_error)? {
        let job_id: String = row.get(0).map_err(storage_error)?;
        let sequence: i64 = row.get(1).map_err(storage_error)?;
        let digest: String = row.get(2).map_err(storage_error)?;
        let command_name: String = row.get(3).map_err(storage_error)?;
        let request: String = row.get(4).map_err(storage_error)?;
        let state: String = row.get(5).map_err(storage_error)?;
        let session_id: Option<String> = row.get(6).map_err(storage_error)?;
        let submitted_at: i64 = row.get(7).map_err(storage_error)?;
        let claimed_at: Option<i64> = row.get(8).map_err(storage_error)?;
        let finished_at: Option<i64> = row.get(9).map_err(storage_error)?;
        let terminal: Option<String> = row.get(10).map_err(storage_error)?;

        let job_id = JobIdV1::new(job_id)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
        let sequence = u64::try_from(sequence)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
        if sequence == 0 {
            return Err(integrity_error(StoreIntegrityCheckV1::InternalCodec));
        }
        let digest = Sha256Digest::new(digest)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
        validate_epoch_v1(submitted_at, StoreIntegrityCheckV1::InternalCodec)?;
        if let Some(claimed_at) = claimed_at {
            validate_epoch_v1(claimed_at, StoreIntegrityCheckV1::InternalCodec)?;
        }
        if let Some(finished_at) = finished_at {
            validate_epoch_v1(finished_at, StoreIntegrityCheckV1::InternalCodec)?;
        }
        if let Some(session_id) = session_id.as_deref() {
            podway_core::SessionId::new(session_id.to_owned())
                .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
        }

        let execution = decode_command_v1(&request)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
        let canonical_request = encode_command_v1(&execution)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
        expected_job_scope_v1(
            execution.command(),
            session_id.as_deref(),
            StoreIntegrityCheckV1::InternalCodec,
        )?;
        if canonical_request != request || command_name != command_name_v1(execution.command()) {
            return Err(integrity_error(StoreIntegrityCheckV1::InternalCodec));
        }

        match state.as_str() {
            "queued" | "running" if terminal.is_none() && finished_at.is_none() => {}
            "succeeded" | "failed" | "cancelled" => {
                let terminal = terminal
                    .ok_or_else(|| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
                let receipt = decode_terminal_receipt_v1(&terminal)
                    .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
                let canonical_terminal = encode_persisted_terminal_receipt_v1(&receipt)
                    .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
                let successful_reset = execution.command() == &crate::CommandV1::SessionReset
                    && matches!(
                        receipt.result(),
                        PersistedTerminalResultV1::Success(
                            PersistedDomainResultV1::SessionChanged { .. }
                        )
                    );
                if successful_reset {
                    if !crate::codec::persisted_graph_reset_receipt_is_exact_v2(&receipt) {
                        return Err(integrity_error(StoreIntegrityCheckV1::InternalCodec));
                    }
                } else {
                    validate_persisted_terminal_result_for_command_v1(
                        execution.command(),
                        receipt.result(),
                    )
                    .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
                }
                if matches!(
                    receipt.result(),
                    PersistedTerminalResultV1::Success(
                        PersistedDomainResultV1::WorkspaceInitialized {
                            workspace_id: result_workspace_id,
                            ..
                        }
                        | PersistedDomainResultV1::WorkspaceReset {
                            workspace_id: result_workspace_id,
                            ..
                        }
                    ) if result_workspace_id.as_str() != workspace_id.as_str()
                ) {
                    return Err(integrity_error(StoreIntegrityCheckV1::InternalCodec));
                }
                let compatible = matches!(
                    (state.as_str(), receipt.result()),
                    ("succeeded", PersistedTerminalResultV1::Success(_))
                        | ("failed", PersistedTerminalResultV1::Failure(_))
                        | ("cancelled", PersistedTerminalResultV1::Cancelled)
                );
                if finished_at.is_none()
                    || canonical_terminal != terminal
                    || receipt.job().identity_sequence() != sequence
                    || receipt.job().job_id() != &job_id
                    || receipt.job().request_digest() != &digest
                    || !compatible
                {
                    return Err(integrity_error(StoreIntegrityCheckV1::InternalCodec));
                }
            }
            _ => return Err(integrity_error(StoreIntegrityCheckV1::InternalCodec)),
        }
    }
    Ok(())
}

fn verify_idempotency_receipts_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    let duplicate_job_id: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM idempotency_records GROUP BY job_id HAVING COUNT(*) > 1)",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if duplicate_job_id != 0 {
        return Err(integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt));
    }

    let max_workspace_sequence: i64 = connection
        .query_row(
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let max_workspace_sequence = u64::try_from(max_workspace_sequence)
        .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
    let mut statement = connection
        .prepare(
            "SELECT r.idempotency_key, r.request_digest, r.job_id, r.scope_kind, r.scope_session_id, \
             r.terminal_response_json, j.job_id, j.idempotency_key, j.request_digest, j.state, \
             j.session_id, j.canonical_request_json, j.terminal_response_json \
             FROM idempotency_records r LEFT JOIN jobs j ON j.job_id = r.job_id",
        )
        .map_err(storage_error)?;
    let mut rows = statement.query([]).map_err(storage_error)?;
    while let Some(row) = rows.next().map_err(storage_error)? {
        let idempotency_key: String = row.get(0).map_err(storage_error)?;
        let request_digest: String = row.get(1).map_err(storage_error)?;
        let job_id: String = row.get(2).map_err(storage_error)?;
        let scope_kind: String = row.get(3).map_err(storage_error)?;
        let scope_session_id: Option<String> = row.get(4).map_err(storage_error)?;
        let terminal: Option<String> = row.get(5).map_err(storage_error)?;
        let retained_job_id: Option<String> = row.get(6).map_err(storage_error)?;
        let retained_key: Option<String> = row.get(7).map_err(storage_error)?;
        let retained_digest: Option<String> = row.get(8).map_err(storage_error)?;
        let retained_state: Option<String> = row.get(9).map_err(storage_error)?;
        let retained_session_id: Option<String> = row.get(10).map_err(storage_error)?;
        let retained_request: Option<String> = row.get(11).map_err(storage_error)?;
        let retained_terminal: Option<String> = row.get(12).map_err(storage_error)?;

        IdempotencyKeyV1::new(idempotency_key.clone())
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
        let request_digest = Sha256Digest::new(request_digest)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
        let job_id = JobIdV1::new(job_id)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
        validate_scope_v1(&scope_kind, scope_session_id.as_deref())?;

        if retained_job_id.is_some() {
            let retained_key = retained_key
                .ok_or_else(|| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let retained_digest = retained_digest
                .ok_or_else(|| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let retained_state = retained_state
                .ok_or_else(|| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let retained_digest = Sha256Digest::new(retained_digest)
                .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let retained_request = retained_request
                .ok_or_else(|| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let execution = decode_command_v1(&retained_request)
                .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let expected_scope = expected_job_scope_v1(
                execution.command(),
                retained_session_id.as_deref(),
                StoreIntegrityCheckV1::IdempotencyReceipt,
            )?;
            let terminal_matches = if terminal_state_v1(&retained_state) {
                terminal == retained_terminal
            } else {
                terminal.is_none()
            };
            if retained_key != idempotency_key
                || retained_digest != request_digest
                || (scope_kind.as_str(), scope_session_id.as_deref()) != expected_scope
                || !terminal_matches
            {
                return Err(integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt));
            }
        } else {
            let terminal = terminal
                .ok_or_else(|| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let receipt = decode_terminal_receipt_v1(&terminal)
                .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            let canonical_terminal = encode_persisted_terminal_receipt_v1(&receipt)
                .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
            if canonical_terminal != terminal
                || receipt.job().identity_sequence() == 0
                || receipt.job().identity_sequence() > max_workspace_sequence
                || receipt.job().job_id() != &job_id
                || receipt.job().request_digest() != &request_digest
            {
                return Err(integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt));
            }
        }
    }

    let job_without_record: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs j WHERE NOT EXISTS \
             (SELECT 1 FROM idempotency_records r WHERE r.job_id = j.job_id))",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if job_without_record != 0 {
        return Err(integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt));
    }
    Ok(())
}

fn validate_scope_v1(scope_kind: &str, session_id: Option<&str>) -> Result<(), StoreErrorV1> {
    match (scope_kind, session_id) {
        ("workspace", None) => Ok(()),
        ("session", Some(session_id)) => podway_core::SessionId::new(session_id.to_owned())
            .map(|_| ())
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt)),
        _ => Err(integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt)),
    }
}
fn expected_job_scope_v1<'a>(
    command: &crate::CommandV1,
    session_id: Option<&'a str>,
    check: StoreIntegrityCheckV1,
) -> Result<(&'static str, Option<&'a str>), StoreErrorV1> {
    if command_is_session_scoped_v1(command) {
        let session_id = session_id.ok_or_else(|| integrity_error(check.clone()))?;
        podway_core::SessionId::new(session_id.to_owned())
            .map_err(|_| integrity_error(check.clone()))?;
        Ok(("session", Some(session_id)))
    } else if session_id.is_some() {
        Err(integrity_error(check))
    } else {
        Ok(("workspace", None))
    }
}

fn validate_epoch_v1(value: i64, check: StoreIntegrityCheckV1) -> Result<(), StoreErrorV1> {
    u64::try_from(value)
        .map(|_| ())
        .map_err(|_| integrity_error(check))
}

fn terminal_state_v1(state: &str) -> bool {
    matches!(state, "succeeded" | "failed" | "cancelled")
}

fn initialize_empty_schema_v1(
    connection: &mut Connection,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<(), StoreErrorV1> {
    let checked_at = now;
    let now = sqlite_integer_v1(now.get(), "initialization timestamp")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let object_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if object_count != 0 {
        return Err(integrity_error(
            StoreIntegrityCheckV1::RequiredSchemaObjects,
        ));
    }
    transaction
        .execute_batch(migration_schema_statements_v1()?)
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V1),
                SQLITE_INITIAL_MIGRATION_NAME_V1,
                sqlite_v1_ddl_checksum(),
                now,
            ],
        )
        .map_err(storage_error)?;
    transaction
        .execute_batch(migration_schema_statements_v2()?)
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V2),
                SQLITE_RESPONSE_CONTEXT_MIGRATION_NAME_V2,
                sqlite_v2_ddl_checksum(),
                now,
            ],
        )
        .map_err(storage_error)?;
    transaction
        .execute_batch(migration_schema_statements_v3()?)
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V3),
                SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3,
                sqlite_v3_ddl_checksum(),
                now,
            ],
        )
        .map_err(storage_error)?;
    apply_schema_v4_migration_v1(&transaction, now)?;
    apply_schema_v5_migration_v1(&transaction, now)?;
    transaction
        .execute(
            "INSERT INTO workspace_state (singleton, workspace_uuid, git_common_fingerprint, \
             git_worktree_fingerprint, last_validated_root, next_workspace_sequence, created_at_ms, \
             updated_at_ms) VALUES (1, ?1, ?2, ?3, ?4, 0, ?5, ?5)",
            params![
                identity.workspace_uuid().as_str(),
                identity.common_dir_identity().as_str(),
                identity.worktree_admin_identity().as_str(),
                root.as_encoded(),
                now,
            ],
        )
        .map_err(storage_error)?;
    transaction
        .pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION_CURRENT)
        .map_err(storage_error)?;
    verify_integrity_connection_inner_v1(
        &transaction,
        identity,
        options,
        IntegrityModeV1::Fast,
        checked_at,
        false,
    )?;
    options.trigger_failpoint(StoreFailpointV1::SchemaBeforeCommit)?;
    transaction.commit().map_err(storage_error)
}

fn update_validated_root_v1(
    connection: &mut Connection,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
    now: EpochMillisV1,
) -> Result<(), StoreErrorV1> {
    let now = sqlite_integer_v1(now.get(), "workspace update timestamp")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let current: Option<(String, i64)> = transaction
        .query_row(
            "SELECT last_validated_root, updated_at_ms FROM workspace_state \
             WHERE singleton = 1 AND workspace_uuid = ?1 AND git_common_fingerprint = ?2 \
             AND git_worktree_fingerprint = ?3",
            params![
                identity.workspace_uuid().as_str(),
                identity.common_dir_identity().as_str(),
                identity.worktree_admin_identity().as_str(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage_error)?;
    let Some((current_root, current_updated_at_ms)) = current else {
        return Err(integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity));
    };
    if current_root == root.as_encoded() && current_updated_at_ms == now {
        return transaction.commit().map_err(storage_error);
    }
    let changed = transaction
        .execute(
            "UPDATE workspace_state SET last_validated_root = ?1, updated_at_ms = ?2 \
             WHERE singleton = 1 AND workspace_uuid = ?3 AND git_common_fingerprint = ?4 \
             AND git_worktree_fingerprint = ?5",
            params![
                root.as_encoded(),
                now,
                identity.workspace_uuid().as_str(),
                identity.common_dir_identity().as_str(),
                identity.worktree_admin_identity().as_str(),
            ],
        )
        .map_err(storage_error)?;
    if changed != 1 {
        return Err(integrity_error(StoreIntegrityCheckV1::WorkspaceIdentity));
    }
    transaction.commit().map_err(storage_error)
}

fn migration_schema_statements_v1() -> Result<&'static str, StoreErrorV1> {
    SQLITE_V1_DDL
        .strip_prefix(CONNECTION_PRAGMA_PREAMBLE_V1)
        .and_then(|sql| sql.strip_suffix(USER_VERSION_SUFFIX_V1))
        .ok_or(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: crate::StoreInvariantV1::SchemaDefinition,
        })
}

fn migration_schema_statements_v2() -> Result<&'static str, StoreErrorV1> {
    SQLITE_V2_DDL.strip_suffix(USER_VERSION_SUFFIX_V2).ok_or(
        StoreErrorV1::InternalInvariantViolationV1 {
            invariant: crate::StoreInvariantV1::SchemaDefinition,
        },
    )
}

fn migration_schema_statements_v3() -> Result<&'static str, StoreErrorV1> {
    SQLITE_V3_DDL.strip_suffix(USER_VERSION_SUFFIX_V3).ok_or(
        StoreErrorV1::InternalInvariantViolationV1 {
            invariant: crate::StoreInvariantV1::SchemaDefinition,
        },
    )
}

fn migration_schema_statements_v4() -> Result<&'static str, StoreErrorV1> {
    SQLITE_V4_DDL.strip_suffix(USER_VERSION_SUFFIX_V4).ok_or(
        StoreErrorV1::InternalInvariantViolationV1 {
            invariant: crate::StoreInvariantV1::SchemaDefinition,
        },
    )
}

fn migration_schema_statements_v5() -> Result<&'static str, StoreErrorV1> {
    SQLITE_V5_DDL.strip_suffix(USER_VERSION_SUFFIX_V5).ok_or(
        StoreErrorV1::InternalInvariantViolationV1 {
            invariant: crate::StoreInvariantV1::SchemaDefinition,
        },
    )
}

fn migrate_schema_v1_to_v5(
    connection: &mut Connection,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<(), StoreErrorV1> {
    let checked_at = now;
    let now = sqlite_integer_v1(now.get(), "migration timestamp")?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    verify_migration_predecessor_v1(&transaction, SQLITE_SCHEMA_VERSION_V1)?;
    reject_legacy_procedure_state_v1(&transaction)?;
    transaction
        .execute_batch(migration_schema_statements_v2()?)
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V2),
                SQLITE_RESPONSE_CONTEXT_MIGRATION_NAME_V2,
                sqlite_v2_ddl_checksum(),
                now,
            ],
        )
        .map_err(storage_error)?;
    transaction
        .execute_batch(migration_schema_statements_v3()?)
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V3),
                SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3,
                sqlite_v3_ddl_checksum(),
                now,
            ],
        )
        .map_err(storage_error)?;
    apply_schema_v4_migration_v1(&transaction, now)?;
    apply_schema_v5_migration_v1(&transaction, now)?;
    transaction
        .pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION_V5)
        .map_err(storage_error)?;
    verify_integrity_connection_inner_v1(
        &transaction,
        identity,
        options,
        IntegrityModeV1::Fast,
        checked_at,
        false,
    )?;
    options.trigger_failpoint(StoreFailpointV1::SchemaBeforeCommit)?;
    transaction.commit().map_err(storage_error)
}

fn migrate_schema_v2_to_v5(
    connection: &mut Connection,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<(), StoreErrorV1> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    verify_migration_predecessor_v1(&transaction, SQLITE_SCHEMA_VERSION_V2)?;
    reject_legacy_procedure_state_v1(&transaction)?;
    transaction
        .execute_batch(migration_schema_statements_v3()?)
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V3),
                SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3,
                sqlite_v3_ddl_checksum(),
                sqlite_integer_v1(now.get(), "migration timestamp")?,
            ],
        )
        .map_err(storage_error)?;
    apply_schema_v4_migration_v1(
        &transaction,
        sqlite_integer_v1(now.get(), "migration timestamp")?,
    )?;
    apply_schema_v5_migration_v1(
        &transaction,
        sqlite_integer_v1(now.get(), "migration timestamp")?,
    )?;
    transaction
        .pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION_V5)
        .map_err(storage_error)?;
    verify_integrity_connection_inner_v1(
        &transaction,
        identity,
        options,
        IntegrityModeV1::Fast,
        now,
        false,
    )?;
    options.trigger_failpoint(StoreFailpointV1::SchemaBeforeCommit)?;
    transaction.commit().map_err(storage_error)
}

fn migrate_schema_v3_to_v5(
    connection: &mut Connection,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<(), StoreErrorV1> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    verify_migration_predecessor_v1(&transaction, SQLITE_SCHEMA_VERSION_V3)?;
    reject_legacy_procedure_state_v1(&transaction)?;
    normalize_schema_v3_terminal_receipts_v1(&transaction)?;
    apply_schema_v4_migration_v1(
        &transaction,
        sqlite_integer_v1(now.get(), "migration timestamp")?,
    )?;
    apply_schema_v5_migration_v1(
        &transaction,
        sqlite_integer_v1(now.get(), "migration timestamp")?,
    )?;
    transaction
        .pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION_V5)
        .map_err(storage_error)?;
    verify_integrity_connection_inner_v1(
        &transaction,
        identity,
        options,
        IntegrityModeV1::Fast,
        now,
        false,
    )?;
    options.trigger_failpoint(StoreFailpointV1::SchemaBeforeCommit)?;
    transaction.commit().map_err(storage_error)
}

fn migrate_schema_v4_to_v5(
    connection: &mut Connection,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<(), StoreErrorV1> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    verify_migration_predecessor_v1(&transaction, SQLITE_SCHEMA_VERSION_V4)?;
    verify_v2_graph_state_connection_v2(&transaction)?;
    apply_schema_v5_migration_v1(
        &transaction,
        sqlite_integer_v1(now.get(), "migration timestamp")?,
    )?;
    transaction
        .pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION_V5)
        .map_err(storage_error)?;
    verify_integrity_connection_inner_v1(
        &transaction,
        identity,
        options,
        IntegrityModeV1::Fast,
        now,
        false,
    )?;
    options.trigger_failpoint(StoreFailpointV1::SchemaBeforeCommit)?;
    transaction.commit().map_err(storage_error)
}

fn apply_schema_v4_migration_v1(connection: &Connection, now: i64) -> Result<(), StoreErrorV1> {
    connection
        .execute_batch(migration_schema_statements_v4()?)
        .map_err(storage_error)?;
    connection
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V4),
                SQLITE_V2_ONLY_MIGRATION_NAME_V4,
                sqlite_v4_ddl_checksum(),
                now,
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn apply_schema_v5_migration_v1(connection: &Connection, now: i64) -> Result<(), StoreErrorV1> {
    connection
        .execute_batch(migration_schema_statements_v5()?)
        .map_err(storage_error)?;
    connection
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(SQLITE_SCHEMA_VERSION_V5),
                SQLITE_PREPARED_LIFECYCLE_MIGRATION_NAME_V5,
                sqlite_v5_ddl_checksum(),
                now,
            ],
        )
        .map_err(storage_error)?;
    verify_foreign_keys_v1(connection)
}

fn reject_legacy_procedure_state_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    let legacy_rows: i64 = connection
        .query_row(
            "SELECT \
               (SELECT COUNT(*) FROM procedure_snapshots) + \
               (SELECT COUNT(*) FROM task_sessions) + \
               (SELECT COUNT(*) FROM stage_progress) + \
               (SELECT COUNT(*) FROM attempts) + \
               (SELECT COUNT(*) FROM item_slots) + \
               (SELECT COUNT(*) FROM blockers)",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if legacy_rows != 0 {
        return Err(StoreErrorV1::LegacyProcedureStateUnsupportedV1);
    }

    let mut jobs = connection
        .prepare("SELECT canonical_request_json FROM jobs ORDER BY workspace_sequence")
        .map_err(storage_error)?;
    let requests = jobs
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage_error)?;
    for request in requests {
        let request = request.map_err(storage_error)?;
        let execution = decode_command_v1(&request)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
        if !execution_is_v2_only_compatible_v1(&execution) {
            return Err(StoreErrorV1::LegacyProcedureStateUnsupportedV1);
        }
    }

    let mut orphaned = connection
        .prepare(
            "SELECT idempotency_records.terminal_response_json \
             FROM idempotency_records \
             LEFT JOIN jobs ON jobs.job_id = idempotency_records.job_id \
             WHERE jobs.job_id IS NULL AND idempotency_records.terminal_response_json IS NOT NULL \
             ORDER BY idempotency_records.idempotency_key",
        )
        .map_err(storage_error)?;
    let receipts = orphaned
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage_error)?;
    for receipt in receipts {
        let receipt = receipt.map_err(storage_error)?;
        let (receipt, _) = normalize_terminal_receipt_for_schema_v4_v1(&receipt, None)
            .map_err(|_| integrity_error(StoreIntegrityCheckV1::IdempotencyReceipt))?;
        let compatible = receipt.execution_flavor() == crate::DurableExecutionFlavorV1::ProcedureV2
            || matches!(
                receipt.lookup_command(),
                Some(
                    crate::codec::PersistedDomainCommandV1::WorkspaceInitialize
                        | crate::codec::PersistedDomainCommandV1::WorkspaceResetAll
                )
            );
        if !compatible {
            return Err(StoreErrorV1::LegacyProcedureStateUnsupportedV1);
        }
    }
    Ok(())
}

fn execution_is_v2_only_compatible_v1(execution: &crate::ClaimedExecutionV1) -> bool {
    execution.execution_flavor() == crate::DurableExecutionFlavorV1::ProcedureV2
        || matches!(
            execution.command(),
            crate::CommandV1::WorkspaceInitialize | crate::CommandV1::WorkspaceResetAll
        )
}

fn normalize_schema_v3_terminal_receipts_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    for (table, key, query) in [
        (
            "jobs",
            "job_id",
            "SELECT job_id, terminal_response_json, canonical_request_json FROM jobs \
             WHERE terminal_response_json IS NOT NULL ORDER BY job_id",
        ),
        (
            "idempotency_records",
            "idempotency_key",
            "SELECT idempotency_records.idempotency_key, \
                    idempotency_records.terminal_response_json, jobs.canonical_request_json \
             FROM idempotency_records LEFT JOIN jobs USING (job_id) \
             WHERE idempotency_records.terminal_response_json IS NOT NULL \
             ORDER BY idempotency_records.idempotency_key",
        ),
    ] {
        let mut statement = connection.prepare(query).map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(storage_error)?;
        let mut updates = Vec::new();
        for row in rows {
            let (identity, encoded, request) = row.map_err(storage_error)?;
            let execution_flavor = request
                .as_deref()
                .map(decode_command_v1)
                .transpose()
                .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?
                .map(|execution| execution.execution_flavor());
            let (_, normalized) =
                normalize_terminal_receipt_for_schema_v4_v1(&encoded, execution_flavor)
                    .map_err(|_| integrity_error(StoreIntegrityCheckV1::InternalCodec))?;
            if normalized != encoded {
                updates.push((identity, normalized));
            }
        }
        drop(statement);
        let update = format!("UPDATE {table} SET terminal_response_json = ?1 WHERE {key} = ?2");
        for (identity, normalized) in updates {
            connection
                .execute(&update, params![normalized, identity])
                .map_err(storage_error)?;
        }
    }
    Ok(())
}

fn verify_migration_predecessor_v1(
    connection: &Connection,
    expected_version: u32,
) -> Result<(), StoreErrorV1> {
    if read_user_version_v1(connection)? != expected_version {
        return Err(integrity_error(StoreIntegrityCheckV1::SchemaVersion));
    }
    verify_exact_schema_objects_through_v1(connection, expected_version)?;
    verify_migration_checksum_through_v1(connection, expected_version)?;
    verify_sqlite_check_v1(
        connection,
        "quick_check",
        StoreIntegrityCheckV1::SqliteQuickCheck,
    )?;
    verify_foreign_keys_v1(connection)
}

fn read_user_version_v1(connection: &Connection) -> Result<u32, StoreErrorV1> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    u32::try_from(version).map_err(|_| integrity_error(StoreIntegrityCheckV1::SchemaVersion))
}

fn user_schema_object_count_v1(connection: &Connection) -> Result<i64, StoreErrorV1> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)
}

fn sqlite_integer_v1(value: u64, field: &'static str) -> Result<i64, StoreErrorV1> {
    i64::try_from(value).map_err(|_| {
        StoreErrorV1::InvalidStateV1(crate::StoreValueErrorV1::IntegerOutOfRange { field })
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DatabasePathStateV1 {
    Existing,
    Missing,
}

pub(crate) fn canonical_database_path_v1(path: &Path) -> Result<PathBuf, StoreErrorV1> {
    validate_database_parent_path_v1(path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(unsafe_database_path_error)?;
    let canonical_parent = fs::canonicalize(parent).map_err(storage_io_error)?;
    Ok(canonical_parent.join(file_name))
}
pub(crate) fn inspect_database_path_v1(path: &Path) -> Result<DatabasePathStateV1, StoreErrorV1> {
    validate_database_parent_path_v1(path)?;
    let state = match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_existing_regular_private_file_v1(path)?;
            DatabasePathStateV1::Existing
        }
        Err(error) if error.kind() == ErrorKind::NotFound => DatabasePathStateV1::Missing,
        Err(error) => return Err(storage_io_error(error)),
    };
    for suffix in ["-wal", "-shm"] {
        let sidecar = sqlite_sidecar_path_v1(path, suffix);
        match fs::symlink_metadata(&sidecar) {
            Ok(_) => validate_existing_regular_private_file_v1(&sidecar)?,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(storage_io_error(error)),
        }
    }
    Ok(state)
}

pub(crate) fn validate_existing_database_path_v1(path: &Path) -> Result<(), StoreErrorV1> {
    match inspect_database_path_v1(path)? {
        DatabasePathStateV1::Existing => Ok(()),
        DatabasePathStateV1::Missing => Err(unsafe_database_path_error()),
    }
}

pub(crate) fn validate_database_parent_path_v1(path: &Path) -> Result<(), StoreErrorV1> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::symlink_metadata(parent).map_err(storage_io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(unsafe_database_path_error());
    }
    Ok(())
}

pub(crate) fn validate_existing_regular_private_file_v1(path: &Path) -> Result<(), StoreErrorV1> {
    let metadata = validate_existing_regular_private_file_metadata_v1(path)?;
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err(unsafe_database_path_error());
    }
    Ok(())
}

pub(crate) fn validate_publication_link_pair_v1(
    temporary: &Path,
    destination: &Path,
) -> Result<(), StoreErrorV1> {
    if !is_store_temporary_database_name_v1(temporary, destination) {
        return Err(unsafe_database_path_error());
    }
    let temporary_metadata = validate_existing_regular_private_file_metadata_v1(temporary)?;
    let destination_metadata = validate_existing_regular_private_file_metadata_v1(destination)?;
    #[cfg(unix)]
    if temporary_metadata.nlink() != 2
        || destination_metadata.nlink() != 2
        || temporary_metadata.dev() != destination_metadata.dev()
        || temporary_metadata.ino() != destination_metadata.ino()
        || !has_exact_private_file_permissions_v1(&temporary_metadata)
        || !has_exact_private_file_permissions_v1(&destination_metadata)
    {
        return Err(unsafe_database_path_error());
    }
    Ok(())
}

#[cfg(all(test, unix))]
struct PublicationRecoveryBarrierV1 {
    destination: PathBuf,
    reached: std::sync::Arc<std::sync::Barrier>,
    release: std::sync::Arc<std::sync::Barrier>,
    claimed: std::sync::atomic::AtomicBool,
}

#[cfg(all(test, unix))]
static PUBLICATION_RECOVERY_AFTER_LINK_COUNT_BARRIER: std::sync::Mutex<
    Option<std::sync::Arc<PublicationRecoveryBarrierV1>>,
> = std::sync::Mutex::new(None);

#[cfg(all(test, unix))]
static PUBLICATION_RECOVERY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(test, unix))]
struct PublicationRecoveryHookGuardV1 {
    hook: std::sync::Arc<PublicationRecoveryBarrierV1>,
    release: Option<std::sync::Arc<std::sync::Barrier>>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(all(test, unix))]
impl PublicationRecoveryHookGuardV1 {
    fn release(&mut self) {
        self.release
            .take()
            .expect("publication recovery test hook release")
            .wait();
    }
}

#[cfg(all(test, unix))]
impl Drop for PublicationRecoveryHookGuardV1 {
    fn drop(&mut self) {
        PUBLICATION_RECOVERY_AFTER_LINK_COUNT_BARRIER
            .lock()
            .expect("publication recovery test hook lock")
            .take();
        if self.release.is_some() && self.hook.claimed.load(std::sync::atomic::Ordering::Acquire) {
            self.release
                .take()
                .expect("claimed publication recovery hook release")
                .wait();
        }
    }
}

#[cfg(all(test, unix))]
fn install_publication_recovery_hook_for_test_v1(
    destination: PathBuf,
    reached: std::sync::Arc<std::sync::Barrier>,
    release: std::sync::Arc<std::sync::Barrier>,
) -> PublicationRecoveryHookGuardV1 {
    let lock = PUBLICATION_RECOVERY_TEST_LOCK
        .lock()
        .expect("publication recovery test lifetime lock");
    let hook = std::sync::Arc::new(PublicationRecoveryBarrierV1 {
        destination,
        reached,
        release: std::sync::Arc::clone(&release),
        claimed: std::sync::atomic::AtomicBool::new(false),
    });
    *PUBLICATION_RECOVERY_AFTER_LINK_COUNT_BARRIER
        .lock()
        .expect("publication recovery test hook lock") = Some(std::sync::Arc::clone(&hook));
    PublicationRecoveryHookGuardV1 {
        hook,
        release: Some(release),
        _lock: lock,
    }
}

#[cfg(all(test, unix))]
fn wait_at_publication_recovery_link_count_for_test(destination: &Path) {
    let hook = {
        let mut hook = PUBLICATION_RECOVERY_AFTER_LINK_COUNT_BARRIER
            .lock()
            .expect("publication recovery test hook lock");
        if hook
            .as_ref()
            .is_some_and(|expected| expected.destination == destination)
        {
            let hook = hook.take();
            if let Some(hook) = &hook {
                hook.claimed
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            hook
        } else {
            None
        }
    };
    if let Some(hook) = hook {
        hook.reached.wait();
        hook.release.wait();
    }
}

pub(crate) fn recover_interrupted_publication_v1(destination: &Path) -> Result<(), StoreErrorV1> {
    validate_database_parent_path_v1(destination)?;
    let destination_metadata = match fs::symlink_metadata(destination) {
        Ok(_) => validate_existing_regular_private_file_metadata_v1(destination)?,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                recover_abandoned_publication_temporary_v1(destination)?;
                return recover_orphaned_ownership_markers_v1(destination);
            }
            #[cfg(not(unix))]
            return Ok(());
        }
        Err(error) => return Err(storage_io_error(error)),
    };
    #[cfg(unix)]
    {
        match destination_metadata.nlink() {
            1 => {
                recover_abandoned_publication_temporary_v1(destination)?;
                recover_orphaned_ownership_markers_v1(destination)?;
                return Ok(());
            }
            2 => {}
            _ => return Err(unsafe_database_path_error()),
        }
    }
    #[cfg(all(test, unix))]
    wait_at_publication_recovery_link_count_for_test(destination);
    #[cfg(not(unix))]
    {
        let _ = destination_metadata;
        return Err(unsafe_database_path_error());
    }

    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    const MAX_PUBLICATION_RECOVERY_DIRECTORY_ENTRIES_V1: usize = 1_024;

    let mut temporary = None;
    let mut inspected_entries = 0usize;
    for entry in fs::read_dir(parent).map_err(storage_io_error)? {
        inspected_entries = inspected_entries
            .checked_add(1)
            .ok_or_else(unsafe_database_path_error)?;
        if inspected_entries > MAX_PUBLICATION_RECOVERY_DIRECTORY_ENTRIES_V1 {
            return Err(unsafe_database_path_error());
        }
        let candidate = entry.map_err(storage_io_error)?.path();
        if !is_store_temporary_database_name_v1(&candidate, destination) {
            continue;
        }
        let candidate_metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if publication_destination_is_finalized_v1(destination, &destination_metadata)? {
                    return Ok(());
                }
                continue;
            }
            Err(error) => return Err(storage_io_error(error)),
        };
        if !is_same_publication_file_v1(&candidate_metadata, &destination_metadata) {
            continue;
        }
        let candidate_file = match open_publication_link_for_recovery_v1(&candidate) {
            Ok(candidate_file) => candidate_file,
            Err(_error)
                if publication_destination_is_finalized_v1(destination, &destination_metadata)? =>
            {
                return Ok(());
            }
            Err(error) => match fs::symlink_metadata(&candidate) {
                Err(missing) if missing.kind() == ErrorKind::NotFound => continue,
                _ => return Err(error),
            },
        };
        if let Err(error) = validate_publication_link_pair_v1(&candidate, destination) {
            if publication_destination_is_finalized_v1(destination, &destination_metadata)? {
                return Ok(());
            }
            return Err(error);
        }
        if temporary.replace((candidate, candidate_file)).is_some() {
            return Err(unsafe_database_path_error());
        }
    }
    let (temporary, (marker, temporary_file)) = match temporary {
        Some(temporary) => temporary,
        None => {
            if publication_destination_is_finalized_v1(destination, &destination_metadata)? {
                File::open(parent)
                    .map_err(storage_io_error)?
                    .sync_all()
                    .map_err(storage_io_error)?;
                return Ok(());
            }
            return Err(unsafe_database_path_error());
        }
    };
    match unlink_publication_link_for_recovery_v1(&temporary, destination, &marker, &temporary_file)
    {
        Ok(()) => {}
        Err(error)
            if matches!(error, StoreErrorV1::StorageUnavailableV1 { .. })
                && publication_destination_is_finalized_v1(destination, &destination_metadata)? => {
        }
        Err(error) => return Err(error),
    }
    validate_existing_regular_private_file_v1(destination)?;
    File::open(parent)
        .map_err(storage_io_error)?
        .sync_all()
        .map_err(storage_io_error)
}
#[cfg(unix)]
fn open_publication_link_for_recovery_v1(path: &Path) -> Result<(File, File), StoreErrorV1> {
    let marker =
        open_ownership_marker_for_recovery_v1(&temporary_ownership_marker_path_v1(path), false)?
            .ok_or_else(unsafe_database_path_error)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(target_os = "macos")]
    options.custom_flags(0x0100);
    #[cfg(target_os = "linux")]
    options.custom_flags(0x0002_0000);
    let temporary = options.open(path).map_err(storage_io_error)?;
    Ok((marker, temporary))
}

#[cfg(unix)]
fn unlink_publication_link_for_recovery_v1(
    temporary: &Path,
    destination: &Path,
    marker: &File,
    temporary_file: &File,
) -> Result<(), StoreErrorV1> {
    validate_publication_link_pair_v1(temporary, destination)?;
    let path_metadata = validate_existing_regular_private_file_metadata_v1(temporary)?;
    let descriptor_metadata = temporary_file.metadata().map_err(storage_io_error)?;
    if !is_same_publication_file_v1(&path_metadata, &descriptor_metadata) {
        return Err(unsafe_database_path_error());
    }
    let marker_path = temporary_ownership_marker_path_v1(temporary);
    validate_ownership_marker_v1(&marker_path, marker, temporary_file)?;
    let sidecars = ["-wal", "-shm"].map(|suffix| sqlite_sidecar_path_v1(temporary, suffix));
    let mut sidecar_files = Vec::with_capacity(sidecars.len());
    for sidecar in &sidecars {
        if let Some(file) = open_recovery_file_after_marker_v1(sidecar, false)? {
            sidecar_files.push((sidecar, file));
        }
    }
    for (sidecar, file) in &sidecar_files {
        unlink_revalidated_recovery_file_v1(sidecar, file)?;
    }
    fs::remove_file(temporary).map_err(storage_io_error)?;
    unlink_revalidated_ownership_marker_v1(&marker_path, marker, temporary_file)
}

#[cfg(unix)]
fn recover_abandoned_publication_temporary_v1(destination: &Path) -> Result<(), StoreErrorV1> {
    validate_database_parent_path_v1(destination)?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    const MAX_PUBLICATION_RECOVERY_DIRECTORY_ENTRIES_V1: usize = 1_024;

    let mut temporaries = Vec::new();
    let mut inspected_entries = 0usize;
    for entry in fs::read_dir(parent).map_err(storage_io_error)? {
        inspected_entries = inspected_entries
            .checked_add(1)
            .ok_or_else(unsafe_database_path_error)?;
        if inspected_entries > MAX_PUBLICATION_RECOVERY_DIRECTORY_ENTRIES_V1 {
            return Err(unsafe_database_path_error());
        }
        let candidate = entry.map_err(storage_io_error)?.path();
        let Some(marker) = open_unowned_store_temporary_for_recovery_v1(&candidate, destination)?
        else {
            continue;
        };
        temporaries.push((candidate, marker));
    }

    for (temporary, marker) in temporaries {
        // A marker whose owner won the create-lock but has not yet written and
        // synced its device/inode record is empty (the macOS O_EXLOCK
        // create-and-lock gap, or a crash inside it). Leave the pair for its
        // owner, or as harmless residue if the owner is gone, rather than
        // hard-failing on the empty marker (which would brick the workspace) or
        // reaping a possibly-live creation. This mirrors the emptiness guard in
        // reap_orphaned_ownership_marker_v1; dropping `marker` releases the lock.
        let marker_length = marker.metadata().map_err(storage_io_error)?.len();
        if marker_length == 0 || marker_length > 256 {
            continue;
        }
        let temporary_file = open_recovery_file_after_marker_v1(&temporary, true)?
            .ok_or_else(unsafe_database_path_error)?;
        validate_ownership_marker_v1(
            &temporary_ownership_marker_path_v1(&temporary),
            &marker,
            &temporary_file,
        )?;
        let sidecars = ["-wal", "-shm"].map(|suffix| sqlite_sidecar_path_v1(&temporary, suffix));
        let mut sidecar_files = Vec::with_capacity(sidecars.len());
        for sidecar in &sidecars {
            if let Some(file) = open_recovery_file_after_marker_v1(sidecar, false)? {
                sidecar_files.push((sidecar, file));
            }
        }
        for (sidecar, file) in &sidecar_files {
            unlink_revalidated_recovery_file_v1(sidecar, file)?;
        }
        unlink_revalidated_recovery_file_v1(&temporary, &temporary_file)?;
        unlink_revalidated_ownership_marker_v1(
            &temporary_ownership_marker_path_v1(&temporary),
            &marker,
            &temporary_file,
        )?;
    }
    File::open(parent)
        .map_err(storage_io_error)?
        .sync_all()
        .map_err(storage_io_error)
}
#[cfg(unix)]
fn recover_orphaned_ownership_markers_v1(destination: &Path) -> Result<(), StoreErrorV1> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    const MAX_PUBLICATION_RECOVERY_DIRECTORY_ENTRIES_V1: usize = 1_024;
    let mut inspected_entries = 0usize;
    for entry in fs::read_dir(parent).map_err(storage_io_error)? {
        inspected_entries = inspected_entries
            .checked_add(1)
            .ok_or_else(unsafe_database_path_error)?;
        if inspected_entries > MAX_PUBLICATION_RECOVERY_DIRECTORY_ENTRIES_V1 {
            return Err(unsafe_database_path_error());
        }
        let marker = entry.map_err(storage_io_error)?.path();
        let Some(temporary) = temporary_path_from_ownership_marker_v1(&marker) else {
            continue;
        };
        if is_store_temporary_database_name_v1(&temporary, destination) {
            match fs::symlink_metadata(&temporary) {
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    reap_orphaned_ownership_marker_v1(&marker, &temporary)?;
                }
                Ok(_) => {}
                Err(error) => return Err(storage_io_error(error)),
            }
        }
    }
    File::open(parent)
        .map_err(storage_io_error)?
        .sync_all()
        .map_err(storage_io_error)
}

#[cfg(unix)]
fn reap_orphaned_ownership_marker_v1(marker: &Path, temporary: &Path) -> Result<(), StoreErrorV1> {
    let marker_file = match open_recovery_file_after_marker_v1(marker, false) {
        Ok(Some(marker_file)) => marker_file,
        Ok(None) => return Ok(()),
        Err(error) => match fs::symlink_metadata(marker) {
            Err(missing) if missing.kind() == ErrorKind::NotFound => return Ok(()),
            _ => return Err(error),
        },
    };
    match marker_file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(()),
        Err(TryLockError::Error(error)) => return Err(storage_io_error(error)),
    }
    // An ownership marker is written and synced only after its owner's
    // marker-creating open has returned with the creation lock held. Holding
    // the lock over an empty marker therefore means the lock was won inside
    // the owner's still-running create-and-lock open; leaving the marker and
    // releasing the lock lets that owner resume its interrupted creation.
    let marker_length = marker_file.metadata().map_err(storage_io_error)?.len();
    if marker_length == 0 || marker_length > 256 {
        return Ok(());
    }
    match fs::symlink_metadata(temporary) {
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Ok(_) => return Ok(()),
        Err(error) => return Err(storage_io_error(error)),
    }
    let sidecars = ["-wal", "-shm"].map(|suffix| sqlite_sidecar_path_v1(temporary, suffix));
    for sidecar in &sidecars {
        if let Some(file) = open_recovery_file_after_marker_v1(sidecar, false)? {
            unlink_revalidated_recovery_file_v1(sidecar, &file)?;
        }
    }
    unlink_revalidated_recovery_file_v1(marker, &marker_file)
}

#[cfg(unix)]
fn temporary_path_from_ownership_marker_v1(marker: &Path) -> Option<PathBuf> {
    let marker_name = marker.file_name()?.as_bytes();
    let temporary_name = marker_name.strip_suffix(b".owner")?;
    let parent = marker.parent().unwrap_or_else(|| Path::new("."));
    Some(parent.join(OsStr::from_bytes(temporary_name)))
}

#[cfg(unix)]
fn open_unowned_store_temporary_for_recovery_v1(
    path: &Path,
    destination: &Path,
) -> Result<Option<File>, StoreErrorV1> {
    if !is_store_temporary_database_name_v1(path, destination) {
        return Ok(None);
    }
    let marker_path = temporary_ownership_marker_path_v1(path);
    let marker = match open_recovery_file_after_marker_v1(&marker_path, false) {
        Ok(Some(marker)) => marker,
        Ok(None) => return Ok(None),
        Err(error) => match fs::symlink_metadata(&marker_path) {
            Err(missing) if missing.kind() == ErrorKind::NotFound => return Ok(None),
            _ => return Err(error),
        },
    };
    match marker.try_lock() {
        Ok(()) => Ok(Some(marker)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(storage_io_error(error)),
    }
}

#[cfg(unix)]
fn open_ownership_marker_for_recovery_v1(
    path: &Path,
    ignore_busy: bool,
) -> Result<Option<File>, StoreErrorV1> {
    let Some(file) = open_recovery_file_after_marker_v1(path, true)? else {
        return Err(unsafe_database_path_error());
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) if ignore_busy => Ok(None),
        Err(TryLockError::WouldBlock) => Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Busy,
        }),
        Err(TryLockError::Error(error)) => Err(storage_io_error(error)),
    }
}

#[cfg(unix)]
fn open_recovery_file_after_marker_v1(
    path: &Path,
    required: bool,
) -> Result<Option<File>, StoreErrorV1> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(target_os = "macos")]
    options.custom_flags(0x0100);
    #[cfg(target_os = "linux")]
    options.custom_flags(0x0002_0000);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if !required && error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage_io_error(error)),
    };
    validate_recovery_file_identity_v1(path, &file)?;
    Ok(Some(file))
}

#[cfg(unix)]
fn validate_recovery_file_identity_v1(path: &Path, file: &File) -> Result<(), StoreErrorV1> {
    let path_metadata = validate_existing_regular_private_file_metadata_v1(path)?;
    let descriptor_metadata = file.metadata().map_err(storage_io_error)?;
    if path_metadata.nlink() != 1
        || !has_exact_private_file_permissions_v1(&path_metadata)
        || !is_same_publication_file_v1(&path_metadata, &descriptor_metadata)
    {
        return Err(unsafe_database_path_error());
    }
    Ok(())
}

#[cfg(unix)]
fn unlink_revalidated_recovery_file_v1(path: &Path, file: &File) -> Result<(), StoreErrorV1> {
    validate_recovery_file_identity_v1(path, file)?;
    fs::remove_file(path).map_err(storage_io_error)
}
#[cfg(unix)]
fn validate_ownership_marker_v1(
    path: &Path,
    file: &File,
    temporary: &File,
) -> Result<(), StoreErrorV1> {
    validate_recovery_file_identity_v1(path, file)?;
    let length = file.metadata().map_err(storage_io_error)?.len();
    if length == 0 || length > 256 {
        return Err(unsafe_database_path_error());
    }
    let mut marker = file.try_clone().map_err(storage_io_error)?;
    marker.seek(SeekFrom::Start(0)).map_err(storage_io_error)?;
    let mut contents = vec![0; length as usize];
    marker.read_exact(&mut contents).map_err(storage_io_error)?;
    let contents = std::str::from_utf8(&contents).map_err(|_| unsafe_database_path_error())?;
    let body = contents
        .strip_prefix(TEMPORARY_OWNERSHIP_MARKER_HEADER_V1)
        .filter(|body| body.ends_with('\n'))
        .ok_or_else(unsafe_database_path_error)?;
    let mut fields = body.split_terminator('\n');
    let device = fields
        .next()
        .and_then(|field| field.strip_prefix("device="))
        .filter(|value| canonical_decimal_v1(value.as_bytes(), true))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(unsafe_database_path_error)?;
    let inode = fields
        .next()
        .and_then(|field| field.strip_prefix("inode="))
        .filter(|value| canonical_decimal_v1(value.as_bytes(), false))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(unsafe_database_path_error)?;
    if fields.next().is_some() {
        return Err(unsafe_database_path_error());
    }
    let descriptor = temporary.metadata().map_err(storage_io_error)?;
    if device != descriptor.dev() || inode != descriptor.ino() {
        return Err(unsafe_database_path_error());
    }
    Ok(())
}

#[cfg(unix)]
fn unlink_revalidated_ownership_marker_v1(
    path: &Path,
    file: &File,
    temporary: &File,
) -> Result<(), StoreErrorV1> {
    validate_ownership_marker_v1(path, file, temporary)?;
    fs::remove_file(path).map_err(storage_io_error)
}
#[cfg(unix)]
fn publication_destination_is_finalized_v1(
    destination: &Path,
    expected_metadata: &fs::Metadata,
) -> Result<bool, StoreErrorV1> {
    let metadata = match fs::symlink_metadata(destination) {
        Ok(_) => validate_existing_regular_private_file_metadata_v1(destination)?,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(storage_io_error(error)),
    };
    Ok(metadata.nlink() == 1 && is_same_publication_file_v1(&metadata, expected_metadata))
}
#[cfg(not(unix))]
fn publication_destination_is_finalized_v1(
    _destination: &Path,
    _expected_metadata: &fs::Metadata,
) -> Result<bool, StoreErrorV1> {
    Ok(false)
}
#[cfg(all(test, unix))]
mod publication_recovery_tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Barrier};

    #[test]
    fn multiple_abandoned_publications_are_all_recovered() {
        let directory = TempDir::new().expect("publication recovery directory");
        let destination = directory.path().join("state.sqlite3");
        let temporaries = [
            directory.path().join(".state.sqlite3.101.1.0.tmp"),
            directory.path().join(".state.sqlite3.102.1.1.tmp"),
        ];

        for temporary in &temporaries {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(temporary)
                .expect("private temporary database");
            file.write_all(b"abandoned").expect("temporary bytes");
            file.sync_all().expect("temporary sync");
            let marker_path = temporary_ownership_marker_path_v1(temporary);
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker_path)
                .expect("private temporary ownership marker");
            write_temporary_ownership_marker_v1(&mut marker, &file)
                .expect("ownership marker bytes");
        }

        recover_interrupted_publication_v1(&destination)
            .expect("all authenticated abandoned publications recover");
        for temporary in &temporaries {
            assert!(!temporary.exists(), "temporary must be removed");
            assert!(
                !temporary_ownership_marker_path_v1(temporary).exists(),
                "ownership marker must be removed"
            );
        }
    }
    #[test]
    fn concurrent_finalization_without_a_directory_candidate_is_accepted() {
        let directory = TempDir::new().expect("publication recovery directory");
        let destination = directory.path().join("state.sqlite3");
        let temporary = directory.path().join(".state.sqlite3.1.0.0.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .expect("private temporary database");
        file.write_all(b"publication").expect("temporary bytes");
        file.sync_all().expect("temporary sync");
        let marker_path = temporary_ownership_marker_path_v1(&temporary);
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&marker_path)
            .expect("private temporary ownership marker");
        write_temporary_ownership_marker_v1(&mut marker, &file).expect("ownership marker bytes");
        fs::hard_link(&temporary, &destination).expect("publication hard link");

        let reached = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let mut hook = install_publication_recovery_hook_for_test_v1(
            destination.clone(),
            Arc::clone(&reached),
            Arc::clone(&release),
        );

        let destination_for_recovery = destination.clone();
        let recovery = std::thread::spawn(move || {
            recover_interrupted_publication_v1(&destination_for_recovery)
        });
        reached.wait();
        fs::remove_file(&temporary).expect("concurrent temporary unlink");
        fs::remove_file(&marker_path).expect("concurrent ownership marker unlink");
        hook.release();

        recovery
            .join()
            .expect("publication recovery thread")
            .expect("concurrent finalization accepted");
        assert!(!temporary.exists());
        assert_eq!(
            fs::symlink_metadata(&destination)
                .expect("published destination")
                .nlink(),
            1
        );
    }
    #[test]
    fn destination_replacement_during_recovery_is_rejected() {
        let directory = TempDir::new().expect("publication recovery directory");
        let destination = directory.path().join("state.sqlite3");
        let temporary = directory.path().join(".state.sqlite3.1.0.0.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .expect("private temporary database");
        file.write_all(b"publication").expect("temporary bytes");
        file.sync_all().expect("temporary sync");
        let marker_path = temporary_ownership_marker_path_v1(&temporary);
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&marker_path)
            .expect("private temporary ownership marker");
        write_temporary_ownership_marker_v1(&mut marker, &file).expect("ownership marker bytes");
        fs::hard_link(&temporary, &destination).expect("publication hard link");

        let reached = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let mut hook = install_publication_recovery_hook_for_test_v1(
            destination.clone(),
            Arc::clone(&reached),
            Arc::clone(&release),
        );

        let destination_for_recovery = destination.clone();
        let recovery = std::thread::spawn(move || {
            recover_interrupted_publication_v1(&destination_for_recovery)
        });
        reached.wait();
        fs::remove_file(&temporary).expect("temporary unlink");
        fs::remove_file(&marker_path).expect("ownership marker unlink");
        fs::remove_file(&destination).expect("published destination unlink");
        let mut replacement = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&destination)
            .expect("replacement destination");
        replacement
            .write_all(b"replacement")
            .expect("replacement bytes");
        replacement.sync_all().expect("replacement sync");
        hook.release();

        assert!(
            recovery
                .join()
                .expect("publication recovery thread")
                .is_err(),
            "recovery must not accept a replacement destination"
        );
        assert_eq!(
            fs::read(&destination).expect("replacement destination bytes"),
            b"replacement"
        );
    }
}

#[cfg(unix)]
fn is_store_temporary_database_name_v1(temporary: &Path, destination: &Path) -> bool {
    store_temporary_database_process_id_v1(temporary, destination).is_some()
}

#[cfg(unix)]
fn store_temporary_database_process_id_v1(temporary: &Path, destination: &Path) -> Option<u32> {
    use std::os::unix::ffi::OsStrExt;

    let name = temporary.file_name()?.as_bytes();
    let destination_name = destination.file_name()?.as_bytes();
    let mut prefix = Vec::with_capacity(destination_name.len() + 2);
    prefix.push(b'.');
    prefix.extend_from_slice(destination_name);
    prefix.push(b'.');
    let body = name
        .strip_prefix(prefix.as_slice())?
        .strip_suffix(b".tmp")?;
    let mut components = body.split(|byte| *byte == b'.');
    let process_id = components.next()?;
    let created_at = components.next()?;
    let attempt = components.next()?;
    if components.next().is_some()
        || !canonical_decimal_v1(process_id, false)
        || !canonical_decimal_v1(created_at, true)
        || !canonical_decimal_v1(attempt, true)
        || !std::str::from_utf8(attempt)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|value| value < 128)
    {
        return None;
    }
    std::str::from_utf8(process_id).ok()?.parse().ok()
}

#[cfg(not(unix))]
fn is_store_temporary_database_name_v1(_temporary: &Path, _destination: &Path) -> bool {
    false
}

#[cfg(unix)]
fn canonical_decimal_v1(value: &[u8], allow_zero: bool) -> bool {
    !value.is_empty()
        && value.iter().all(u8::is_ascii_digit)
        && (value.len() == 1 || value[0] != b'0')
        && (allow_zero || value != b"0")
}

#[cfg(unix)]
fn has_exact_private_file_permissions_v1(metadata: &fs::Metadata) -> bool {
    metadata.mode() & 0o777 == 0o600
}

#[cfg(unix)]
fn is_same_publication_file_v1(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn is_same_publication_file_v1(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

pub(crate) fn validate_existing_regular_private_file_metadata_v1(
    path: &Path,
) -> Result<fs::Metadata, StoreErrorV1> {
    let metadata = fs::symlink_metadata(path).map_err(storage_io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(unsafe_database_path_error());
    }
    if !has_private_permissions_v1(&metadata)? {
        return Err(unsafe_database_path_error());
    }
    Ok(metadata)
}

fn prepare_database_path_for_write_open_v1(path: &Path) -> Result<(), StoreErrorV1> {
    if inspect_database_path_v1(path)? == DatabasePathStateV1::Missing {
        create_private_file_if_missing_v1(path)?;
    }
    for suffix in ["-wal", "-shm"] {
        create_private_file_if_missing_v1(&sqlite_sidecar_path_v1(path, suffix))?;
    }
    validate_existing_database_path_v1(path)
}

fn create_private_file_if_missing_v1(path: &Path) -> Result<(), StoreErrorV1> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    match options.open(path) {
        Ok(file) => {
            drop(file);
            validate_existing_regular_private_file_v1(path)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            validate_existing_regular_private_file_v1(path)
        }
        Err(error) => Err(storage_io_error(error)),
    }
}

fn sqlite_sidecar_path_v1(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}
fn temporary_ownership_marker_path_v1(temporary: &Path) -> std::path::PathBuf {
    sqlite_sidecar_path_v1(temporary, ".owner")
}

const INSPECTION_SNAPSHOT_MAX_ATTEMPTS_V1: u8 = 3;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectionSnapshotCopyOutcomeV1 {
    Stable,
    Unstable,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectionSnapshotCopyPhaseV1 {
    Main,
    Wal,
}

#[cfg(test)]
type InspectionSnapshotCopyHookV1 = Box<dyn Fn(InspectionSnapshotCopyPhaseV1) + Send + Sync>;

#[cfg(test)]
static INSPECTION_SNAPSHOT_COPY_HOOK_FOR_TEST: std::sync::Mutex<
    Option<InspectionSnapshotCopyHookV1>,
> = std::sync::Mutex::new(None);

#[cfg(test)]
static INSPECTION_SNAPSHOT_COPY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
struct InspectionSnapshotCopyHookGuardV1 {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for InspectionSnapshotCopyHookGuardV1 {
    fn drop(&mut self) {
        *INSPECTION_SNAPSHOT_COPY_HOOK_FOR_TEST
            .lock()
            .expect("inspection snapshot copy hook lock") = None;
    }
}

#[cfg(test)]
fn install_inspection_snapshot_copy_hook_for_test_v1(
    hook: impl Fn(InspectionSnapshotCopyPhaseV1) + Send + Sync + 'static,
) -> InspectionSnapshotCopyHookGuardV1 {
    let lock = INSPECTION_SNAPSHOT_COPY_TEST_LOCK
        .lock()
        .expect("inspection snapshot copy test lock");
    let mut installed = INSPECTION_SNAPSHOT_COPY_HOOK_FOR_TEST
        .lock()
        .expect("inspection snapshot copy hook lock");
    assert!(
        installed.is_none(),
        "inspection snapshot copy hook already installed"
    );
    *installed = Some(Box::new(hook));
    drop(installed);
    InspectionSnapshotCopyHookGuardV1 { _lock: lock }
}

#[cfg(test)]
fn run_inspection_snapshot_copy_hook_for_test_v1(phase: InspectionSnapshotCopyPhaseV1) {
    if let Some(hook) = INSPECTION_SNAPSHOT_COPY_HOOK_FOR_TEST
        .lock()
        .expect("inspection snapshot copy hook lock")
        .as_ref()
    {
        hook(phase);
    }
}

#[derive(Debug)]
struct InspectionSnapshotFileV1 {
    file: File,
    fingerprint: InspectionFileFingerprintV1,
}

#[derive(Debug, Eq, PartialEq)]
struct InspectionFileFingerprintV1 {
    length: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
    content_digest: [u8; 32],
}

fn copy_database_for_inspection_v1(source: &Path, destination: &Path) -> Result<(), StoreErrorV1> {
    let source_wal = sqlite_sidecar_path_v1(source, "-wal");
    let destination_wal = sqlite_sidecar_path_v1(destination, "-wal");
    let destination_shm = sqlite_sidecar_path_v1(destination, "-shm");

    for _ in 0..INSPECTION_SNAPSHOT_MAX_ATTEMPTS_V1 {
        clear_inspection_clone_v1(destination)?;
        clear_inspection_clone_v1(&destination_wal)?;
        clear_inspection_clone_v1(&destination_shm)?;

        let mut main = capture_inspection_file_v1(source)?;
        let mut wal = capture_optional_inspection_file_v1(&source_wal)?;
        #[cfg(test)]
        run_inspection_snapshot_copy_hook_for_test_v1(InspectionSnapshotCopyPhaseV1::Main);
        if copy_inspection_file_v1(&mut main, destination)?
            == InspectionSnapshotCopyOutcomeV1::Unstable
        {
            continue;
        }
        if let Some(wal) = wal.as_mut() {
            #[cfg(test)]
            run_inspection_snapshot_copy_hook_for_test_v1(InspectionSnapshotCopyPhaseV1::Wal);
            if copy_inspection_file_v1(wal, &destination_wal)?
                == InspectionSnapshotCopyOutcomeV1::Unstable
            {
                continue;
            }
        }

        let main_after = fingerprint_inspection_path_v1(source)?;
        let wal_after = fingerprint_optional_inspection_path_v1(&source_wal)?;
        if main.fingerprint == main_after
            && wal.as_ref().map(|file| &file.fingerprint) == wal_after.as_ref()
        {
            return Ok(());
        }
    }

    Err(StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::Busy,
    })
}

fn clear_inspection_clone_v1(path: &Path) -> Result<(), StoreErrorV1> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage_io_error(error)),
    }
}

fn capture_inspection_file_v1(path: &Path) -> Result<InspectionSnapshotFileV1, StoreErrorV1> {
    validate_existing_regular_private_file_v1(path)?;
    let mut file = File::open(path).map_err(storage_io_error)?;
    let fingerprint = fingerprint_open_inspection_file_v1(&mut file)?;
    Ok(InspectionSnapshotFileV1 { file, fingerprint })
}

fn capture_optional_inspection_file_v1(
    path: &Path,
) -> Result<Option<InspectionSnapshotFileV1>, StoreErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(_) => capture_inspection_file_v1(path).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(storage_io_error(error)),
    }
}

fn fingerprint_inspection_path_v1(
    path: &Path,
) -> Result<InspectionFileFingerprintV1, StoreErrorV1> {
    capture_inspection_file_v1(path).map(|snapshot| snapshot.fingerprint)
}

fn fingerprint_optional_inspection_path_v1(
    path: &Path,
) -> Result<Option<InspectionFileFingerprintV1>, StoreErrorV1> {
    capture_optional_inspection_file_v1(path).map(|snapshot| snapshot.map(|file| file.fingerprint))
}

fn fingerprint_open_inspection_file_v1(
    file: &mut File,
) -> Result<InspectionFileFingerprintV1, StoreErrorV1> {
    let metadata = file.metadata().map_err(storage_io_error)?;
    if !metadata.file_type().is_file() || !has_private_permissions_v1(&metadata)? {
        return Err(unsafe_database_path_error());
    }

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8 * 1024];
    file.seek(SeekFrom::Start(0)).map_err(storage_io_error)?;
    loop {
        let read = file.read(&mut buffer).map_err(storage_io_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0)).map_err(storage_io_error)?;

    Ok(InspectionFileFingerprintV1 {
        length: metadata.len(),
        modified: metadata.modified().map_err(storage_io_error)?,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed_seconds: metadata.ctime(),
        #[cfg(unix)]
        changed_nanoseconds: metadata.ctime_nsec(),
        content_digest: hasher.finalize().into(),
    })
}

fn copy_inspection_file_v1(
    source: &mut InspectionSnapshotFileV1,
    destination: &Path,
) -> Result<InspectionSnapshotCopyOutcomeV1, StoreErrorV1> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut destination_file = options.open(destination).map_err(storage_io_error)?;
    source
        .file
        .seek(SeekFrom::Start(0))
        .map_err(storage_io_error)?;
    let copied =
        std::io::copy(&mut source.file, &mut destination_file).map_err(storage_io_error)?;
    destination_file.sync_all().map_err(storage_io_error)?;
    let destination_fingerprint = fingerprint_open_inspection_file_v1(&mut destination_file)?;
    if copied != source.fingerprint.length
        || destination_fingerprint.length != source.fingerprint.length
        || destination_fingerprint.content_digest != source.fingerprint.content_digest
    {
        return Ok(InspectionSnapshotCopyOutcomeV1::Unstable);
    }
    Ok(InspectionSnapshotCopyOutcomeV1::Stable)
}

#[cfg(unix)]
fn has_private_permissions_v1(metadata: &fs::Metadata) -> Result<bool, StoreErrorV1> {
    Ok(metadata.mode() & 0o077 == 0)
}

#[cfg(not(unix))]
fn has_private_permissions_v1(_metadata: &fs::Metadata) -> Result<bool, StoreErrorV1> {
    Err(StoreErrorV1::InvalidStateV1(
        crate::StoreValueErrorV1::UnsupportedPrivatePermissionPlatform,
    ))
}

fn unsafe_database_path_error() -> StoreErrorV1 {
    StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::StorageIo,
    }
}

fn storage_io_error(_error: std::io::Error) -> StoreErrorV1 {
    StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::StorageIo,
    }
}
fn integrity_error(check: StoreIntegrityCheckV1) -> StoreErrorV1 {
    StoreErrorV1::StorageIntegrityV1 { check }
}

fn storage_error(error: rusqlite::Error) -> StoreErrorV1 {
    map_rusqlite_error_v1(
        error,
        RusqliteErrorContextV1::Integrity(StoreIntegrityCheckV1::SqliteQuickCheck),
    )
}

#[cfg(all(test, unix))]
mod inspection_snapshot_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn write_private_file(path: &Path, contents: &[u8]) {
        fs::write(path, contents).expect("write private inspection source");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("set private inspection source permissions");
    }

    fn directory_entries(path: &Path) -> Vec<std::ffi::OsString> {
        let mut entries = fs::read_dir(path)
            .expect("read inspection source directory")
            .map(|entry| entry.expect("read inspection source entry").file_name())
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }

    #[test]
    fn unbound_inspection_never_finalizes_an_interrupted_publication() {
        let directory = TempDir::new().expect("inspection source directory");
        let source = directory.path().join("state.sqlite3");
        let publication = directory.path().join(".state.sqlite3.1.0.0.tmp");
        write_private_file(&source, b"unbound inspection source");
        fs::hard_link(&source, &publication).expect("interrupted publication link");

        let source_bytes = fs::read(&source).expect("authoritative bytes before inspection");
        let entries_before = directory_entries(directory.path());
        let result = inspect_database_snapshot_unbound_v1(
            &source,
            &SqliteStoreOptionsV1::new(8).expect("inspection options"),
            |_| Ok(()),
        );

        assert!(matches!(
            result,
            Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::StorageIo
            })
        ));
        assert_eq!(
            fs::read(&source).expect("authoritative bytes after inspection"),
            source_bytes
        );
        assert_eq!(directory_entries(directory.path()), entries_before);
        assert!(publication.exists());
    }

    #[test]
    fn unstable_main_copy_retries_and_accepts_the_next_stable_snapshot() {
        let directory = TempDir::new().expect("inspection source directory");
        let source = directory.path().join("state.sqlite3");
        let destination = directory.path().join("inspection.sqlite3");
        write_private_file(&source, b"main-before");

        let main_copy_calls = Arc::new(AtomicUsize::new(0));
        let hook_source = source.clone();
        let hook_calls = Arc::clone(&main_copy_calls);
        let hook = install_inspection_snapshot_copy_hook_for_test_v1(move |phase| {
            if phase == InspectionSnapshotCopyPhaseV1::Main
                && hook_calls.fetch_add(1, Ordering::SeqCst) == 0
            {
                write_private_file(&hook_source, b"main-after!");
            }
        });

        copy_database_for_inspection_v1(&source, &destination).expect("stable retry succeeds");
        drop(hook);

        assert_eq!(main_copy_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            fs::read(&destination).expect("stable inspection snapshot"),
            fs::read(&source).expect("stable authoritative source")
        );
    }

    #[test]
    fn unstable_wal_copy_exhaustion_is_busy() {
        let directory = TempDir::new().expect("inspection source directory");
        let source = directory.path().join("state.sqlite3");
        let source_wal = sqlite_sidecar_path_v1(&source, "-wal");
        let destination = directory.path().join("inspection.sqlite3");
        write_private_file(&source, b"main-stable");
        write_private_file(&source_wal, b"wal-before");

        let wal_copy_calls = Arc::new(AtomicUsize::new(0));
        let hook_wal = source_wal.clone();
        let hook_calls = Arc::clone(&wal_copy_calls);
        let hook = install_inspection_snapshot_copy_hook_for_test_v1(move |phase| {
            if phase == InspectionSnapshotCopyPhaseV1::Wal {
                let next = hook_calls.fetch_add(1, Ordering::SeqCst);
                let contents: &[u8] = if next & 1 == 0 {
                    b"wal-after!"
                } else {
                    b"wal-before"
                };
                write_private_file(&hook_wal, contents);
            }
        });

        let result = copy_database_for_inspection_v1(&source, &destination);
        drop(hook);

        assert!(matches!(
            result,
            Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy
            })
        ));
        assert_eq!(
            wal_copy_calls.load(Ordering::SeqCst),
            usize::from(INSPECTION_SNAPSHOT_MAX_ATTEMPTS_V1)
        );
    }
}
