//! Phase-2 Store-owned crash boundaries: abrupt child death before recovery assertions.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

#[cfg(unix)]
use std::os::unix::{
    fs::{MetadataExt, PermissionsExt},
    process::ExitStatusExt,
};

use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use podway_core::{
    AttemptId, CanonicalProcedureJsonV1, CanonicalProcedureSnapshotInputV1, DomainCommand,
    DomainError, DomainResult, JobId, ProcedureSnapshotId, ProcedureSnapshotV1,
    ProcedureSourceLabelV1, Revision, SessionAggregateV1, SessionId, Sha256Digest, UnixMillis,
    WorkspaceId,
};
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, ClaimedJobV1, DurableWorktreeIdentityV1, IdempotencyKeyV1,
    JobReceiptOrTerminalV1, JobReceiptV1, PHASE2_CRASH_BOUNDARY_REGISTRY_V1,
    PersistedSessionMutationV1, PersistedTerminalJobStateV1, PersistedTerminalReceiptV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StateTransitionV1,
    StoreContractV1, StoreCrashBoundaryDurabilityV1, StoreCrashBoundaryV1, StoreErrorV1,
    StoreFailpointActionV1, StoreFailpointV1, TerminalReceiptV1, TerminalResultV1,
    ValidatedWorkspaceRootV1, WorkerIdV1,
    codec::{
        PersistedDomainErrorV1, PersistedDomainResultV1, PersistedSessionLifecycleV1,
        PersistedTerminalResultV1,
    },
};

const CHILD_TEST_NAME: &str = "phase2_crash_child_aborts_at_configured_failpoint";
const CRASH_CASE_ENV: &str = "PODWAY_PHASE2_CRASH_CASE";
const PRUNE_CALLER_ENV: &str = "PODWAY_PHASE2_PRUNE_CALLER";
const PUBLICATION_CALLER_ENV: &str = "PODWAY_PHASE2_PUBLICATION_CALLER";
const DATABASE_PATH_ENV: &str = "PODWAY_PHASE2_CRASH_DATABASE_PATH";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CrashScenario {
    Admission,
    Claim,
    TerminalPreCommit,
    TerminalRelationalPreCommit,
    TerminalRelationalPostCommit,
    RecoveryPostCommit,
    Prune,
    Schema,
    Publication,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CrashCase {
    id: &'static str,
    failpoint: StoreFailpointV1,
    scenario: CrashScenario,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExpectedStoreCrashBoundary {
    id: &'static str,
    failpoints: &'static [StoreFailpointV1],
    durability: StoreCrashBoundaryDurabilityV1,
    recovery_invariant: &'static str,
    requirements: &'static [&'static str],
}

const EXPECTED_STORE_CRASH_BOUNDARIES: &[ExpectedStoreCrashBoundary] = &[
    ExpectedStoreCrashBoundary {
        id: "C01",
        failpoints: &[StoreFailpointV1::AdmissionBeforeTransaction],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "no admission rows exist",
        requirements: &["STO-001", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C02",
        failpoints: &[StoreFailpointV1::AdmissionAfterDurableRowsBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "no job or idempotency record exists",
        requirements: &["STO-001", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C03",
        failpoints: &[StoreFailpointV1::AdmissionAfterCommit],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "one queued job replays by idempotency",
        requirements: &["STO-001", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C04",
        failpoints: &[StoreFailpointV1::ClaimAfterCommit],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "one running job is requeued once on restart",
        requirements: &["STO-002", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C07",
        failpoints: &[StoreFailpointV1::TerminalAfterTransactionBegin],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "claimed job remains recoverable",
        requirements: &["STO-002", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C08",
        failpoints: &[StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "relational state and job terminal receipt roll back together",
        requirements: &["STO-002", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C09",
        failpoints: &[StoreFailpointV1::TerminalAfterJobTerminalUpdateBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "job and idempotency terminal updates roll back together",
        requirements: &["STO-002", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C10",
        failpoints: &[
            StoreFailpointV1::TerminalAfterCommitBeforeResponse,
            StoreFailpointV1::RecoveryAfterCommitBeforeReturn,
        ],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "one committed outcome is replayable after a lost response",
        requirements: &["STO-002", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C11",
        failpoints: &[StoreFailpointV1::TerminalFailureBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "failed receipt commits once on retry without domain mutation",
        requirements: &["STO-002", "STO-003"],
    },
    ExpectedStoreCrashBoundary {
        id: "C12",
        failpoints: &[StoreFailpointV1::PruneAfterDeleteStagingBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "prune deletes roll back with the caller transaction",
        requirements: &["STO-003", "STO-009"],
    },
    ExpectedStoreCrashBoundary {
        id: "C13",
        failpoints: &[StoreFailpointV1::SchemaBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "migration either commits once or leaves no partial schema",
        requirements: &["STO-007"],
    },
    ExpectedStoreCrashBoundary {
        id: "P01",
        failpoints: &[StoreFailpointV1::PublicationAfterDestinationLinkBeforeTemporaryUnlink],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "durable destination wins and the matching Store temporary hard link is removed",
        requirements: &["STO-007", "STO-008"],
    },
];
const EXCLUDED_DAEMON_PREPARATION_IDS: &[&str] = &["C05", "C06"];

type CrashRegistryTuple = (
    &'static str,
    Vec<StoreFailpointV1>,
    StoreCrashBoundaryDurabilityV1,
    &'static str,
    Vec<&'static str>,
);

fn crash_registry_tuple(boundary: &StoreCrashBoundaryV1) -> CrashRegistryTuple {
    (
        boundary.id(),
        boundary.failpoints().to_vec(),
        boundary.durability(),
        boundary.recovery_invariant(),
        boundary.requirements().to_vec(),
    )
}

fn expected_phase2_crash_registry() -> Vec<ExpectedStoreCrashBoundary> {
    let mut expected = EXPECTED_STORE_CRASH_BOUNDARIES.to_vec();
    expected.insert(
        4,
        ExpectedStoreCrashBoundary {
            id: "C05",
            failpoints: &[],
            durability: StoreCrashBoundaryDurabilityV1::DaemonPreparation,
            recovery_invariant: "procedure preparation is daemon-owned",
            requirements: &[],
        },
    );
    expected.insert(
        5,
        ExpectedStoreCrashBoundary {
            id: "C06",
            failpoints: &[],
            durability: StoreCrashBoundaryDurabilityV1::DaemonPreparation,
            recovery_invariant: "artifact hashing is daemon-owned",
            requirements: &[],
        },
    );
    expected
}

fn expected_phase2_crash_registry_tuples() -> Vec<CrashRegistryTuple> {
    expected_phase2_crash_registry()
        .into_iter()
        .map(|boundary| {
            (
                boundary.id,
                boundary.failpoints.to_vec(),
                boundary.durability,
                boundary.recovery_invariant,
                boundary.requirements.to_vec(),
            )
        })
        .collect()
}

fn registered_store_crash_boundaries() -> Vec<ExpectedStoreCrashBoundary> {
    PHASE2_CRASH_BOUNDARY_REGISTRY_V1
        .iter()
        .filter(|boundary| !EXCLUDED_DAEMON_PREPARATION_IDS.contains(&boundary.id()))
        .map(|boundary| ExpectedStoreCrashBoundary {
            id: boundary.id(),
            failpoints: boundary.failpoints(),
            durability: boundary.durability(),
            recovery_invariant: boundary.recovery_invariant(),
            requirements: boundary.requirements(),
        })
        .collect()
}

fn expected_store_failpoint_coverage() -> Vec<(&'static str, StoreFailpointV1)> {
    EXPECTED_STORE_CRASH_BOUNDARIES
        .iter()
        .flat_map(|boundary| {
            boundary
                .failpoints
                .iter()
                .copied()
                .map(move |failpoint| (boundary.id, failpoint))
        })
        .collect()
}

const STORE_CRASH_CASES: &[CrashCase] = &[
    CrashCase {
        id: "C01",
        failpoint: StoreFailpointV1::AdmissionBeforeTransaction,
        scenario: CrashScenario::Admission,
    },
    CrashCase {
        id: "C02",
        failpoint: StoreFailpointV1::AdmissionAfterDurableRowsBeforeCommit,
        scenario: CrashScenario::Admission,
    },
    CrashCase {
        id: "C03",
        failpoint: StoreFailpointV1::AdmissionAfterCommit,
        scenario: CrashScenario::Admission,
    },
    CrashCase {
        id: "C04",
        failpoint: StoreFailpointV1::ClaimAfterCommit,
        scenario: CrashScenario::Claim,
    },
    CrashCase {
        id: "C07",
        failpoint: StoreFailpointV1::TerminalAfterTransactionBegin,
        scenario: CrashScenario::TerminalPreCommit,
    },
    CrashCase {
        id: "C08",
        failpoint: StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        scenario: CrashScenario::TerminalRelationalPreCommit,
    },
    CrashCase {
        id: "C09",
        failpoint: StoreFailpointV1::TerminalAfterJobTerminalUpdateBeforeCommit,
        scenario: CrashScenario::TerminalPreCommit,
    },
    CrashCase {
        id: "C10",
        failpoint: StoreFailpointV1::TerminalAfterCommitBeforeResponse,
        scenario: CrashScenario::TerminalRelationalPostCommit,
    },
    CrashCase {
        id: "C10",
        failpoint: StoreFailpointV1::RecoveryAfterCommitBeforeReturn,
        scenario: CrashScenario::RecoveryPostCommit,
    },
    CrashCase {
        id: "C11",
        failpoint: StoreFailpointV1::TerminalFailureBeforeCommit,
        scenario: CrashScenario::TerminalPreCommit,
    },
    CrashCase {
        id: "C12",
        failpoint: StoreFailpointV1::PruneAfterDeleteStagingBeforeCommit,
        scenario: CrashScenario::Prune,
    },
    CrashCase {
        id: "C13",
        failpoint: StoreFailpointV1::SchemaBeforeCommit,
        scenario: CrashScenario::Schema,
    },
    CrashCase {
        id: "P01",
        failpoint: StoreFailpointV1::PublicationAfterDestinationLinkBeforeTemporaryUnlink,
        scenario: CrashScenario::Publication,
    },
];
const PRUNE_NOW: u64 = 8 * 24 * 60 * 60 * 1_000;
const DISTINCT_SCHEMA0_APPLICATION_ID: i64 = 0x504f_4457;

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000101").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase2-crash-matrix")).unwrap()
}

fn job(number: u8) -> JobId {
    JobId::new(format!("00000000-0000-4000-8000-{:012x}", number)).unwrap()
}

fn request_for_command(
    command: DomainCommand,
    job_id: JobId,
    key: &str,
    digest_nibble: char,
    now: u64,
) -> AdmitRequestV1 {
    let admitted_snapshot = matches!(
        command,
        DomainCommand::SessionStart | DomainCommand::SessionStartReplace
    )
    .then(|| aggregate().snapshot().clone());
    let request = AdmitRequestV1::new(
        command,
        IdempotencyKeyV1::new(key).unwrap(),
        job_id,
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest(digest_nibble),
        UnixMillis::new(now),
    );
    match admitted_snapshot {
        Some(snapshot) => request.with_admitted_procedure_snapshot(snapshot),
        None => request,
    }
}

fn crash_request(job_id: JobId, key: &str, digest_nibble: char, now: u64) -> AdmitRequestV1 {
    request_for_command(
        DomainCommand::WorkspaceInitialize,
        job_id,
        key,
        digest_nibble,
        now,
    )
}

fn case_request(case: CrashCase, number: u8) -> AdmitRequestV1 {
    let digest_nibble = match case.scenario {
        CrashScenario::Admission => 'c',
        CrashScenario::Claim | CrashScenario::RecoveryPostCommit => 'd',
        CrashScenario::TerminalPreCommit
        | CrashScenario::TerminalRelationalPreCommit
        | CrashScenario::TerminalRelationalPostCommit => 'e',
        CrashScenario::Prune | CrashScenario::Schema | CrashScenario::Publication => {
            unreachable!("case has no request")
        }
    };
    request_for_command(
        if matches!(
            case.scenario,
            CrashScenario::TerminalRelationalPreCommit
                | CrashScenario::TerminalRelationalPostCommit
        ) {
            DomainCommand::SessionStart
        } else {
            DomainCommand::WorkspaceInitialize
        },
        job(number),
        &format!("{}-{:?}", case.id, case.failpoint),
        digest_nibble,
        2,
    )
}

fn expected_job_receipt(number: u8, digest_nibble: char) -> JobReceiptV1 {
    JobReceiptV1::new(1, job(number), digest(digest_nibble))
}

fn expected_preconditions() -> RevisionAttemptItemPreconditionsV1 {
    RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap()
}

fn open(
    path: &Path,
    failpoint: Option<StoreFailpointV1>,
    now: u64,
) -> Result<SqliteStoreV1, StoreErrorV1> {
    SqliteStoreV1::open(
        path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(failpoint),
        UnixMillis::new(now),
    )
}

fn open_for_abort(
    path: &Path,
    failpoint: StoreFailpointV1,
    now: u64,
) -> Result<SqliteStoreV1, StoreErrorV1> {
    SqliteStoreV1::open(
        path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(Some(failpoint))
            .with_failpoint_action(StoreFailpointActionV1::AbortProcess),
        UnixMillis::new(now),
    )
}

fn durable_counts(path: &Path) -> (i64, i64, i64) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let jobs = connection
        .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
        .unwrap();
    let idempotency = connection
        .query_row("SELECT COUNT(*) FROM idempotency_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    let journal = connection
        .query_row("SELECT COUNT(*) FROM operational_journal", [], |row| {
            row.get(0)
        })
        .unwrap();
    (jobs, idempotency, journal)
}

fn journal_event_count(path: &Path, event_name: &str) -> i64 {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection
        .query_row(
            "SELECT COUNT(*) FROM operational_journal WHERE event_name = ?1",
            [event_name],
            |row| row.get(0),
        )
        .unwrap()
}
fn journal_row_count(path: &Path) -> i64 {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection
        .query_row("SELECT COUNT(*) FROM operational_journal", [], |row| {
            row.get(0)
        })
        .unwrap()
}
fn prune_candidate_rows(path: &Path) -> Vec<(i64, String, String, String)> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT recorded_at_ms, level, event_name, summary \
             FROM operational_journal \
             WHERE event_name = 'phase2.prune-candidate' \
             ORDER BY recorded_at_ms ASC, journal_id ASC",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn expected_prune_candidate_rows(
    ordinals: impl IntoIterator<Item = i64>,
) -> Vec<(i64, String, String, String)> {
    ordinals
        .into_iter()
        .map(|ordinal| {
            (
                ordinal,
                "info".to_owned(),
                "phase2.prune-candidate".to_owned(),
                "phase2 crash matrix".to_owned(),
            )
        })
        .collect()
}

fn assert_prune_rollback(path: &Path) {
    assert_eq!(journal_event_count(path, "retention.pruned"), 0);
    assert_eq!(journal_row_count(path), 201);
    assert_eq!(
        prune_candidate_rows(path),
        expected_prune_candidate_rows(0_i64..201)
    );
}

fn assert_prune_recovery(path: &Path, retained_ordinals: impl IntoIterator<Item = i64>) {
    assert_eq!(
        prune_candidate_rows(path),
        expected_prune_candidate_rows(retained_ordinals)
    );
    assert_eq!(journal_event_count(path, "retention.pruned"), 1);
    assert_eq!(journal_row_count(path), 201);
}

fn preseed_distinguishable_schema0_database(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .pragma_update(None, "application_id", DISTINCT_SCHEMA0_APPLICATION_ID)
        .unwrap();
    drop(connection);
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn assert_distinguishable_schema0_after_abort(path: &Path) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let user_object_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let initialization_object_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema \
             WHERE name IN ('schema_migrations', 'workspace_state')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let application_id: i64 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .unwrap();

    assert_eq!(user_version, 0);
    assert_eq!(user_object_count, 0);
    assert_eq!(initialization_object_count, 0);
    assert_eq!(application_id, DISTINCT_SCHEMA0_APPLICATION_ID);
}

fn assert_schema_recovered_from_distinguishable_schema0(path: &Path) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let schema_migration_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    let workspace_state_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM workspace_state", [], |row| row.get(0))
        .unwrap();
    let application_id: i64 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .unwrap();

    assert_eq!(user_version, 1);
    assert_eq!(schema_migration_count, 1);
    assert_eq!(workspace_state_count, 1);
    assert_eq!(application_id, DISTINCT_SCHEMA0_APPLICATION_ID);
}
#[cfg(unix)]
fn linked_publication_temporaries(path: &Path) -> Vec<PathBuf> {
    let destination_metadata = std::fs::symlink_metadata(path).unwrap();
    let parent = path.parent().unwrap();
    std::fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| candidate.as_path() != path)
        .filter(|candidate| {
            let metadata = std::fs::symlink_metadata(candidate).unwrap();
            metadata.file_type().is_file()
                && metadata.dev() == destination_metadata.dev()
                && metadata.ino() == destination_metadata.ino()
        })
        .collect()
}

#[cfg(unix)]
fn assert_raw_interrupted_publication(path: &Path) {
    let destination_metadata = std::fs::symlink_metadata(path).unwrap();
    assert!(destination_metadata.file_type().is_file());
    assert!(!destination_metadata.file_type().is_symlink());
    assert_eq!(destination_metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        destination_metadata.nlink(),
        2,
        "the destination link must survive abort after its parent-directory sync"
    );

    let temporary_links = linked_publication_temporaries(path);
    assert_eq!(
        temporary_links.len(),
        1,
        "exactly one Store temporary link must pair with the published destination"
    );
    let temporary = &temporary_links[0];
    let temporary_name = temporary.file_name().unwrap().to_str().unwrap();
    let destination_name = path.file_name().unwrap().to_str().unwrap();
    assert!(temporary_name.starts_with(&format!(".{destination_name}.")));
    assert!(temporary_name.ends_with(".tmp"));

    let temporary_metadata = std::fs::symlink_metadata(temporary).unwrap();
    assert!(temporary_metadata.file_type().is_file());
    assert!(!temporary_metadata.file_type().is_symlink());
    assert_eq!(temporary_metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(temporary_metadata.nlink(), 2);
    assert_eq!(temporary_metadata.dev(), destination_metadata.dev());
    assert_eq!(temporary_metadata.ino(), destination_metadata.ino());
}

#[cfg(unix)]
fn assert_interrupted_publication_recovered(path: &Path) {
    let destination_metadata = std::fs::symlink_metadata(path).unwrap();
    assert_eq!(
        destination_metadata.nlink(),
        1,
        "recovery must leave exactly one destination link"
    );
    assert!(
        linked_publication_temporaries(path).is_empty(),
        "recovery must remove the Store-owned temporary link"
    );
}

fn assert_raw_initialized_publication(path: &Path) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let migration_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    let workspace: (String, String, String, String, i64) = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
             last_validated_root, next_workspace_sequence FROM workspace_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(user_version, 1);
    assert_eq!(migration_count, 1);
    assert_eq!(
        workspace,
        (
            identity().workspace_uuid().as_str().to_owned(),
            identity().common_dir_identity().as_str().to_owned(),
            identity().worktree_admin_identity().as_str().to_owned(),
            root().as_encoded().to_owned(),
            0,
        )
    );
    assert_eq!(durable_counts(path), (0, 0, 0));
}

fn publication_reset_request() -> AdmitRequestV1 {
    request_for_command(
        DomainCommand::WorkspaceResetAll,
        job(94),
        "c14-publication-reset",
        'a',
        2,
    )
}

fn publication_reset_result() -> DomainResult {
    DomainResult::WorkspaceReset {
        workspace_id: identity().workspace_uuid().clone(),
        revision: Revision::ZERO,
    }
}

fn expected_publication_reset_receipt() -> TerminalReceiptV1 {
    TerminalReceiptV1::new(
        expected_job_receipt(94, 'a'),
        TerminalResultV1::Success(publication_reset_result()),
    )
}

fn assert_raw_reset_publication(path: &Path) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let workspace_sequence: i64 = connection
        .query_row(
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let job_row: (String, i64, String, String, String) = connection
        .query_row(
            "SELECT job_id, workspace_sequence, request_digest, command_name, state FROM jobs",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    let idempotency_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM idempotency_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    let seeded_journal_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM operational_journal WHERE event_name = 'workspace.reset_all.seeded'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(user_version, 1);
    assert_eq!(workspace_sequence, 1);
    assert_eq!(
        job_row,
        (
            job(94).as_str().to_owned(),
            1,
            digest('a').as_str().to_owned(),
            "workspace.reset_all".to_owned(),
            "succeeded".to_owned(),
        )
    );
    assert_eq!(idempotency_count, 1);
    assert_eq!(seeded_journal_count, 1);
}

fn job_state(path: &Path) -> String {
    let connection = Connection::open(path).unwrap();
    connection
        .query_row("SELECT state FROM jobs", [], |row| row.get(0))
        .unwrap()
}

fn assert_job_receipt(receipt: &JobReceiptV1, number: u8, digest_nibble: char) {
    assert_eq!(receipt.identity_sequence(), 1);
    assert_eq!(receipt.job_id(), &job(number));
    assert_eq!(receipt.request_digest(), &digest(digest_nibble));
    assert_eq!(receipt, &expected_job_receipt(number, digest_nibble));
}

fn assert_claimed_job_for_command(
    claimed: &ClaimedJobV1,
    number: u8,
    digest_nibble: char,
    command: &DomainCommand,
) {
    assert_job_receipt(claimed.job(), number, digest_nibble);
    assert_eq!(claimed.execution().command(), command);
    assert_eq!(
        claimed.execution().preconditions(),
        &expected_preconditions()
    );
    assert_eq!(claimed.current_session(), None);
}

fn assert_claimed_job(claimed: &ClaimedJobV1, number: u8, digest_nibble: char) {
    assert_claimed_job_for_command(
        claimed,
        number,
        digest_nibble,
        &DomainCommand::WorkspaceInitialize,
    );
}

fn assert_new_admission(
    store: &SqliteStoreV1,
    request: AdmitRequestV1,
    number: u8,
    digest_nibble: char,
) {
    match store.admit(&identity(), request).unwrap() {
        AdmitOutcomeV1::New(receipt) => assert_job_receipt(&receipt, number, digest_nibble),
        outcome => panic!("expected a new admission, got {outcome:?}"),
    }
}

fn assert_job_replay(
    store: &SqliteStoreV1,
    request: AdmitRequestV1,
    number: u8,
    digest_nibble: char,
) {
    match store.admit(&identity(), request).unwrap() {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt)) => {
            assert_job_receipt(&receipt, number, digest_nibble);
        }
        outcome => panic!("expected a non-terminal idempotency replay, got {outcome:?}"),
    }
}

fn terminal_replay(store: &SqliteStoreV1, request: AdmitRequestV1) -> PersistedTerminalReceiptV1 {
    match store.admit(&identity(), request).unwrap() {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => receipt,
        outcome => panic!("expected terminal idempotency replay, got {outcome:?}"),
    }
}

fn failure_result_for_crash() -> TerminalResultV1 {
    TerminalResultV1::Failure(DomainError::InvalidState {
        reason: "phase2-crash-matrix",
    })
}

fn expected_failure_result() -> TerminalResultV1 {
    TerminalResultV1::Failure(DomainError::InvalidState {
        reason: "phase2-crash-matrix",
    })
}

fn expected_persisted_failure(number: u8, digest_nibble: char) -> PersistedTerminalReceiptV1 {
    PersistedTerminalReceiptV1::new(
        expected_job_receipt(number, digest_nibble),
        PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::InvalidState {
            reason: "phase2-crash-matrix".to_owned(),
        }),
    )
}

fn assert_terminal_job_projection(
    replay: &PersistedTerminalReceiptV1,
    expected_state: PersistedTerminalJobStateV1,
) {
    let projection = replay
        .job_projection()
        .expect("SQLite terminal replay must preserve immutable job facts");
    assert_eq!(projection.state(), expected_state);
    assert!(projection.submitted_at() <= projection.finished_at());
    if let Some(claimed_at) = projection.claimed_at() {
        assert!(projection.submitted_at() <= claimed_at);
        assert!(claimed_at <= projection.finished_at());
    }
}

fn assert_failure_terminal_receipt(receipt: &TerminalReceiptV1, number: u8, digest_nibble: char) {
    let expected = TerminalReceiptV1::new(
        expected_job_receipt(number, digest_nibble),
        expected_failure_result(),
    );
    assert_job_receipt(receipt.job(), number, digest_nibble);
    assert_eq!(receipt.result(), &expected_failure_result());
    assert_eq!(receipt, &expected);
}

fn assert_failure_terminal_replay(
    replay: &PersistedTerminalReceiptV1,
    number: u8,
    digest_nibble: char,
) {
    let expected = expected_persisted_failure(number, digest_nibble);
    assert_job_receipt(replay.job(), number, digest_nibble);
    assert_eq!(replay.job(), expected.job());
    assert_eq!(replay.result(), expected.result());
    assert_terminal_job_projection(replay, PersistedTerminalJobStateV1::Failed);
    assert!(replay.session_projection().is_none());
}

fn session_id() -> SessionId {
    SessionId::new("00000000-0000-4000-8000-000000000121").unwrap()
}

fn aggregate() -> SessionAggregateV1 {
    let authored = serde_json::json!({
        "schema": "podway.procedure/v1",
        "id": "phase2-crash-matrix",
        "version": "1",
        "name": "Phase-2 crash matrix",
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
    });
    let canonical = podway_core::canonicalize_json_v1(&authored).unwrap();
    let snapshot = ProcedureSnapshotV1::from_canonical_json(CanonicalProcedureSnapshotInputV1 {
        snapshot_id: ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000120").unwrap(),
        schema_id: "podway.procedure/v1".to_owned(),
        procedure_id: "phase2-crash-matrix".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Phase-2 crash matrix".to_owned(),
        source_label: ProcedureSourceLabelV1::file("phase2-crash-matrix").unwrap(),
        canonical_json: CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
        digest: Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
            .unwrap(),
        created_at: UnixMillis::new(1),
    })
    .unwrap();
    SessionAggregateV1::start(
        session_id(),
        "Phase-2 crash matrix",
        snapshot,
        AttemptId::new("00000000-0000-4000-8000-000000000122").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap()
}

fn relational_terminal_for_commit() -> (StateTransitionV1, TerminalResultV1) {
    let aggregate = aggregate();
    let transition = StateTransitionV1::new_persisted(
        Some(aggregate.session_id().clone()),
        Revision::ZERO,
        aggregate.revision(),
        PersistedSessionMutationV1::Replace(aggregate.clone()),
    )
    .unwrap();
    let result = TerminalResultV1::Success(DomainResult::SessionChanged {
        session_id: aggregate.session_id().clone(),
        revision_before: Revision::ZERO,
        revision_after: aggregate.revision(),
        changed: true,
    });
    (transition, result)
}

fn expected_persisted_relational_terminal(
    number: u8,
    digest_nibble: char,
) -> PersistedTerminalReceiptV1 {
    PersistedTerminalReceiptV1::new(
        expected_job_receipt(number, digest_nibble),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
            session_id: session_id(),
            revision_before: Revision::ZERO,
            revision_after: Revision::new(1),
            changed: true,
        }),
    )
}

fn assert_relational_terminal_receipt(
    receipt: &TerminalReceiptV1,
    number: u8,
    digest_nibble: char,
) {
    let expected = TerminalReceiptV1::new(
        expected_job_receipt(number, digest_nibble),
        TerminalResultV1::Success(DomainResult::SessionChanged {
            session_id: session_id(),
            revision_before: Revision::ZERO,
            revision_after: Revision::new(1),
            changed: true,
        }),
    );
    assert_job_receipt(receipt.job(), number, digest_nibble);
    assert_eq!(receipt, &expected);
}

fn assert_relational_terminal_replay(
    replay: &PersistedTerminalReceiptV1,
    number: u8,
    digest_nibble: char,
) {
    let expected = expected_persisted_relational_terminal(number, digest_nibble);
    assert_job_receipt(replay.job(), number, digest_nibble);
    assert_eq!(replay.job(), expected.job());
    assert_eq!(replay.result(), expected.result());
    assert_terminal_job_projection(replay, PersistedTerminalJobStateV1::Succeeded);
    let session = replay
        .session_projection()
        .expect("relational replay must preserve immutable session facts");
    assert_eq!(session.session_id(), &session_id());
    assert_eq!(session.task_title(), "Phase-2 crash matrix");
    assert_eq!(session.lifecycle(), PersistedSessionLifecycleV1::Running);
    assert_eq!(session.revision_before(), Revision::ZERO);
    assert_eq!(session.revision_after(), Revision::new(1));
}

fn assert_relational_state(store: &SqliteStoreV1) {
    let expected = aggregate();
    let actual = store
        .read_session_aggregate(&identity())
        .unwrap()
        .expect("relational terminal must persist the session");
    assert_eq!(actual.revision(), Revision::new(1));
    assert_eq!(actual, expected);
    assert_eq!(
        store
            .read_workspace_view(&identity())
            .unwrap()
            .state()
            .revision(),
        Revision::new(1)
    );
}

fn insert_prune_candidates(path: &Path) {
    let connection = Connection::open(path).unwrap();
    for ordinal in 0_i64..201 {
        connection
            .execute(
                "INSERT INTO operational_journal (recorded_at_ms, level, event_name, summary) \
                 VALUES (?1, 'info', 'phase2.prune-candidate', 'phase2 crash matrix')",
                params![ordinal],
            )
            .unwrap();
    }
}

fn assert_aborted(status: ExitStatus, label: &str) {
    assert!(
        !status.success(),
        "{label} child unexpectedly returned without crashing"
    );
    #[cfg(unix)]
    assert_eq!(
        status.signal(),
        Some(6),
        "{label} child must terminate with SIGABRT after reaching its failpoint"
    );
}

fn run_child(
    path: &Path,
    case_index: Option<usize>,
    prune_caller: Option<&str>,
    publication_caller: Option<&str>,
) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg(CHILD_TEST_NAME)
        .arg("--nocapture")
        .env(DATABASE_PATH_ENV, path.as_os_str());
    if let Some(case_index) = case_index {
        command.env(CRASH_CASE_ENV, case_index.to_string());
    }
    if let Some(prune_caller) = prune_caller {
        command.env(PRUNE_CALLER_ENV, prune_caller);
    }
    if let Some(publication_caller) = publication_caller {
        command.env(PUBLICATION_CALLER_ENV, publication_caller);
    }
    assert_aborted(command.output().unwrap().status, CHILD_TEST_NAME);
}

fn prepare_crash_case(path: &Path, case: CrashCase, number: u8) {
    match case.scenario {
        CrashScenario::Claim
        | CrashScenario::TerminalPreCommit
        | CrashScenario::TerminalRelationalPreCommit
        | CrashScenario::TerminalRelationalPostCommit => {
            let initial = open(path, None, 1).unwrap();
            let request = case_request(case, number);
            let digest_nibble = match case.scenario {
                CrashScenario::Claim => 'd',
                CrashScenario::TerminalPreCommit
                | CrashScenario::TerminalRelationalPreCommit
                | CrashScenario::TerminalRelationalPostCommit => 'e',
                _ => unreachable!("prepared case has an unexpected scenario"),
            };
            assert_new_admission(&initial, request, number, digest_nibble);
        }
        CrashScenario::RecoveryPostCommit => {
            let initial = open(path, None, 1).unwrap();
            let request = case_request(case, number);
            assert_new_admission(&initial, request, number, 'd');
            let claimed = initial
                .claim_next(
                    &identity(),
                    WorkerIdV1::new("recovery-preparation").unwrap(),
                    UnixMillis::new(3),
                )
                .unwrap()
                .expect("prepared queued job must be claimed");
            assert_claimed_job(&claimed, number, 'd');
        }
        CrashScenario::Schema => preseed_distinguishable_schema0_database(path),
        CrashScenario::Admission | CrashScenario::Prune | CrashScenario::Publication => {}
    }
}

fn assert_case_recovery(path: &Path, case: CrashCase, number: u8) {
    match case.scenario {
        CrashScenario::Admission => {
            let reopened = open(path, None, 3).unwrap();
            let request = case_request(case, number);
            match case.failpoint {
                StoreFailpointV1::AdmissionAfterCommit => {
                    assert_eq!(
                        reopened
                            .read_workspace_view(&identity())
                            .unwrap()
                            .queued_job_count(),
                        1
                    );
                    assert_job_replay(&reopened, request, number, 'c');
                }
                StoreFailpointV1::AdmissionBeforeTransaction
                | StoreFailpointV1::AdmissionAfterDurableRowsBeforeCommit => {
                    assert_eq!(
                        reopened
                            .read_workspace_view(&identity())
                            .unwrap()
                            .queued_job_count(),
                        0
                    );
                    assert_new_admission(&reopened, request, number, 'c');
                }
                _ => unreachable!("invalid admission failpoint for {}", case.id),
            }
            drop(reopened);
            assert_eq!(durable_counts(path), (1, 1, 0));
        }
        CrashScenario::Claim => {
            let reopened = open(path, None, 4).unwrap();
            assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
            assert_eq!(
                reopened
                    .read_workspace_view(&identity())
                    .unwrap()
                    .queued_job_count(),
                1
            );
            assert_job_replay(&reopened, case_request(case, number), number, 'd');
            let recovered = reopened
                .claim_next(
                    &identity(),
                    WorkerIdV1::new("recovered-claim").unwrap(),
                    UnixMillis::new(5),
                )
                .unwrap()
                .expect("recovered queued job must be claimable");
            assert_claimed_job(&recovered, number, 'd');
            drop(reopened);
            assert_eq!(durable_counts(path), (1, 1, 1));
        }
        CrashScenario::TerminalPreCommit | CrashScenario::TerminalRelationalPreCommit => {
            let relational = case.scenario == CrashScenario::TerminalRelationalPreCommit;
            let reopened = open(path, None, 5).unwrap();
            assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
            assert_eq!(
                reopened
                    .read_workspace_view(&identity())
                    .unwrap()
                    .queued_job_count(),
                1
            );
            assert_eq!(reopened.read_session_aggregate(&identity()).unwrap(), None);
            assert_eq!(
                reopened
                    .read_workspace_view(&identity())
                    .unwrap()
                    .state()
                    .revision(),
                Revision::ZERO
            );
            let recovered = reopened
                .claim_next(
                    &identity(),
                    WorkerIdV1::new("terminal-recovered").unwrap(),
                    UnixMillis::new(6),
                )
                .unwrap()
                .expect("recovered terminal job must be claimable");
            assert_claimed_job_for_command(
                &recovered,
                number,
                'e',
                if relational {
                    &DomainCommand::SessionStart
                } else {
                    &DomainCommand::WorkspaceInitialize
                },
            );
            if relational {
                let (transition, result) = relational_terminal_for_commit();
                let committed = reopened
                    .commit_terminal(
                        recovered.claim().clone(),
                        Revision::ZERO,
                        Some(transition),
                        result,
                        UnixMillis::new(7),
                    )
                    .unwrap();
                assert_relational_terminal_receipt(&committed, number, 'e');
                assert_relational_state(&reopened);
                assert_relational_terminal_replay(
                    &terminal_replay(&reopened, case_request(case, number)),
                    number,
                    'e',
                );
            } else {
                let committed = reopened
                    .commit_terminal(
                        recovered.claim().clone(),
                        Revision::ZERO,
                        None,
                        expected_failure_result(),
                        UnixMillis::new(7),
                    )
                    .unwrap();
                assert_failure_terminal_receipt(&committed, number, 'e');
                assert_failure_terminal_replay(
                    &terminal_replay(&reopened, case_request(case, number)),
                    number,
                    'e',
                );
            }
            drop(reopened);
            assert_eq!(durable_counts(path), (1, 1, 1));
        }
        CrashScenario::TerminalRelationalPostCommit => {
            let reopened = open(path, None, 5).unwrap();
            assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 0);
            assert_eq!(
                reopened
                    .read_workspace_view(&identity())
                    .unwrap()
                    .queued_job_count(),
                0
            );
            assert_relational_state(&reopened);
            assert_relational_terminal_replay(
                &terminal_replay(&reopened, case_request(case, number)),
                number,
                'e',
            );
            drop(reopened);
            assert_eq!(durable_counts(path), (1, 1, 0));
        }
        CrashScenario::RecoveryPostCommit => {
            let reopened = open(path, None, 5).unwrap();
            assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 0);
            assert_eq!(
                reopened
                    .read_workspace_view(&identity())
                    .unwrap()
                    .queued_job_count(),
                1
            );
            assert_job_replay(&reopened, case_request(case, number), number, 'd');
            let recovered = reopened
                .claim_next(
                    &identity(),
                    WorkerIdV1::new("recovery-after-commit").unwrap(),
                    UnixMillis::new(6),
                )
                .unwrap()
                .expect("recovered queued job must be claimable");
            assert_claimed_job(&recovered, number, 'd');
            drop(reopened);
            assert_eq!(durable_counts(path), (1, 1, 1));
        }
        CrashScenario::Prune => {
            assert_eq!(durable_counts(path), (0, 0, 201));
            let recovered = open(path, None, PRUNE_NOW + 1).unwrap();
            assert_eq!(recovered.startup_recovery_report().requeued_job_count(), 0);
            drop(recovered);
            assert_eq!(durable_counts(path), (0, 0, 201));
            assert_prune_recovery(path, 1_i64..201);
        }
        CrashScenario::Schema => {
            let recovered = open(path, None, 2).unwrap();
            assert_eq!(
                recovered
                    .read_workspace_view(&identity())
                    .unwrap()
                    .queued_job_count(),
                0
            );
            assert_eq!(
                recovered
                    .read_workspace_view(&identity())
                    .unwrap()
                    .state()
                    .revision(),
                Revision::ZERO
            );
            drop(recovered);
            assert_eq!(durable_counts(path), (0, 0, 0));
            assert_schema_recovered_from_distinguishable_schema0(path);
        }
        CrashScenario::Publication => {
            let recovered = open(path, None, 2).unwrap();
            assert_eq!(recovered.startup_recovery_report().requeued_job_count(), 0);
            assert_eq!(
                recovered
                    .read_workspace_view(&identity())
                    .unwrap()
                    .queued_job_count(),
                0
            );
            assert_eq!(
                recovered
                    .read_workspace_view(&identity())
                    .unwrap()
                    .state()
                    .revision(),
                Revision::ZERO
            );
            drop(recovered);
            assert_eq!(durable_counts(path), (0, 0, 0));
            #[cfg(unix)]
            assert_interrupted_publication_recovered(path);
        }
    }
}

fn child_crash_case(case: CrashCase, path: &Path, number: u8) -> ! {
    match case.scenario {
        CrashScenario::Admission => {
            let failing = open_for_abort(path, case.failpoint, 1).unwrap();
            let _ = failing.admit(&identity(), case_request(case, number));
        }
        CrashScenario::Claim => {
            let failing = open_for_abort(path, case.failpoint, 3).unwrap();
            let _ = failing.claim_next(
                &identity(),
                WorkerIdV1::new("lost-claim").unwrap(),
                UnixMillis::new(3),
            );
        }
        CrashScenario::TerminalPreCommit
        | CrashScenario::TerminalRelationalPreCommit
        | CrashScenario::TerminalRelationalPostCommit => {
            let failing = open_for_abort(path, case.failpoint, 3).unwrap();
            let claimed = failing
                .claim_next(
                    &identity(),
                    WorkerIdV1::new("terminal-first").unwrap(),
                    UnixMillis::new(3),
                )
                .unwrap()
                .expect("prepared terminal job must be claimable");
            assert_claimed_job_for_command(
                &claimed,
                number,
                'e',
                if matches!(
                    case.scenario,
                    CrashScenario::TerminalRelationalPreCommit
                        | CrashScenario::TerminalRelationalPostCommit
                ) {
                    &DomainCommand::SessionStart
                } else {
                    &DomainCommand::WorkspaceInitialize
                },
            );
            if matches!(
                case.scenario,
                CrashScenario::TerminalRelationalPreCommit
                    | CrashScenario::TerminalRelationalPostCommit
            ) {
                let (transition, result) = relational_terminal_for_commit();
                let _ = failing.commit_terminal(
                    claimed.claim().clone(),
                    Revision::ZERO,
                    Some(transition),
                    result,
                    UnixMillis::new(4),
                );
            } else {
                let _ = failing.commit_terminal(
                    claimed.claim().clone(),
                    Revision::ZERO,
                    None,
                    failure_result_for_crash(),
                    UnixMillis::new(4),
                );
            }
        }
        CrashScenario::RecoveryPostCommit => {
            let _ = open_for_abort(path, case.failpoint, 4);
        }
        CrashScenario::Prune => {
            let failing = open_for_abort(path, case.failpoint, 1).unwrap();
            insert_prune_candidates(path);
            let _ = failing.prune_terminal_history(&identity(), UnixMillis::new(PRUNE_NOW));
        }
        CrashScenario::Schema => {
            let _ = open_for_abort(path, case.failpoint, 1);
        }
        CrashScenario::Publication => {
            let _ = open_for_abort(path, case.failpoint, 1);
        }
    }
    panic!("configured Store failpoint returned instead of aborting");
}

#[test]
fn phase2_crash_child_aborts_at_configured_failpoint() {
    let Some(path) = std::env::var_os(DATABASE_PATH_ENV).map(PathBuf::from) else {
        return;
    };
    if let Ok(case_index) = std::env::var(CRASH_CASE_ENV) {
        let case_index = case_index
            .parse::<usize>()
            .expect("crash case index must be an unsigned integer");
        let case = *STORE_CRASH_CASES
            .get(case_index)
            .expect("crash case index must identify a registered case");
        child_crash_case(case, &path, case_index as u8 + 1);
    }
    if let Ok(caller) = std::env::var(PRUNE_CALLER_ENV) {
        child_prune_caller(&caller, &path);
    }
    if let Ok(caller) = std::env::var(PUBLICATION_CALLER_ENV) {
        child_publication_caller(&caller, &path);
    }
}

#[test]
fn crash_registry_has_exact_unique_store_owned_failpoint_coverage() {
    let registered_ids: Vec<_> = PHASE2_CRASH_BOUNDARY_REGISTRY_V1
        .iter()
        .map(|boundary| boundary.id())
        .collect();
    let unique_ids: BTreeSet<_> = registered_ids.iter().copied().collect();
    assert_eq!(
        unique_ids.len(),
        registered_ids.len(),
        "Phase-2 crash registry IDs must be unique"
    );
    assert_eq!(
        registered_ids,
        expected_phase2_crash_registry()
            .iter()
            .map(|boundary| boundary.id)
            .collect::<Vec<_>>(),
        "Phase-2 crash registry IDs must exactly match the independent contract"
    );

    let daemon_preparation_ids: Vec<_> = PHASE2_CRASH_BOUNDARY_REGISTRY_V1
        .iter()
        .filter(|boundary| {
            boundary.durability() == StoreCrashBoundaryDurabilityV1::DaemonPreparation
        })
        .map(|boundary| boundary.id())
        .collect();
    assert_eq!(
        daemon_preparation_ids, EXCLUDED_DAEMON_PREPARATION_IDS,
        "only C05 and C06 may be excluded as daemon preparation boundaries"
    );
    assert_eq!(
        PHASE2_CRASH_BOUNDARY_REGISTRY_V1
            .iter()
            .map(crash_registry_tuple)
            .collect::<Vec<_>>(),
        expected_phase2_crash_registry_tuples(),
        "Phase-2 crash registry tuples must exactly match the independent contract"
    );
    assert_eq!(
        registered_store_crash_boundaries(),
        EXPECTED_STORE_CRASH_BOUNDARIES,
        "Store crash registry must exactly match the independently defined boundary contract"
    );

    let case_pairs: Vec<_> = STORE_CRASH_CASES
        .iter()
        .map(|case| (case.id, case.failpoint))
        .collect();
    assert_eq!(
        case_pairs,
        expected_store_failpoint_coverage(),
        "every defined Store failpoint must be exercised exactly once, with no extras"
    );
}

#[test]
fn store_owned_crash_matrix_aborts_children_then_recovers_exactly_once() {
    for (index, case) in STORE_CRASH_CASES.iter().enumerate() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("state.sqlite3");
        let number = (index + 1) as u8;
        prepare_crash_case(&path, *case, number);
        run_child(&path, Some(index), None, None);
        if case.scenario == CrashScenario::Prune {
            assert_prune_rollback(&path);
        }
        if case.scenario == CrashScenario::Schema {
            assert_distinguishable_schema0_after_abort(&path);
        }
        if case.scenario == CrashScenario::Publication {
            #[cfg(unix)]
            assert_raw_interrupted_publication(&path);
            assert_raw_initialized_publication(&path);
        }
        assert_case_recovery(&path, *case, number);
    }
}

fn child_prune_caller(caller: &str, path: &Path) -> ! {
    match caller {
        "cancel" => {
            let failing = open_for_abort(
                path,
                StoreFailpointV1::PruneAfterDeleteStagingBeforeCommit,
                1,
            )
            .unwrap();
            insert_prune_candidates(path);
            let _ = failing.cancel_before_claim(
                &identity(),
                job(90),
                Revision::new(1),
                UnixMillis::new(PRUNE_NOW),
            );
        }
        "terminal" => {
            let failing = open_for_abort(
                path,
                StoreFailpointV1::PruneAfterDeleteStagingBeforeCommit,
                1,
            )
            .unwrap();
            let claimed = failing
                .claim_next(
                    &identity(),
                    WorkerIdV1::new("c12-terminal").unwrap(),
                    UnixMillis::new(3),
                )
                .unwrap()
                .expect("prepared C12 terminal job must be claimable");
            assert_claimed_job(&claimed, 91, 'b');
            insert_prune_candidates(path);
            let _ = failing.commit_terminal(
                claimed.claim().clone(),
                Revision::ZERO,
                None,
                failure_result_for_crash(),
                UnixMillis::new(PRUNE_NOW),
            );
        }
        "recovery" => {
            let _ = open_for_abort(
                path,
                StoreFailpointV1::PruneAfterDeleteStagingBeforeCommit,
                PRUNE_NOW,
            );
        }
        _ => panic!("unknown prune caller {caller}"),
    }
    panic!("configured Store failpoint returned instead of aborting");
}
fn child_publication_caller(caller: &str, path: &Path) -> ! {
    let result = match caller {
        "reset" => SqliteStoreV1::seed_or_verify_reset_target(
            path,
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8)
                .unwrap()
                .with_failpoint(Some(
                    StoreFailpointV1::PublicationAfterDestinationLinkBeforeTemporaryUnlink,
                ))
                .with_failpoint_action(StoreFailpointActionV1::AbortProcess),
            publication_reset_request(),
            publication_reset_result(),
            UnixMillis::new(2),
        ),
        _ => panic!("unknown publication caller {caller}"),
    };
    panic!("configured Store failpoint returned instead of aborting: {result:?}");
}

#[cfg(unix)]
#[test]
fn publication_failpoint_recovers_interrupted_links_for_reset_publication() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");

    run_child(&path, None, None, Some("reset"));
    assert_raw_interrupted_publication(&path);
    assert_raw_reset_publication(&path);

    let replay = SqliteStoreV1::seed_or_verify_reset_target(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        publication_reset_request(),
        publication_reset_result(),
        UnixMillis::new(2),
    )
    .unwrap();
    assert_eq!(replay, expected_publication_reset_receipt());
    assert_interrupted_publication_recovered(&path);

    let repeated = SqliteStoreV1::seed_or_verify_reset_target(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        publication_reset_request(),
        publication_reset_result(),
        UnixMillis::new(3),
    )
    .unwrap();
    assert_eq!(repeated, expected_publication_reset_receipt());
}

#[test]
fn pruning_failpoint_aborts_child_for_cancel_terminal_and_recovery_callers() {
    prune_cancel_crash_recovers();
    prune_terminal_crash_recovers();
    prune_recovery_crash_recovers();
}

fn prune_cancel_crash_recovers() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    {
        let initial = open(&path, None, 1).unwrap();
        assert_new_admission(
            &initial,
            crash_request(job(90), "c12-cancel", 'a', 2),
            90,
            'a',
        );
    }
    run_child(&path, None, Some("cancel"), None);
    assert_prune_rollback(&path);
    assert_eq!(job_state(&path), "queued");
    assert_eq!(durable_counts(&path), (1, 1, 201));

    {
        let recovered = open(&path, None, 3).unwrap();
        assert_eq!(recovered.startup_recovery_report().requeued_job_count(), 0);
        assert_job_replay(
            &recovered,
            crash_request(job(90), "c12-cancel", 'a', 2),
            90,
            'a',
        );
        match recovered
            .cancel_before_claim(
                &identity(),
                job(90),
                Revision::new(1),
                UnixMillis::new(PRUNE_NOW + 1),
            )
            .unwrap()
        {
            podway_store::CancelOutcomeV1::Cancelled(receipt) => {
                assert_job_receipt(&receipt, 90, 'a');
            }
            outcome => panic!("expected a successful cancellation retry, got {outcome:?}"),
        }
    }
    assert_eq!(durable_counts(&path), (1, 1, 201));
    assert_prune_recovery(&path, 1_i64..201);
}

fn prune_terminal_crash_recovers() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    {
        let initial = open(&path, None, 1).unwrap();
        assert_new_admission(
            &initial,
            crash_request(job(91), "c12-terminal", 'b', 2),
            91,
            'b',
        );
    }
    run_child(&path, None, Some("terminal"), None);
    assert_prune_rollback(&path);
    assert_eq!(job_state(&path), "running");
    assert_eq!(durable_counts(&path), (1, 1, 201));

    {
        let recovered = open(&path, None, PRUNE_NOW + 1).unwrap();
        assert_eq!(recovered.startup_recovery_report().requeued_job_count(), 1);
        assert_eq!(
            recovered
                .read_workspace_view(&identity())
                .unwrap()
                .queued_job_count(),
            1
        );
        assert_job_replay(
            &recovered,
            crash_request(job(91), "c12-terminal", 'b', 2),
            91,
            'b',
        );
    }
    assert_eq!(durable_counts(&path), (1, 1, 201));
    assert_prune_recovery(&path, 2_i64..201);
}

fn prune_recovery_crash_recovers() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    {
        let initial = open(&path, None, 1).unwrap();
        assert_new_admission(
            &initial,
            crash_request(job(92), "c12-recovery", 'c', 2),
            92,
            'c',
        );
        let claimed = initial
            .claim_next(
                &identity(),
                WorkerIdV1::new("c12-recovery").unwrap(),
                UnixMillis::new(3),
            )
            .unwrap()
            .expect("prepared C12 recovery job must be claimable");
        assert_claimed_job(&claimed, 92, 'c');
    }
    insert_prune_candidates(&path);
    run_child(&path, None, Some("recovery"), None);
    assert_prune_rollback(&path);
    assert_eq!(job_state(&path), "running");
    assert_eq!(durable_counts(&path), (1, 1, 201));

    {
        let recovered = open(&path, None, PRUNE_NOW + 1).unwrap();
        assert_eq!(recovered.startup_recovery_report().requeued_job_count(), 1);
        assert_eq!(
            recovered
                .read_workspace_view(&identity())
                .unwrap()
                .queued_job_count(),
            1
        );
        assert_job_replay(
            &recovered,
            crash_request(job(92), "c12-recovery", 'c', 2),
            92,
            'c',
        );
    }
    assert_eq!(durable_counts(&path), (1, 1, 201));
    assert_prune_recovery(&path, 2_i64..201);
}
