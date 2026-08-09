//! Procedure v2 graph/action persistence across transactions and process reopen.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

use podway_core::{
    AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, CanonicalProcedureJsonV1,
    DomainCommand, GraphNodeId, JobId, ProcedureSnapshotId, ProcedureSourceLabelV1, ReasonV2,
    Revision, ReworkKindV2, ReworkRecordInputV2, ReworkRecordV2, SessionAttemptV2, SessionId,
    SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis, WorkspaceId,
    canonicalize_json_v1,
};
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, AttemptMetadataV2, AttemptWorkflowMemoryV2,
    DurableWorktreeIdentityV1, GraphNodeCounterV2, GraphSessionStateV2, IdempotencyKeyV1,
    ProcedureSnapshotV2, RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1,
    StoreContractV1, StoreErrorV1, StoreFailpointV1, StoreGraphReadContractV2,
    StoreGraphStateContractV2, StoreIntegrityCheckV1, StoreRecordKindV1, StoreUnavailableReasonV1,
    ValidatedWorkspaceRootV1, WorkerIdV1, WorkflowMemoryStateV2,
};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
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

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-v2-graph-state")).unwrap()
}

fn options() -> SqliteStoreOptionsV1 {
    SqliteStoreOptionsV1::new(8).unwrap()
}

fn database_path(temporary: &TempDir) -> PathBuf {
    temporary.path().join("state.sqlite3")
}

fn database_artifacts(temporary: &TempDir) -> Vec<Option<Vec<u8>>> {
    ["", "-wal", "-shm"]
        .into_iter()
        .map(|suffix| {
            let mut path = database_path(temporary).into_os_string();
            path.push(suffix);
            match fs::read(PathBuf::from(path)) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => panic!("database artifact must be readable: {error}"),
            }
        })
        .collect()
}

fn open(temporary: &TempDir, options: SqliteStoreOptionsV1) -> SqliteStoreV1 {
    SqliteStoreV1::open(
        database_path(temporary),
        &root(),
        identity(),
        options,
        UnixMillis::new(1),
    )
    .unwrap()
}

fn attempt_id(number: u64) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap()
}

fn job_id(number: u64) -> JobId {
    JobId::new(format!("00000000-0000-4000-9000-{number:012x}")).unwrap()
}

fn node(value: &str) -> GraphNodeId {
    GraphNodeId::new(value).unwrap()
}

fn snapshot() -> ProcedureSnapshotV2 {
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "persist-graph",
        "version": "1",
        "name": "Persist graph",
        "purpose": "Prove graph state survives reopen.",
        "node_definitions": {
            "draft": {"type": "action", "title": "Draft", "intent": "Draft."},
            "review": {"type": "action", "title": "Review", "intent": "Review."}
        },
        "graph": {
            "entry": "draft",
            "nodes": [
                {"id": "draft", "use": "draft", "next": "review"},
                {"id": "review", "use": "review", "terminal": true}
            ]
        },
        "manual_rework": {"allowed_targets": ["draft"]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000010").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("procedure.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

fn attempt(
    number: u64,
    graph_node: &str,
    node_attempt: u64,
    trace: u64,
    lifecycle: AttemptLifecycle,
    validity: AttemptValidityV2,
) -> SessionAttemptV2 {
    SessionAttemptV2::new(
        attempt_id(number),
        node(graph_node),
        AttemptNumberV2::new(node_attempt),
        TraceSequenceV2::new(trace),
        lifecycle,
        validity,
        None,
    )
    .unwrap()
}

fn initial_state() -> GraphSessionStateV2 {
    let trace = SessionTraceV2::from_parts(
        SessionId::new("00000000-0000-4000-8000-000000000020").unwrap(),
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt(
            1,
            "draft",
            1,
            1,
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
        )],
    )
    .unwrap();
    GraphSessionStateV2::new(
        Revision::new(1),
        "Persist this task",
        snapshot(),
        trace,
        vec![
            GraphNodeCounterV2::new(node("draft"), 1, 0),
            GraphNodeCounterV2::new(node("review"), 0, 0),
        ],
        vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap()
}

fn advanced_state() -> GraphSessionStateV2 {
    let trace = SessionTraceV2::from_parts(
        SessionId::new("00000000-0000-4000-8000-000000000020").unwrap(),
        SessionLifecycle::Running,
        Revision::new(2),
        vec![
            attempt(
                1,
                "draft",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
            ),
            attempt(
                2,
                "review",
                1,
                2,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            ),
        ],
    )
    .unwrap();
    GraphSessionStateV2::new(
        Revision::new(2),
        "Persist this task",
        snapshot(),
        trace,
        vec![
            GraphNodeCounterV2::new(node("draft"), 1, 0),
            GraphNodeCounterV2::new(node("review"), 1, 0),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(20)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(2), UnixMillis::new(20), None, None).unwrap(),
        ],
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap()
}

fn reworked_state() -> GraphSessionStateV2 {
    let trace = SessionTraceV2::from_parts(
        SessionId::new("00000000-0000-4000-8000-000000000020").unwrap(),
        SessionLifecycle::Running,
        Revision::new(3),
        vec![
            attempt(
                1,
                "draft",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
            ),
            attempt(
                2,
                "review",
                1,
                2,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
            ),
            attempt(
                3,
                "draft",
                2,
                3,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            ),
        ],
    )
    .unwrap();
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(3),
        "Persist this task",
        snapshot(),
        trace,
        vec![
            GraphNodeCounterV2::new(node("draft"), 2, 1),
            GraphNodeCounterV2::new(node("review"), 1, 0),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(20)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                attempt_id(2),
                UnixMillis::new(20),
                Some(UnixMillis::new(30)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(3), UnixMillis::new(30), None, None).unwrap(),
        ],
        workflow_memory(3),
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap()
}

fn completed_state() -> GraphSessionStateV2 {
    let trace = SessionTraceV2::from_parts(
        SessionId::new("00000000-0000-4000-8000-000000000020").unwrap(),
        SessionLifecycle::Completed,
        Revision::new(4),
        vec![
            attempt(
                1,
                "draft",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
            ),
            attempt(
                2,
                "review",
                1,
                2,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
            ),
            attempt(
                3,
                "draft",
                2,
                3,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
            ),
        ],
    )
    .unwrap();
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(4),
        "Persist this task",
        snapshot(),
        trace,
        vec![
            GraphNodeCounterV2::new(node("draft"), 2, 1),
            GraphNodeCounterV2::new(node("review"), 1, 0),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(20)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                attempt_id(2),
                UnixMillis::new(20),
                Some(UnixMillis::new(30)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                attempt_id(3),
                UnixMillis::new(30),
                Some(UnixMillis::new(40)),
                None,
            )
            .unwrap(),
        ],
        workflow_memory(3),
        UnixMillis::new(10),
        Some(UnixMillis::new(40)),
        None,
        None,
    )
    .unwrap()
}

fn workflow_memory(attempt_count: u64) -> WorkflowMemoryStateV2 {
    let attempts = (1..=attempt_count)
        .map(|number| {
            AttemptWorkflowMemoryV2::new(attempt_id(number), Vec::new(), Vec::new(), Vec::new())
                .unwrap()
        })
        .collect();
    let rework = ReworkRecordV2::new(ReworkRecordInputV2 {
        trace: TraceSequenceV2::new(3),
        kind: ReworkKindV2::Manual,
        from_node: node("review"),
        to_node: node("draft"),
        target_attempt_id: attempt_id(3),
        reason: ReasonV2::new("Rework the draft.").unwrap(),
        reactivated: false,
        actor: None,
        recorded_at: UnixMillis::new(30),
    })
    .unwrap();
    WorkflowMemoryStateV2::new(attempts, Vec::new(), vec![rework]).unwrap()
}

#[test]
fn graph_state_round_trips_successors_and_rework_across_reopen() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    let initial = initial_state();
    store
        .create_graph_session_v2(&identity(), initial.clone())
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(initial)
    );

    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            advanced_state(),
        )
        .unwrap();
    drop(store);
    let reopened = open(&temporary, options());
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(advanced_state())
    );

    reopened
        .replace_graph_session_v2(
            &identity(),
            Revision::new(2),
            Revision::new(2),
            reworked_state(),
        )
        .unwrap();
    drop(reopened);
    let reopened = open(&temporary, options());
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(reworked_state())
    );
    reopened
        .replace_graph_session_v2(
            &identity(),
            Revision::new(3),
            Revision::new(3),
            completed_state(),
        )
        .unwrap();
    drop(reopened);
    let reopened = open(&temporary, options());
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(completed_state())
    );
}

#[test]
fn retry_repeats_the_active_node_with_clean_memory_and_durable_reason() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    let initial = initial_state();
    store
        .create_graph_session_v2(&identity(), initial.clone())
        .unwrap();

    let outcome = initial
        .retry_active_attempt_v2(
            Revision::new(1),
            &attempt_id(1),
            attempt_id(2),
            ReasonV2::new("Repeat the draft with a clean attempt.").unwrap(),
            UnixMillis::new(20),
        )
        .unwrap();
    let retried = outcome.state();
    assert_eq!(outcome.graph_node_id(), &node("draft"));
    assert_eq!(outcome.from_attempt_id(), &attempt_id(1));
    assert_eq!(outcome.to_attempt_id(), &attempt_id(2));
    assert_eq!(retried.workspace_revision(), Revision::new(2));
    assert_eq!(retried.trace().revision(), Revision::new(2));
    assert_eq!(
        retried.trace().attempts()[0].lifecycle(),
        AttemptLifecycle::Abandoned
    );
    assert_eq!(
        retried.trace().attempts()[0].validity(),
        AttemptValidityV2::Stale
    );
    let fresh = retried.trace().active_attempt().unwrap();
    assert_eq!(fresh.graph_node_id(), &node("draft"));
    assert_eq!(fresh.number(), AttemptNumberV2::new(2));
    assert_eq!(fresh.trace(), TraceSequenceV2::new(2));
    assert_eq!(retried.counters()[0].attempt_count(), 2);
    assert_eq!(retried.counters()[0].rework_traversal_count(), 0);
    assert_eq!(
        retried.attempt_metadata()[0].ended_at(),
        Some(UnixMillis::new(20))
    );
    assert_eq!(
        retried.attempt_metadata()[0].terminal_reason(),
        Some("Repeat the draft with a clean attempt.")
    );
    assert!(
        retried.workflow_memory().attempts()[1]
            .item_slots()
            .is_empty()
    );
    assert!(
        retried.workflow_memory().attempts()[1]
            .blockers()
            .is_empty()
    );
    assert!(retried.goal_state().attempt_assessments().is_empty());

    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            outcome.into_state(),
        )
        .unwrap();
    drop(store);
    let reopened = open(&temporary, options());
    assert_eq!(
        reopened
            .read_graph_session_v2(&identity())
            .unwrap()
            .unwrap()
            .attempt_metadata()[0]
            .terminal_reason(),
        Some("Repeat the draft with a clean attempt.")
    );
}

#[test]
fn procedure_v2_terminal_reason_accepts_2000_characters_and_reopen_rejects_2001() {
    let at_limit = "r".repeat(2_000);
    let over_limit = "r".repeat(2_001);
    assert!(
        AttemptMetadataV2::new(
            attempt_id(1),
            UnixMillis::new(10),
            Some(UnixMillis::new(20)),
            Some(at_limit.clone()),
        )
        .is_ok()
    );
    assert!(
        AttemptMetadataV2::new(
            attempt_id(1),
            UnixMillis::new(10),
            Some(UnixMillis::new(20)),
            Some(over_limit.clone()),
        )
        .is_err()
    );

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    let initial = initial_state();
    store
        .create_graph_session_v2(&identity(), initial.clone())
        .unwrap();
    let retried = initial
        .retry_active_attempt_v2(
            Revision::new(1),
            &attempt_id(1),
            attempt_id(2),
            ReasonV2::new(at_limit.clone()).unwrap(),
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(&identity(), Revision::new(1), Revision::new(1), retried)
        .unwrap();
    drop(store);
    let reopened = open(&temporary, options());
    assert_eq!(
        reopened
            .read_graph_session_v2(&identity())
            .unwrap()
            .unwrap()
            .attempt_metadata()[0]
            .terminal_reason(),
        Some(at_limit.as_str())
    );
    drop(reopened);

    Connection::open(database_path(&temporary))
        .unwrap()
        .execute(
            "UPDATE v2_attempts SET terminal_reason = ?1 WHERE attempt_id = ?2",
            [&over_limit, attempt_id(1).as_str()],
        )
        .unwrap();
    assert!(
        SqliteStoreV1::open(
            database_path(&temporary),
            &root(),
            identity(),
            options(),
            UnixMillis::new(30),
        )
        .is_err()
    );
}

#[test]
fn graph_workspace_view_reads_graph_queue_and_sequence_from_one_store_boundary() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    let state = initial_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();

    let queued_job = job_id(1);
    let request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("v2-view-queued").unwrap(),
        queued_job.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(20),
    );
    assert!(matches!(
        store.admit(&identity(), request),
        Ok(AdmitOutcomeV1::New(_))
    ));

    let queued = store.read_graph_workspace_view_v2(&identity()).unwrap();
    assert_eq!(queued.identity(), &identity());
    assert_eq!(queued.graph_state(), Some(&state));
    assert_eq!(queued.queued_job_count(), 1);
    assert_eq!(queued.running_job_id(), None);
    assert_eq!(queued.latest_workspace_sequence(), 1);
    assert_eq!(queued.observed_at(), UnixMillis::new(20));

    let claimed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-view-worker").unwrap(),
            UnixMillis::new(21),
        )
        .unwrap()
        .expect("the queued job must be claimable");
    assert_eq!(claimed.claim().job_id(), &queued_job);
    let running = store.read_graph_workspace_view_v2(&identity()).unwrap();
    assert_eq!(running.graph_state(), Some(&state));
    assert_eq!(running.queued_job_count(), 0);
    assert_eq!(running.running_job_id(), Some(&queued_job));
    assert_eq!(running.latest_workspace_sequence(), 1);

    drop(store);
    let reopened = open(&temporary, options());
    let recovered = reopened.read_graph_workspace_view_v2(&identity()).unwrap();
    assert_eq!(recovered.graph_state(), Some(&state));
    assert_eq!(recovered.queued_job_count(), 1);
    assert_eq!(recovered.running_job_id(), None);
    assert_eq!(recovered.latest_workspace_sequence(), 1);
}

#[test]
fn disposable_graph_workspace_inspection_preserves_authoritative_artifact_bytes() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());

    let empty_before = database_artifacts(&temporary);
    let empty = SqliteStoreV1::inspect_graph_workspace_view_v2(
        database_path(&temporary),
        &identity(),
        &options(),
        UnixMillis::new(10),
    )
    .unwrap();
    assert_eq!(empty.graph_state(), None);
    assert_eq!(database_artifacts(&temporary), empty_before);

    let state = initial_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let queued_job = job_id(2);
    let request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("v2-disposable-view").unwrap(),
        queued_job.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('9'),
        UnixMillis::new(20),
    );
    assert!(matches!(
        store.admit(&identity(), request),
        Ok(AdmitOutcomeV1::New(_))
    ));

    let before = database_artifacts(&temporary);
    let inspected = SqliteStoreV1::inspect_graph_workspace_view_v2(
        database_path(&temporary),
        &identity(),
        &options(),
        UnixMillis::new(21),
    )
    .unwrap();
    assert_eq!(inspected.identity(), &identity());
    assert_eq!(inspected.graph_state(), Some(&state));
    assert_eq!(inspected.queued_job_count(), 1);
    assert_eq!(inspected.running_job_id(), None);
    assert_eq!(inspected.latest_workspace_sequence(), 1);
    assert_eq!(inspected.observed_at(), UnixMillis::new(20));
    assert_eq!(database_artifacts(&temporary), before);

    let (coherent, queued_state) = SqliteStoreV1::inspect_graph_workspace_view_and_job_state_v2(
        database_path(&temporary),
        &identity(),
        &options(),
        Some(&queued_job),
        UnixMillis::new(21),
    )
    .unwrap();
    assert_eq!(coherent, inspected);
    assert_eq!(queued_state, Some(podway_store::JobStateV1::Queued));
    let (_, missing_state) = SqliteStoreV1::inspect_graph_workspace_view_and_job_state_v2(
        database_path(&temporary),
        &identity(),
        &options(),
        Some(&job_id(99)),
        UnixMillis::new(21),
    )
    .unwrap();
    assert_eq!(missing_state, None);
    assert_eq!(database_artifacts(&temporary), before);

    drop(store);
    let reopened = open(&temporary, options());
    let reopened_before = database_artifacts(&temporary);
    let recovered = SqliteStoreV1::inspect_graph_workspace_view_v2(
        database_path(&temporary),
        &identity(),
        &options(),
        UnixMillis::new(22),
    )
    .unwrap();
    assert_eq!(recovered.graph_state(), Some(&state));
    assert_eq!(recovered.queued_job_count(), 1);
    assert_eq!(recovered.running_job_id(), None);
    assert_eq!(recovered.latest_workspace_sequence(), 1);
    assert_eq!(database_artifacts(&temporary), reopened_before);
    drop(reopened);
}

#[test]
fn graph_workspace_view_rejects_wrong_identity_and_v1_v2_coexistence() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    let wrong_identity = DurableWorktreeIdentityV1::new(
        digest('d'),
        identity().workspace_uuid().clone(),
        digest('e'),
    );
    assert!(matches!(
        store.read_graph_workspace_view_v2(&wrong_identity),
        Err(StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::WorkspaceIdentity
        })
    ));

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "INSERT INTO procedure_snapshots (snapshot_id, schema_id, procedure_id, \
             procedure_version, name, digest, canonical_json, source_kind, source_label, \
             created_at_ms) VALUES (?1, 'podway.procedure/v1', 'legacy', '1', 'Legacy', ?2, \
             '{}', 'file', 'legacy.yaml', 30)",
            ("00000000-0000-4000-8000-000000000030", digest('f').as_str()),
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO task_sessions (singleton, session_id, task_title, \
             procedure_snapshot_id, lifecycle, session_revision, active_stage_id, \
             active_attempt_id, created_at_ms) VALUES (1, ?1, 'Legacy task', ?2, \
             'running', 1, 'work', ?3, 30)",
            (
                "00000000-0000-4000-8000-000000000031",
                "00000000-0000-4000-8000-000000000030",
                "00000000-0000-4000-8000-000000000032",
            ),
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        store.read_graph_workspace_view_v2(&identity()).unwrap_err(),
        StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Session
        }
    );
}

#[test]
fn stale_revision_and_precommit_failure_leave_prior_state_atomic() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    let error = store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(9),
            Revision::new(1),
            advanced_state(),
        )
        .unwrap_err();
    assert!(matches!(error, StoreErrorV1::PreconditionConflictV1 { .. }));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(initial_state())
    );
    drop(store);

    let failing = open(
        &temporary,
        options().with_failpoint(Some(StoreFailpointV1::V2GraphStateBeforeCommit)),
    );
    let error = failing
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            advanced_state(),
        )
        .unwrap_err();
    assert_eq!(
        error,
        StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        }
    );
    drop(failing);
    let reopened = open(&temporary, options());
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(initial_state())
    );
}

#[test]
fn concurrent_graph_replacements_have_one_winner_and_one_stale_revision() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    drop(store);

    let barrier = Arc::new(Barrier::new(2));
    let run = |store: SqliteStoreV1, barrier: Arc<Barrier>| {
        thread::spawn(move || {
            barrier.wait();
            store.replace_graph_session_v2(
                &identity(),
                Revision::new(1),
                Revision::new(1),
                advanced_state(),
            )
        })
    };
    let left = run(open(&temporary, options()), Arc::clone(&barrier));
    let right = run(open(&temporary, options()), barrier);
    let outcomes = [left.join().unwrap(), right.join().unwrap()];

    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(StoreErrorV1::PreconditionConflictV1 { .. })))
            .count(),
        1
    );

    let reopened = open(&temporary, options());
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(advanced_state())
    );
}

#[test]
fn malformed_snapshot_and_counter_corruption_fail_closed() {
    let valid = snapshot();
    let error = ProcedureSnapshotV2::new(
        valid.snapshot_id().clone(),
        valid.canonical_json().clone(),
        digest('f'),
        valid.source().clone(),
        valid.created_at(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("digest"));

    let invalid_document = json!({
        "schema": "podway.procedure/v2",
        "id": "invalid-decision",
        "version": "1",
        "name": "Invalid decision",
        "purpose": "Prove schema validation precedes persistence.",
        "node_definitions": {
            "choose": {"type": "decision", "title": "Choose", "objective": "Choose.", "prompt": "Which?"}
        },
        "graph": {"entry": "choose", "nodes": [{"id": "choose", "use": "choose"}]}
    });
    let invalid_canonical = canonicalize_json_v1(&invalid_document).unwrap();
    let invalid_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(invalid_canonical.as_bytes())
    ))
    .unwrap();
    let error = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000011").unwrap(),
        CanonicalProcedureJsonV1::new(invalid_canonical).unwrap(),
        invalid_digest,
        ProcedureSourceLabelV1::file("invalid.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap_err();
    assert!(error.to_string().contains("canonical schema"));

    let oversized_integer_document = json!({
        "schema": "podway.procedure/v2",
        "id": "invalid-integer-bound",
        "version": "1",
        "name": "Invalid integer bound",
        "purpose": "Prove exact integer schema bounds.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Work",
                "intent": "Work.",
                "items": [{
                    "id": "count",
                    "type": "integer",
                    "prompt": "Count.",
                    "required": true,
                    "maximum": 9_223_372_036_854_775_808_u64
                }]
            }
        },
        "graph": {"entry": "work", "nodes": [{"id": "work", "use": "work", "terminal": true}]}
    });
    let oversized_integer_canonical = canonicalize_json_v1(&oversized_integer_document).unwrap();
    let oversized_integer_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(oversized_integer_canonical.as_bytes())
    ))
    .unwrap();
    let error = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000012").unwrap(),
        CanonicalProcedureJsonV1::new(oversized_integer_canonical).unwrap(),
        oversized_integer_digest,
        ProcedureSourceLabelV1::file("invalid-bound.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap_err();
    assert!(error.to_string().contains("canonical schema"));

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    drop(store);
    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE v2_graph_node_counters SET attempt_count = 7 WHERE graph_node_id = 'draft'",
            [],
        )
        .unwrap();
    drop(connection);
    let error = SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        options(),
        UnixMillis::new(2),
    )
    .err()
    .expect("corrupt Procedure v2 state must fail startup");
    assert_eq!(
        error,
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::SessionCursor
        }
    );
}

#[test]
fn orphan_snapshot_fails_startup_integrity() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, options());
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    drop(store);
    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "INSERT INTO v2_procedure_snapshots (snapshot_id, schema_id, procedure_id, \
             procedure_version, name, purpose, digest, canonical_json, source_kind, source_label, \
             goal_tracking, created_at_ms) SELECT '00000000-0000-4000-8000-000000000099', \
             schema_id, procedure_id, procedure_version, name, purpose, digest, canonical_json, \
             source_kind, source_label, goal_tracking, created_at_ms FROM v2_procedure_snapshots",
            [],
        )
        .unwrap();
    drop(connection);
    let error = SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        options(),
        UnixMillis::new(2),
    )
    .err()
    .expect("orphan Procedure v2 snapshot must fail startup");
    assert_eq!(
        error,
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::SessionCursor
        }
    );
}
