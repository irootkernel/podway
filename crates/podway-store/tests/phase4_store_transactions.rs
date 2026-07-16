//! Transactional SQLite Store v1 contracts using direct store/domain values.

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;

use podway_core::{
    AttemptId, CancelSessionV1, CanonicalProcedureJsonV1, CanonicalProcedureSnapshotInputV1,
    CommandContextV1, DomainCommand, DomainError, DomainResult, ItemId, JobId, ProcedureSnapshotId,
    ProcedureSnapshotV1, ProcedureSourceLabelV1, Revision, SessionAggregateV1, SessionCommandV1,
    SessionId, SessionLifecycle, Sha256Digest, UnixMillis, WorkspaceId, apply_transition_v1,
};
use podway_store::codec::{
    PersistedDomainErrorV1, PersistedDomainResultV1, PersistedSessionLifecycleV1,
    PersistedTerminalResultV1, PersistedTerminalSessionProjectionV1,
    encode_persisted_terminal_receipt_v1,
};
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, CancelOutcomeV1, DurableWorktreeIdentityV1, IdempotencyKeyV1,
    JobListQueryV1, JobReceiptOrTerminalV1, JobReceiptV1, JobStateV1, PersistedSessionMutationV1,
    PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1, PersistedTerminalReceiptV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StateTransitionV1,
    StoreContractV1, StoreErrorV1, StoreFailpointV1, StoreIdempotencyReadContractV1,
    StoreIntegrityCheckV1, StoreInvariantV1, StoreReadContractV1, StoreRecordKindV1,
    StoreUnavailableReasonV1, TerminalReceiptV1, TerminalResultV1, ValidatedWorkspaceRootV1,
    WorkerIdV1,
};
use tempfile::TempDir;

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

fn job(number: u8) -> JobId {
    JobId::new(format!("00000000-0000-4000-8000-{:012x}", number)).unwrap()
}

fn store(temporary: &TempDir) -> SqliteStoreV1 {
    SqliteStoreV1::open(
        temporary.path().join("state.sqlite3"),
        &ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4")).unwrap(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(1),
    )
    .unwrap()
}
fn store_with_options(
    temporary: &TempDir,
    options: SqliteStoreOptionsV1,
    now: UnixMillis,
) -> SqliteStoreV1 {
    SqliteStoreV1::open(
        temporary.path().join("state.sqlite3"),
        &ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4")).unwrap(),
        identity(),
        options,
        now,
    )
    .unwrap()
}

fn contention_store_options() -> SqliteStoreOptionsV1 {
    SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_busy_timeout_ms(100)
        .unwrap()
}
fn database_path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}

fn read_optional_database_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn snapshot_database_artifacts(path: &Path) -> io::Result<Vec<Option<Vec<u8>>>> {
    ["", "-wal", "-shm"]
        .into_iter()
        .map(|suffix| read_optional_database_file(&database_path_with_suffix(path, suffix)))
        .collect()
}

fn workspace_directory_entries(path: &Path) -> io::Result<Vec<std::ffi::OsString>> {
    let mut entries = fs::read_dir(path.parent().expect("database path must have a parent"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    Ok(entries)
}

fn admit_for_command(
    store: &SqliteStoreV1,
    command: DomainCommand,
    job_id: JobId,
    key: &str,
    digest_nibble: char,
    now: u64,
) {
    let request = AdmitRequestV1::new(
        command,
        IdempotencyKeyV1::new(key).unwrap(),
        job_id,
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest(digest_nibble),
        UnixMillis::new(now),
    );
    assert!(matches!(
        store.admit(&identity(), request),
        Ok(AdmitOutcomeV1::New(_))
    ));
}

fn admit(store: &SqliteStoreV1, job_id: JobId, key: &str, digest_nibble: char, now: u64) {
    admit_for_command(
        store,
        DomainCommand::WorkspaceInitialize,
        job_id,
        key,
        digest_nibble,
        now,
    );
}

fn reset_seed_request(number: u8) -> AdmitRequestV1 {
    AdmitRequestV1::new(
        DomainCommand::WorkspaceResetAll,
        IdempotencyKeyV1::new(format!("seeded-reset-{number}")).unwrap(),
        job(number),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('f'),
        UnixMillis::new(10),
    )
}

fn reset_seed_result() -> DomainResult {
    DomainResult::WorkspaceReset {
        workspace_id: identity().workspace_uuid().clone(),
        revision: Revision::ZERO,
    }
}

fn seed_reset_target(
    path: &Path,
    request: AdmitRequestV1,
    result: DomainResult,
    now: u64,
) -> Result<TerminalReceiptV1, StoreErrorV1> {
    SqliteStoreV1::seed_or_verify_reset_target(
        path,
        &ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4")).unwrap(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        request,
        result,
        UnixMillis::new(now),
    )
}

fn overwrite_seeded_terminal(path: &Path, terminal: &PersistedTerminalReceiptV1) {
    let encoded = encode_persisted_terminal_receipt_v1(terminal).unwrap();
    let connection = Connection::open(path).unwrap();
    assert_eq!(
        connection
            .execute("UPDATE jobs SET terminal_response_json = ?1", [&encoded],)
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .execute(
                "UPDATE idempotency_records SET terminal_response_json = ?1",
                [&encoded],
            )
            .unwrap(),
        1
    );
}

fn seeded_reset_terminal_without_projection(
    request: &AdmitRequestV1,
) -> PersistedTerminalReceiptV1 {
    PersistedTerminalReceiptV1::new(
        JobReceiptV1::new(
            1,
            request.job_id().clone(),
            request.request_digest().clone(),
        ),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::WorkspaceReset {
            workspace_id: identity().workspace_uuid().clone(),
            revision: Revision::ZERO,
        }),
    )
}

#[test]
fn seeded_reset_target_rejects_a_missing_terminal_job_projection() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("reset-target.sqlite3");
    let request = reset_seed_request(240);
    let result = reset_seed_result();
    seed_reset_target(&path, request.clone(), result.clone(), 20).unwrap();
    overwrite_seeded_terminal(&path, &seeded_reset_terminal_without_projection(&request));

    assert!(matches!(
        seed_reset_target(&path, request, result, 21),
        Err(StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Job
        })
    ));
}

#[test]
fn seeded_reset_target_rejects_a_wrong_terminal_job_projection() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("reset-target.sqlite3");
    let request = reset_seed_request(241);
    let result = reset_seed_result();
    seed_reset_target(&path, request.clone(), result.clone(), 20).unwrap();
    let terminal = terminal_with_job_projection(
        JobReceiptV1::new(
            1,
            request.job_id().clone(),
            request.request_digest().clone(),
        ),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::WorkspaceReset {
            workspace_id: identity().workspace_uuid().clone(),
            revision: Revision::ZERO,
        }),
        PersistedTerminalJobStateV1::Succeeded,
        request.submitted_at().get(),
        None,
        21,
    );
    overwrite_seeded_terminal(&path, &terminal);

    assert!(matches!(
        seed_reset_target(&path, request, result, 21),
        Err(StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Job
        })
    ));
}
#[test]
fn direct_admission_uses_a_complete_deterministic_store_command_document() {
    let item_id = ItemId::new("direct-item").unwrap();
    let preconditions = RevisionAttemptItemPreconditionsV1::new(
        Some(Revision::new(11)),
        Some(AttemptId::new("00000000-0000-4000-8000-000000000099").unwrap()),
        Some(item_id.clone()),
        Some(Revision::new(7)),
    )
    .unwrap();
    let request = AdmitRequestV1::new(
        DomainCommand::ItemSet { item_id },
        IdempotencyKeyV1::new("direct-execution").unwrap(),
        job(1),
        preconditions,
        digest('a'),
        UnixMillis::new(2),
    );
    let expected = r#"{"command":{"item_id":"direct-item","kind":"item_set"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000099","expected_item_id":"direct-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#;
    assert_eq!(request.canonical_execution().as_str(), expected);

    let executable = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("direct-execution").unwrap(),
        job(1),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('a'),
        UnixMillis::new(2),
    );
    let executable_json = r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#;
    assert_eq!(executable.canonical_execution().as_str(), executable_json);

    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    assert!(matches!(
        store.admit(&identity(), executable),
        Ok(AdmitOutcomeV1::New(_))
    ));
    let claimed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("direct-execution-worker").unwrap(),
            UnixMillis::new(3),
        )
        .unwrap()
        .expect("admitted job must be claimable");
    assert_eq!(
        claimed.execution().canonical_execution().as_str(),
        executable_json
    );
}
fn assert_terminal_replay(
    outcome: Result<AdmitOutcomeV1, StoreErrorV1>,
    expected: PersistedTerminalReceiptV1,
) {
    let replay = match outcome {
        Ok(AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt))) => receipt,
        outcome => panic!("expected terminal idempotency replay, got {outcome:?}"),
    };
    assert_eq!(replay, expected);
}

fn terminal_with_job_projection(
    job: JobReceiptV1,
    result: PersistedTerminalResultV1,
    state: PersistedTerminalJobStateV1,
    submitted_at: u64,
    claimed_at: Option<u64>,
    finished_at: u64,
) -> PersistedTerminalReceiptV1 {
    PersistedTerminalReceiptV1::new_with_projections(
        job,
        result,
        PersistedTerminalJobProjectionV1::new(
            state,
            UnixMillis::new(submitted_at),
            claimed_at.map(UnixMillis::new),
            UnixMillis::new(finished_at),
        )
        .unwrap(),
        None,
    )
    .unwrap()
}
fn assert_job_replay(outcome: Result<AdmitOutcomeV1, StoreErrorV1>, expected: JobReceiptV1) {
    let replay = match outcome {
        Ok(AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt))) => receipt,
        outcome => panic!("expected non-terminal idempotency replay, got {outcome:?}"),
    };
    assert_eq!(replay.identity_sequence(), expected.identity_sequence());
    assert_eq!(replay.job_id(), expected.job_id());
    assert_eq!(replay.request_digest(), expected.request_digest());
    assert_eq!(replay, expected);
}
fn read_idempotent_outcome_without_mutation(
    store: &SqliteStoreV1,
    database: &Path,
    idempotency_key: &IdempotencyKeyV1,
    request_digest: &Sha256Digest,
) -> Result<Result<Option<AdmitOutcomeV1>, StoreErrorV1>, Box<dyn std::error::Error>> {
    let logical_before = (
        store.read_workspace_view(&identity())?,
        store.list_jobs(&identity(), JobListQueryV1::new(1_000)?)?,
    );
    let artifacts_before = snapshot_database_artifacts(database)?;
    let outcome = store.read_idempotent_outcome(&identity(), idempotency_key, request_digest);
    assert_eq!(artifacts_before, snapshot_database_artifacts(database)?);
    let logical_after = (
        store.read_workspace_view(&identity())?,
        store.list_jobs(&identity(), JobListQueryV1::new(1_000)?)?,
    );
    assert_eq!(logical_before, logical_after);
    Ok(outcome)
}

fn assert_workspace_initialize_claim(
    claimed: &podway_store::ClaimedJobV1,
    expected: &JobReceiptV1,
    worker: &WorkerIdV1,
) {
    assert_eq!(
        claimed.job().identity_sequence(),
        expected.identity_sequence()
    );
    assert_eq!(claimed.job().job_id(), expected.job_id());
    assert_eq!(claimed.job().request_digest(), expected.request_digest());
    assert_eq!(claimed.job(), expected);
    assert_eq!(
        claimed.execution().command(),
        &DomainCommand::WorkspaceInitialize
    );
    assert_eq!(
        claimed.execution().preconditions(),
        &RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap()
    );
    assert_eq!(claimed.claim().identity(), &identity());
    assert_eq!(claimed.claim().job_id(), expected.job_id());
    assert_eq!(claimed.claim().worker(), worker);
    assert_ne!(claimed.claim().job_revision(), Revision::ZERO);
    assert!(claimed.current_session().is_none());
}
fn aggregate() -> SessionAggregateV1 {
    let authored = serde_json::json!({
        "schema": "podway.procedure/v1",
        "id": "fixture",
        "version": "1",
        "name": "Fixture",
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
        snapshot_id: ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000020").unwrap(),
        schema_id: "podway.procedure/v1".to_owned(),
        procedure_id: "fixture".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Fixture".to_owned(),
        source_label: ProcedureSourceLabelV1::file("fixture").unwrap(),
        canonical_json: CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
        digest: Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
            .unwrap(),
        created_at: UnixMillis::new(10),
    })
    .unwrap();
    SessionAggregateV1::start(
        SessionId::new("00000000-0000-4000-8000-000000000021").unwrap(),
        "Fixture task",
        snapshot,
        AttemptId::new("00000000-0000-4000-8000-000000000022").unwrap(),
        UnixMillis::new(11),
    )
    .unwrap()
}
const CONCURRENCY_RACE_ITERATIONS: u8 = 4;

#[derive(Debug, Eq, PartialEq)]
enum SessionBarrierAdmissionOutcome {
    AdmittedAfterCancellation,
    BlockedByBarrier,
    RejectedAfterCleanup,
}

fn seed_current_session(
    store: &SqliteStoreV1,
    aggregate: &SessionAggregateV1,
    job_id: JobId,
    key: &str,
    now: u64,
) {
    admit_for_command(store, DomainCommand::SessionStart, job_id, key, 'a', now);
    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("session-seed-worker").unwrap(),
            UnixMillis::new(now + 1),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            claim.claim().clone(),
            Revision::ZERO,
            Some(
                StateTransitionV1::new_persisted(
                    Some(aggregate.session_id().clone()),
                    Revision::ZERO,
                    aggregate.revision(),
                    PersistedSessionMutationV1::Replace(aggregate.clone()),
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: aggregate.session_id().clone(),
                revision_before: Revision::ZERO,
                revision_after: aggregate.revision(),
                changed: true,
            }),
            UnixMillis::new(now + 2),
        )
        .unwrap();
}

fn session_complete_request(
    aggregate: &SessionAggregateV1,
    job_id: JobId,
    key: &str,
    digest_nibble: char,
    now: u64,
) -> AdmitRequestV1 {
    AdmitRequestV1::new(
        DomainCommand::SessionComplete,
        IdempotencyKeyV1::new(key).unwrap(),
        job_id,
        RevisionAttemptItemPreconditionsV1::new(
            Some(aggregate.revision()),
            aggregate.active_attempt_id().cloned(),
            None,
            None,
        )
        .unwrap(),
        digest(digest_nibble),
        UnixMillis::new(now),
    )
}

fn session_reset_request(
    aggregate: &SessionAggregateV1,
    job_id: JobId,
    key: &str,
    digest_nibble: char,
    now: u64,
) -> AdmitRequestV1 {
    AdmitRequestV1::new(
        DomainCommand::SessionReset,
        IdempotencyKeyV1::new(key).unwrap(),
        job_id,
        RevisionAttemptItemPreconditionsV1::new(Some(aggregate.revision()), None, None, None)
            .unwrap(),
        digest(digest_nibble),
        UnixMillis::new(now),
    )
}
#[test]
fn terminal_session_projection_is_immutable_after_a_later_lifecycle_transition() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let aggregate = aggregate();
    let start_job = job(250);
    seed_current_session(&store, &aggregate, start_job.clone(), "projection-start", 2);

    let cancelled = apply_transition_v1(
        Some(&aggregate),
        &SessionCommandV1::Cancel(CancelSessionV1 {
            expected_attempt_id: aggregate.active_attempt_id().cloned().unwrap(),
            reason: "fixture".to_owned(),
        }),
        CommandContextV1 {
            expected_revision: aggregate.revision(),
            now: UnixMillis::new(12),
        },
    )
    .unwrap()
    .next_aggregate()
    .cloned()
    .unwrap();
    let cancel_request = AdmitRequestV1::new(
        DomainCommand::SessionCancel,
        IdempotencyKeyV1::new("projection-cancel").unwrap(),
        job(251),
        RevisionAttemptItemPreconditionsV1::new(
            Some(aggregate.revision()),
            aggregate.active_attempt_id().cloned(),
            None,
            None,
        )
        .unwrap(),
        digest('b'),
        UnixMillis::new(13),
    );
    store.admit(&identity(), cancel_request).unwrap();
    let cancel_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("projection-worker").unwrap(),
            UnixMillis::new(14),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            cancel_claim.claim().clone(),
            aggregate.revision(),
            Some(
                StateTransitionV1::new_persisted(
                    Some(cancelled.session_id().clone()),
                    aggregate.revision(),
                    cancelled.revision(),
                    PersistedSessionMutationV1::Replace(cancelled.clone()),
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: cancelled.session_id().clone(),
                revision_before: aggregate.revision(),
                revision_after: cancelled.revision(),
                changed: true,
            }),
            UnixMillis::new(15),
        )
        .unwrap();
    assert_eq!(
        store
            .read_session_aggregate(&identity())
            .unwrap()
            .expect("cancelled aggregate remains durable")
            .lifecycle(),
        SessionLifecycle::Cancelled
    );

    let replay_request = AdmitRequestV1::new(
        DomainCommand::SessionStart,
        IdempotencyKeyV1::new("projection-start").unwrap(),
        start_job,
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('a'),
        UnixMillis::new(2),
    );
    let replay = match store.admit(&identity(), replay_request).unwrap() {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => receipt,
        outcome => panic!("expected terminal replay, got {outcome:?}"),
    };
    let job_projection = replay.job_projection().expect("job projection is durable");
    assert_eq!(
        job_projection.state(),
        PersistedTerminalJobStateV1::Succeeded
    );
    assert_eq!(job_projection.submitted_at(), UnixMillis::new(2));
    assert_eq!(job_projection.claimed_at(), Some(UnixMillis::new(3)));
    assert_eq!(job_projection.finished_at(), UnixMillis::new(4));
    let session_projection = replay
        .session_projection()
        .expect("session projection is durable");
    assert_eq!(session_projection.session_id(), aggregate.session_id());
    assert_eq!(session_projection.task_title(), aggregate.task_title());
    assert_eq!(
        session_projection.lifecycle(),
        PersistedSessionLifecycleV1::Running
    );
    assert_eq!(session_projection.revision_before(), Revision::ZERO);
    assert_eq!(session_projection.revision_after(), aggregate.revision());
}
#[test]
fn mismatched_terminal_projection_is_rejected_as_corrupt_storage() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    let terminal_job = job(252);
    admit(
        &store,
        terminal_job.clone(),
        "projection-corruption",
        'c',
        2,
    );
    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("projection-worker").unwrap(),
            UnixMillis::new(3),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            claim.claim().clone(),
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(4),
        )
        .unwrap();

    let connection = Connection::open(&path).unwrap();
    let encoded: String = connection
        .query_row(
            "SELECT terminal_response_json FROM jobs WHERE job_id = ?1",
            [terminal_job.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let tampered = encoded.replacen("\"finished_at\":4", "\"finished_at\":3", 1);
    assert_ne!(tampered, encoded);
    connection
        .execute(
            "UPDATE jobs SET terminal_response_json = ?1 WHERE job_id = ?2",
            [tampered.as_str(), terminal_job.as_str()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE idempotency_records SET terminal_response_json = ?1 WHERE job_id = ?2",
            [tampered.as_str(), terminal_job.as_str()],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        store.read_job(&identity(), &terminal_job),
        Err(StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Job
        })
    ));
}

#[test]
fn schema_zero_identity_fifo_and_idempotent_admission_are_durable() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let first = job(3);
    let second = job(4);
    admit(&store, first.clone(), "first", 'c', 2);
    admit(&store, second.clone(), "second", 'd', 3);

    let replay = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("first").unwrap(),
        first.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(4),
    );
    assert_job_replay(
        store.admit(&identity(), replay),
        JobReceiptV1::new(1, first.clone(), digest('c')),
    );

    let worker = WorkerIdV1::new("worker-a").unwrap();
    let claimed = store
        .claim_next(&identity(), worker, UnixMillis::new(5))
        .unwrap()
        .unwrap();
    assert_eq!(claimed.job().job_id(), &first);
    assert_eq!(
        claimed.execution().command(),
        &DomainCommand::WorkspaceInitialize
    );
    assert!(claimed.current_session().is_none());
    assert!(
        store
            .claim_next(
                &identity(),
                WorkerIdV1::new("worker-b").unwrap(),
                UnixMillis::new(6),
            )
            .unwrap()
            .is_none()
    );

    let view = store.read_workspace_view(&identity()).unwrap();
    assert_eq!(view.queued_job_count(), 1);
    assert_eq!(view.running_job_id(), Some(&first));

    let conflict = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("first").unwrap(),
        job(5),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('e'),
        UnixMillis::new(6),
    );
    assert!(matches!(
        store.admit(&identity(), conflict),
        Err(StoreErrorV1::IdempotencyDigestConflictV1 { .. })
    ));
}
#[test]
fn idempotency_outcome_reads_replay_every_durable_state_without_mutating_storage()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let database = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    let missing_key = IdempotencyKeyV1::new("missing-idempotency-key")?;
    assert_eq!(
        read_idempotent_outcome_without_mutation(&store, &database, &missing_key, &digest('a'),)??,
        None
    );

    let idempotency_key = IdempotencyKeyV1::new("idempotency-read-replay")?;
    let request_digest = digest('b');
    let request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        idempotency_key.clone(),
        job(6),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None)?,
        request_digest.clone(),
        UnixMillis::new(2),
    );
    let receipt = match store.admit(&identity(), request)? {
        AdmitOutcomeV1::New(receipt) => receipt,
        outcome => panic!("expected new admission, got {outcome:?}"),
    };
    let expected_job_replay = Some(AdmitOutcomeV1::Existing(
        JobReceiptOrTerminalV1::JobReceipt(receipt.clone()),
    ));
    assert_eq!(
        read_idempotent_outcome_without_mutation(
            &store,
            &database,
            &idempotency_key,
            &request_digest,
        )??,
        expected_job_replay
    );

    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("idempotency-read-worker")?,
            UnixMillis::new(3),
        )?
        .expect("queued admission must be claimable");
    assert_eq!(claim.job(), &receipt);
    assert_eq!(
        read_idempotent_outcome_without_mutation(
            &store,
            &database,
            &idempotency_key,
            &request_digest,
        )??,
        Some(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::JobReceipt(receipt.clone()),
        ))
    );

    store.commit_terminal(
        claim.claim().clone(),
        Revision::ZERO,
        None,
        TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
        UnixMillis::new(4),
    )?;
    let expected_terminal = store
        .read_job(&identity(), receipt.job_id())?
        .expect("terminal job must remain readable")
        .terminal_receipt()
        .cloned()
        .expect("terminal replay must retain its v1 projections");
    assert!(expected_terminal.job_projection().is_some());
    assert_eq!(
        read_idempotent_outcome_without_mutation(
            &store,
            &database,
            &idempotency_key,
            &request_digest,
        )??,
        Some(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::TerminalReceipt(expected_terminal),
        ))
    );

    assert_eq!(
        read_idempotent_outcome_without_mutation(
            &store,
            &database,
            &idempotency_key,
            &digest('c'),
        )?,
        Err(StoreErrorV1::IdempotencyDigestConflictV1 {
            expected: request_digest,
            actual: digest('c'),
        })
    );
    Ok(())
}
#[test]
fn bound_identity_rejects_another_worktree_before_any_queue_access() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let mismatched = DurableWorktreeIdentityV1::new(
        digest('f'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        digest('b'),
    );
    assert!(matches!(
        store.read_workspace_view(&mismatched),
        Err(StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::WorkspaceIdentity,
        })
    ));
}
#[test]
fn workspace_binding_inspection_uses_a_disposable_snapshot_without_touching_or_locking_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let database = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    admit(&store, job(12), "binding-snapshot", 'c', 2);

    let artifacts_before = snapshot_database_artifacts(&database)?;
    let entries_before = workspace_directory_entries(&database)?;
    let binding =
        SqliteStoreV1::inspect_workspace_binding(&database, &SqliteStoreOptionsV1::new(8)?)?
            .expect("an initialized database must expose its binding");
    assert_eq!(binding.identity(), &identity());
    assert_eq!(artifacts_before, snapshot_database_artifacts(&database)?);
    assert_eq!(entries_before, workspace_directory_entries(&database)?);

    let writer = Connection::open(&database)?;
    writer.execute_batch("BEGIN IMMEDIATE; ROLLBACK;")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn workspace_binding_inspection_never_recovers_an_interrupted_publication()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let database = temporary.path().join("state.sqlite3");
    drop(store(&temporary));
    let publication = temporary.path().join(".state.sqlite3.1.0.0.tmp");
    fs::hard_link(&database, &publication)?;

    let artifacts_before = snapshot_database_artifacts(&database)?;
    let entries_before = workspace_directory_entries(&database)?;
    let inspection =
        SqliteStoreV1::inspect_workspace_binding(&database, &SqliteStoreOptionsV1::new(8)?);

    assert!(matches!(
        inspection,
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::StorageIo
        })
    ));
    assert_eq!(artifacts_before, snapshot_database_artifacts(&database)?);
    assert_eq!(entries_before, workspace_directory_entries(&database)?);
    assert!(publication.exists());
    Ok(())
}

#[test]
fn workspace_binding_inspection_classifies_malformed_authoritative_rows_as_corrupt_state()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let database = temporary.path().join("state.sqlite3");
    drop(store(&temporary));

    let connection = Connection::open(&database)?;
    connection.execute(
        "UPDATE workspace_state SET git_common_fingerprint = 'not-a-canonical-digest' \
         WHERE singleton = 1",
        [],
    )?;
    drop(connection);

    let artifacts_before = snapshot_database_artifacts(&database)?;
    assert!(matches!(
        SqliteStoreV1::inspect_workspace_binding(&database, &SqliteStoreOptionsV1::new(8)?),
        Err(StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Workspace
        })
    ));
    assert_eq!(artifacts_before, snapshot_database_artifacts(&database)?);
    Ok(())
}
#[test]
fn workspace_binding_inspection_classifies_a_missing_required_row_as_corrupt_state()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let database = temporary.path().join("state.sqlite3");
    drop(store(&temporary));

    let connection = Connection::open(&database)?;
    connection.execute("DELETE FROM workspace_state WHERE singleton = 1", [])?;
    drop(connection);

    assert!(matches!(
        SqliteStoreV1::inspect_workspace_binding(&database, &SqliteStoreOptionsV1::new(8)?),
        Err(StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Workspace
        })
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn workspace_binding_inspection_keeps_authority_availability_failures_transient()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let database = temporary.path().join("state.sqlite3");
    drop(store(&temporary));

    fs::set_permissions(&database, fs::Permissions::from_mode(0o000))?;
    let inspection =
        SqliteStoreV1::inspect_workspace_binding(&database, &SqliteStoreOptionsV1::new(8)?);
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600))?;

    assert!(matches!(
        inspection,
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::StorageIo
        })
    ));
    Ok(())
}
#[test]
fn ordinary_admission_rejects_workspace_reset_all_before_idempotency_or_queue_writes() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let key = IdempotencyKeyV1::new("reset-all-ordinary-admission").unwrap();
    let request = AdmitRequestV1::new(
        DomainCommand::WorkspaceResetAll,
        key.clone(),
        job(13),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('f'),
        UnixMillis::new(2),
    );

    assert!(matches!(
        store.admit(&identity(), request),
        Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::TransitionMutationShape
        })
    ));
    assert_eq!(
        store
            .read_idempotent_outcome(&identity(), &key, &digest('f'))
            .unwrap(),
        None,
        "ordinary reset admission must not create an idempotency record"
    );
    assert!(
        store
            .list_jobs(&identity(), JobListQueryV1::new(100).unwrap())
            .unwrap()
            .is_empty(),
        "ordinary reset admission must not create a queued job"
    );
}
#[test]
fn reopen_recovers_running_work_and_replays_persisted_execution() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let root = ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4")).unwrap();
    let options = SqliteStoreOptionsV1::new(8).unwrap();
    let store = SqliteStoreV1::open(
        &path,
        &root,
        identity(),
        options.clone(),
        UnixMillis::new(1),
    )
    .unwrap();
    admit(&store, job(12), "reopen", 'e', 2);
    let first_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(3),
        )
        .unwrap()
        .unwrap();
    drop(store);

    let reopened = SqliteStoreV1::open(
        &path,
        &root,
        identity(),
        options.clone(),
        UnixMillis::new(4),
    )
    .unwrap();
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    drop(reopened);
    let reopened =
        SqliteStoreV1::open(&path, &root, identity(), options, UnixMillis::new(5)).unwrap();
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 0);
    let connection = Connection::open(&path).unwrap();
    let recovered_journal_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM operational_journal WHERE event_name = 'job.recovered'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recovered_journal_count, 1);
    drop(connection);
    let recovered = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-b").unwrap(),
            UnixMillis::new(5),
        )
        .unwrap()
        .unwrap();
    assert_eq!(recovered.job().job_id(), first_claim.job().job_id());
    assert_eq!(recovered.execution(), first_claim.execution());
    assert!(matches!(
        reopened.commit_terminal(
            first_claim.claim().clone(),
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(6),
        ),
        Err(StoreErrorV1::ClaimStaleV1 { .. })
    ));
}

#[test]
fn cancellation_and_restart_recovery_make_claim_tokens_stale() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    let queued = job(6);
    admit(&store, queued.clone(), "queued", 'c', 2);
    assert!(matches!(
        store.cancel_before_claim(
            &identity(),
            queued.clone(),
            Revision::new(1),
            UnixMillis::new(3)
        ),
        Ok(CancelOutcomeV1::Cancelled(_))
    ));
    assert!(matches!(
        store.cancel_before_claim(&identity(), queued, Revision::new(1), UnixMillis::new(4)),
        Ok(CancelOutcomeV1::AlreadyTerminal(_))
    ));

    admit(&store, job(7), "running", 'd', 5);
    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(6),
        )
        .unwrap()
        .unwrap()
        .claim()
        .clone();
    drop(store);

    let root = ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4")).unwrap();
    let reopened = SqliteStoreV1::open(
        &path,
        &root,
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(7),
    )
    .unwrap();
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    assert!(matches!(
        reopened.commit_terminal(
            claim,
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(8),
        ),
        Err(StoreErrorV1::ClaimStaleV1 { .. })
    ));
}

#[test]
fn domain_failure_ignores_stale_admitted_preconditions_and_replays_immutably() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let job_id = job(8);
    let request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("conflict").unwrap(),
        job_id.clone(),
        RevisionAttemptItemPreconditionsV1::new(Some(Revision::new(1)), None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(2),
    );
    store.admit(&identity(), request).unwrap();
    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(3),
        )
        .unwrap()
        .unwrap();
    let receipt = store
        .commit_terminal(
            claim.claim().clone(),
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(4),
        )
        .unwrap();
    assert_eq!(
        receipt,
        TerminalReceiptV1::new(
            JobReceiptV1::new(1, job(8), digest('c')),
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
        )
    );
    assert!(store.read_session_aggregate(&identity()).unwrap().is_none());

    let replay = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("conflict").unwrap(),
        job_id,
        RevisionAttemptItemPreconditionsV1::new(Some(Revision::new(1)), None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(5),
    );
    assert_terminal_replay(
        store.admit(&identity(), replay),
        terminal_with_job_projection(
            JobReceiptV1::new(1, job(8), digest('c')),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::InvalidState {
                reason: "fixture".to_owned(),
            }),
            PersistedTerminalJobStateV1::Failed,
            2,
            Some(3),
            4,
        ),
    );
}
#[test]
fn successful_terminal_replaces_normalized_rows_and_hydrates_them_on_read() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let aggregate = aggregate();
    let job_id = job(9);
    admit_for_command(
        &store,
        DomainCommand::SessionStart,
        job_id,
        "replace",
        'e',
        2,
    );
    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(12),
        )
        .unwrap()
        .unwrap();
    let transition = StateTransitionV1::new_persisted(
        Some(aggregate.session_id().clone()),
        Revision::ZERO,
        aggregate.revision(),
        PersistedSessionMutationV1::Replace(aggregate.clone()),
    )
    .unwrap();
    store
        .commit_terminal(
            claim.claim().clone(),
            Revision::ZERO,
            Some(transition),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: aggregate.session_id().clone(),
                revision_before: Revision::ZERO,
                revision_after: aggregate.revision(),
                changed: true,
            }),
            UnixMillis::new(13),
        )
        .unwrap();
    assert_eq!(
        store.read_session_aggregate(&identity()).unwrap(),
        Some(aggregate)
    );
}
#[test]
fn terminal_failure_replays_exactly_without_mutating_a_current_session() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let aggregate = aggregate();
    admit_for_command(&store, DomainCommand::SessionStart, job(10), "seed", 'c', 2);
    let seed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(12),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            seed.claim().clone(),
            Revision::ZERO,
            Some(
                StateTransitionV1::new_persisted(
                    Some(aggregate.session_id().clone()),
                    Revision::ZERO,
                    aggregate.revision(),
                    PersistedSessionMutationV1::Replace(aggregate.clone()),
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: aggregate.session_id().clone(),
                revision_before: Revision::ZERO,
                revision_after: aggregate.revision(),
                changed: true,
            }),
            UnixMillis::new(13),
        )
        .unwrap();

    let failed_job = job(11);
    admit(&store, failed_job.clone(), "failure", 'd', 14);
    let failed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(15),
        )
        .unwrap()
        .unwrap();
    let receipt = store
        .commit_terminal(
            failed.claim().clone(),
            aggregate.revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(16),
        )
        .unwrap();
    assert_eq!(
        receipt,
        TerminalReceiptV1::new(
            JobReceiptV1::new(2, job(11), digest('d')),
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
        )
    );
    assert_eq!(
        store.read_session_aggregate(&identity()).unwrap(),
        Some(aggregate)
    );

    let replay = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("failure").unwrap(),
        failed_job,
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('d'),
        UnixMillis::new(17),
    );
    assert_terminal_replay(
        store.admit(&identity(), replay),
        terminal_with_job_projection(
            JobReceiptV1::new(2, job(11), digest('d')),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::InvalidState {
                reason: "fixture".to_owned(),
            }),
            PersistedTerminalJobStateV1::Failed,
            14,
            Some(15),
            16,
        ),
    );
}
#[test]
fn post_commit_admission_and_claim_failpoints_are_retryable() {
    let admission_temporary = TempDir::new().unwrap();
    let admission_options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(StoreFailpointV1::AdmissionAfterCommit));
    let admission_store =
        store_with_options(&admission_temporary, admission_options, UnixMillis::new(1));
    let admission_request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("lost-admission").unwrap(),
        job(15),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('a'),
        UnixMillis::new(2),
    );
    assert!(matches!(
        admission_store.admit(&identity(), admission_request.clone()),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery,
        })
    ));
    drop(admission_store);
    let admission_retry = store_with_options(
        &admission_temporary,
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(3),
    );
    assert_job_replay(
        admission_retry.admit(&identity(), admission_request),
        JobReceiptV1::new(1, job(15), digest('a')),
    );
    let admission_worker = WorkerIdV1::new("worker-a").unwrap();
    let admission_claim = admission_retry
        .claim_next(&identity(), admission_worker.clone(), UnixMillis::new(4))
        .unwrap()
        .expect("post-commit admission replay must remain claimable");
    assert_workspace_initialize_claim(
        &admission_claim,
        &JobReceiptV1::new(1, job(15), digest('a')),
        &admission_worker,
    );

    let claim_temporary = TempDir::new().unwrap();
    let claim_options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(StoreFailpointV1::ClaimAfterCommit));
    let claim_store = store_with_options(&claim_temporary, claim_options, UnixMillis::new(1));
    admit(&claim_store, job(16), "lost-claim", 'b', 2);
    assert!(matches!(
        claim_store.claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(3),
        ),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery,
        })
    ));
    assert_eq!(
        claim_store
            .claim_next(
                &identity(),
                WorkerIdV1::new("worker-b").unwrap(),
                UnixMillis::new(4),
            )
            .unwrap(),
        None
    );
    drop(claim_store);
    let claim_retry = store_with_options(
        &claim_temporary,
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(5),
    );
    let claim_retry_worker = WorkerIdV1::new("worker-c").unwrap();
    let claim_retry_claim = claim_retry
        .claim_next(&identity(), claim_retry_worker.clone(), UnixMillis::new(6))
        .unwrap()
        .expect("recovered post-commit claim must be claimable once");
    assert_workspace_initialize_claim(
        &claim_retry_claim,
        &JobReceiptV1::new(1, job(16), digest('b')),
        &claim_retry_worker,
    );
}

#[test]
fn pre_commit_terminal_and_recovery_failpoints_roll_back() {
    let terminal_temporary = TempDir::new().unwrap();
    let terminal_options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(StoreFailpointV1::TerminalBeforeCommit));
    let terminal_store =
        store_with_options(&terminal_temporary, terminal_options, UnixMillis::new(1));
    let terminal_job = job(17);
    admit(
        &terminal_store,
        terminal_job.clone(),
        "terminal-rollback",
        'c',
        2,
    );
    let terminal_claim = terminal_store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(3),
        )
        .unwrap()
        .unwrap();
    let terminal_worker = WorkerIdV1::new("worker-a").unwrap();
    assert_workspace_initialize_claim(
        &terminal_claim,
        &JobReceiptV1::new(1, job(17), digest('c')),
        &terminal_worker,
    );
    assert!(matches!(
        terminal_store.commit_terminal(
            terminal_claim.claim().clone(),
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(4),
        ),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery,
        })
    ));
    let terminal_replay = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("terminal-rollback").unwrap(),
        terminal_job,
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(5),
    );
    assert_job_replay(
        terminal_store.admit(&identity(), terminal_replay),
        JobReceiptV1::new(1, job(17), digest('c')),
    );

    let recovery_temporary = TempDir::new().unwrap();
    let recovery_store = store(&recovery_temporary);
    admit(&recovery_store, job(18), "recovery-rollback", 'd', 2);
    let recovery_initial_worker = WorkerIdV1::new("worker-a").unwrap();
    let recovery_initial_claim = recovery_store
        .claim_next(
            &identity(),
            recovery_initial_worker.clone(),
            UnixMillis::new(3),
        )
        .unwrap()
        .expect("recovery fixture must claim its admitted job");
    assert_workspace_initialize_claim(
        &recovery_initial_claim,
        &JobReceiptV1::new(1, job(18), digest('d')),
        &recovery_initial_worker,
    );
    drop(recovery_store);
    let recovery_path = recovery_temporary.path().join("state.sqlite3");
    let recovery_root =
        ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4")).unwrap();
    let recovery_fail_options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(StoreFailpointV1::RecoveryBeforeCommit));
    assert!(matches!(
        SqliteStoreV1::open(
            &recovery_path,
            &recovery_root,
            identity(),
            recovery_fail_options,
            UnixMillis::new(4),
        ),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery,
        })
    ));
    let recovery_retry = store_with_options(
        &recovery_temporary,
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(5),
    );
    assert_eq!(
        recovery_retry
            .startup_recovery_report()
            .requeued_job_count(),
        1
    );
    let recovery_retry_worker = WorkerIdV1::new("worker-b").unwrap();
    let recovery_retry_claim = recovery_retry
        .claim_next(
            &identity(),
            recovery_retry_worker.clone(),
            UnixMillis::new(6),
        )
        .unwrap()
        .expect("recovery retry must reclaim the requeued job");
    assert_workspace_initialize_claim(
        &recovery_retry_claim,
        &JobReceiptV1::new(1, job(18), digest('d')),
        &recovery_retry_worker,
    );
    assert_ne!(
        recovery_retry_claim.claim().job_revision(),
        recovery_initial_claim.claim().job_revision()
    );
}
#[test]
fn session_scoped_jobs_retain_admitted_scope_through_terminal_failure() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    let aggregate = aggregate();
    admit_for_command(
        &store,
        DomainCommand::SessionStart,
        job(19),
        "scope-seed",
        'e',
        2,
    );
    let seed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(12),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            seed.claim().clone(),
            Revision::ZERO,
            Some(
                StateTransitionV1::new_persisted(
                    Some(aggregate.session_id().clone()),
                    Revision::ZERO,
                    aggregate.revision(),
                    PersistedSessionMutationV1::Replace(aggregate.clone()),
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: aggregate.session_id().clone(),
                revision_before: Revision::ZERO,
                revision_after: aggregate.revision(),
                changed: true,
            }),
            UnixMillis::new(13),
        )
        .unwrap();

    let scoped_job = job(20);
    let scoped_request = AdmitRequestV1::new(
        DomainCommand::SessionComplete,
        IdempotencyKeyV1::new("scope-failure").unwrap(),
        scoped_job.clone(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(aggregate.revision()),
            aggregate.active_attempt_id().cloned(),
            None,
            None,
        )
        .unwrap(),
        digest('f'),
        UnixMillis::new(14),
    );
    store.admit(&identity(), scoped_request).unwrap();
    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(15),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            claim.claim().clone(),
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(16),
        )
        .unwrap();
    drop(store);

    let connection = Connection::open(path).unwrap();
    let persisted_scope: String = connection
        .query_row(
            "SELECT session_id FROM jobs WHERE job_id = ?1",
            [scoped_job.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_scope, aggregate.session_id().as_str());
}
#[test]
fn journal_pruning_honors_the_protected_boundary_age_and_tie_breaker() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    let connection = Connection::open(&path).unwrap();
    for _ in 0..201 {
        connection
            .execute(
                "INSERT INTO operational_journal
                 (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, details_json)
                 VALUES (0, 'info', 'fixture', NULL, NULL, 'fixture', NULL)",
                [],
            )
            .unwrap();
    }
    drop(connection);

    let underflow = store
        .prune_terminal_history(&identity(), UnixMillis::new(0))
        .unwrap();
    assert_eq!(underflow.deleted_journal_entries(), 0);
    let report = store
        .prune_terminal_history(&identity(), UnixMillis::new(604_800_001))
        .unwrap();
    assert_eq!(report.deleted_journal_entries(), 1);

    let connection = Connection::open(path).unwrap();
    let retained_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM operational_journal", [], |row| {
            row.get(0)
        })
        .unwrap();
    let oldest_retained_id: i64 = connection
        .query_row(
            "SELECT MIN(journal_id) FROM operational_journal",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_count, 200);
    assert_eq!(oldest_retained_id, 2);
}
#[test]
fn terminal_pruning_keeps_exact_replay_for_the_oldest_tied_receipt() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    for number in 1..=101 {
        let job_id = job(number);
        let key = format!("terminal-{number}");
        admit(&store, job_id, &key, 'a', 0);
        let claim = store
            .claim_next(
                &identity(),
                WorkerIdV1::new("worker-a").unwrap(),
                UnixMillis::new(0),
            )
            .unwrap()
            .unwrap();
        store
            .commit_terminal(
                claim.claim().clone(),
                Revision::ZERO,
                None,
                TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
                UnixMillis::new(0),
            )
            .unwrap();
    }

    let report = store
        .prune_terminal_history(&identity(), UnixMillis::new(604_800_001))
        .unwrap();
    assert_eq!(report.deleted_terminal_jobs(), 1);

    let connection = Connection::open(&path).unwrap();
    let pruned_job_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE job_id = ?1",
            [job(1).as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let retained_terminal_json: String = connection
        .query_row(
            "SELECT terminal_response_json FROM idempotency_records WHERE idempotency_key = ?1",
            ["terminal-1"],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert_eq!(pruned_job_count, 0);

    let replay = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("terminal-1").unwrap(),
        job(1),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('a'),
        UnixMillis::new(604_800_002),
    );
    let replay = match store.admit(&identity(), replay).unwrap() {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => receipt,
        outcome => panic!("expected terminal replay, got {outcome:?}"),
    };
    assert_eq!(replay.job(), &JobReceiptV1::new(1, job(1), digest('a')));
    assert_eq!(
        replay.result(),
        &PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::InvalidState {
            reason: "fixture".to_owned(),
        })
    );
    let job_projection = replay
        .job_projection()
        .expect("pruned replay keeps job facts");
    assert_eq!(job_projection.state(), PersistedTerminalJobStateV1::Failed);
    assert_eq!(job_projection.submitted_at(), UnixMillis::new(0));
    assert_eq!(job_projection.claimed_at(), Some(UnixMillis::new(0)));
    assert_eq!(job_projection.finished_at(), UnixMillis::new(0));
    assert!(replay.session_projection().is_none());
    assert_eq!(
        podway_store::codec::decode_terminal_receipt_v1(&retained_terminal_json).unwrap(),
        replay
    );
    let automatic_prune_job = job(102);
    admit(
        &store,
        automatic_prune_job.clone(),
        "terminal-automatic-prune",
        'b',
        604_800_003,
    );
    let automatic_prune_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-b").unwrap(),
            UnixMillis::new(604_800_004),
        )
        .unwrap()
        .unwrap();
    assert_eq!(automatic_prune_claim.job().job_id(), &automatic_prune_job);
    store
        .commit_terminal(
            automatic_prune_claim.claim().clone(),
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(604_800_005),
        )
        .unwrap();

    let connection = Connection::open(path).unwrap();
    let retention_pruned_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM operational_journal WHERE event_name = 'retention.pruned'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let (retention_pruned_summary, retention_pruned_details): (String, Option<String>) = connection
        .query_row(
            "SELECT summary, details_json FROM operational_journal
             WHERE event_name = 'retention.pruned'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(retention_pruned_count, 1);
    assert_eq!(
        retention_pruned_summary,
        "terminal_jobs_deleted=1; journal_entries_deleted=0; orphan_workspace_receipts_deleted=0"
    );
    assert_eq!(retention_pruned_details, None);
    let retained_jobs: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('succeeded', 'failed', 'cancelled')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_jobs, 100);
}
#[test]
fn session_reset_barrier_preserves_failed_state_then_cleans_old_session_rows() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    let aggregate = aggregate();

    admit_for_command(
        &store,
        DomainCommand::SessionStart,
        job(40),
        "barrier-seed",
        'a',
        2,
    );
    let seed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(3),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            seed.claim().clone(),
            Revision::ZERO,
            Some(
                StateTransitionV1::new_persisted(
                    Some(aggregate.session_id().clone()),
                    Revision::ZERO,
                    aggregate.revision(),
                    PersistedSessionMutationV1::Replace(aggregate.clone()),
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: aggregate.session_id().clone(),
                revision_before: Revision::ZERO,
                revision_after: aggregate.revision(),
                changed: true,
            }),
            UnixMillis::new(4),
        )
        .unwrap();

    let old_job = job(41);
    let old_request = AdmitRequestV1::new(
        DomainCommand::SessionComplete,
        IdempotencyKeyV1::new("old-session-job").unwrap(),
        old_job.clone(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(aggregate.revision()),
            aggregate.active_attempt_id().cloned(),
            None,
            None,
        )
        .unwrap(),
        digest('b'),
        UnixMillis::new(5),
    );
    store.admit(&identity(), old_request).unwrap();
    let old_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(6),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            old_claim.claim().clone(),
            aggregate.revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(7),
        )
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO operational_journal
             (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, details_json)
             VALUES (8, 'info', 'fixture', NULL, ?1, 'fixture', NULL)",
            [old_job.as_str()],
        )
        .unwrap();
    drop(connection);

    let failed_reset = job(42);
    let failed_reset_request = AdmitRequestV1::new(
        DomainCommand::SessionReset,
        IdempotencyKeyV1::new("failed-reset").unwrap(),
        failed_reset.clone(),
        RevisionAttemptItemPreconditionsV1::new(Some(aggregate.revision()), None, None, None)
            .unwrap(),
        digest('c'),
        UnixMillis::new(9),
    );
    store
        .admit(&identity(), failed_reset_request.clone())
        .unwrap();
    let failed_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(10),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            failed_claim.claim().clone(),
            aggregate.revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
            UnixMillis::new(11),
        )
        .unwrap();

    assert_eq!(
        store.read_session_aggregate(&identity()).unwrap(),
        Some(aggregate.clone())
    );
    let connection = Connection::open(&path).unwrap();
    let old_job_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE session_id = ?1",
            [aggregate.session_id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let old_receipt_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM idempotency_records
             WHERE scope_kind = 'session' AND scope_session_id = ?1",
            [aggregate.session_id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let old_journal_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM operational_journal WHERE job_id = ?1",
            [old_job.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        (old_job_count, old_receipt_count, old_journal_count),
        (1, 1, 1)
    );
    drop(connection);

    let reset_job = job(43);
    let reset_request = AdmitRequestV1::new(
        DomainCommand::SessionReset,
        IdempotencyKeyV1::new("successful-reset").unwrap(),
        reset_job,
        RevisionAttemptItemPreconditionsV1::new(Some(aggregate.revision()), None, None, None)
            .unwrap(),
        digest('d'),
        UnixMillis::new(12),
    );
    store.admit(&identity(), reset_request).unwrap();
    let reset_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("worker-a").unwrap(),
            UnixMillis::new(13),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            reset_claim.claim().clone(),
            aggregate.revision(),
            Some(
                StateTransitionV1::new_persisted(
                    None,
                    aggregate.revision(),
                    Revision::ZERO,
                    PersistedSessionMutationV1::Clear,
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: aggregate.session_id().clone(),
                revision_before: aggregate.revision(),
                revision_after: Revision::ZERO,
                changed: true,
            }),
            UnixMillis::new(14),
        )
        .unwrap();

    assert!(store.read_session_aggregate(&identity()).unwrap().is_none());
    let connection = Connection::open(path).unwrap();
    let remaining_old_rows: i64 = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM jobs WHERE session_id = ?1) +
                (SELECT COUNT(*) FROM idempotency_records
                 WHERE scope_kind = 'session' AND scope_session_id = ?1) +
                (SELECT COUNT(*) FROM operational_journal WHERE job_id = ?2) +
                (SELECT COUNT(*) FROM procedure_snapshots WHERE snapshot_id = ?3)",
            [
                aggregate.session_id().as_str(),
                old_job.as_str(),
                aggregate.snapshot().snapshot_id().as_str(),
            ],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining_old_rows, 0);
    assert_terminal_replay(
        store.admit(&identity(), failed_reset_request),
        terminal_with_job_projection(
            JobReceiptV1::new(3, failed_reset, digest('c')),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::InvalidState {
                reason: "fixture".to_owned(),
            }),
            PersistedTerminalJobStateV1::Failed,
            9,
            Some(10),
            11,
        ),
    );
}
#[test]
fn independent_sqlite_handles_claim_the_fifo_head_once() {
    for iteration in 0..CONCURRENCY_RACE_ITERATIONS {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("state.sqlite3");
        let seeded = store(&temporary);
        let fifo_head = job(60 + iteration * 2);
        let follower = job(61 + iteration * 2);
        admit(
            &seeded,
            fifo_head.clone(),
            &format!("concurrent-head-{iteration}"),
            'a',
            2,
        );
        admit(
            &seeded,
            follower,
            &format!("concurrent-follower-{iteration}"),
            'b',
            3,
        );
        drop(seeded);

        let contention_options = contention_store_options();
        let first_handle =
            store_with_options(&temporary, contention_options.clone(), UnixMillis::new(4));
        let second_handle =
            store_with_options(&temporary, contention_options.clone(), UnixMillis::new(4));
        let observer = store_with_options(&temporary, contention_options, UnixMillis::new(4));
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let start = Arc::new(Barrier::new(3));
        let retry = Arc::new(Barrier::new(3));
        let (busy_sender, busy_receiver) = mpsc::channel();

        let first_start = Arc::clone(&start);
        let first_retry = Arc::clone(&retry);
        let first_busy_sender = busy_sender.clone();
        let first_head = fifo_head.clone();
        let first = thread::spawn(move || {
            first_start.wait();
            let busy = match first_handle.claim_next(
                &identity(),
                WorkerIdV1::new(format!("claim-first-{iteration}")).unwrap(),
                UnixMillis::new(5),
            ) {
                Err(error) => Ok(error),
                outcome => Err(format!("unexpected first contention outcome: {outcome:?}")),
            };
            first_busy_sender.send(("first", busy)).unwrap();

            first_retry.wait();
            match first_handle
                .claim_next(
                    &identity(),
                    WorkerIdV1::new(format!("claim-first-{iteration}")).unwrap(),
                    UnixMillis::new(5),
                )
                .unwrap()
            {
                Some(claimed) => {
                    assert_eq!(claimed.job().job_id(), &first_head);
                    assert_eq!(
                        claimed.execution().command(),
                        &DomainCommand::WorkspaceInitialize
                    );
                    true
                }
                None => false,
            }
        });

        let second_start = Arc::clone(&start);
        let second_retry = Arc::clone(&retry);
        let second_busy_sender = busy_sender.clone();
        let second_head = fifo_head.clone();
        let second = thread::spawn(move || {
            second_start.wait();
            let busy = match second_handle.claim_next(
                &identity(),
                WorkerIdV1::new(format!("claim-second-{iteration}")).unwrap(),
                UnixMillis::new(5),
            ) {
                Err(error) => Ok(error),
                outcome => Err(format!("unexpected second contention outcome: {outcome:?}")),
            };
            second_busy_sender.send(("second", busy)).unwrap();

            second_retry.wait();
            match second_handle
                .claim_next(
                    &identity(),
                    WorkerIdV1::new(format!("claim-second-{iteration}")).unwrap(),
                    UnixMillis::new(5),
                )
                .unwrap()
            {
                Some(claimed) => {
                    assert_eq!(claimed.job().job_id(), &second_head);
                    assert_eq!(
                        claimed.execution().command(),
                        &DomainCommand::WorkspaceInitialize
                    );
                    true
                }
                None => false,
            }
        });
        drop(busy_sender);

        start.wait();
        let mut busy_contenders = Vec::with_capacity(2);
        for _ in 0..2 {
            let (contender, busy) = busy_receiver.recv().unwrap();
            assert!(matches!(
                busy.unwrap_or_else(|outcome| panic!("{outcome}")),
                StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Busy,
                }
            ));
            busy_contenders.push(contender);
        }
        busy_contenders.sort_unstable();
        assert_eq!(busy_contenders, vec!["first", "second"]);

        let queued_before_retry: i64 = blocker
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE state = 'queued'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let running_before_retry: i64 = blocker
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE state = 'running'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued_before_retry, 2);
        assert_eq!(running_before_retry, 0);
        blocker.execute_batch("ROLLBACK").unwrap();
        drop(blocker);

        retry.wait();
        let first_claimed = first.join().unwrap();
        let second_claimed = second.join().unwrap();
        assert_ne!(first_claimed, second_claimed);

        let durable = observer.read_workspace_view(&identity()).unwrap();
        assert_eq!(durable.queued_job_count(), 1);
        assert_eq!(durable.running_job_id(), Some(&fifo_head));
        assert!(
            observer
                .claim_next(
                    &identity(),
                    WorkerIdV1::new(format!("claim-observer-{iteration}")).unwrap(),
                    UnixMillis::new(6),
                )
                .unwrap()
                .is_none()
        );
        drop(observer);
        let recovered = store_with_options(
            &temporary,
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(7),
        );
        assert_eq!(recovered.startup_recovery_report().requeued_job_count(), 1);
        let recovered_view = recovered.read_workspace_view(&identity()).unwrap();
        assert_eq!(recovered_view.queued_job_count(), 2);
        assert!(recovered_view.running_job_id().is_none());
        let recovered_claim = recovered
            .claim_next(
                &identity(),
                WorkerIdV1::new(format!("claim-recovered-{iteration}")).unwrap(),
                UnixMillis::new(8),
            )
            .unwrap()
            .unwrap();
        assert_eq!(recovered_claim.job().job_id(), &fifo_head);
    }
}

#[test]
fn session_barrier_cancellation_serializes_later_session_admission() {
    for iteration in 0..CONCURRENCY_RACE_ITERATIONS {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("state.sqlite3");
        let aggregate = aggregate();
        let seeded = store(&temporary);
        seed_current_session(
            &seeded,
            &aggregate,
            job(80 + iteration * 3),
            &format!("cancel-seed-{iteration}"),
            2,
        );

        let barrier_job = job(81 + iteration * 3);
        let barrier_request = session_reset_request(
            &aggregate,
            barrier_job.clone(),
            &format!("cancel-barrier-{iteration}"),
            'b',
            5,
        );
        assert!(matches!(
            seeded.admit(&identity(), barrier_request.clone()),
            Ok(AdmitOutcomeV1::New(_))
        ));
        drop(seeded);

        let contention_options = contention_store_options();
        let cancellation_handle =
            store_with_options(&temporary, contention_options.clone(), UnixMillis::new(6));
        let admission_handle =
            store_with_options(&temporary, contention_options.clone(), UnixMillis::new(6));
        let observer = store_with_options(&temporary, contention_options, UnixMillis::new(6));
        let later_job = job(82 + iteration * 3);
        let later_request = session_complete_request(
            &aggregate,
            later_job.clone(),
            &format!("cancel-later-{iteration}"),
            'c',
            6,
        );
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let start = Arc::new(Barrier::new(3));
        let retry = Arc::new(Barrier::new(3));
        let (busy_sender, busy_receiver) = mpsc::channel();

        let cancellation_start = Arc::clone(&start);
        let cancellation_retry = Arc::clone(&retry);
        let cancellation_busy_sender = busy_sender.clone();
        let cancellation_barrier_job = barrier_job.clone();
        let cancellation = thread::spawn(move || {
            cancellation_start.wait();
            let busy = match cancellation_handle.cancel_before_claim(
                &identity(),
                cancellation_barrier_job.clone(),
                Revision::new(2),
                UnixMillis::new(7),
            ) {
                Err(error) => Ok(error),
                outcome => Err(format!(
                    "unexpected barrier-cancellation contention outcome: {outcome:?}"
                )),
            };
            cancellation_busy_sender
                .send(("cancellation", busy))
                .unwrap();

            cancellation_retry.wait();
            match cancellation_handle.cancel_before_claim(
                &identity(),
                cancellation_barrier_job.clone(),
                Revision::new(2),
                UnixMillis::new(7),
            ) {
                Ok(CancelOutcomeV1::Cancelled(receipt)) => {
                    assert_eq!(receipt.identity_sequence(), 2);
                    assert_eq!(receipt.job_id(), &cancellation_barrier_job);
                }
                outcome => panic!("unexpected barrier-cancellation retry outcome: {outcome:?}"),
            }
        });

        let admission_start = Arc::clone(&start);
        let admission_retry = Arc::clone(&retry);
        let admission_busy_sender = busy_sender.clone();
        let admitted_later_job = later_job.clone();
        let admission = thread::spawn(move || {
            admission_start.wait();
            let busy = match admission_handle.admit(&identity(), later_request.clone()) {
                Err(error) => Ok(error),
                outcome => Err(format!(
                    "unexpected later-session admission contention outcome: {outcome:?}"
                )),
            };
            admission_busy_sender.send(("admission", busy)).unwrap();

            admission_retry.wait();
            match admission_handle.admit(&identity(), later_request) {
                Ok(AdmitOutcomeV1::New(receipt)) => {
                    assert_eq!(receipt.job_id(), &admitted_later_job);
                    SessionBarrierAdmissionOutcome::AdmittedAfterCancellation
                }
                Err(StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Busy,
                }) => SessionBarrierAdmissionOutcome::BlockedByBarrier,
                outcome => panic!("unexpected later-session admission retry outcome: {outcome:?}"),
            }
        });
        drop(busy_sender);

        start.wait();
        let mut busy_contenders = Vec::with_capacity(2);
        for _ in 0..2 {
            let (contender, busy) = busy_receiver.recv().unwrap();
            assert!(matches!(
                busy.unwrap_or_else(|outcome| panic!("{outcome}")),
                StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Busy,
                }
            ));
            busy_contenders.push(contender);
        }
        busy_contenders.sort_unstable();
        assert_eq!(busy_contenders, vec!["admission", "cancellation"]);

        let queued_before_retry: i64 = blocker
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE state = 'queued'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let barrier_state: String = blocker
            .query_row(
                "SELECT state FROM jobs WHERE job_id = ?1",
                [barrier_job.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let later_rows_before_retry: i64 = blocker
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE job_id = ?1",
                [later_job.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued_before_retry, 1);
        assert_eq!(barrier_state, "queued");
        assert_eq!(later_rows_before_retry, 0);
        blocker.execute_batch("ROLLBACK").unwrap();
        drop(blocker);

        retry.wait();
        cancellation.join().unwrap();
        let admission = admission.join().unwrap();
        assert!(matches!(
            admission,
            SessionBarrierAdmissionOutcome::AdmittedAfterCancellation
                | SessionBarrierAdmissionOutcome::BlockedByBarrier
        ));

        let durable = observer.read_workspace_view(&identity()).unwrap();
        assert_eq!(
            durable.queued_job_count(),
            u32::from(admission == SessionBarrierAdmissionOutcome::AdmittedAfterCancellation)
        );
        assert!(durable.running_job_id().is_none());
        assert_eq!(
            observer.read_session_aggregate(&identity()).unwrap(),
            Some(aggregate)
        );
        assert_terminal_replay(
            observer.admit(&identity(), barrier_request),
            terminal_with_job_projection(
                JobReceiptV1::new(2, barrier_job, digest('b')),
                PersistedTerminalResultV1::Cancelled,
                PersistedTerminalJobStateV1::Cancelled,
                5,
                None,
                7,
            ),
        );
    }
}

#[test]
fn session_barrier_cleanup_serializes_later_session_admission() {
    for iteration in 0..CONCURRENCY_RACE_ITERATIONS {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("state.sqlite3");
        let aggregate = aggregate();
        let seeded = store(&temporary);
        seed_current_session(
            &seeded,
            &aggregate,
            job(100 + iteration * 4),
            &format!("cleanup-seed-{iteration}"),
            2,
        );

        let old_session_job = job(101 + iteration * 4);
        let old_session_request = session_complete_request(
            &aggregate,
            old_session_job.clone(),
            &format!("cleanup-old-{iteration}"),
            'b',
            5,
        );
        assert!(matches!(
            seeded.admit(&identity(), old_session_request),
            Ok(AdmitOutcomeV1::New(_))
        ));
        let old_claim = seeded
            .claim_next(
                &identity(),
                WorkerIdV1::new(format!("cleanup-old-worker-{iteration}")).unwrap(),
                UnixMillis::new(6),
            )
            .unwrap()
            .unwrap();
        seeded
            .commit_terminal(
                old_claim.claim().clone(),
                aggregate.revision(),
                None,
                TerminalResultV1::Failure(DomainError::InvalidState { reason: "fixture" }),
                UnixMillis::new(7),
            )
            .unwrap();

        let barrier_job = job(102 + iteration * 4);
        let barrier_request = session_reset_request(
            &aggregate,
            barrier_job.clone(),
            &format!("cleanup-barrier-{iteration}"),
            'c',
            8,
        );
        assert!(matches!(
            seeded.admit(&identity(), barrier_request.clone()),
            Ok(AdmitOutcomeV1::New(_))
        ));
        drop(seeded);

        let contention_options = contention_store_options();
        let committer =
            store_with_options(&temporary, contention_options.clone(), UnixMillis::new(9));
        let admission_handle =
            store_with_options(&temporary, contention_options.clone(), UnixMillis::new(9));
        let observer = store_with_options(&temporary, contention_options, UnixMillis::new(9));
        let barrier_claim = committer
            .claim_next(
                &identity(),
                WorkerIdV1::new(format!("cleanup-barrier-worker-{iteration}")).unwrap(),
                UnixMillis::new(10),
            )
            .unwrap()
            .unwrap();
        assert_eq!(barrier_claim.job().job_id(), &barrier_job);

        let expected_revision = aggregate.revision();
        let later_job = job(103 + iteration * 4);
        let later_request = session_complete_request(
            &aggregate,
            later_job.clone(),
            &format!("cleanup-later-{iteration}"),
            'd',
            10,
        );
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let start = Arc::new(Barrier::new(3));
        let retry = Arc::new(Barrier::new(3));
        let (busy_sender, busy_receiver) = mpsc::channel();

        let commit_start = Arc::clone(&start);
        let commit_retry = Arc::clone(&retry);
        let commit_busy_sender = busy_sender.clone();
        let commit_aggregate = aggregate.clone();
        let committed_barrier_job = barrier_job.clone();
        let commit = thread::spawn(move || {
            let terminal_result = TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: commit_aggregate.session_id().clone(),
                revision_before: commit_aggregate.revision(),
                revision_after: Revision::ZERO,
                changed: true,
            });
            commit_start.wait();
            let busy = match committer.commit_terminal(
                barrier_claim.claim().clone(),
                commit_aggregate.revision(),
                Some(
                    StateTransitionV1::new_persisted(
                        None,
                        commit_aggregate.revision(),
                        Revision::ZERO,
                        PersistedSessionMutationV1::Clear,
                    )
                    .unwrap(),
                ),
                terminal_result.clone(),
                UnixMillis::new(11),
            ) {
                Err(error) => Ok(error),
                outcome => Err(format!(
                    "unexpected barrier-cleanup contention outcome: {outcome:?}"
                )),
            };
            commit_busy_sender.send(("commit", busy)).unwrap();

            commit_retry.wait();
            let receipt = committer
                .commit_terminal(
                    barrier_claim.claim().clone(),
                    commit_aggregate.revision(),
                    Some(
                        StateTransitionV1::new_persisted(
                            None,
                            commit_aggregate.revision(),
                            Revision::ZERO,
                            PersistedSessionMutationV1::Clear,
                        )
                        .unwrap(),
                    ),
                    terminal_result.clone(),
                    UnixMillis::new(11),
                )
                .unwrap();
            assert_eq!(receipt.job().job_id(), &committed_barrier_job);
            assert_eq!(receipt.result(), &terminal_result);
        });

        let admission_start = Arc::clone(&start);
        let admission_retry = Arc::clone(&retry);
        let admission_busy_sender = busy_sender.clone();
        let admission = thread::spawn(move || {
            admission_start.wait();
            let busy = match admission_handle.admit(&identity(), later_request.clone()) {
                Err(error) => Ok(error),
                outcome => Err(format!(
                    "unexpected post-cleanup admission contention outcome: {outcome:?}"
                )),
            };
            admission_busy_sender.send(("admission", busy)).unwrap();

            admission_retry.wait();
            match admission_handle.admit(&identity(), later_request) {
                Err(StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Busy,
                }) => SessionBarrierAdmissionOutcome::BlockedByBarrier,
                Err(StoreErrorV1::PreconditionConflictV1 { expected, actual }) => {
                    assert_eq!(expected, Some(expected_revision));
                    assert_eq!(actual, None);
                    SessionBarrierAdmissionOutcome::RejectedAfterCleanup
                }
                outcome => panic!("unexpected post-cleanup admission retry outcome: {outcome:?}"),
            }
        });
        drop(busy_sender);

        start.wait();
        let mut busy_contenders = Vec::with_capacity(2);
        for _ in 0..2 {
            let (contender, busy) = busy_receiver.recv().unwrap();
            assert!(matches!(
                busy.unwrap_or_else(|outcome| panic!("{outcome}")),
                StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Busy,
                }
            ));
            busy_contenders.push(contender);
        }
        busy_contenders.sort_unstable();
        assert_eq!(busy_contenders, vec!["admission", "commit"]);

        let queued_before_retry: i64 = blocker
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE state = 'queued'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let barrier_state: String = blocker
            .query_row(
                "SELECT state FROM jobs WHERE job_id = ?1",
                [barrier_job.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let later_rows_before_retry: i64 = blocker
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE job_id = ?1",
                [later_job.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued_before_retry, 0);
        assert_eq!(barrier_state, "running");
        assert_eq!(later_rows_before_retry, 0);
        blocker.execute_batch("ROLLBACK").unwrap();
        drop(blocker);

        retry.wait();
        commit.join().unwrap();
        let admission = admission.join().unwrap();
        assert!(matches!(
            admission,
            SessionBarrierAdmissionOutcome::BlockedByBarrier
                | SessionBarrierAdmissionOutcome::RejectedAfterCleanup
        ));

        let durable = observer.read_workspace_view(&identity()).unwrap();
        assert_eq!(durable.queued_job_count(), 0);
        assert!(durable.running_job_id().is_none());
        assert!(
            observer
                .read_session_aggregate(&identity())
                .unwrap()
                .is_none()
        );
        assert_terminal_replay(
            observer.admit(&identity(), barrier_request),
            terminal_with_job_projection(
                JobReceiptV1::new(3, barrier_job.clone(), digest('c')),
                PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                    session_id: aggregate.session_id().clone(),
                    revision_before: aggregate.revision(),
                    revision_after: Revision::ZERO,
                    changed: true,
                }),
                PersistedTerminalJobStateV1::Succeeded,
                8,
                Some(10),
                11,
            ),
        );

        let connection = Connection::open(path).unwrap();
        let old_session_rows: i64 = connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM jobs WHERE session_id = ?1) +
                    (SELECT COUNT(*) FROM idempotency_records
                     WHERE scope_kind = 'session' AND scope_session_id = ?1)",
                [aggregate.session_id().as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let cleanup_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM operational_journal
                 WHERE event_name = 'session.barrier.cleanup' AND job_id = ?1",
                [barrier_job.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let (cleanup_summary, cleanup_details): (String, Option<String>) = connection
            .query_row(
                "SELECT summary, details_json FROM operational_journal
                 WHERE event_name = 'session.barrier.cleanup' AND job_id = ?1",
                [barrier_job.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(old_session_rows, 0);
        assert_eq!(cleanup_events, 1);
        assert_eq!(
            cleanup_summary,
            "journal_entries_deleted=0; terminal_jobs_deleted=1; idempotency_records_deleted=1; snapshots_deleted=1"
        );
        assert_eq!(cleanup_details, None);
    }
}

#[test]
fn coherent_workspace_and_job_reads_return_bounded_sequence_ordered_terminal_facts() {
    let temporary = TempDir::new().unwrap();
    let store = store(&temporary);
    let first = job(240);
    let second = job(241);
    let first_request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("coherent-first").unwrap(),
        first.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('a'),
        UnixMillis::new(2),
    );
    let second_request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("coherent-second").unwrap(),
        second.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('b'),
        UnixMillis::new(3),
    );
    assert!(matches!(
        store.admit(&identity(), first_request.clone()),
        Ok(AdmitOutcomeV1::New(_))
    ));
    assert!(matches!(
        store.admit(&identity(), second_request),
        Ok(AdmitOutcomeV1::New(_))
    ));

    let initial_view = store.read_workspace_view(&identity()).unwrap();
    assert_eq!(initial_view.identity(), &identity());
    assert!(initial_view.current_session().is_none());
    assert_eq!(initial_view.queued_job_count(), 2);
    assert_eq!(initial_view.running_job_id(), None);
    assert_eq!(initial_view.latest_workspace_sequence(), 2);
    assert_eq!(initial_view.observed_at(), UnixMillis::new(3));

    let first_page = store
        .list_jobs(&identity(), JobListQueryV1::new(1).unwrap())
        .unwrap();
    assert_eq!(first_page.len(), 1);
    assert_eq!(first_page[0].job().job_id(), &first);
    assert_eq!(first_page[0].state(), JobStateV1::Queued);
    assert_eq!(first_page[0].submitted_at(), UnixMillis::new(2));
    assert_eq!(first_page[0].claimed_at(), None);
    assert_eq!(first_page[0].finished_at(), None);
    assert_eq!(first_page[0].terminal_receipt(), None);

    let cancelled = store
        .cancel_before_claim(
            &identity(),
            first.clone(),
            Revision::new(1),
            UnixMillis::new(4),
        )
        .unwrap();
    assert!(matches!(cancelled, CancelOutcomeV1::Cancelled(_)));

    let terminal = store
        .read_job(&identity(), &first)
        .unwrap()
        .expect("terminal job must remain readable");
    let expected_terminal = PersistedTerminalReceiptV1::new_with_projections(
        JobReceiptV1::new(1, first.clone(), digest('a')),
        PersistedTerminalResultV1::Cancelled,
        podway_store::PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Cancelled,
            UnixMillis::new(2),
            None,
            UnixMillis::new(4),
        )
        .unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(terminal.state(), JobStateV1::Cancelled);
    assert_eq!(terminal.finished_at(), Some(UnixMillis::new(4)));
    assert_eq!(terminal.terminal_receipt(), Some(&expected_terminal));

    let jobs = store
        .list_jobs(&identity(), JobListQueryV1::new(2).unwrap())
        .unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].job().job_id(), &first);
    assert_eq!(jobs[1].job().job_id(), &second);
    assert_eq!(jobs[0].state(), JobStateV1::Cancelled);
    assert_eq!(jobs[1].state(), JobStateV1::Queued);
    assert!(JobListQueryV1::new(0).is_err());
    assert!(JobListQueryV1::new(1_001).is_err());

    let final_view = store.read_workspace_view(&identity()).unwrap();
    assert!(final_view.current_session().is_none());
    assert_eq!(final_view.queued_job_count(), 1);
    assert_eq!(final_view.running_job_id(), None);
    assert_eq!(final_view.latest_workspace_sequence(), 2);
    assert_eq!(final_view.observed_at(), UnixMillis::new(4));
    assert_terminal_replay(store.admit(&identity(), first_request), expected_terminal);
}
#[test]
fn fresh_session_start_replace_retires_old_history_and_replays_its_terminal_receipt() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let store = store(&temporary);
    let initial = aggregate();
    seed_current_session(&store, &initial, job(220), "fresh-replacement-seed", 2);
    let cancelled = apply_transition_v1(
        Some(&initial),
        &SessionCommandV1::Cancel(CancelSessionV1 {
            expected_attempt_id: initial.active_attempt_id().cloned().unwrap(),
            reason: "fixture".to_owned(),
        }),
        CommandContextV1 {
            expected_revision: initial.revision(),
            now: UnixMillis::new(12),
        },
    )
    .unwrap()
    .next_aggregate()
    .cloned()
    .unwrap();
    let old_job = job(221);
    let old_request = AdmitRequestV1::new(
        DomainCommand::SessionCancel,
        IdempotencyKeyV1::new("fresh-replacement-old-history").unwrap(),
        old_job.clone(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(initial.revision()),
            initial.active_attempt_id().cloned(),
            None,
            None,
        )
        .unwrap(),
        digest('b'),
        UnixMillis::new(13),
    );
    store.admit(&identity(), old_request).unwrap();
    let old_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("fresh-replacement-worker").unwrap(),
            UnixMillis::new(14),
        )
        .unwrap()
        .unwrap();
    store
        .commit_terminal(
            old_claim.claim().clone(),
            initial.revision(),
            Some(
                StateTransitionV1::new_persisted(
                    Some(cancelled.session_id().clone()),
                    initial.revision(),
                    cancelled.revision(),
                    PersistedSessionMutationV1::Replace(cancelled.clone()),
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: cancelled.session_id().clone(),
                revision_before: initial.revision(),
                revision_after: cancelled.revision(),
                changed: true,
            }),
            UnixMillis::new(15),
        )
        .unwrap();

    let replacement_snapshot =
        ProcedureSnapshotV1::from_canonical_json(CanonicalProcedureSnapshotInputV1 {
            snapshot_id: ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000225").unwrap(),
            schema_id: "podway.procedure/v1".to_owned(),
            procedure_id: initial.snapshot().procedure_id().to_owned(),
            procedure_version: initial.snapshot().procedure_version().to_owned(),
            name: initial.snapshot().name().to_owned(),
            source_label: initial.snapshot().source_label().clone(),
            canonical_json: initial.snapshot().canonical_json().clone(),
            digest: initial.snapshot().digest().clone(),
            created_at: UnixMillis::new(16),
        })
        .unwrap();
    let replacement = SessionAggregateV1::start(
        SessionId::new("00000000-0000-4000-8000-000000000223").unwrap(),
        "Fresh replacement",
        replacement_snapshot,
        AttemptId::new("00000000-0000-4000-8000-000000000224").unwrap(),
        UnixMillis::new(16),
    )
    .unwrap();
    let replacement_job = job(222);
    let replacement_request = AdmitRequestV1::new(
        DomainCommand::SessionStartReplace,
        IdempotencyKeyV1::new("fresh-replacement").unwrap(),
        replacement_job.clone(),
        RevisionAttemptItemPreconditionsV1::new(Some(cancelled.revision()), None, None, None)
            .unwrap(),
        digest('c'),
        UnixMillis::new(16),
    );
    store
        .admit(&identity(), replacement_request.clone())
        .unwrap();
    let replacement_claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("fresh-replacement-worker").unwrap(),
            UnixMillis::new(17),
        )
        .unwrap()
        .unwrap();
    let terminal_result = TerminalResultV1::Success(DomainResult::SessionChanged {
        session_id: replacement.session_id().clone(),
        revision_before: cancelled.revision(),
        revision_after: replacement.revision(),
        changed: true,
    });
    let receipt = store
        .commit_terminal(
            replacement_claim.claim().clone(),
            cancelled.revision(),
            Some(
                StateTransitionV1::new_persisted(
                    Some(replacement.session_id().clone()),
                    cancelled.revision(),
                    replacement.revision(),
                    PersistedSessionMutationV1::ReplaceFresh(replacement.clone()),
                )
                .unwrap(),
            ),
            terminal_result,
            UnixMillis::new(18),
        )
        .unwrap();
    assert_eq!(
        receipt.result(),
        &TerminalResultV1::Success(DomainResult::SessionChanged {
            session_id: replacement.session_id().clone(),
            revision_before: cancelled.revision(),
            revision_after: Revision::new(1),
            changed: true,
        })
    );
    assert_eq!(
        store.read_session_aggregate(&identity()).unwrap(),
        Some(replacement.clone())
    );

    let connection = Connection::open(path).unwrap();
    let retired_old_history: i64 = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM jobs
                 WHERE session_id = ?1 AND job_id != ?2) +
                (SELECT COUNT(*) FROM idempotency_records
                 WHERE scope_kind = 'session'
                   AND scope_session_id = ?1
                   AND job_id != ?2) +
                (SELECT COUNT(*) FROM operational_journal WHERE job_id = ?3) +
                (SELECT COUNT(*) FROM procedure_snapshots WHERE snapshot_id = ?4)",
            [
                cancelled.session_id().as_str(),
                replacement_job.as_str(),
                old_job.as_str(),
                initial.snapshot().snapshot_id().as_str(),
            ],
            |row| row.get(0),
        )
        .unwrap();
    let retained_terminal_rows: i64 = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM jobs WHERE job_id = ?1) +
                (SELECT COUNT(*) FROM idempotency_records WHERE job_id = ?1)",
            [replacement_job.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired_old_history, 0);
    assert_eq!(retained_terminal_rows, 2);
    drop(connection);

    assert_terminal_replay(
        store.admit(&identity(), replacement_request),
        PersistedTerminalReceiptV1::new_with_projections(
            JobReceiptV1::new(3, replacement_job, digest('c')),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                session_id: replacement.session_id().clone(),
                revision_before: cancelled.revision(),
                revision_after: replacement.revision(),
                changed: true,
            }),
            PersistedTerminalJobProjectionV1::new(
                PersistedTerminalJobStateV1::Succeeded,
                UnixMillis::new(16),
                Some(UnixMillis::new(17)),
                UnixMillis::new(18),
            )
            .unwrap(),
            Some(
                PersistedTerminalSessionProjectionV1::new(
                    replacement.session_id().clone(),
                    replacement.task_title().to_owned(),
                    replacement.lifecycle().into(),
                    cancelled.revision(),
                    replacement.revision(),
                )
                .unwrap(),
            ),
        )
        .unwrap(),
    );
}

#[test]
fn fresh_session_start_replace_invalid_shapes_fail_closed() {
    let replacement = aggregate();
    assert!(
        StateTransitionV1::new_persisted(
            Some(replacement.session_id().clone()),
            Revision::new(2),
            replacement.revision(),
            PersistedSessionMutationV1::Replace(replacement.clone()),
        )
        .is_err()
    );
    assert!(
        StateTransitionV1::new_persisted(
            Some(replacement.session_id().clone()),
            Revision::new(2),
            Revision::new(2),
            PersistedSessionMutationV1::ReplaceFresh(replacement.clone()),
        )
        .is_err()
    );
    assert!(
        StateTransitionV1::new_persisted(
            Some(SessionId::new("00000000-0000-4000-8000-000000000299").unwrap()),
            Revision::new(2),
            replacement.revision(),
            PersistedSessionMutationV1::ReplaceFresh(replacement.clone()),
        )
        .is_err()
    );
    assert!(
        PersistedTerminalSessionProjectionV1::new(
            replacement.session_id().clone(),
            replacement.task_title().to_owned(),
            replacement.lifecycle().into(),
            Revision::new(2),
            Revision::ZERO,
        )
        .is_err()
    );

    for invalid_shape in [
        FreshReplacementInvalidShape::SameSession,
        FreshReplacementInvalidShape::OtherCommand,
        FreshReplacementInvalidShape::StaleRevision,
    ] {
        let temporary = TempDir::new().unwrap();
        let store = store(&temporary);
        let current = aggregate();
        seed_current_session(
            &store,
            &current,
            job(230),
            &format!("fresh-replacement-invalid-{invalid_shape:?}"),
            2,
        );
        let replacement = SessionAggregateV1::start(
            SessionId::new("00000000-0000-4000-8000-000000000232").unwrap(),
            "Replacement",
            current.snapshot().clone(),
            AttemptId::new("00000000-0000-4000-8000-000000000233").unwrap(),
            UnixMillis::new(12),
        )
        .unwrap();
        let (
            command,
            next,
            result_session_id,
            expected_revision,
            request_job,
            request_key,
            request_digest,
        ) = match invalid_shape {
            FreshReplacementInvalidShape::SameSession => (
                DomainCommand::SessionStartReplace,
                current.clone(),
                current.session_id().clone(),
                current.revision(),
                job(231),
                "fresh-replacement-invalid-same-session-request",
                'd',
            ),
            FreshReplacementInvalidShape::OtherCommand => (
                DomainCommand::SessionComplete,
                replacement.clone(),
                replacement.session_id().clone(),
                current.revision(),
                job(232),
                "fresh-replacement-invalid-other-command-request",
                'e',
            ),
            FreshReplacementInvalidShape::StaleRevision => (
                DomainCommand::SessionStartReplace,
                replacement.clone(),
                replacement.session_id().clone(),
                Revision::ZERO,
                job(233),
                "fresh-replacement-invalid-stale-revision-request",
                'f',
            ),
        };
        let request = AdmitRequestV1::new(
            command,
            IdempotencyKeyV1::new(request_key).unwrap(),
            request_job,
            RevisionAttemptItemPreconditionsV1::new(Some(current.revision()), None, None, None)
                .unwrap(),
            digest(request_digest),
            UnixMillis::new(13),
        );
        store.admit(&identity(), request).unwrap();
        let claim = store
            .claim_next(
                &identity(),
                WorkerIdV1::new("fresh-replacement-invalid-worker").unwrap(),
                UnixMillis::new(14),
            )
            .unwrap()
            .unwrap();
        let result = store.commit_terminal(
            claim.claim().clone(),
            expected_revision,
            Some(
                StateTransitionV1::new_persisted(
                    Some(next.session_id().clone()),
                    current.revision(),
                    next.revision(),
                    PersistedSessionMutationV1::ReplaceFresh(next),
                )
                .unwrap(),
            ),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: result_session_id,
                revision_before: current.revision(),
                revision_after: Revision::new(1),
                changed: true,
            }),
            UnixMillis::new(15),
        );
        match invalid_shape {
            FreshReplacementInvalidShape::StaleRevision => {
                assert!(matches!(
                    result,
                    Err(StoreErrorV1::PreconditionConflictV1 { .. })
                ));
            }
            FreshReplacementInvalidShape::SameSession => {
                assert!(matches!(
                    result,
                    Err(StoreErrorV1::InternalInvariantViolationV1 {
                        invariant: StoreInvariantV1::TransitionMutationShape
                    })
                ));
            }
            FreshReplacementInvalidShape::OtherCommand => {
                assert!(matches!(
                    result,
                    Err(StoreErrorV1::CorruptStateV1 {
                        record: StoreRecordKindV1::Job
                    })
                ));
            }
        }
        assert_eq!(
            store.read_session_aggregate(&identity()).unwrap(),
            Some(current)
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum FreshReplacementInvalidShape {
    SameSession,
    OtherCommand,
    StaleRevision,
}
