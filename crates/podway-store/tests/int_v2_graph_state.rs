//! Procedure v2 graph/action persistence across transactions and process reopen.

use std::path::{Path, PathBuf};

use podway_core::{
    AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, CanonicalProcedureJsonV1,
    GraphNodeId, ProcedureSnapshotId, ProcedureSourceLabelV1, ReasonV2, Revision, ReworkKindV2,
    ReworkRecordInputV2, ReworkRecordV2, SessionAttemptV2, SessionId, SessionLifecycle,
    SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{
    AttemptMetadataV2, AttemptWorkflowMemoryV2, DurableWorktreeIdentityV1, GraphNodeCounterV2,
    GraphSessionStateV2, ProcedureSnapshotV2, SqliteStoreOptionsV1, SqliteStoreV1, StoreErrorV1,
    StoreFailpointV1, StoreGraphStateContractV2, StoreIntegrityCheckV1, StoreUnavailableReasonV1,
    ValidatedWorkspaceRootV1, WorkflowMemoryStateV2,
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
