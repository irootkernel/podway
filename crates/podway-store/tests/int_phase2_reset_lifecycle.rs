//! Reset-target lifecycle contracts for the Store-owned SQLite boundary.

use std::fs;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};

use podway_core::{
    DomainCommand, DomainResult, JobId, Revision, Sha256Digest, UnixMillis, WorkspaceId,
};
use podway_store::schema::{SQLITE_INITIAL_MIGRATION_NAME_V1, SQLITE_SCHEMA_VERSION_V1};
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, ClaimedExecutionV1, DurableWorktreeIdentityV1,
    IdempotencyKeyV1, JobReceiptOrTerminalV1, JobReceiptV1, PersistedTerminalJobProjectionV1,
    PersistedTerminalJobStateV1, PersistedTerminalReceiptV1, RevisionAttemptItemPreconditionsV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1, StoreErrorV1, StoreFailpointActionV1,
    StoreFailpointV1, StoreIntegrityCheckV1, StoreInvariantV1, StoreRecordKindV1,
    TerminalReceiptV1, TerminalResultV1, ValidatedWorkspaceRootV1,
    codec::{
        PersistedDomainResultV1, PersistedTerminalResultV1, encode_command_v1,
        encode_persisted_terminal_receipt_v1,
    },
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
const EXPECTED_SQLITE_V1_MIGRATION_SHA256: &str =
    "sha256:20ea04d9635b8e1632e6d3aa5f3a888eaca49307b43ade9b9991363b30607423";

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        digest('b'),
    )
}

fn alternate_identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000002").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-reset-lifecycle")).unwrap()
}

fn options(failpoint: Option<StoreFailpointV1>) -> SqliteStoreOptionsV1 {
    SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(failpoint)
}
fn abort_options(failpoint: StoreFailpointV1) -> SqliteStoreOptionsV1 {
    options(Some(failpoint)).with_failpoint_action(StoreFailpointActionV1::AbortProcess)
}

fn job(number: u8) -> JobId {
    JobId::new(format!("00000000-0000-4000-8000-{:012x}", number)).unwrap()
}

fn request(number: u8, digest_nibble: char) -> AdmitRequestV1 {
    AdmitRequestV1::new(
        DomainCommand::WorkspaceResetAll,
        IdempotencyKeyV1::new(format!("reset-{number}")).unwrap(),
        job(number),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest(digest_nibble),
        UnixMillis::new(10),
    )
}

fn reset_result(identity: &DurableWorktreeIdentityV1) -> DomainResult {
    DomainResult::WorkspaceReset {
        workspace_id: identity.workspace_uuid().clone(),
        revision: Revision::ZERO,
    }
}

fn seed(
    path: &Path,
    target_identity: DurableWorktreeIdentityV1,
    options: SqliteStoreOptionsV1,
    request: AdmitRequestV1,
    result: DomainResult,
    now: u64,
) -> Result<podway_store::TerminalReceiptV1, StoreErrorV1> {
    SqliteStoreV1::seed_or_verify_reset_target(
        path,
        &root(),
        target_identity,
        options,
        request,
        result,
        UnixMillis::new(now),
    )
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}
const RESET_CRASH_CHILD_PATH_ENV: &str = "PODWAY_RESET_CRASH_CHILD_PATH";
const RESET_CRASH_CASE_ENV: &str = "PODWAY_RESET_CRASH_CASE";
const RESET_CRASH_CHILD_TEST_NAME: &str =
    "pac_030_interrupted_reset_all_publication_recovers_and_retries_idempotently";
const RESET_CRASH_CASES: &[(u8, StoreFailpointV1, bool)] = &[
    (10, StoreFailpointV1::ResetBeforeSeedCommit, false),
    (
        11,
        StoreFailpointV1::ResetBeforeSeedCommitAndTemporaryCleanup,
        false,
    ),
    (
        12,
        StoreFailpointV1::ResetAfterSeedCommitBeforePublication,
        false,
    ),
    (
        13,
        StoreFailpointV1::ResetAfterPublicationBeforeResponse,
        true,
    ),
    (
        14,
        StoreFailpointV1::ResetAfterPublicationBeforeResponseAndTemporaryCleanup,
        true,
    ),
    (
        15,
        StoreFailpointV1::PublicationAfterDestinationLinkBeforeTemporaryUnlink,
        true,
    ),
];
const RESET_INTERRUPTED_LINK_CRASH_CASE_INDEX: usize = RESET_CRASH_CASES.len() - 1;

type ResetJobSnapshot = (
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

type ResetIdempotencySnapshot = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

type ResetJournalSnapshot = (
    i64,
    String,
    String,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    i64,
);

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResetTargetSnapshot {
    database_bytes: Vec<u8>,
    wal_bytes: Option<Vec<u8>>,
    shm_bytes: Option<Vec<u8>>,
    schema_version: i64,
    migration: (i64, String, String, i64),
    workspace: (String, String, String, String, i64, i64, i64),
    counts: (i64, i64, i64),
    empty_reset_tables: Vec<(String, i64)>,
    job: ResetJobSnapshot,
    idempotency: ResetIdempotencySnapshot,
    journal: ResetJournalSnapshot,
}

fn optional_file_bytes(path: &Path) -> Option<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => panic!("failed to read {path:?}: {error}"),
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct DatabaseFileSnapshot {
    database_bytes: Vec<u8>,
    wal_bytes: Option<Vec<u8>>,
    shm_bytes: Option<Vec<u8>>,
}

fn snapshot_database_files(path: &Path) -> DatabaseFileSnapshot {
    DatabaseFileSnapshot {
        database_bytes: fs::read(path).unwrap(),
        wal_bytes: optional_file_bytes(&sidecar(path, "-wal")),
        shm_bytes: optional_file_bytes(&sidecar(path, "-shm")),
    }
}

fn assert_database_files_unchanged(actual: DatabaseFileSnapshot, expected: &DatabaseFileSnapshot) {
    assert_eq!(
        actual.database_bytes, expected.database_bytes,
        "main database bytes must be preserved exactly"
    );
    assert_eq!(
        actual.wal_bytes, expected.wal_bytes,
        "WAL sidecar bytes must be preserved exactly"
    );
    assert_eq!(
        actual.shm_bytes, expected.shm_bytes,
        "shared-memory sidecar bytes must be preserved exactly"
    );
}

#[cfg(unix)]
fn linked_publication_temporaries(path: &Path) -> Vec<PathBuf> {
    let destination_metadata = fs::symlink_metadata(path).unwrap();
    let parent = path.parent().unwrap();
    fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| candidate.as_path() != path)
        .filter(|candidate| {
            let metadata = fs::symlink_metadata(candidate).unwrap();
            metadata.file_type().is_file()
                && metadata.dev() == destination_metadata.dev()
                && metadata.ino() == destination_metadata.ino()
        })
        .collect()
}

fn assert_recovered_reset_publication_directory_clean(path: &Path, failpoint: StoreFailpointV1) {
    let parent = path.parent().unwrap();
    let valid_destination_set = [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ];
    let entries = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    for candidate in &entries {
        assert!(
            valid_destination_set.contains(candidate),
            "{failpoint:?} recovery left Store-owned publication artifact {candidate:?} outside the valid destination set"
        );
    }
    let destination_metadata = fs::symlink_metadata(path).unwrap();
    assert!(
        destination_metadata.file_type().is_file(),
        "{failpoint:?} recovery must leave a regular destination database"
    );

    #[cfg(unix)]
    {
        assert_eq!(
            destination_metadata.nlink(),
            1,
            "{failpoint:?} recovery must leave exactly one destination hard link"
        );
        let duplicate_links = entries
            .iter()
            .filter(|candidate| candidate.as_path() != path)
            .filter(|candidate| {
                let metadata = fs::symlink_metadata(candidate).unwrap();
                metadata.file_type().is_file()
                    && metadata.dev() == destination_metadata.dev()
                    && metadata.ino() == destination_metadata.ino()
            })
            .collect::<Vec<_>>();
        assert!(
            duplicate_links.is_empty(),
            "{failpoint:?} recovery left a second destination or temporary publication link: {duplicate_links:?}"
        );
    }
}

fn snapshot_reset_target(path: &Path) -> ResetTargetSnapshot {
    let database_bytes = fs::read(path).unwrap();
    let wal_bytes = optional_file_bytes(&sidecar(path, "-wal"));
    let shm_bytes = optional_file_bytes(&sidecar(path, "-shm"));
    let canonical_path = fs::canonicalize(path).unwrap();
    let immutable_uri = format!("file:{}?immutable=1", canonical_path.display());
    let connection = Connection::open_with_flags(
        immutable_uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let schema_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let migration = connection
        .query_row(
            "SELECT version, name, checksum, applied_at_ms FROM schema_migrations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let workspace = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
             last_validated_root, next_workspace_sequence, created_at_ms, updated_at_ms \
             FROM workspace_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();
    let counts = (
        connection
            .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
            .unwrap(),
        connection
            .query_row("SELECT COUNT(*) FROM idempotency_records", [], |row| {
                row.get(0)
            })
            .unwrap(),
        connection
            .query_row("SELECT COUNT(*) FROM operational_journal", [], |row| {
                row.get(0)
            })
            .unwrap(),
    );
    let empty_reset_tables = [
        "procedure_snapshots",
        "task_sessions",
        "stage_progress",
        "attempts",
        "item_slots",
        "blockers",
    ]
    .into_iter()
    .map(|table| {
        (
            table.to_owned(),
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap(),
        )
    })
    .collect();
    let job = connection
        .query_row(
            "SELECT job_id, workspace_sequence, idempotency_key, request_digest, command_name, \
             canonical_request_json, state, session_id, submitted_at_ms, claimed_at_ms, \
             finished_at_ms, terminal_response_json FROM jobs",
            [],
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
                ))
            },
        )
        .unwrap();
    let idempotency = connection
        .query_row(
            "SELECT idempotency_key, request_digest, job_id, scope_kind, scope_session_id, \
             terminal_response_json, created_at_ms, updated_at_ms FROM idempotency_records",
            [],
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
                ))
            },
        )
        .unwrap();
    let journal = connection
        .query_row(
            "SELECT journal_id, level, event_name, workspace_sequence, job_id, summary, \
             details_json, recorded_at_ms FROM operational_journal",
            [],
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
                ))
            },
        )
        .unwrap();
    drop(connection);

    ResetTargetSnapshot {
        database_bytes,
        wal_bytes,
        shm_bytes,
        schema_version,
        migration,
        workspace,
        counts,
        empty_reset_tables,
        job,
        idempotency,
        journal,
    }
}

fn assert_allowed_reset_target_sidecar_lifecycle(
    actual: &ResetTargetSnapshot,
    expected: &ResetTargetSnapshot,
) {
    for (suffix, actual_bytes, expected_bytes) in [
        (
            "-wal",
            actual.wal_bytes.as_deref(),
            expected.wal_bytes.as_deref(),
        ),
        (
            "-shm",
            actual.shm_bytes.as_deref(),
            expected.shm_bytes.as_deref(),
        ),
    ] {
        match (actual_bytes, expected_bytes) {
            (None, None) => {}
            (Some(actual_bytes), Some(expected_bytes)) => assert_eq!(
                actual_bytes, expected_bytes,
                "reset target {suffix} sidecar contents must be preserved exactly"
            ),
            (actual_bytes, expected_bytes) => panic!(
                "reset target {suffix} sidecar lifecycle changed from {expected_bytes:?} to {actual_bytes:?}"
            ),
        }
    }
}

fn assert_reset_target_unchanged(actual: ResetTargetSnapshot, expected: &ResetTargetSnapshot) {
    assert_eq!(
        actual.database_bytes, expected.database_bytes,
        "reset retry must preserve the main database bytes exactly"
    );
    assert_allowed_reset_target_sidecar_lifecycle(&actual, expected);
    assert_eq!(actual.schema_version, expected.schema_version);
    assert_eq!(actual.migration, expected.migration);
    assert_eq!(actual.workspace, expected.workspace);
    assert_eq!(actual.counts, expected.counts);
    assert_eq!(actual.empty_reset_tables, expected.empty_reset_tables);
    assert_eq!(actual.job, expected.job);
    assert_eq!(actual.idempotency, expected.idempotency);
    assert_eq!(actual.journal, expected.journal);
}

fn reset_receipt(request: &AdmitRequestV1, result: DomainResult) -> TerminalReceiptV1 {
    TerminalReceiptV1::new(
        JobReceiptV1::new(
            1,
            request.job_id().clone(),
            request.request_digest().clone(),
        ),
        TerminalResultV1::Success(result),
    )
}

fn canonical_reset_terminal_receipt_v1_json(
    request: &AdmitRequestV1,
    target_identity: &DurableWorktreeIdentityV1,
    now: u64,
) -> String {
    let receipt = PersistedTerminalReceiptV1::new_with_projections(
        JobReceiptV1::new(
            1,
            request.job_id().clone(),
            request.request_digest().clone(),
        ),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::WorkspaceReset {
            workspace_id: target_identity.workspace_uuid().clone(),
            revision: Revision::ZERO,
        }),
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Succeeded,
            request.submitted_at(),
            None,
            UnixMillis::new(now),
        )
        .unwrap(),
        None,
    )
    .unwrap();
    encode_persisted_terminal_receipt_v1(&receipt).unwrap()
}

fn assert_exact_published_reset_target(
    path: &Path,
    target_identity: &DurableWorktreeIdentityV1,
    request: &AdmitRequestV1,
    result: &DomainResult,
    now: u64,
) -> ResetTargetSnapshot {
    let snapshot = snapshot_reset_target(path);
    assert_eq!(result, &reset_result(target_identity));
    let canonical_request = encode_command_v1(&ClaimedExecutionV1::new_with_canonical_execution(
        request.command().clone(),
        request.preconditions().clone(),
        request.canonical_execution().clone(),
    ))
    .unwrap();
    let terminal_response = canonical_reset_terminal_receipt_v1_json(request, target_identity, now);

    assert!(!snapshot.database_bytes.is_empty());
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
    #[cfg(unix)]
    for (suffix, bytes) in [
        ("-wal", snapshot.wal_bytes.as_ref()),
        ("-shm", snapshot.shm_bytes.as_ref()),
    ] {
        if bytes.is_some() {
            let metadata = fs::symlink_metadata(sidecar(path, suffix)).unwrap();
            assert!(metadata.is_file());
            assert!(!metadata.file_type().is_symlink());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }
    assert_eq!(snapshot.schema_version, i64::from(SQLITE_SCHEMA_VERSION_V1));
    assert_eq!(
        snapshot.migration,
        (
            i64::from(SQLITE_SCHEMA_VERSION_V1),
            SQLITE_INITIAL_MIGRATION_NAME_V1.to_owned(),
            EXPECTED_SQLITE_V1_MIGRATION_SHA256.to_owned(),
            now as i64,
        )
    );
    assert_eq!(
        snapshot.workspace,
        (
            target_identity.workspace_uuid().as_str().to_owned(),
            target_identity.common_dir_identity().as_str().to_owned(),
            target_identity
                .worktree_admin_identity()
                .as_str()
                .to_owned(),
            root().as_encoded().to_owned(),
            1,
            now as i64,
            now as i64,
        )
    );
    assert_eq!(snapshot.counts, (1, 1, 1));
    assert_eq!(
        snapshot.empty_reset_tables,
        vec![
            ("procedure_snapshots".to_owned(), 0),
            ("task_sessions".to_owned(), 0),
            ("stage_progress".to_owned(), 0),
            ("attempts".to_owned(), 0),
            ("item_slots".to_owned(), 0),
            ("blockers".to_owned(), 0),
        ]
    );
    assert_eq!(
        snapshot.job,
        (
            request.job_id().as_str().to_owned(),
            1,
            request.idempotency_key().as_str().to_owned(),
            request.request_digest().as_str().to_owned(),
            "workspace.reset_all".to_owned(),
            canonical_request,
            "succeeded".to_owned(),
            None,
            request.submitted_at().get() as i64,
            None,
            Some(now as i64),
            Some(terminal_response.clone()),
        )
    );
    assert_eq!(
        snapshot.idempotency,
        (
            request.idempotency_key().as_str().to_owned(),
            request.request_digest().as_str().to_owned(),
            request.job_id().as_str().to_owned(),
            "workspace".to_owned(),
            None,
            Some(terminal_response),
            now as i64,
            now as i64,
        )
    );
    assert_eq!(
        snapshot.journal,
        (
            1,
            "info".to_owned(),
            "workspace.reset_all.seeded".to_owned(),
            Some(1),
            Some(request.job_id().as_str().to_owned()),
            "workspace reset target seeded".to_owned(),
            None,
            now as i64,
        )
    );
    snapshot
}
fn assert_exact_initialized_destination(
    path: &Path,
    target_identity: &DurableWorktreeIdentityV1,
    now: u64,
) {
    let database_bytes = fs::read(path).unwrap();
    assert!(!database_bytes.is_empty());
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    let canonical_path = fs::canonicalize(path).unwrap();
    let immutable_uri = format!("file:{}?immutable=1", canonical_path.display());
    let connection = Connection::open_with_flags(
        immutable_uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let schema_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .unwrap();
    assert_eq!(schema_version, i64::from(SQLITE_SCHEMA_VERSION_V1));
    let mut statement = connection
        .prepare(
            "SELECT type, name FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .unwrap();
    let schema_objects: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        schema_objects,
        vec![
            ("index".to_owned(), "ix_attempts_stage".to_owned()),
            ("index".to_owned(), "ix_blockers_attempt_state".to_owned()),
            ("index".to_owned(), "ix_idempotency_scope".to_owned()),
            ("index".to_owned(), "ix_jobs_state_sequence".to_owned()),
            ("index".to_owned(), "ix_jobs_terminal_time".to_owned()),
            ("index".to_owned(), "ix_operational_journal_time".to_owned(),),
            ("index".to_owned(), "ux_attempts_one_active".to_owned()),
            (
                "index".to_owned(),
                "ux_stage_progress_one_current".to_owned(),
            ),
            ("table".to_owned(), "attempts".to_owned()),
            ("table".to_owned(), "blockers".to_owned()),
            ("table".to_owned(), "idempotency_records".to_owned()),
            ("table".to_owned(), "item_slots".to_owned()),
            ("table".to_owned(), "jobs".to_owned()),
            ("table".to_owned(), "operational_journal".to_owned()),
            ("table".to_owned(), "procedure_snapshots".to_owned()),
            ("table".to_owned(), "schema_migrations".to_owned()),
            ("table".to_owned(), "stage_progress".to_owned()),
            ("table".to_owned(), "task_sessions".to_owned()),
            ("table".to_owned(), "workspace_state".to_owned()),
        ]
    );
    let migration_count = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(migration_count, 1);
    let migration = connection
        .query_row(
            "SELECT version, name, checksum, applied_at_ms FROM schema_migrations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        migration,
        (
            i64::from(SQLITE_SCHEMA_VERSION_V1),
            SQLITE_INITIAL_MIGRATION_NAME_V1.to_owned(),
            EXPECTED_SQLITE_V1_MIGRATION_SHA256.to_owned(),
            now as i64,
        )
    );
    let workspace_count = connection
        .query_row("SELECT COUNT(*) FROM workspace_state", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(workspace_count, 1);
    let workspace = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
             last_validated_root, next_workspace_sequence, created_at_ms, updated_at_ms \
             FROM workspace_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        workspace,
        (
            target_identity.workspace_uuid().as_str().to_owned(),
            target_identity.common_dir_identity().as_str().to_owned(),
            target_identity
                .worktree_admin_identity()
                .as_str()
                .to_owned(),
            root().as_encoded().to_owned(),
            0,
            now as i64,
            now as i64,
        )
    );
    for table in [
        "procedure_snapshots",
        "task_sessions",
        "stage_progress",
        "attempts",
        "item_slots",
        "blockers",
        "jobs",
        "idempotency_records",
        "operational_journal",
    ] {
        let count = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(count, 0, "new destination table {table} must be empty");
    }
}

struct ResetMismatchCase {
    identity: DurableWorktreeIdentityV1,
    request: AdmitRequestV1,
    result: DomainResult,
    now: u64,
    expected_error: StoreErrorV1,
}

fn assert_mismatch_preserves_reset_target(
    path: &Path,
    original_identity: &DurableWorktreeIdentityV1,
    original_request: &AdmitRequestV1,
    original_result: &DomainResult,
    expected_snapshot: &ResetTargetSnapshot,
    mismatch: ResetMismatchCase,
) {
    let before = snapshot_reset_target(path);
    assert_reset_target_unchanged(before.clone(), expected_snapshot);
    assert_eq!(
        seed(
            path,
            mismatch.identity,
            options(None),
            mismatch.request,
            mismatch.result,
            mismatch.now,
        )
        .unwrap_err(),
        mismatch.expected_error
    );
    assert_reset_target_unchanged(snapshot_reset_target(path), &before);

    let replay = seed(
        path,
        original_identity.clone(),
        options(None),
        original_request.clone(),
        original_result.clone(),
        mismatch.now + 1,
    )
    .unwrap();
    assert_eq!(
        replay,
        reset_receipt(original_request, original_result.clone())
    );
    assert_reset_target_unchanged(snapshot_reset_target(path), &before);
}

#[test]
fn maintenance_close_checkpoints_dirty_wal_before_daemon_removal() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let workspace = identity();
    let store = SqliteStoreV1::open(
        &path,
        &root(),
        workspace.clone(),
        options(None),
        UnixMillis::new(1),
    )
    .unwrap();
    let maintenance_request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("dirty-wal").unwrap(),
        job(1),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(2),
    );
    match store
        .admit(&workspace, maintenance_request.clone())
        .unwrap()
    {
        AdmitOutcomeV1::New(receipt) => {
            assert_eq!(receipt.identity_sequence(), 1);
            assert_eq!(receipt.job_id(), maintenance_request.job_id());
            assert_eq!(
                receipt.request_digest(),
                maintenance_request.request_digest()
            );
        }
        outcome => panic!("expected a new dirty-WAL admission, got {outcome:?}"),
    }
    assert!(sidecar(&path, "-wal").exists());

    store.close_for_maintenance().unwrap();
    for suffix in ["-wal", "-shm"] {
        match fs::remove_file(sidecar(&path, suffix)) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => panic!("failed to remove closed SQLite sidecar {suffix}: {error}"),
        }
    }
    assert!(path.exists());

    let reopened = SqliteStoreV1::open(
        &path,
        &root(),
        workspace.clone(),
        options(None),
        UnixMillis::new(3),
    )
    .unwrap();
    let view = reopened.read_workspace_view(&workspace).unwrap();
    assert_eq!(view.queued_job_count(), 1);
    assert!(view.running_job_id().is_none());
    match reopened.admit(&workspace, maintenance_request).unwrap() {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt)) => {
            assert_eq!(receipt.identity_sequence(), 1);
            assert_eq!(receipt.job_id(), &job(1));
            assert_eq!(receipt.request_digest(), &digest('c'));
        }
        outcome => panic!("expected exact idempotency replay after maintenance, got {outcome:?}"),
    }
}

#[test]
fn pac_030_interrupted_reset_all_publication_recovers_and_retries_idempotently() {
    match (
        std::env::var_os(RESET_CRASH_CHILD_PATH_ENV).map(PathBuf::from),
        std::env::var(RESET_CRASH_CASE_ENV),
    ) {
        (Some(path), Ok(case_index)) => {
            let case_index = case_index
                .parse::<usize>()
                .expect("PAC-030 crash child case must be a numeric configured index");
            let (number, failpoint, _) = *RESET_CRASH_CASES
                .get(case_index)
                .expect("PAC-030 crash case index must identify a configured failpoint");
            let workspace = identity();
            let _ = seed(
                &path,
                workspace.clone(),
                abort_options(failpoint),
                request(number, 'c'),
                reset_result(&workspace),
                20,
            );
            panic!("PAC-030 configured reset failpoint returned instead of aborting");
        }
        (None, Err(_)) => {}
        state => panic!("PAC-030 crash child mode requires both inputs, got {state:?}"),
    }

    for (case_index, (number, failpoint, published)) in RESET_CRASH_CASES.iter().enumerate() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("target.sqlite3");
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(RESET_CRASH_CHILD_TEST_NAME)
            .env(RESET_CRASH_CHILD_PATH_ENV, &path)
            .env(RESET_CRASH_CASE_ENV, case_index.to_string())
            .status()
            .unwrap();
        assert!(
            !child.success(),
            "{failpoint:?} child unexpectedly returned success"
        );
        #[cfg(unix)]
        assert_eq!(
            child.signal(),
            Some(6),
            "{failpoint:?} child must terminate via SIGABRT at the failpoint"
        );

        let workspace = identity();
        let original_request = request(*number, 'c');
        let original_result = reset_result(&workspace);
        if *published {
            let before = assert_exact_published_reset_target(
                &path,
                &workspace,
                &original_request,
                &original_result,
                20,
            );
            let replay = seed(
                &path,
                workspace.clone(),
                options(None),
                original_request.clone(),
                original_result.clone(),
                21,
            )
            .unwrap();
            assert_eq!(
                replay,
                reset_receipt(&original_request, original_result.clone())
            );
            assert_recovered_reset_publication_directory_clean(&path, *failpoint);
            let repeated = seed(
                &path,
                workspace.clone(),
                options(None),
                original_request.clone(),
                original_result.clone(),
                22,
            )
            .unwrap();
            assert_eq!(repeated, reset_receipt(&original_request, original_result));
            assert_reset_target_unchanged(snapshot_reset_target(&path), &before);
            assert_recovered_reset_publication_directory_clean(&path, *failpoint);
        } else {
            assert!(!path.exists());
            let replay = seed(
                &path,
                workspace.clone(),
                options(None),
                original_request.clone(),
                original_result.clone(),
                21,
            )
            .unwrap();
            assert_eq!(
                replay,
                reset_receipt(&original_request, original_result.clone())
            );
            assert_exact_published_reset_target(
                &path,
                &workspace,
                &original_request,
                &original_result,
                21,
            );
            assert_recovered_reset_publication_directory_clean(&path, *failpoint);
            let repeated = seed(
                &path,
                workspace.clone(),
                options(None),
                original_request.clone(),
                original_result.clone(),
                22,
            )
            .unwrap();
            assert_eq!(
                repeated,
                reset_receipt(&original_request, original_result.clone())
            );
            assert_recovered_reset_publication_directory_clean(&path, *failpoint);
        }
    }
}
#[cfg(unix)]
#[test]
fn reset_cleanup_failpoint_leaves_real_residual_artifacts_and_combines_errors() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let workspace = identity();
    let error = seed(
        &path,
        workspace.clone(),
        options(Some(
            StoreFailpointV1::ResetAfterPublicationBeforeResponseAndTemporaryCleanup,
        )),
        request(31, 'c'),
        reset_result(&workspace),
        20,
    )
    .expect_err("cleanup fault must be returned with the primary failpoint error");

    assert!(matches!(
        error,
        StoreErrorV1::PrimaryOperationAndCleanupFailureV1 { .. }
    ));
    assert!(path.exists(), "publication must remain durable");
    let residuals = fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".target.sqlite3."))
        .collect::<Vec<_>>();
    let temporary_name = residuals
        .iter()
        .find(|name| name.ends_with(".tmp"))
        .expect("database cleanup failure must retain the temporary database");
    let owned_temporary = temporary.path().join(temporary_name);
    let owned_marker = sidecar(&owned_temporary, ".owner");
    let owned_wal = sidecar(&owned_temporary, "-wal");
    let owned_shm = sidecar(&owned_temporary, "-shm");
    assert!(
        owned_marker.exists(),
        "database cleanup failure must retain the ownership marker"
    );

    let sentinel = temporary.path().join("unrelated-sentinel");
    let marker_sentinel = temporary.path().join(".unrelated-sentinel.owner");
    fs::write(&sentinel, b"sentinel").unwrap();
    fs::write(&marker_sentinel, b"marker-sentinel").unwrap();

    SqliteStoreV1::open(
        &path,
        &root(),
        workspace,
        options(None),
        UnixMillis::new(21),
    )
    .expect("next open must converge interrupted cleanup");

    for residual in [&owned_temporary, &owned_marker, &owned_wal, &owned_shm] {
        assert!(
            !residual.exists(),
            "next open must remove owned cleanup residual {residual:?}"
        );
    }
    assert_eq!(fs::read(&sentinel).unwrap(), b"sentinel");
    assert_eq!(fs::read(&marker_sentinel).unwrap(), b"marker-sentinel");
}
#[cfg(unix)]
#[test]
fn next_open_reaps_unlocked_orphaned_ownership_marker_without_temporary() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let workspace = identity();
    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            workspace.clone(),
            options(None),
            UnixMillis::new(20),
        )
        .expect("initial open must publish the workspace"),
    );

    // Simulate a crash between the published temporary unlink and the
    // ownership-marker unlink: the marker and sidecars survive without their
    // temporary database and without a live owner lock.
    let orphaned_temporary = temporary
        .path()
        .join(format!(".target.sqlite3.{}.77.0.tmp", std::process::id()));
    let orphaned_marker = sidecar(&orphaned_temporary, ".owner");
    let orphaned_wal = sidecar(&orphaned_temporary, "-wal");
    let orphaned_shm = sidecar(&orphaned_temporary, "-shm");
    for residual in [&orphaned_marker, &orphaned_wal, &orphaned_shm] {
        fs::write(residual, b"orphaned-residual").unwrap();
        fs::set_permissions(residual, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let sentinel_marker = temporary.path().join(".unrelated-sentinel.owner");
    fs::write(&sentinel_marker, b"marker-sentinel").unwrap();

    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            workspace,
            options(None),
            UnixMillis::new(21),
        )
        .expect("next open must reap the unlocked orphaned ownership marker"),
    );

    for residual in [&orphaned_marker, &orphaned_wal, &orphaned_shm] {
        assert!(
            !residual.exists(),
            "next open must remove orphaned publication residual {residual:?}"
        );
    }
    assert_eq!(fs::read(&sentinel_marker).unwrap(), b"marker-sentinel");
}
#[cfg(unix)]
#[test]
fn next_open_tolerates_empty_create_gap_marker_with_present_temporary() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let workspace = identity();
    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            workspace.clone(),
            options(None),
            UnixMillis::new(20),
        )
        .expect("initial open must publish the workspace"),
    );

    // Simulate a crash inside create_temporary_database's create-and-lock gap:
    // the ownership marker and its temporary database were created but the
    // marker's device/inode record was never written and synced, so both are
    // empty and the marker holds no live lock. Recovery must neither hard-fail
    // (which would permanently brick an already-initialized workspace) nor reap
    // a possibly-live creation; it leaves the empty pair, exactly like the
    // unlocked-orphan-marker path tolerates the same create-gap emptiness.
    let gap_temporary = temporary
        .path()
        .join(format!(".target.sqlite3.{}.79.0.tmp", std::process::id()));
    let gap_marker = sidecar(&gap_temporary, ".owner");
    for empty in [&gap_temporary, &gap_marker] {
        fs::write(empty, b"").unwrap();
        fs::set_permissions(empty, fs::Permissions::from_mode(0o600)).unwrap();
    }

    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            workspace.clone(),
            options(None),
            UnixMillis::new(21),
        )
        .expect("next open must tolerate an empty create-gap marker and temporary"),
    );
    // Tolerance is idempotent and the destination workspace stays openable.
    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            workspace,
            options(None),
            UnixMillis::new(22),
        )
        .expect("workspace must remain openable after the create-gap residue"),
    );
}
#[cfg(unix)]
#[test]
fn next_open_leaves_live_locked_ownership_marker_without_temporary() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let workspace = identity();
    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            workspace.clone(),
            options(None),
            UnixMillis::new(20),
        )
        .expect("initial open must publish the workspace"),
    );

    // A locked ownership marker without its temporary database is a live
    // publisher between its temporary unlink and marker unlink; recovery must
    // leave it for its owner instead of reaping or failing.
    let live_temporary = temporary
        .path()
        .join(format!(".target.sqlite3.{}.78.0.tmp", std::process::id()));
    let live_marker = sidecar(&live_temporary, ".owner");
    fs::write(&live_marker, b"live-owner").unwrap();
    fs::set_permissions(&live_marker, fs::Permissions::from_mode(0o600)).unwrap();
    let owner_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&live_marker)
        .unwrap();
    owner_lock.lock().unwrap();

    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            workspace,
            options(None),
            UnixMillis::new(21),
        )
        .expect("open must skip a live locked ownership marker"),
    );

    assert!(
        live_marker.exists(),
        "a live locked ownership marker must be left for its owner"
    );
    assert_eq!(fs::read(&live_marker).unwrap(), b"live-owner");
    drop(owner_lock);
}
#[cfg(unix)]
#[test]
fn interrupted_publication_link_recovers_one_destination_without_temporary_link() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let (number, failpoint, published) = *RESET_CRASH_CASES
        .get(RESET_INTERRUPTED_LINK_CRASH_CASE_INDEX)
        .expect("interrupted-link crash case must be configured");
    assert_eq!(
        failpoint,
        StoreFailpointV1::PublicationAfterDestinationLinkBeforeTemporaryUnlink
    );
    assert!(published);

    let child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(RESET_CRASH_CHILD_TEST_NAME)
        .env(RESET_CRASH_CHILD_PATH_ENV, &path)
        .env(
            RESET_CRASH_CASE_ENV,
            RESET_INTERRUPTED_LINK_CRASH_CASE_INDEX.to_string(),
        )
        .status()
        .unwrap();
    assert!(!child.success());
    assert_eq!(child.signal(), Some(6));

    let destination_metadata = fs::symlink_metadata(&path).unwrap();
    assert_eq!(
        destination_metadata.nlink(),
        2,
        "the destination link must survive the child crash after the parent sync"
    );
    let temporary_links = linked_publication_temporaries(&path);
    assert_eq!(
        temporary_links.len(),
        1,
        "the interrupted Store-owned temporary link must still pair with the destination"
    );
    for (suffix, contents) in [
        ("-wal", b"interrupted-link-wal".as_slice()),
        ("-shm", b"interrupted-link-shm".as_slice()),
    ] {
        let destination_sidecar = sidecar(&path, suffix);
        fs::write(&destination_sidecar, contents).unwrap();
        fs::set_permissions(&destination_sidecar, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let workspace = identity();
    let original_request = request(number, 'c');
    let original_result = reset_result(&workspace);
    let before_recovery = assert_exact_published_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        20,
    );
    assert_eq!(
        before_recovery.wal_bytes.as_deref(),
        Some(b"interrupted-link-wal".as_slice())
    );
    assert_eq!(
        before_recovery.shm_bytes.as_deref(),
        Some(b"interrupted-link-shm".as_slice())
    );
    let replay = seed(
        &path,
        workspace.clone(),
        options(None),
        original_request.clone(),
        original_result.clone(),
        21,
    )
    .unwrap();
    assert_eq!(
        replay,
        reset_receipt(&original_request, original_result.clone())
    );
    assert_eq!(
        fs::symlink_metadata(&path).unwrap().nlink(),
        1,
        "recovery must durably leave exactly one destination link"
    );
    assert!(
        linked_publication_temporaries(&path).is_empty(),
        "recovery must remove the Store-owned temporary link"
    );
    assert_reset_target_unchanged(snapshot_reset_target(&path), &before_recovery);

    let repeated = seed(
        &path,
        workspace,
        options(None),
        original_request.clone(),
        original_result.clone(),
        22,
    )
    .unwrap();
    assert_eq!(repeated, reset_receipt(&original_request, original_result));
    assert_reset_target_unchanged(snapshot_reset_target(&path), &before_recovery);
}

#[test]
fn reset_target_retry_replays_exact_receipt_despite_stale_sidecars() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    for (suffix, contents) in [("-wal", b"stale-wal".as_slice()), ("-shm", b"stale-shm")] {
        let sidecar = sidecar(&path, suffix);
        fs::write(&sidecar, contents).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let workspace = identity();
    let original_request = request(20, 'd');
    let original_result = reset_result(&workspace);
    let expected_receipt = reset_receipt(&original_request, original_result.clone());

    let first = seed(
        &path,
        workspace.clone(),
        options(None),
        original_request.clone(),
        original_result.clone(),
        30,
    )
    .unwrap();
    let first_target = assert_exact_published_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        30,
    );
    assert_eq!(first, expected_receipt);
    assert_eq!(
        first_target.wal_bytes.as_deref(),
        Some(b"stale-wal".as_slice())
    );
    assert_eq!(
        first_target.shm_bytes.as_deref(),
        Some(b"stale-shm".as_slice())
    );

    let replay = seed(
        &path,
        workspace.clone(),
        options(None),
        original_request.clone(),
        original_result.clone(),
        31,
    )
    .unwrap();
    let replay_target = assert_exact_published_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        30,
    );
    assert_eq!(replay, expected_receipt);
    assert_reset_target_unchanged(replay_target, &first_target);
}

#[test]
fn existing_destination_never_gets_replaced_by_mismatched_reset_seed() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let workspace = identity();
    seed(
        &path,
        workspace.clone(),
        options(None),
        request(30, 'e'),
        reset_result(&workspace),
        40,
    )
    .unwrap();

    assert!(
        seed(
            &path,
            workspace.clone(),
            options(None),
            request(31, 'f'),
            reset_result(&workspace),
            41,
        )
        .is_err()
    );
    let connection = Connection::open(&path).unwrap();
    let stored_job: String = connection
        .query_row("SELECT job_id FROM jobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored_job, job(30).as_str());
}

#[test]
fn concurrent_reset_seeds_publish_once_and_verify_the_winner() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let first_identity = identity();
    let first_request = request(35, 'e');
    let first_result = reset_result(&first_identity);
    let second_identity = alternate_identity();
    let second_request = request(36, 'f');
    let second_result = reset_result(&second_identity);
    let publication_boundary = Arc::new(Barrier::new(3));

    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            seed(
                &path,
                first_identity.clone(),
                options(Some(
                    StoreFailpointV1::ResetAfterSeedCommitBeforePublication,
                ))
                .with_failpoint_action(StoreFailpointActionV1::Barrier(
                    Arc::clone(&publication_boundary),
                )),
                first_request.clone(),
                first_result.clone(),
                45,
            )
        });
        let second = scope.spawn(|| {
            seed(
                &path,
                second_identity.clone(),
                options(Some(
                    StoreFailpointV1::ResetAfterSeedCommitBeforePublication,
                ))
                .with_failpoint_action(StoreFailpointActionV1::Barrier(
                    Arc::clone(&publication_boundary),
                )),
                second_request.clone(),
                second_result.clone(),
                45,
            )
        });
        publication_boundary.wait();
        (first.join().unwrap(), second.join().unwrap())
    });

    let expected_loser_error = StoreErrorV1::StorageIntegrityV1 {
        check: StoreIntegrityCheckV1::WorkspaceIdentity,
    };
    let (
        winner_identity,
        winner_request,
        winner_result,
        loser_identity,
        loser_request,
        loser_result,
    ) = match (first, second) {
        (Ok(first_receipt), Err(second_error)) => {
            assert_eq!(
                first_receipt,
                reset_receipt(&first_request, first_result.clone())
            );
            assert_eq!(second_error, expected_loser_error);
            (
                &first_identity,
                &first_request,
                &first_result,
                &second_identity,
                &second_request,
                &second_result,
            )
        }
        (Err(first_error), Ok(second_receipt)) => {
            assert_eq!(first_error, expected_loser_error);
            assert_eq!(
                second_receipt,
                reset_receipt(&second_request, second_result.clone())
            );
            (
                &second_identity,
                &second_request,
                &second_result,
                &first_identity,
                &first_request,
                &first_result,
            )
        }
        (first_outcome, second_outcome) => {
            panic!(
                "exactly one distinct reset candidate must publish: first={first_outcome:?}, second={second_outcome:?}"
            );
        }
    };

    let winner_receipt = reset_receipt(winner_request, (*winner_result).clone());
    let before = assert_exact_published_reset_target(
        &path,
        winner_identity,
        winner_request,
        winner_result,
        45,
    );
    let replay = seed(
        &path,
        (*winner_identity).clone(),
        options(None),
        (*winner_request).clone(),
        (*winner_result).clone(),
        46,
    )
    .unwrap();
    assert_eq!(replay, winner_receipt);
    assert_reset_target_unchanged(snapshot_reset_target(&path), &before);

    let mismatch = seed(
        &path,
        (*loser_identity).clone(),
        options(None),
        (*loser_request).clone(),
        (*loser_result).clone(),
        47,
    )
    .unwrap_err();
    assert_eq!(mismatch, expected_loser_error);
    assert_reset_target_unchanged(snapshot_reset_target(&path), &before);
}

#[test]
fn reset_seed_fails_closed_for_identity_request_command_and_result_mismatches() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("target.sqlite3");
    let workspace = identity();
    let original_request = request(40, 'a');
    let original_result = reset_result(&workspace);
    let first = seed(
        &path,
        workspace.clone(),
        options(None),
        original_request.clone(),
        original_result.clone(),
        50,
    )
    .unwrap();
    assert_eq!(
        first,
        reset_receipt(&original_request, original_result.clone())
    );
    let expected_snapshot = assert_exact_published_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        50,
    );

    let other = alternate_identity();
    assert_mismatch_preserves_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        &expected_snapshot,
        ResetMismatchCase {
            identity: other.clone(),
            request: original_request.clone(),
            result: reset_result(&other),
            now: 51,
            expected_error: StoreErrorV1::StorageIntegrityV1 {
                check: StoreIntegrityCheckV1::WorkspaceIdentity,
            },
        },
    );
    assert_mismatch_preserves_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        &expected_snapshot,
        ResetMismatchCase {
            identity: workspace.clone(),
            request: request(40, 'b'),
            result: original_result.clone(),
            now: 52,
            expected_error: StoreErrorV1::CorruptStateV1 {
                record: StoreRecordKindV1::Job,
            },
        },
    );

    let bad_command = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("reset-40").unwrap(),
        job(40),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('a'),
        UnixMillis::new(10),
    );
    assert_mismatch_preserves_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        &expected_snapshot,
        ResetMismatchCase {
            identity: workspace.clone(),
            request: bad_command,
            result: original_result.clone(),
            now: 53,
            expected_error: StoreErrorV1::InternalInvariantViolationV1 {
                invariant: StoreInvariantV1::ResetSeed,
            },
        },
    );
    assert_mismatch_preserves_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        &expected_snapshot,
        ResetMismatchCase {
            identity: workspace.clone(),
            request: original_request.clone(),
            result: DomainResult::WorkspaceReset {
                workspace_id: other.workspace_uuid().clone(),
                revision: Revision::ZERO,
            },
            now: 54,
            expected_error: StoreErrorV1::InternalInvariantViolationV1 {
                invariant: StoreInvariantV1::ResetSeed,
            },
        },
    );
    assert_mismatch_preserves_reset_target(
        &path,
        &workspace,
        &original_request,
        &original_result,
        &expected_snapshot,
        ResetMismatchCase {
            identity: workspace.clone(),
            request: original_request.clone(),
            result: DomainResult::WorkspaceReset {
                workspace_id: workspace.workspace_uuid().clone(),
                revision: Revision::new(1),
            },
            now: 55,
            expected_error: StoreErrorV1::InternalInvariantViolationV1 {
                invariant: StoreInvariantV1::ResetSeed,
            },
        },
    );
}

#[test]
fn ordinary_open_fails_closed_without_clobbering_an_existing_destination() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let workspace = identity();
    SqliteStoreV1::open(
        &path,
        &root(),
        workspace.clone(),
        options(None),
        UnixMillis::new(1),
    )
    .unwrap()
    .close_for_maintenance()
    .unwrap();

    let before = snapshot_database_files(&path);
    let mismatch = match SqliteStoreV1::open(
        &path,
        &root(),
        alternate_identity(),
        options(None),
        UnixMillis::new(2),
    ) {
        Ok(store) => {
            store.close_for_maintenance().unwrap();
            panic!("ordinary open with a mismatched identity unexpectedly succeeded");
        }
        Err(error) => error,
    };
    assert_eq!(
        mismatch,
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::WorkspaceIdentity,
        }
    );
    assert_database_files_unchanged(snapshot_database_files(&path), &before);
    assert_exact_initialized_destination(&path, &workspace, 1);
}

#[test]
fn concurrent_ordinary_initialization_publishes_one_distinguishable_winner() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let first_identity = identity();
    let first_now = 60;
    let second_identity = alternate_identity();
    let second_now = 61;
    let publication_boundary = Arc::new(Barrier::new(3));

    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            SqliteStoreV1::open(
                &path,
                &root(),
                first_identity.clone(),
                options(Some(
                    StoreFailpointV1::SchemaAfterInitializationBeforePublication,
                ))
                .with_failpoint_action(StoreFailpointActionV1::Barrier(
                    Arc::clone(&publication_boundary),
                )),
                UnixMillis::new(first_now),
            )
        });
        let second = scope.spawn(|| {
            SqliteStoreV1::open(
                &path,
                &root(),
                second_identity.clone(),
                options(Some(
                    StoreFailpointV1::SchemaAfterInitializationBeforePublication,
                ))
                .with_failpoint_action(StoreFailpointActionV1::Barrier(
                    Arc::clone(&publication_boundary),
                )),
                UnixMillis::new(second_now),
            )
        });
        publication_boundary.wait();
        (first.join().unwrap(), second.join().unwrap())
    });

    let expected_loser_error = StoreErrorV1::StorageIntegrityV1 {
        check: StoreIntegrityCheckV1::WorkspaceIdentity,
    };
    let (winner_identity, winner_now, winner_store) = match (first, second) {
        (Ok(store), Err(error)) => {
            assert_eq!(error, expected_loser_error);
            (&first_identity, first_now, store)
        }
        (Err(error), Ok(store)) => {
            assert_eq!(error, expected_loser_error);
            (&second_identity, second_now, store)
        }
        (Ok(first_store), Ok(second_store)) => {
            first_store.close_for_maintenance().unwrap();
            second_store.close_for_maintenance().unwrap();
            panic!("both ordinary initialization contenders published");
        }
        (Err(first_error), Err(second_error)) => {
            panic!(
                "both ordinary initialization contenders failed: first={first_error:?}, second={second_error:?}"
            );
        }
    };
    winner_store.close_for_maintenance().unwrap();

    assert_exact_initialized_destination(&path, winner_identity, winner_now);
    let before_replay = snapshot_database_files(&path);
    SqliteStoreV1::open(
        &path,
        &root(),
        winner_identity.clone(),
        options(None),
        UnixMillis::new(winner_now),
    )
    .unwrap()
    .close_for_maintenance()
    .unwrap();
    assert_database_files_unchanged(snapshot_database_files(&path), &before_replay);
    assert_exact_initialized_destination(&path, winner_identity, winner_now);
}

#[test]
fn concurrent_reset_and_ordinary_publication_publish_one_distinguishable_winner() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let reset_identity = identity();
    let reset_request = request(60, 'e');
    let reset_result = reset_result(&reset_identity);
    let reset_now = 70;
    let ordinary_identity = alternate_identity();
    let ordinary_now = 71;
    let publication_boundary = Arc::new(Barrier::new(3));

    let (reset, ordinary) = std::thread::scope(|scope| {
        let reset = scope.spawn(|| {
            seed(
                &path,
                reset_identity.clone(),
                options(Some(
                    StoreFailpointV1::ResetAfterSeedCommitBeforePublication,
                ))
                .with_failpoint_action(StoreFailpointActionV1::Barrier(
                    Arc::clone(&publication_boundary),
                )),
                reset_request.clone(),
                reset_result.clone(),
                reset_now,
            )
        });
        let ordinary = scope.spawn(|| {
            SqliteStoreV1::open(
                &path,
                &root(),
                ordinary_identity.clone(),
                options(Some(
                    StoreFailpointV1::SchemaAfterInitializationBeforePublication,
                ))
                .with_failpoint_action(StoreFailpointActionV1::Barrier(
                    Arc::clone(&publication_boundary),
                )),
                UnixMillis::new(ordinary_now),
            )
        });
        publication_boundary.wait();
        (reset.join().unwrap(), ordinary.join().unwrap())
    });

    let expected_loser_error = StoreErrorV1::StorageIntegrityV1 {
        check: StoreIntegrityCheckV1::WorkspaceIdentity,
    };
    match (reset, ordinary) {
        (Ok(receipt), Err(error)) => {
            assert_eq!(error, expected_loser_error);
            assert_eq!(receipt, reset_receipt(&reset_request, reset_result.clone()));
            let before_replay = assert_exact_published_reset_target(
                &path,
                &reset_identity,
                &reset_request,
                &reset_result,
                reset_now,
            );
            let replay = seed(
                &path,
                reset_identity,
                options(None),
                reset_request.clone(),
                reset_result.clone(),
                reset_now + 1,
            )
            .unwrap();
            assert_eq!(replay, reset_receipt(&reset_request, reset_result));
            assert_reset_target_unchanged(snapshot_reset_target(&path), &before_replay);
        }
        (Err(error), Ok(store)) => {
            assert_eq!(error, expected_loser_error);
            store.close_for_maintenance().unwrap();
            assert_exact_initialized_destination(&path, &ordinary_identity, ordinary_now);
            let before_replay = snapshot_database_files(&path);
            SqliteStoreV1::open(
                &path,
                &root(),
                ordinary_identity.clone(),
                options(None),
                UnixMillis::new(ordinary_now),
            )
            .unwrap()
            .close_for_maintenance()
            .unwrap();
            assert_database_files_unchanged(snapshot_database_files(&path), &before_replay);
            assert_exact_initialized_destination(&path, &ordinary_identity, ordinary_now);
        }
        _ => panic!("exactly one reset-or-ordinary publication contender must publish"),
    }
}

#[test]
fn generic_terminal_commit_cannot_succeed_workspace_reset_in_old_database() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let workspace = identity();
    let store = SqliteStoreV1::open(
        &path,
        &root(),
        workspace.clone(),
        options(None),
        UnixMillis::new(1),
    )
    .unwrap();
    let reset = request(50, 'f');
    assert!(matches!(
        store.admit(&workspace, reset),
        Err(StoreErrorV1::InternalInvariantViolationV1 { .. })
    ));
    let view = store.read_workspace_view(&workspace).unwrap();
    assert_eq!(view.queued_job_count(), 0);
    assert_eq!(view.running_job_id(), None);
}
