//! Atomic Procedure v2 start publication and durable replay.

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use podway_core::{
    ActorAttributionV2, ArtifactValueV1, AttemptId, AttemptLifecycle, AttemptNumberV2,
    AttemptValidityV2, CanonicalProcedureJsonV1, CriterionId, DomainCommand, DomainError,
    DomainResult, GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2, GoalStatementV2,
    GraphNodeId, ItemId, OptionId, ProcedureSnapshotId, ProcedureSourceLabelV1, ReasonV2, Revision,
    SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest,
    TerminalDispositionKindV2, TerminalDispositionV2, TraceSequenceV2, UnixMillis, WorkspaceId,
    canonicalize_json_v1,
};
use podway_store::codec::encode_persisted_terminal_receipt_v1;
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, AdmissionSessionIdentityV1, AdmitRequestV1,
    AttemptMetadataV2, CanonicalExecutionJsonV1, DurableWorktreeIdentityV1, GraphInitialGoalV2,
    GraphNodeCounterV2, GraphSessionStateV2, GraphStartCurrentTaskV2, IdempotencyKeyV1, JobStateV1,
    PersistedGraphItemMutationV2, PersistedGraphMutationFailureV2,
    PersistedGraphTerminalOperationV2, PersistedResponseContextV1, ProcedureSnapshotV2,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreErrorV1, StoreFailpointActionV1, StoreFailpointV1, StoreGraphMutationContractV2,
    StoreGraphStateContractV2, StoreIdempotencyReadContractV1, StoreInvariantV1,
    StoreReadContractV1, StoreTerminalDispositionContractV2, StoreUnavailableReasonV1,
    TerminalResultV1, ValidatedWorkspaceRootV1, WorkerIdV1,
};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const V2_FAILURE_CRASH_DATABASE_PATH_ENV: &str = "PODWAY_V2_FAILURE_CRASH_DATABASE_PATH";
const V2_FAILURE_CRASH_CHILD_TEST: &str =
    "int_v2_graph_start_terminal::graph_mutation_failure_precommit_abort_child";

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000301").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-v2-graph-start")).unwrap()
}

fn database_path(temporary: &TempDir) -> PathBuf {
    temporary.path().join("state.sqlite3")
}

fn open(temporary: &TempDir, options: SqliteStoreOptionsV1, now: u64) -> SqliteStoreV1 {
    SqliteStoreV1::open(
        database_path(temporary),
        &root(),
        identity(),
        options,
        UnixMillis::new(now),
    )
    .unwrap()
}

fn uuid(number: u64) -> String {
    format!("00000000-0000-4000-8000-{number:012x}")
}

fn node(value: &str) -> GraphNodeId {
    GraphNodeId::new(value).unwrap()
}

fn graph_state(session_number: u64, snapshot_number: u64, created_at: u64) -> GraphSessionStateV2 {
    graph_state_with_required_artifact(session_number, snapshot_number, created_at, false)
}

fn graph_state_with_required_artifact(
    session_number: u64,
    snapshot_number: u64,
    created_at: u64,
    required_artifact: bool,
) -> GraphSessionStateV2 {
    graph_state_with_options(
        session_number,
        snapshot_number,
        created_at,
        required_artifact,
        false,
    )
}

fn graph_state_with_options(
    session_number: u64,
    snapshot_number: u64,
    created_at: u64,
    required_artifact: bool,
    goal_tracking: bool,
) -> GraphSessionStateV2 {
    let mut items = vec![
        json!({
            "id":"done","type":"confirm","prompt":"Done","required":required_artifact
        }),
        json!({
            "id":"note","type":"text","prompt":"Note","required":false,"max_length":16384
        }),
    ];
    if required_artifact {
        items.push(json!({
            "id":"proof","type":"artifact","prompt":"Proof","required":true
        }));
    }
    let mut document = json!({
        "schema": "podway.procedure/v2",
        "id": "atomic-start",
        "version": "1",
        "name": "Atomic start",
        "purpose": "Publish graph state and receipt atomically.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Work",
                "intent": "Work.",
                "items": items
            }
        },
        "graph": {
            "entry": "work",
            "nodes": [{"id": "work", "use": "work", "skip":{"allowed":true,"reason_required":true}, "terminal": true}]
        }
    });
    if goal_tracking {
        document
            .as_object_mut()
            .unwrap()
            .insert("goal_tracking".to_owned(), json!(true));
    }
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new(uuid(snapshot_number)).unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("procedure.yaml").unwrap(),
        UnixMillis::new(created_at),
    )
    .unwrap();
    let attempt_id = AttemptId::new(uuid(session_number + 1)).unwrap();
    let attempt = SessionAttemptV2::new(
        attempt_id.clone(),
        node("work"),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        None,
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(uuid(session_number)).unwrap(),
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt],
    )
    .unwrap();
    GraphSessionStateV2::new(
        Revision::new(1),
        "Atomic graph start",
        snapshot,
        trace,
        vec![GraphNodeCounterV2::new(node("work"), 1, 0)],
        vec![AttemptMetadataV2::new(attempt_id, UnixMillis::new(created_at), None, None).unwrap()],
        UnixMillis::new(created_at),
        None,
        None,
        None,
    )
    .unwrap()
}

fn initial_goal() -> GraphInitialGoalV2 {
    GraphInitialGoalV2::new(
        GoalStatementV2::new("Persist the prepared lifecycle.").unwrap(),
        GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(
                CriterionId::new("verified").unwrap(),
                "Prepared begin survives restart.",
            )
            .unwrap(),
        ])
        .unwrap(),
        Some(ActorAttributionV2::new("planner").unwrap()),
    )
}

#[test]
fn prepared_session_and_begin_with_optional_goal_reconstruct_exactly() {
    for (with_goal, session_number) in [(false, 701_u64), (true, 711_u64)] {
        let temporary = TempDir::new().unwrap();
        let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        let template =
            graph_state_with_options(session_number, session_number + 1, 10, false, true);
        let prepared = GraphSessionStateV2::prepared(
            Revision::new(1),
            "Prepared graph session",
            template.snapshot().clone(),
            SessionId::new(uuid(session_number + 2)).unwrap(),
            UnixMillis::new(10),
        )
        .unwrap();
        store
            .create_graph_session_v2(&identity(), prepared.clone())
            .unwrap();
        assert_eq!(
            store.read_graph_session_v2(&identity()).unwrap(),
            Some(prepared.clone())
        );
        drop(store);

        let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 11);
        assert_eq!(
            reopened.read_graph_session_v2(&identity()).unwrap(),
            Some(prepared.clone())
        );
        assert_eq!(
            prepared
                .begin_v2(
                    Revision::new(1),
                    AttemptId::new(uuid(session_number + 4)).unwrap(),
                    None,
                    UnixMillis::new(12),
                )
                .unwrap_err(),
            podway_store::GraphMutationErrorV2::SessionRevisionConflict {
                expected: Revision::new(1),
                actual: Revision::ZERO,
            }
        );
        let attempt_id = AttemptId::new(uuid(session_number + 3)).unwrap();
        let begun = prepared
            .begin_v2(
                Revision::ZERO,
                attempt_id.clone(),
                with_goal.then(initial_goal),
                UnixMillis::new(12),
            )
            .unwrap();
        assert_eq!(begun.state().trace().lifecycle(), SessionLifecycle::Running);
        assert_eq!(begun.state().trace().revision(), Revision::new(1));
        assert_eq!(begun.attempt_id(), &attempt_id);
        assert_eq!(
            begun.goal_revision(),
            with_goal.then_some(GoalRevisionNumberV2::FIRST)
        );
        assert_eq!(
            begun.state().goal_state().current_revision(),
            with_goal.then_some(GoalRevisionNumberV2::FIRST)
        );
        reopened
            .replace_graph_session_v2(
                &identity(),
                prepared.workspace_revision(),
                Revision::ZERO,
                begun.state().clone(),
            )
            .unwrap();
        let expected = begun.into_state();
        drop(reopened);

        let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 13);
        assert_eq!(
            reopened.read_graph_session_v2(&identity()).unwrap(),
            Some(expected.clone())
        );
        assert_eq!(
            expected
                .begin_v2(
                    expected.trace().revision(),
                    AttemptId::new(uuid(session_number + 4)).unwrap(),
                    None,
                    UnixMillis::new(14),
                )
                .unwrap_err(),
            podway_store::GraphMutationErrorV2::SessionNotPrepared
        );
    }

    let template = graph_state_with_options(741, 742, 10, false, false);
    let prepared = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Prepared graph session",
        template.snapshot().clone(),
        SessionId::new(uuid(743)).unwrap(),
        UnixMillis::new(10),
    )
    .unwrap();
    assert_eq!(
        prepared
            .begin_v2(
                Revision::ZERO,
                AttemptId::new(uuid(744)).unwrap(),
                Some(initial_goal()),
                UnixMillis::new(11),
            )
            .unwrap_err(),
        podway_store::GraphMutationErrorV2::GoalTrackingNotEnabled
    );
}

#[test]
fn prepared_reconstruction_rejects_session_scoped_history() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let template = graph_state(721, 722, 10);
    let prepared = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Prepared graph session",
        template.snapshot().clone(),
        SessionId::new(uuid(723)).unwrap(),
        UnixMillis::new(10),
    )
    .unwrap();
    store
        .create_graph_session_v2(&identity(), prepared.clone())
        .unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "INSERT INTO v2_terminal_dispositions (
                session_id, terminal_session_revision, kind, reason, recorded_at_ms
             ) VALUES (?1, 1, 'not_required', 'invalid prepared history', 11)",
            [prepared.trace().session_id().as_str()],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        SqliteStoreV1::open(
            database_path(&temporary),
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(12),
        ),
        Err(StoreErrorV1::StorageIntegrityV1 { .. })
    ));
}

#[test]
fn terminal_dispositions_persist_and_remain_bound_to_the_terminal_revision() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let running = graph_state(731, 732, 10);
    store
        .create_graph_session_v2(&identity(), running.clone())
        .unwrap();
    assert!(
        store
            .record_terminal_disposition_v2(
                &identity(),
                TerminalDispositionV2::not_required(
                    running.trace().session_id().clone(),
                    running.trace().revision(),
                    "No handoff",
                    None,
                    UnixMillis::new(11),
                )
                .unwrap(),
            )
            .is_err()
    );

    let active = running.trace().active_attempt().unwrap();
    let completed = running
        .complete_active_action_v2(
            running.trace().revision(),
            active.attempt_id(),
            None,
            UnixMillis::new(12),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            running.workspace_revision(),
            running.trace().revision(),
            completed.clone(),
        )
        .unwrap();
    assert!(
        store
            .record_terminal_disposition_v2(
                &identity(),
                TerminalDispositionV2::not_required(
                    completed.trace().session_id().clone(),
                    running.trace().revision(),
                    "Stale revision",
                    None,
                    UnixMillis::new(13),
                )
                .unwrap(),
            )
            .is_err()
    );
    assert!(
        store
            .record_terminal_disposition_v2(
                &identity(),
                TerminalDispositionV2::not_required(
                    completed.trace().session_id().clone(),
                    completed.trace().revision(),
                    "Regressed timestamp",
                    None,
                    UnixMillis::new(9),
                )
                .unwrap(),
            )
            .is_err()
    );
    let disposition = TerminalDispositionV2::handed_off(
        completed.trace().session_id().clone(),
        completed.trace().revision(),
        "Delivered to the owning task",
        "commit:abc123",
        Some(ActorAttributionV2::new("agent").unwrap()),
        UnixMillis::new(13),
    )
    .unwrap();
    store
        .record_terminal_disposition_v2(&identity(), disposition.clone())
        .unwrap();
    assert!(
        store
            .record_terminal_disposition_v2(&identity(), disposition.clone())
            .is_err()
    );
    assert_eq!(
        store.read_terminal_dispositions_v2(&identity()).unwrap(),
        vec![disposition.clone()]
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 14);
    let history = reopened.read_terminal_dispositions_v2(&identity()).unwrap();
    assert_eq!(history, vec![disposition]);
    assert_eq!(history[0].kind(), TerminalDispositionKindV2::HandedOff);
    assert_eq!(
        history[0].terminal_session_revision(),
        completed.trace().revision()
    );
}

fn decision_graph_state(
    session_number: u64,
    snapshot_number: u64,
    created_at: u64,
) -> GraphSessionStateV2 {
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "atomic-decision",
        "version": "1",
        "name": "Atomic decision",
        "purpose": "Bind a durable decision to its admitted successor identity.",
        "node_definitions": {
            "choose": {
                "type": "decision",
                "title": "Choose",
                "objective": "Choose the next route.",
                "prompt": "Approve?",
                "options": [{"id":"approve","label":"Approve"}],
                "reason": {"required":true}
            },
            "work": {"type":"action","title":"Work","intent":"Work."}
        },
        "graph": {
            "entry": "choose",
            "nodes": [
                {
                    "id":"choose",
                    "use":"choose",
                    "routes":{"approve":{"to":"work","effect":"advance"}}
                },
                {"id":"work","use":"work","terminal":true}
            ]
        }
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new(uuid(snapshot_number)).unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("decision.yaml").unwrap(),
        UnixMillis::new(created_at),
    )
    .unwrap();
    let attempt_id = AttemptId::new(uuid(session_number + 1)).unwrap();
    let attempt = SessionAttemptV2::new(
        attempt_id.clone(),
        node("choose"),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        None,
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(uuid(session_number)).unwrap(),
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt],
    )
    .unwrap();
    GraphSessionStateV2::new(
        Revision::new(1),
        "Atomic decision",
        snapshot,
        trace,
        vec![
            GraphNodeCounterV2::new(node("choose"), 1, 0),
            GraphNodeCounterV2::new(node("work"), 0, 0),
        ],
        vec![AttemptMetadataV2::new(attempt_id, UnixMillis::new(created_at), None, None).unwrap()],
        UnixMillis::new(created_at),
        None,
        None,
        None,
    )
    .unwrap()
}

fn decision_record_projection(record: &podway_core::DecisionRecordV2) -> serde_json::Value {
    json!({
        "trace_sequence": record.trace().get(),
        "session_id": record.session_id(),
        "session_revision": record.session_revision().get(),
        "procedure_schema": "podway.procedure/v2",
        "procedure_snapshot_id": record.procedure_snapshot_id(),
        "procedure_digest": record.procedure_digest(),
        "graph_node_id": record.graph_node_id(),
        "node_definition_id": record.node_definition_id(),
        "attempt_id": record.attempt_id(),
        "attempt_number": record.attempt_number().get(),
        "goal_revision": record.goal_revision().map(|revision| revision.get()),
        "option_id": record.selected_option(),
        "effect": record.route_effect().as_str(),
        "target_graph_node_id": record.route_target(),
        "reason": record.reason().as_str(),
        "recorded_at": "1970-01-01T00:00:00.033Z",
        "references": []
    })
}

fn artifact_failure_ready_state(session_number: u64, snapshot_number: u64) -> GraphSessionStateV2 {
    let initial = graph_state_with_required_artifact(session_number, snapshot_number, 10, true);
    let active = initial.trace().active_attempt().unwrap();
    let with_artifact = initial
        .mutate_active_item_v2(
            active.attempt_id(),
            &ItemId::new("proof").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Attach {
                value: ArtifactValueV1::local_path("proof.txt", digest('e'), 42, "text/plain")
                    .unwrap(),
            },
            UnixMillis::new(11),
        )
        .unwrap()
        .into_state();
    with_artifact
        .mutate_active_item_v2(
            with_artifact.trace().active_attempt().unwrap().attempt_id(),
            &ItemId::new("done").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Check,
            UnixMillis::new(12),
        )
        .unwrap()
        .into_state()
}

fn admit_and_claim(
    store: &SqliteStoreV1,
    command: DomainCommand,
    key: &str,
    job_number: u64,
    identity_fence: AdmissionSessionIdentityV1,
    revision: Option<Revision>,
) -> podway_store::ClaimedJobV1 {
    let command_name = match command {
        DomainCommand::SessionStart => "session.start",
        DomainCommand::SessionStartReplace => "session.start_replace",
        _ => unreachable!(),
    };
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": command_name,
            "execution_version": 6,
            "procedure": {"canonical": true}
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        command,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(revision, None, None, None).unwrap(),
        digest('d'),
        UnixMillis::new(20),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(identity_fence)
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            command_name,
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-start-worker").unwrap(),
            UnixMillis::new(21),
        )
        .unwrap()
        .unwrap()
}

fn admit_complete_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let active = state.trace().active_attempt().unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "session.complete",
            "execution_version": 6
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::SessionComplete,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
        digest('e'),
        UnixMillis::new(30),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            "session.complete",
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-complete-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap()
}

fn admit_decide_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    fresh_attempt_id: &AttemptId,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let active = state.trace().active_attempt().unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "session.decide",
            "execution_version": 9,
            "fresh_attempt_id": fresh_attempt_id,
            "payload": {
                "actor": null,
                "option_id": "approve",
                "reason": "Ready."
            },
            "preconditions": {
                "attempt_id": active.attempt_id(),
                "session_id": state.trace().session_id(),
                "session_revision": state.trace().revision().get()
            },
            "selector": {},
            "workspace_id": identity().workspace_uuid()
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::SessionDecide,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
        digest('7'),
        UnixMillis::new(30),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            "session.decide",
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-decide-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap()
}

fn admit_skip_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let active = state.trace().active_attempt().unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "attached_artifact": null,
            "command": "session.skip",
            "execution_version": 7,
            "fresh_attempt_id": null,
            "payload": {"reason": "Not applicable."},
            "preconditions": {},
            "selector": {},
            "workspace_id": identity().workspace_uuid().as_str(),
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::SessionSkip,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
        digest('f'),
        UnixMillis::new(30),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            "session.skip",
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-skip-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap()
}

fn admit_set_with_value(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
    value: &str,
) {
    let active = state.trace().active_attempt().unwrap();
    let item_id = ItemId::new("note").unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "item.set",
            "execution_version": 6,
            "item_id": item_id,
            "value": value,
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::ItemSet {
            item_id: item_id.clone(),
        },
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(active.attempt_id().clone()),
            Some(item_id),
            Some(Revision::ZERO),
        )
        .unwrap(),
        digest('f'),
        UnixMillis::new(30),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            "item.set",
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
}

fn admit_reset_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "attached_artifact": null,
            "command": "session.reset",
            "execution_version": 8,
            "fresh_attempt_id": null,
            "fresh_blocker_id": null,
            "payload": {"confirmed": true},
            "preconditions": {},
            "selector": {},
            "workspace_id": identity().workspace_uuid().as_str(),
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::SessionReset,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(Some(state.trace().revision()), None, None, None)
            .unwrap(),
        digest('9'),
        UnixMillis::new(30),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            "session.reset",
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-reset-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn admit_blocker_command_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    command: DomainCommand,
    command_name: &str,
    payload: serde_json::Value,
    fresh_blocker_id: Option<&podway_core::BlockerId>,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let active = state.trace().active_attempt().unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "attached_artifact": null,
            "command": command_name,
            "execution_version": 8,
            "fresh_attempt_id": null,
            "fresh_blocker_id": fresh_blocker_id.map(podway_core::BlockerId::as_str),
            "payload": payload,
            "preconditions": {},
            "selector": {},
            "workspace_id": identity().workspace_uuid().as_str(),
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        command,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
        digest('8'),
        UnixMillis::new(30),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            command_name,
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-blocker-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap()
}

fn admit_uncheck_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let active = state.trace().active_attempt().unwrap();
    let item_id = ItemId::new("done").unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "item.uncheck",
            "execution_version": 6,
            "item_id": "done"
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::ItemUncheck {
            item_id: item_id.clone(),
        },
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(active.attempt_id().clone()),
            Some(item_id),
            Some(Revision::ZERO),
        )
        .unwrap(),
        digest('f'),
        UnixMillis::new(30),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            "item.uncheck",
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-item-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap()
}

fn seed_graph_session(store: &SqliteStoreV1, state: &GraphSessionStateV2, job_number: u64) {
    let claimed = admit_and_claim(
        store,
        DomainCommand::SessionStart,
        &format!("v2-seed-{job_number}"),
        job_number,
        AdmissionSessionIdentityV1::Absent,
        None,
    );
    store
        .commit_graph_start_terminal_v2(
            claimed.claim().clone(),
            GraphStartCurrentTaskV2::Absent,
            state.clone(),
            UnixMillis::new(23),
        )
        .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn action_runtime_admit_request(
    state: &GraphSessionStateV2,
    command: DomainCommand,
    command_name: &str,
    key: &str,
    job_number: u64,
    request_digest: Sha256Digest,
    preconditions: RevisionAttemptItemPreconditionsV1,
) -> AdmitRequestV1 {
    AdmitRequestV1::new_with_canonical_execution(
        command,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(uuid(job_number)).unwrap(),
        preconditions,
        request_digest,
        UnixMillis::new(30),
        CanonicalExecutionJsonV1::new(
            canonicalize_json_v1(&json!({
                "command": command_name,
                "execution_version": 8,
                "test_request": key,
            }))
            .unwrap(),
        )
        .unwrap(),
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            uuid(job_number + 100),
            command_name,
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2-graph-start",
            0,
        )
        .unwrap(),
    )
}

fn admit_item_record_many_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let active = state.trace().active_attempt().unwrap();
    let request = action_runtime_admit_request(
        state,
        DomainCommand::ItemRecordMany,
        "item.record_many",
        key,
        job_number,
        digest('9'),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-item-batch-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn graph_action_admission_rejects_stale_fences_without_rows_and_replays_first() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let state = graph_state(2_300, 2_310, 22);
    seed_graph_session(&store, &state, 2_320);
    let active = state.trace().active_attempt().unwrap();
    let initial_sequence = store
        .read_workspace_view(&identity())
        .unwrap()
        .latest_workspace_sequence();

    let stale_revision = action_runtime_admit_request(
        &state,
        DomainCommand::SessionComplete,
        "session.complete",
        "v2run007-store-stale-revision",
        2_330,
        digest('1'),
        RevisionAttemptItemPreconditionsV1::new(
            Some(Revision::new(2)),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        store.admit(&identity(), stale_revision),
        Err(
            podway_store::StoreErrorV1::ProcedureV2PreconditionFailedV1 {
                failure: PersistedGraphMutationFailureV2::SessionRevisionConflict {
                    expected: Revision::new(2),
                    actual: Revision::new(1),
                },
            }
        )
    );

    let wrong_attempt = AttemptId::new(uuid(2_399)).unwrap();
    let stale_attempt = action_runtime_admit_request(
        &state,
        DomainCommand::SessionComplete,
        "session.complete",
        "v2run007-store-stale-attempt",
        2_331,
        digest('2'),
        RevisionAttemptItemPreconditionsV1::new(
            Some(Revision::new(1)),
            Some(wrong_attempt.clone()),
            None,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        store.admit(&identity(), stale_attempt),
        Err(
            podway_store::StoreErrorV1::ProcedureV2PreconditionFailedV1 {
                failure: PersistedGraphMutationFailureV2::AttemptNotCurrent {
                    expected: wrong_attempt,
                    actual: Some(active.attempt_id().clone()),
                },
            }
        )
    );

    let stale_rework_revision = action_runtime_admit_request(
        &state,
        DomainCommand::SessionRework,
        "session.rework",
        "v2drw003-store-stale-rework-revision",
        2_338,
        digest('8'),
        RevisionAttemptItemPreconditionsV1::new(
            Some(Revision::new(2)),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        store.admit(&identity(), stale_rework_revision),
        Err(
            podway_store::StoreErrorV1::ProcedureV2PreconditionFailedV1 {
                failure: PersistedGraphMutationFailureV2::SessionRevisionConflict {
                    expected: Revision::new(2),
                    actual: Revision::new(1),
                },
            }
        )
    );

    let wrong_rework_attempt = AttemptId::new(uuid(2_398)).unwrap();
    let stale_rework_attempt = action_runtime_admit_request(
        &state,
        DomainCommand::SessionRework,
        "session.rework",
        "v2drw003-store-stale-rework-attempt",
        2_339,
        digest('9'),
        RevisionAttemptItemPreconditionsV1::new(
            Some(Revision::new(1)),
            Some(wrong_rework_attempt.clone()),
            None,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        store.admit(&identity(), stale_rework_attempt),
        Err(
            podway_store::StoreErrorV1::ProcedureV2PreconditionFailedV1 {
                failure: PersistedGraphMutationFailureV2::AttemptNotCurrent {
                    expected: wrong_rework_attempt,
                    actual: Some(active.attempt_id().clone()),
                },
            }
        )
    );

    let item_id = ItemId::new("done").unwrap();
    let stale_item = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck {
            item_id: item_id.clone(),
        },
        "item.check",
        "v2run007-store-stale-item",
        2_332,
        digest('3'),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(active.attempt_id().clone()),
            Some(item_id.clone()),
            Some(Revision::new(1)),
        )
        .unwrap(),
    );
    assert_eq!(
        store.admit(&identity(), stale_item),
        Err(
            podway_store::StoreErrorV1::ProcedureV2PreconditionFailedV1 {
                failure: PersistedGraphMutationFailureV2::ItemRevisionConflict {
                    expected: Revision::new(1),
                    actual: Revision::ZERO,
                },
            }
        )
    );
    let missing_item_id = ItemId::new("missing").unwrap();
    let missing_item = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck {
            item_id: missing_item_id.clone(),
        },
        "item.check",
        "v2run007-store-missing-item",
        2_336,
        digest('6'),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(active.attempt_id().clone()),
            Some(missing_item_id.clone()),
            Some(Revision::ZERO),
        )
        .unwrap(),
    );
    assert_eq!(
        store.admit(&identity(), missing_item),
        Err(
            podway_store::StoreErrorV1::ProcedureV2PreconditionFailedV1 {
                failure: PersistedGraphMutationFailureV2::ItemNotFound {
                    item_id: missing_item_id,
                },
            }
        )
    );

    let stale_reset = action_runtime_admit_request(
        &state,
        DomainCommand::SessionReset,
        "session.reset",
        "v2run007-store-stale-reset",
        2_337,
        digest('7'),
        RevisionAttemptItemPreconditionsV1::new(Some(Revision::new(2)), None, None, None).unwrap(),
    );
    assert_eq!(
        store.admit(&identity(), stale_reset),
        Err(
            podway_store::StoreErrorV1::ProcedureV2PreconditionFailedV1 {
                failure: PersistedGraphMutationFailureV2::SessionRevisionConflict {
                    expected: Revision::new(2),
                    actual: Revision::new(1),
                },
            }
        )
    );
    for (job_number, key) in [
        (2_330, "v2run007-store-stale-revision"),
        (2_331, "v2run007-store-stale-attempt"),
        (2_332, "v2run007-store-stale-item"),
        (2_336, "v2run007-store-missing-item"),
        (2_337, "v2run007-store-stale-reset"),
        (2_338, "v2drw003-store-stale-rework-revision"),
        (2_339, "v2drw003-store-stale-rework-attempt"),
    ] {
        assert!(
            store
                .read_job(
                    &identity(),
                    &podway_core::JobId::new(uuid(job_number)).unwrap()
                )
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .read_idempotency_lookup(&identity(), &IdempotencyKeyV1::new(key).unwrap())
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        store
            .read_workspace_view(&identity())
            .unwrap()
            .latest_workspace_sequence(),
        initial_sequence
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(state.clone())
    );

    let valid = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck {
            item_id: item_id.clone(),
        },
        "item.check",
        "v2run007-store-replay",
        2_333,
        digest('4'),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(active.attempt_id().clone()),
            Some(item_id.clone()),
            Some(Revision::ZERO),
        )
        .unwrap(),
    );
    let admitted = store.admit(&identity(), valid).unwrap();
    let changed = state
        .mutate_active_item_v2(
            active.attempt_id(),
            &item_id,
            Revision::ZERO,
            ActiveItemMutationV2::Check,
            UnixMillis::new(31),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            state.workspace_revision(),
            state.trace().revision(),
            changed,
        )
        .unwrap();
    let replay = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck {
            item_id: item_id.clone(),
        },
        "item.check",
        "v2run007-store-replay",
        2_334,
        digest('4'),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(active.attempt_id().clone()),
            Some(item_id.clone()),
            Some(Revision::ZERO),
        )
        .unwrap(),
    );
    let replayed = store.admit(&identity(), replay).unwrap();
    assert!(matches!(
        (&admitted, &replayed),
        (
            podway_store::AdmitOutcomeV1::New(new),
            podway_store::AdmitOutcomeV1::Existing(
                podway_store::JobReceiptOrTerminalV1::JobReceipt(existing)
            )
        ) if new == existing
    ));
    let reused = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck { item_id },
        "item.check",
        "v2run007-store-replay",
        2_335,
        digest('5'),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(active.attempt_id().clone()),
            Some(ItemId::new("done").unwrap()),
            Some(Revision::ZERO),
        )
        .unwrap(),
    );
    assert!(matches!(
        store.admit(&identity(), reused),
        Err(podway_store::StoreErrorV1::IdempotencyDigestConflictV1 { .. })
    ));
}

#[test]
fn graph_action_queue_exhaustion_and_running_restart_preserve_one_durable_job_and_cursor() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(1).unwrap(), 1);
    let state = graph_state(2_400, 2_410, 22);
    seed_graph_session(&store, &state, 2_420);
    let active = state.trace().active_attempt().unwrap();
    let item_id = ItemId::new("done").unwrap();
    let preconditions = RevisionAttemptItemPreconditionsV1::new(
        None,
        Some(active.attempt_id().clone()),
        Some(item_id.clone()),
        Some(Revision::ZERO),
    )
    .unwrap();
    let first = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck {
            item_id: item_id.clone(),
        },
        "item.check",
        "v2run008-durable-first",
        2_430,
        digest('8'),
        preconditions.clone(),
    );
    let admitted = store.admit(&identity(), first).unwrap();
    assert!(matches!(admitted, podway_store::AdmitOutcomeV1::New(_)));

    let replay = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck {
            item_id: item_id.clone(),
        },
        "item.check",
        "v2run008-durable-first",
        2_431,
        digest('8'),
        preconditions.clone(),
    );
    assert!(matches!(
        store.admit(&identity(), replay),
        Ok(podway_store::AdmitOutcomeV1::Existing(
            podway_store::JobReceiptOrTerminalV1::JobReceipt(_)
        ))
    ));

    let overflow_job_id = podway_core::JobId::new(uuid(2_432)).unwrap();
    let overflow = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck { item_id },
        "item.check",
        "v2run008-capacity-overflow",
        2_432,
        digest('9'),
        preconditions,
    );
    assert_eq!(
        store.admit(&identity(), overflow),
        Err(podway_store::StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Busy,
        })
    );
    assert!(
        store
            .read_job(&identity(), &overflow_job_id)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .read_idempotency_lookup(
                &identity(),
                &IdempotencyKeyV1::new("v2run008-capacity-overflow").unwrap(),
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(state.clone())
    );
    assert_eq!(
        store
            .read_workspace_view(&identity())
            .unwrap()
            .queued_job_count(),
        1
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(1).unwrap(), 31);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 0);
    assert_eq!(
        reopened
            .read_workspace_view(&identity())
            .unwrap()
            .queued_job_count(),
        1
    );
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(state.clone())
    );
    let claimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2run008-restart-worker").unwrap(),
            UnixMillis::new(32),
        )
        .unwrap()
        .unwrap();
    assert_eq!(claimed.job().job_id().as_str(), uuid(2_430));
    drop(reopened);

    let recovered = open(&temporary, SqliteStoreOptionsV1::new(1).unwrap(), 33);
    assert_eq!(recovered.startup_recovery_report().requeued_job_count(), 1);
    assert_eq!(
        recovered
            .read_workspace_view(&identity())
            .unwrap()
            .queued_job_count(),
        1
    );
    assert_eq!(
        recovered.read_graph_session_v2(&identity()).unwrap(),
        Some(state.clone())
    );
    let replay_after_restart = action_runtime_admit_request(
        &state,
        DomainCommand::ItemCheck {
            item_id: ItemId::new("done").unwrap(),
        },
        "item.check",
        "v2run008-durable-first",
        2_433,
        digest('8'),
        RevisionAttemptItemPreconditionsV1::new(
            None,
            Some(state.trace().active_attempt().unwrap().attempt_id().clone()),
            Some(ItemId::new("done").unwrap()),
            Some(Revision::ZERO),
        )
        .unwrap(),
    );
    assert!(matches!(
        recovered.admit(&identity(), replay_after_restart),
        Ok(podway_store::AdmitOutcomeV1::Existing(
            podway_store::JobReceiptOrTerminalV1::JobReceipt(receipt)
        )) if receipt.job_id().as_str() == uuid(2_430)
    ));
    assert_eq!(
        recovered
            .read_workspace_view(&identity())
            .unwrap()
            .queued_job_count(),
        1
    );
}

#[test]
fn graph_start_state_and_terminal_receipt_survive_reopen() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let state = graph_state(310, 320, 22);
    let claimed = admit_and_claim(
        &store,
        DomainCommand::SessionStart,
        "v2-start",
        330,
        AdmissionSessionIdentityV1::Absent,
        None,
    );
    let job_id = claimed.job().job_id().clone();
    store
        .commit_graph_start_terminal_v2(
            claimed.claim().clone(),
            GraphStartCurrentTaskV2::Absent,
            state.clone(),
            UnixMillis::new(23),
        )
        .unwrap();
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    let terminal = job.terminal_receipt().unwrap();
    assert_eq!(
        terminal.graph_session_projection().unwrap().session_id(),
        state.trace().session_id()
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 24);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(state)
    );
    assert!(
        reopened
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_some()
    );
}

#[test]
fn graph_start_rolls_back_with_receipt_and_requeues_after_reopen() {
    let temporary = TempDir::new().unwrap();
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(
            StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        ))
        .with_failpoint_action(StoreFailpointActionV1::ReturnInjectedStorageIo);
    let store = open(&temporary, options, 1);
    let claimed = admit_and_claim(
        &store,
        DomainCommand::SessionStart,
        "v2-start-rollback",
        340,
        AdmissionSessionIdentityV1::Absent,
        None,
    );
    assert!(
        store
            .commit_graph_start_terminal_v2(
                claimed.claim().clone(),
                GraphStartCurrentTaskV2::Absent,
                graph_state(350, 360, 22),
                UnixMillis::new(23),
            )
            .is_err()
    );
    assert!(store.read_graph_session_v2(&identity()).unwrap().is_none());
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 24);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    let reclaimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-start-recovery").unwrap(),
            UnixMillis::new(25),
        )
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.job().job_id(), claimed.job().job_id());
}

#[test]
fn graph_start_replace_atomically_replaces_a_graph_task() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let previous = graph_state(370, 380, 10);
    store
        .create_graph_session_v2(&identity(), previous.clone())
        .unwrap();
    let next = graph_state(390, 400, 22);
    let claimed = admit_and_claim(
        &store,
        DomainCommand::SessionStartReplace,
        "v2-start-replace",
        410,
        AdmissionSessionIdentityV1::Exact(previous.trace().session_id().clone()),
        Some(Revision::new(1)),
    );
    store
        .commit_graph_start_terminal_v2(
            claimed.claim().clone(),
            GraphStartCurrentTaskV2::Exact {
                session_id: previous.trace().session_id().clone(),
                session_revision: Revision::new(1),
            },
            next.clone(),
            UnixMillis::new(23),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
}

#[test]
fn graph_start_absent_fence_failure_preserves_prior_and_requeues_claim() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let previous = graph_state(460, 470, 10);
    let claimed = admit_and_claim(
        &store,
        DomainCommand::SessionStart,
        "v2-start-absent-stale",
        480,
        AdmissionSessionIdentityV1::Absent,
        None,
    );
    let job_id = claimed.job().job_id().clone();
    store
        .create_graph_session_v2(&identity(), previous.clone())
        .unwrap();

    assert!(
        store
            .commit_graph_start_terminal_v2(
                claimed.claim().clone(),
                GraphStartCurrentTaskV2::Absent,
                graph_state(490, 500, 22),
                UnixMillis::new(23),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(previous.clone())
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 24);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    assert_eq!(
        reopened
            .read_graph_session_v2(&identity())
            .unwrap()
            .unwrap()
            .trace()
            .session_id(),
        previous.trace().session_id()
    );
}

#[test]
fn graph_start_replace_stale_terminal_fence_preserves_prior_and_claim() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let previous = graph_state(510, 520, 10);
    store
        .create_graph_session_v2(&identity(), previous.clone())
        .unwrap();
    let claimed = admit_and_claim(
        &store,
        DomainCommand::SessionStartReplace,
        "v2-replace-terminal-stale",
        530,
        AdmissionSessionIdentityV1::Exact(previous.trace().session_id().clone()),
        Some(previous.trace().revision()),
    );
    let job_id = claimed.job().job_id().clone();

    assert!(matches!(
        store.commit_graph_start_terminal_v2(
            claimed.claim().clone(),
            GraphStartCurrentTaskV2::Exact {
                session_id: previous.trace().session_id().clone(),
                session_revision: Revision::new(2),
            },
            graph_state(540, 550, 22),
            UnixMillis::new(23),
        ),
        Err(podway_store::StoreErrorV1::PreconditionConflictV1 {
            expected: Some(expected),
            actual: Some(actual),
        }) if expected == Revision::new(2) && actual == previous.trace().revision()
    ));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(previous)
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
}

#[test]
fn graph_start_replace_failpoint_rolls_back_prior_state_and_receipt() {
    let temporary = TempDir::new().unwrap();
    let previous = graph_state(560, 570, 10);
    {
        let seed = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        seed.create_graph_session_v2(&identity(), previous.clone())
            .unwrap();
    }
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(
            StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        ))
        .with_failpoint_action(StoreFailpointActionV1::ReturnInjectedStorageIo);
    let store = open(&temporary, options, 11);
    let claimed = admit_and_claim(
        &store,
        DomainCommand::SessionStartReplace,
        "v2-replace-rollback",
        580,
        AdmissionSessionIdentityV1::Exact(previous.trace().session_id().clone()),
        Some(previous.trace().revision()),
    );
    let job_id = claimed.job().job_id().clone();

    assert!(
        store
            .commit_graph_start_terminal_v2(
                claimed.claim().clone(),
                GraphStartCurrentTaskV2::Exact {
                    session_id: previous.trace().session_id().clone(),
                    session_revision: previous.trace().revision(),
                },
                graph_state(590, 600, 22),
                UnixMillis::new(23),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(previous.clone())
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 24);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(previous)
    );
}

#[test]
fn graph_complete_state_and_typed_terminal_projection_commit_and_reopen_together() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = graph_state(610, 620, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_complete_and_claim(&store, &current, "v2-complete", 630);
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let completed = current
        .complete_active_action_v2(
            current.trace().revision(),
            active.attempt_id(),
            None,
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::complete(
        completed.from_graph_node_id().clone(),
        completed.from_attempt_id().clone(),
        None,
        None,
    )
    .unwrap();
    let next = completed.into_state();

    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            Some(next.clone()),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: current.trace().session_id().clone(),
                revision_before: current.trace().revision(),
                revision_after: next.trace().revision(),
                changed: true,
            }),
            operation.clone(),
            UnixMillis::new(33),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(next.clone())
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Succeeded);
    assert_eq!(
        job.terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(&operation)
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
    assert_eq!(
        reopened
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(&operation)
    );
}

#[test]
fn decision_terminal_rejects_successor_identity_that_differs_from_admitted_execution() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = decision_graph_state(640, 650, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let admitted_fresh_attempt_id = AttemptId::new(uuid(660)).unwrap();
    let forged_fresh_attempt_id = AttemptId::new(uuid(661)).unwrap();
    let claimed = admit_decide_and_claim(
        &store,
        &current,
        &admitted_fresh_attempt_id,
        "v2-decide-successor-identity",
        670,
    );
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let forged = current
        .decide_active_route_v2(
            current.trace().revision(),
            active.attempt_id(),
            OptionId::new("approve").unwrap(),
            forged_fresh_attempt_id.clone(),
            Some(ReasonV2::new("Ready.").unwrap()),
            None,
            UnixMillis::new(33),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::decide(
        decision_record_projection(forged.decision_record()),
        forged_fresh_attempt_id,
    )
    .unwrap();
    let next = forged.into_state();

    assert!(matches!(
        store.commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            Some(next.clone()),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: current.trace().session_id().clone(),
                revision_before: current.trace().revision(),
                revision_after: next.trace().revision(),
                changed: true,
            }),
            operation,
            UnixMillis::new(33),
        ),
        Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::TransitionMutationShape
        })
    ));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
}

#[test]
fn graph_reset_clear_receipt_and_failpoint_are_one_atomic_boundary() {
    let temporary = TempDir::new().unwrap();
    let initial = graph_state(670, 680, 10);
    let seed = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    seed.create_graph_session_v2(&identity(), initial.clone())
        .unwrap();
    let prior_claim = admit_complete_and_claim(&seed, &initial, "v2-prior-terminal", 685);
    let prior_job_id = prior_claim.job().job_id().clone();
    let prior_key = IdempotencyKeyV1::new("v2-prior-terminal").unwrap();
    let completed = initial
        .complete_active_action_v2(
            initial.trace().revision(),
            initial.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::complete(
        completed.from_graph_node_id().clone(),
        completed.from_attempt_id().clone(),
        None,
        None,
    )
    .unwrap();
    let current = completed.into_state();
    seed.commit_graph_mutation_terminal_v2(
        prior_claim.claim().clone(),
        initial.workspace_revision(),
        initial.trace().revision(),
        Some(current.clone()),
        TerminalResultV1::Success(DomainResult::SessionChanged {
            session_id: initial.trace().session_id().clone(),
            revision_before: initial.trace().revision(),
            revision_after: current.trace().revision(),
            changed: true,
        }),
        operation,
        UnixMillis::new(33),
    )
    .unwrap();
    let initial_claim = admit_reset_and_claim(&seed, &current, "v2-reset-atomic", 690);
    let job_id = initial_claim.job().job_id().clone();
    let reset_key = IdempotencyKeyV1::new("v2-reset-atomic").unwrap();
    drop(seed);
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(
            StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        ))
        .with_failpoint_action(StoreFailpointActionV1::ReturnInjectedStorageIo);
    let store = open(&temporary, options, 11);
    let claimed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-reset-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .commit_graph_reset_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                current.trace().session_id().clone(),
                UnixMillis::new(33),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        store
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .state(),
        JobStateV1::Running
    );
    assert!(
        store
            .read_job(&identity(), &prior_job_id)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .read_idempotency_lookup(&identity(), &prior_key)
            .unwrap()
            .is_some()
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    let claimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-reset-retry").unwrap(),
            UnixMillis::new(35),
        )
        .unwrap()
        .unwrap();
    reopened
        .commit_graph_reset_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            current.trace().session_id().clone(),
            UnixMillis::new(36),
        )
        .unwrap();
    assert_eq!(reopened.read_graph_session_v2(&identity()).unwrap(), None);
    assert!(
        reopened
            .read_job(&identity(), &prior_job_id)
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .read_idempotency_lookup(&identity(), &prior_key)
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .read_idempotency_lookup(&identity(), &reset_key)
            .unwrap()
            .is_some()
    );
    let receipt = reopened
        .read_job(&identity(), &job_id)
        .unwrap()
        .unwrap()
        .terminal_receipt()
        .unwrap()
        .clone();
    assert!(
        matches!(receipt.graph_session_projection().unwrap().operation(), Some(PersistedGraphTerminalOperationV2::Reset { session_id }) if session_id == current.trace().session_id())
    );
    drop(reopened);
    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 37);
    assert_eq!(reopened.read_graph_session_v2(&identity()).unwrap(), None);
    assert!(
        reopened
            .read_idempotency_lookup(&identity(), &reset_key)
            .unwrap()
            .is_some()
    );
    drop(reopened);

    let mut forged: serde_json::Value =
        serde_json::from_str(&encode_persisted_terminal_receipt_v1(&receipt).unwrap()).unwrap();
    forged["graph_session_projection"]["operation"]["session_id"] =
        serde_json::Value::String(uuid(691));
    let forged = canonicalize_json_v1(&forged).unwrap();
    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE jobs SET terminal_response_json = ?1 WHERE job_id = ?2",
            [forged.as_str(), job_id.as_str()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE idempotency_records SET terminal_response_json = ?1 WHERE job_id = ?2",
            [forged.as_str(), job_id.as_str()],
        )
        .unwrap();
    drop(connection);
    assert!(
        SqliteStoreV1::open(
            database_path(&temporary),
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(38),
        )
        .is_err()
    );
}

#[test]
fn graph_reset_stale_admission_terminalizes_and_replays_revision_conflict() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let initial = graph_state(695, 696, 10);
    store
        .create_graph_session_v2(&identity(), initial.clone())
        .unwrap();
    let claimed = admit_reset_and_claim(&store, &initial, "v2-reset-stale", 697);
    let job_id = claimed.job().job_id().clone();
    let active = initial.trace().active_attempt().unwrap();
    let advanced = initial
        .block_active_attempt_v2(
            initial.trace().revision(),
            active.attempt_id(),
            podway_core::BlockerId::new(uuid(698)).unwrap(),
            "Earlier queued mutation.",
            UnixMillis::new(32),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            initial.workspace_revision(),
            initial.trace().revision(),
            advanced.clone(),
        )
        .unwrap();
    let failure = PersistedGraphMutationFailureV2::SessionRevisionConflict {
        expected: initial.trace().revision(),
        actual: advanced.trace().revision(),
    };
    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            advanced.workspace_revision(),
            advanced.trace().revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            PersistedGraphTerminalOperationV2::failure(failure.clone()).unwrap(),
            UnixMillis::new(33),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(advanced.clone())
    );
    assert!(
        matches!(store.read_job(&identity(), &job_id).unwrap().unwrap().terminal_receipt().unwrap().graph_session_projection().unwrap().operation(), Some(PersistedGraphTerminalOperationV2::Failure { error }) if error == &failure)
    );
    drop(store);
    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(advanced)
    );
    assert!(
        reopened
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_some()
    );
}

#[test]
fn graph_reset_terminal_rejects_equal_revision_conflict_without_mutation() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = graph_state(699, 700, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_reset_and_claim(&store, &current, "v2-reset-equal-conflict", 701);
    let job_id = claimed.job().job_id().clone();
    let failure = PersistedGraphMutationFailureV2::SessionRevisionConflict {
        expected: current.trace().revision(),
        actual: current.trace().revision(),
    };
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                None,
                TerminalResultV1::Failure(DomainError::InvalidState {
                    reason: "Procedure v2 graph mutation failed",
                }),
                PersistedGraphTerminalOperationV2::failure(failure).unwrap(),
                UnixMillis::new(33),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
}

#[test]
fn graph_block_and_unblock_terminal_reject_unrelated_workflow_changes() {
    let block_temporary = TempDir::new().unwrap();
    let block_store = open(&block_temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let initial = graph_state(700, 701, 10);
    block_store
        .create_graph_session_v2(&identity(), initial.clone())
        .unwrap();
    let active = initial.trace().active_attempt().unwrap();
    let named = podway_core::BlockerId::new(uuid(702)).unwrap();
    let extra = podway_core::BlockerId::new(uuid(703)).unwrap();
    let claimed = admit_blocker_command_and_claim(
        &block_store,
        &initial,
        DomainCommand::SessionBlock,
        "session.block",
        json!({"reason":"Named blocker."}),
        Some(&named),
        "v2-forged-block",
        704,
    );
    let job_id = claimed.job().job_id().clone();
    let legitimate = initial
        .block_active_attempt_v2(
            initial.trace().revision(),
            active.attempt_id(),
            named.clone(),
            "Named blocker.",
            UnixMillis::new(32),
        )
        .unwrap()
        .into_state();
    let with_extra = legitimate
        .block_active_attempt_v2(
            legitimate.trace().revision(),
            legitimate.trace().active_attempt().unwrap().attempt_id(),
            extra,
            "Unrelated blocker.",
            UnixMillis::new(33),
        )
        .unwrap()
        .into_state();
    let with_unrelated_item_change = with_extra
        .mutate_active_item_v2(
            with_extra.trace().active_attempt().unwrap().attempt_id(),
            &ItemId::new("done").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Check,
            UnixMillis::new(34),
        )
        .unwrap()
        .into_state();
    let forged = GraphSessionStateV2::new_with_goal_state(
        legitimate.workspace_revision(),
        legitimate.task_title(),
        legitimate.snapshot().clone(),
        legitimate.trace().clone(),
        legitimate.counters().to_vec(),
        legitimate.attempt_metadata().to_vec(),
        with_unrelated_item_change.workflow_memory().clone(),
        legitimate.goal_state().clone(),
        legitimate.created_at(),
        legitimate.completed_at(),
        legitimate.cancelled_at(),
        legitimate.cancel_reason().map(str::to_owned),
    )
    .unwrap();
    assert!(
        block_store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                initial.workspace_revision(),
                initial.trace().revision(),
                Some(forged),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: initial.trace().session_id().clone(),
                    revision_before: initial.trace().revision(),
                    revision_after: legitimate.trace().revision(),
                    changed: true
                }),
                PersistedGraphTerminalOperationV2::block(
                    active.graph_node_id().clone(),
                    active.attempt_id().clone(),
                    named,
                    "Named blocker.".to_owned()
                )
                .unwrap(),
                UnixMillis::new(35),
            )
            .is_err()
    );
    assert_eq!(
        block_store.read_graph_session_v2(&identity()).unwrap(),
        Some(initial)
    );
    let job = block_store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());

    let unblock_temporary = TempDir::new().unwrap();
    let unblock_store = open(&unblock_temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let base = graph_state(710, 711, 10);
    let first = podway_core::BlockerId::new(uuid(712)).unwrap();
    let second = podway_core::BlockerId::new(uuid(713)).unwrap();
    let blocked_once = base
        .block_active_attempt_v2(
            base.trace().revision(),
            base.trace().active_attempt().unwrap().attempt_id(),
            first.clone(),
            "First.",
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    let blocked = blocked_once
        .block_active_attempt_v2(
            blocked_once.trace().revision(),
            blocked_once.trace().active_attempt().unwrap().attempt_id(),
            second,
            "Second.",
            UnixMillis::new(21),
        )
        .unwrap()
        .into_state();
    unblock_store
        .create_graph_session_v2(&identity(), blocked.clone())
        .unwrap();
    let claimed = admit_blocker_command_and_claim(
        &unblock_store,
        &blocked,
        DomainCommand::SessionUnblock,
        "session.unblock",
        json!({"all":false,"blocker_id":first.as_str()}),
        None,
        "v2-forged-unblock",
        714,
    );
    let job_id = claimed.job().job_id().clone();
    let legitimate = blocked
        .unblock_active_attempt_v2(
            blocked.trace().revision(),
            blocked.trace().active_attempt().unwrap().attempt_id(),
            Some(&first),
            false,
            UnixMillis::new(32),
        )
        .unwrap()
        .into_state();
    let with_extra_resolution = legitimate
        .unblock_active_attempt_v2(
            legitimate.trace().revision(),
            legitimate.trace().active_attempt().unwrap().attempt_id(),
            None,
            true,
            UnixMillis::new(33),
        )
        .unwrap()
        .into_state();
    let with_unrelated_item_change = with_extra_resolution
        .mutate_active_item_v2(
            with_extra_resolution
                .trace()
                .active_attempt()
                .unwrap()
                .attempt_id(),
            &ItemId::new("done").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Check,
            UnixMillis::new(34),
        )
        .unwrap()
        .into_state();
    let forged = GraphSessionStateV2::new_with_goal_state(
        legitimate.workspace_revision(),
        legitimate.task_title(),
        legitimate.snapshot().clone(),
        legitimate.trace().clone(),
        legitimate.counters().to_vec(),
        legitimate.attempt_metadata().to_vec(),
        with_unrelated_item_change.workflow_memory().clone(),
        legitimate.goal_state().clone(),
        legitimate.created_at(),
        legitimate.completed_at(),
        legitimate.cancelled_at(),
        legitimate.cancel_reason().map(str::to_owned),
    )
    .unwrap();
    assert!(
        unblock_store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                blocked.workspace_revision(),
                blocked.trace().revision(),
                Some(forged),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: blocked.trace().session_id().clone(),
                    revision_before: blocked.trace().revision(),
                    revision_after: legitimate.trace().revision(),
                    changed: true
                }),
                PersistedGraphTerminalOperationV2::unblock(
                    blocked
                        .trace()
                        .active_attempt()
                        .unwrap()
                        .graph_node_id()
                        .clone(),
                    blocked
                        .trace()
                        .active_attempt()
                        .unwrap()
                        .attempt_id()
                        .clone(),
                    false,
                    vec![first]
                )
                .unwrap(),
                UnixMillis::new(35),
            )
            .is_err()
    );
    assert_eq!(
        unblock_store.read_graph_session_v2(&identity()).unwrap(),
        Some(blocked)
    );
    let job = unblock_store
        .read_job(&identity(), &job_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
}

#[test]
fn graph_block_and_unblock_terminal_reject_forged_transition_timestamps() {
    let block_temporary = TempDir::new().unwrap();
    let block_store = open(&block_temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let initial = graph_state(720, 721, 10);
    block_store
        .create_graph_session_v2(&identity(), initial.clone())
        .unwrap();
    let active = initial.trace().active_attempt().unwrap();
    let blocker_id = podway_core::BlockerId::new(uuid(722)).unwrap();
    let claimed = admit_blocker_command_and_claim(
        &block_store,
        &initial,
        DomainCommand::SessionBlock,
        "session.block",
        json!({"reason":"Timestamp-bound blocker."}),
        Some(&blocker_id),
        "v2-forged-block-timestamp",
        723,
    );
    let job_id = claimed.job().job_id().clone();
    let forged = initial
        .block_active_attempt_v2(
            initial.trace().revision(),
            active.attempt_id(),
            blocker_id.clone(),
            "Timestamp-bound blocker.",
            UnixMillis::new(31),
        )
        .unwrap()
        .into_state();
    assert!(
        block_store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                initial.workspace_revision(),
                initial.trace().revision(),
                Some(forged.clone()),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: initial.trace().session_id().clone(),
                    revision_before: initial.trace().revision(),
                    revision_after: forged.trace().revision(),
                    changed: true,
                }),
                PersistedGraphTerminalOperationV2::block(
                    active.graph_node_id().clone(),
                    active.attempt_id().clone(),
                    blocker_id,
                    "Timestamp-bound blocker.".to_owned(),
                )
                .unwrap(),
                UnixMillis::new(32),
            )
            .is_err()
    );
    assert_eq!(
        block_store.read_graph_session_v2(&identity()).unwrap(),
        Some(initial)
    );
    let job = block_store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());

    let unblock_temporary = TempDir::new().unwrap();
    let unblock_store = open(&unblock_temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let base = graph_state(730, 731, 10);
    let blocker_id = podway_core::BlockerId::new(uuid(732)).unwrap();
    let blocked = base
        .block_active_attempt_v2(
            base.trace().revision(),
            base.trace().active_attempt().unwrap().attempt_id(),
            blocker_id.clone(),
            "Timestamp-bound blocker.",
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    unblock_store
        .create_graph_session_v2(&identity(), blocked.clone())
        .unwrap();
    let active = blocked.trace().active_attempt().unwrap();
    let claimed = admit_blocker_command_and_claim(
        &unblock_store,
        &blocked,
        DomainCommand::SessionUnblock,
        "session.unblock",
        json!({"all":false,"blocker_id":blocker_id.as_str()}),
        None,
        "v2-forged-unblock-timestamp",
        733,
    );
    let job_id = claimed.job().job_id().clone();
    let forged = blocked
        .unblock_active_attempt_v2(
            blocked.trace().revision(),
            active.attempt_id(),
            Some(&blocker_id),
            false,
            UnixMillis::new(31),
        )
        .unwrap()
        .into_state();
    assert!(
        unblock_store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                blocked.workspace_revision(),
                blocked.trace().revision(),
                Some(forged.clone()),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: blocked.trace().session_id().clone(),
                    revision_before: blocked.trace().revision(),
                    revision_after: forged.trace().revision(),
                    changed: true,
                }),
                PersistedGraphTerminalOperationV2::unblock(
                    active.graph_node_id().clone(),
                    active.attempt_id().clone(),
                    false,
                    vec![blocker_id],
                )
                .unwrap(),
                UnixMillis::new(32),
            )
            .is_err()
    );
    assert_eq!(
        unblock_store.read_graph_session_v2(&identity()).unwrap(),
        Some(blocked)
    );
    let job = unblock_store
        .read_job(&identity(), &job_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
}

#[test]
fn graph_skip_state_and_typed_terminal_projection_commit_and_reopen_together() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = graph_state(611, 621, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_skip_and_claim(&store, &current, "v2-skip", 631);
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let reason = ReasonV2::new("Not applicable.").unwrap();
    let skipped = current
        .skip_active_action_v2(
            current.trace().revision(),
            active.attempt_id(),
            None,
            Some(reason.clone()),
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::skip(
        skipped.from_graph_node_id().clone(),
        skipped.from_attempt_id().clone(),
        None,
        None,
        Some(reason),
    )
    .unwrap();
    let next = skipped.into_state();

    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            Some(next.clone()),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: current.trace().session_id().clone(),
                revision_before: current.trace().revision(),
                revision_after: next.trace().revision(),
                changed: true,
            }),
            operation.clone(),
            UnixMillis::new(33),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(next.clone())
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Succeeded);
    assert_eq!(
        job.terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(&operation)
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
    assert_eq!(
        reopened
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(&operation)
    );
}

#[test]
fn sqlite_full_rolls_back_rich_v2_terminal_mutation_and_retry_commits_once() {
    let temporary = TempDir::new().unwrap();
    let path = database_path(&temporary);
    let current = graph_state(612, 622, 10);
    let key = IdempotencyKeyV1::new("v2-set-sqlite-full").unwrap();
    let value = "x".repeat(16_384);
    let job_id = podway_core::JobId::new(uuid(632)).unwrap();
    {
        let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        store
            .create_graph_session_v2(&identity(), current.clone())
            .unwrap();
        admit_set_with_value(&store, &current, key.as_str(), 632, &value);
    }

    let constrained_page_count = {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA wal_checkpoint(TRUNCATE); \
                 VACUUM;",
            )
            .unwrap();
        let page_count = connection
            .query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))
            .unwrap();
        let freelist_count = connection
            .query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(freelist_count, 0, "VACUUM must remove reusable pages");
        page_count
    };

    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 20);
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-sqlite-full-worker").unwrap(),
            UnixMillis::new(31),
        )
        .unwrap()
        .unwrap();
    drop(store);

    let store = open(
        &temporary,
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_max_page_count_for_test(u32::try_from(constrained_page_count).unwrap())
            .unwrap(),
        32,
    );
    assert_eq!(store.startup_recovery_report().requeued_job_count(), 1);
    let claimed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-sqlite-full-worker").unwrap(),
            UnixMillis::new(32),
        )
        .unwrap()
        .unwrap();
    assert_eq!(claimed.job().job_id(), &job_id);
    let active = current.trace().active_attempt().unwrap();
    let item_id = ItemId::new("note").unwrap();
    let mutated = current
        .mutate_active_item_v2(
            active.attempt_id(),
            &item_id,
            Revision::ZERO,
            ActiveItemMutationV2::Set { value },
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::item_mutation(
        active.graph_node_id().clone(),
        active.attempt_id().clone(),
        active.number(),
        item_id.clone(),
        None,
    )
    .unwrap();
    let next = mutated.into_state();
    let success = TerminalResultV1::Success(DomainResult::ItemChanged {
        session_id: current.trace().session_id().clone(),
        item_id,
        revision_before: current.trace().revision(),
        revision_after: next.trace().revision(),
        changed: true,
    });

    assert_eq!(
        store.commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            Some(next.clone()),
            success.clone(),
            operation.clone(),
            UnixMillis::new(33),
        ),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::StorageIo,
        })
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone()),
        "SQLITE_FULL must not advance the graph cursor or session revision"
    );
    let running = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(running.state(), JobStateV1::Running);
    assert!(running.terminal_receipt().is_none());
    assert!(
        store
            .read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    let reclaimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-sqlite-full-retry-worker").unwrap(),
            UnixMillis::new(35),
        )
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.job().job_id(), &job_id);
    reopened
        .commit_graph_mutation_terminal_v2(
            reclaimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            Some(next.clone()),
            success,
            operation.clone(),
            UnixMillis::new(36),
        )
        .unwrap();
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
    let succeeded = reopened.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(succeeded.state(), JobStateV1::Succeeded);
    let frozen = succeeded.terminal_receipt().unwrap();
    assert_eq!(
        frozen.graph_session_projection().unwrap().operation(),
        Some(&operation)
    );
    assert_eq!(
        reopened
            .read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(frozen)
    );
}

#[test]
fn graph_complete_failpoint_rolls_back_state_and_terminal_receipt() {
    let temporary = TempDir::new().unwrap();
    let current = graph_state(640, 650, 10);
    {
        let seed = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        seed.create_graph_session_v2(&identity(), current.clone())
            .unwrap();
    }
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(
            StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        ))
        .with_failpoint_action(StoreFailpointActionV1::ReturnInjectedStorageIo);
    let store = open(&temporary, options, 11);
    let claimed = admit_complete_and_claim(&store, &current, "v2-complete-rollback", 660);
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let completed = current
        .complete_active_action_v2(
            current.trace().revision(),
            active.attempt_id(),
            None,
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::complete(
        completed.from_graph_node_id().clone(),
        completed.from_attempt_id().clone(),
        None,
        None,
    )
    .unwrap();
    let next = completed.into_state();
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                Some(next.clone()),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: current.trace().session_id().clone(),
                    revision_before: current.trace().revision(),
                    revision_after: next.trace().revision(),
                    changed: true,
                }),
                operation,
                UnixMillis::new(33),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
}

#[test]
fn graph_skip_failpoint_rolls_back_state_receipt_and_requeues_after_reopen() {
    let temporary = TempDir::new().unwrap();
    let initial = graph_state(641, 651, 10);
    let current = initial
        .mutate_active_item_v2(
            initial.trace().active_attempt().unwrap().attempt_id(),
            &ItemId::new("done").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Check,
            UnixMillis::new(11),
        )
        .unwrap()
        .into_state();
    let recorded_before = current.workflow_memory().attempts()[0].item_slots()[0]
        .value()
        .cloned();
    assert!(recorded_before.is_some());
    {
        let seed = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        seed.create_graph_session_v2(&identity(), current.clone())
            .unwrap();
    }
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(
            StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        ))
        .with_failpoint_action(StoreFailpointActionV1::ReturnInjectedStorageIo);
    let store = open(&temporary, options, 11);
    let claimed = admit_skip_and_claim(&store, &current, "v2-skip-rollback", 661);
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let reason = ReasonV2::new("Not applicable.").unwrap();
    let skipped = current
        .skip_active_action_v2(
            current.trace().revision(),
            active.attempt_id(),
            None,
            Some(reason.clone()),
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::skip(
        skipped.from_graph_node_id().clone(),
        skipped.from_attempt_id().clone(),
        None,
        None,
        Some(reason),
    )
    .unwrap();
    let next = skipped.into_state();

    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                Some(next.clone()),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: current.trace().session_id().clone(),
                    revision_before: current.trace().revision(),
                    revision_after: next.trace().revision(),
                    changed: true,
                }),
                operation,
                UnixMillis::new(33),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        store
            .read_graph_session_v2(&identity())
            .unwrap()
            .unwrap()
            .workflow_memory()
            .attempts()[0]
            .item_slots()[0]
            .value(),
        recorded_before.as_ref()
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        reopened
            .read_graph_session_v2(&identity())
            .unwrap()
            .unwrap()
            .workflow_memory()
            .attempts()[0]
            .item_slots()[0]
            .value(),
        recorded_before.as_ref()
    );
    let recovered_job = reopened.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(recovered_job.state(), JobStateV1::Queued);
    assert!(recovered_job.terminal_receipt().is_none());
}

#[test]
fn graph_skip_terminal_rejects_reason_drift_and_reason_present_missing_failure() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = graph_state(642, 652, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_skip_and_claim(&store, &current, "v2-skip-reason-forgery", 662);
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let forged_reason = ReasonV2::new("Forged after admission.").unwrap();
    let skipped = current
        .skip_active_action_v2(
            current.trace().revision(),
            active.attempt_id(),
            None,
            Some(forged_reason.clone()),
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::skip(
        skipped.from_graph_node_id().clone(),
        skipped.from_attempt_id().clone(),
        None,
        None,
        Some(forged_reason),
    )
    .unwrap();
    let next = skipped.into_state();

    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                Some(next.clone()),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: current.trace().session_id().clone(),
                    revision_before: current.trace().revision(),
                    revision_after: next.trace().revision(),
                    changed: true,
                }),
                operation,
                UnixMillis::new(33),
            )
            .is_err()
    );
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                None,
                TerminalResultV1::Failure(DomainError::InvalidState {
                    reason: "Procedure v2 graph mutation failed",
                }),
                PersistedGraphTerminalOperationV2::failure(
                    PersistedGraphMutationFailureV2::SkipReasonRequired {
                        graph_node_id: node("work"),
                    },
                )
                .unwrap(),
                UnixMillis::new(34),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Running);
    assert!(job.terminal_receipt().is_none());
}

#[test]
fn graph_item_no_op_commits_terminal_receipt_without_replacing_state() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = graph_state(670, 680, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_uncheck_and_claim(&store, &current, "v2-item-no-op", 690);
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let item_id = ItemId::new("done").unwrap();
    let outcome = current
        .mutate_active_item_v2(
            active.attempt_id(),
            &item_id,
            Revision::ZERO,
            ActiveItemMutationV2::Uncheck,
            UnixMillis::new(32),
        )
        .unwrap();
    assert!(!outcome.changed());
    let operation = PersistedGraphTerminalOperationV2::item_mutation(
        active.graph_node_id().clone(),
        active.attempt_id().clone(),
        active.number(),
        item_id.clone(),
        None,
    )
    .unwrap();

    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            None,
            TerminalResultV1::Success(DomainResult::ItemChanged {
                session_id: current.trace().session_id().clone(),
                item_id,
                revision_before: current.trace().revision(),
                revision_after: current.trace().revision(),
                changed: false,
            }),
            operation.clone(),
            UnixMillis::new(33),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Succeeded);
    assert_eq!(
        job.terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(&operation)
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
}

#[test]
fn graph_item_batch_rolls_back_then_recovers_and_commits_one_durable_effect() {
    let temporary = TempDir::new().unwrap();
    let current = graph_state(6_700, 6_710, 10);
    {
        let seed = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        seed.create_graph_session_v2(&identity(), current.clone())
            .unwrap();
    }
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(
            StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        ))
        .with_failpoint_action(StoreFailpointActionV1::ReturnInjectedStorageIo);
    let store = open(&temporary, options, 11);
    let claimed =
        admit_item_record_many_and_claim(&store, &current, "v2-item-batch-rollback", 6_720);
    let job_id = claimed.job().job_id().clone();
    let active = current.trace().active_attempt().unwrap();
    let outcome = current
        .mutate_active_items_v2(
            active.attempt_id(),
            &[
                ActiveItemMutationRequestV2::new(
                    ItemId::new("note").unwrap(),
                    Revision::ZERO,
                    ActiveItemMutationV2::Set {
                        value: "recorded".to_owned(),
                    },
                ),
                ActiveItemMutationRequestV2::new(
                    ItemId::new("done").unwrap(),
                    Revision::ZERO,
                    ActiveItemMutationV2::Check,
                ),
            ],
            UnixMillis::new(32),
        )
        .unwrap();
    let operation = PersistedGraphTerminalOperationV2::item_mutations(
        active.graph_node_id().clone(),
        active.attempt_id().clone(),
        active.number(),
        outcome
            .items()
            .iter()
            .map(|item| {
                PersistedGraphItemMutationV2::new(
                    item.item_id().clone(),
                    Revision::ZERO,
                    item.changed(),
                    item.item_revision(),
                    item.value_digest().cloned(),
                )
            })
            .collect(),
    )
    .unwrap();
    let forged_operation = PersistedGraphTerminalOperationV2::item_mutations(
        active.graph_node_id().clone(),
        active.attempt_id().clone(),
        active.number(),
        outcome
            .items()
            .iter()
            .map(|item| {
                PersistedGraphItemMutationV2::new(
                    item.item_id().clone(),
                    Revision::new(1),
                    item.changed(),
                    item.item_revision(),
                    item.value_digest().cloned(),
                )
            })
            .collect(),
    )
    .unwrap();
    let next = outcome.into_state();
    let success = TerminalResultV1::Success(DomainResult::ItemsChanged {
        session_id: current.trace().session_id().clone(),
        revision_before: current.trace().revision(),
        revision_after: next.trace().revision(),
        changed: true,
    });

    assert_eq!(
        store.commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            Some(next.clone()),
            success.clone(),
            forged_operation,
            UnixMillis::new(33),
        ),
        Err(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::TransitionMutationShape,
        })
    );

    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                Some(next.clone()),
                success.clone(),
                operation.clone(),
                UnixMillis::new(33),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let running = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(running.state(), JobStateV1::Running);
    assert!(running.terminal_receipt().is_none());
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    let reclaimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-item-batch-recovery-worker").unwrap(),
            UnixMillis::new(35),
        )
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.job().job_id(), &job_id);
    reopened
        .commit_graph_mutation_terminal_v2(
            reclaimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            Some(next.clone()),
            success,
            operation.clone(),
            UnixMillis::new(36),
        )
        .unwrap();
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(next.clone())
    );
    let succeeded = reopened.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(succeeded.state(), JobStateV1::Succeeded);
    let receipt = succeeded.terminal_receipt().unwrap();
    assert_eq!(
        receipt.graph_session_projection().unwrap().operation(),
        Some(&operation)
    );
    assert_eq!(
        reopened
            .read_idempotency_lookup(
                &identity(),
                &IdempotencyKeyV1::new("v2-item-batch-rollback").unwrap(),
            )
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(receipt)
    );
    drop(reopened);

    let restarted = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 37);
    assert_eq!(restarted.startup_recovery_report().requeued_job_count(), 0);
    assert_eq!(
        restarted.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
}

#[test]
fn graph_mutation_failure_replays_exact_v2_error_without_mutating_state() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let initial = graph_state_with_required_artifact(700, 710, 10, true);
    let active = initial.trace().active_attempt().unwrap();
    let with_artifact = initial
        .mutate_active_item_v2(
            active.attempt_id(),
            &ItemId::new("proof").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Attach {
                value: ArtifactValueV1::local_path("proof.txt", digest('c'), 42, "text/plain")
                    .unwrap(),
            },
            UnixMillis::new(11),
        )
        .unwrap()
        .into_state();
    let active = with_artifact.trace().active_attempt().unwrap();
    let current = with_artifact
        .mutate_active_item_v2(
            active.attempt_id(),
            &ItemId::new("done").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Check,
            UnixMillis::new(12),
        )
        .unwrap()
        .into_state();
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_complete_and_claim(&store, &current, "v2-complete-artifact-changed", 720);
    let job_id = claimed.job().job_id().clone();
    let operation = PersistedGraphTerminalOperationV2::failure(
        PersistedGraphMutationFailureV2::ArtifactChanged,
    )
    .unwrap();

    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            None,
            TerminalResultV1::Failure(podway_core::DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            operation.clone(),
            UnixMillis::new(33),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let job = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(job.state(), JobStateV1::Failed);
    assert_eq!(
        job.terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(&operation)
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    assert_eq!(
        reopened
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(&operation)
    );
}

#[test]
fn v2drw006_graph_mutation_failure_precommit_io_rolls_back_and_retry_freezes_one_receipt() {
    let temporary = TempDir::new().unwrap();
    let initial = graph_state_with_required_artifact(820, 830, 10, true);
    let active = initial.trace().active_attempt().unwrap();
    let with_artifact = initial
        .mutate_active_item_v2(
            active.attempt_id(),
            &ItemId::new("proof").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Attach {
                value: ArtifactValueV1::local_path("proof.txt", digest('e'), 42, "text/plain")
                    .unwrap(),
            },
            UnixMillis::new(11),
        )
        .unwrap()
        .into_state();
    let current = with_artifact
        .mutate_active_item_v2(
            with_artifact.trace().active_attempt().unwrap().attempt_id(),
            &ItemId::new("done").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Check,
            UnixMillis::new(12),
        )
        .unwrap()
        .into_state();
    {
        let seed = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        seed.create_graph_session_v2(&identity(), current.clone())
            .unwrap();
    }
    let key = IdempotencyKeyV1::new("v2-failure-precommit-io").unwrap();
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_failpoint(Some(StoreFailpointV1::TerminalFailureBeforeCommit))
        .with_failpoint_action(StoreFailpointActionV1::ReturnInjectedStorageIo);
    let store = open(&temporary, options, 20);
    let claimed = admit_complete_and_claim(&store, &current, key.as_str(), 840);
    let job_id = claimed.job().job_id().clone();
    let operation = PersistedGraphTerminalOperationV2::failure(
        PersistedGraphMutationFailureV2::ArtifactChanged,
    )
    .unwrap();
    let failure_result = TerminalResultV1::Failure(DomainError::InvalidState {
        reason: "Procedure v2 graph mutation failed",
    });

    assert_eq!(
        store.commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            None,
            failure_result.clone(),
            operation.clone(),
            UnixMillis::new(33),
        ),
        Err(podway_store::StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::StorageIo,
        })
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let running = store.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(running.state(), JobStateV1::Running);
    assert!(running.terminal_receipt().is_none());
    assert!(
        store
            .read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 34);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let recovered = reopened.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(recovered.state(), JobStateV1::Queued);
    assert!(recovered.terminal_receipt().is_none());
    assert_eq!(
        reopened
            .read_workspace_view(&identity())
            .unwrap()
            .queued_job_count(),
        1
    );
    let reclaimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-failure-retry-worker").unwrap(),
            UnixMillis::new(35),
        )
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.job().job_id(), &job_id);
    reopened
        .commit_graph_mutation_terminal_v2(
            reclaimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            None,
            failure_result,
            operation.clone(),
            UnixMillis::new(36),
        )
        .unwrap();
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let failed = reopened.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(failed.state(), JobStateV1::Failed);
    let frozen = failed.terminal_receipt().unwrap().clone();
    assert_eq!(
        frozen.graph_session_projection().unwrap().operation(),
        Some(&operation)
    );
    assert_eq!(
        reopened
            .read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&frozen)
    );
    drop(reopened);

    let cold = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 37);
    assert_eq!(cold.startup_recovery_report().requeued_job_count(), 0);
    assert_eq!(
        cold.read_workspace_view(&identity())
            .unwrap()
            .queued_job_count(),
        0
    );
    assert_eq!(
        cold.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    assert_eq!(
        cold.read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&frozen)
    );
    assert_eq!(
        cold.read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&frozen)
    );
}

#[test]
fn graph_mutation_failure_precommit_abort_child() {
    let Some(path) = std::env::var_os(V2_FAILURE_CRASH_DATABASE_PATH_ENV).map(PathBuf::from) else {
        return;
    };
    let store = SqliteStoreV1::open(
        path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(Some(StoreFailpointV1::TerminalFailureBeforeCommit))
            .with_failpoint_action(StoreFailpointActionV1::AbortProcess),
        UnixMillis::new(41),
    )
    .unwrap();
    let claimed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-failure-crash-worker").unwrap(),
            UnixMillis::new(42),
        )
        .unwrap()
        .expect("prepared v2 graph failure job must be claimable");
    let current = artifact_failure_ready_state(850, 860);
    let operation = PersistedGraphTerminalOperationV2::failure(
        PersistedGraphMutationFailureV2::ArtifactChanged,
    )
    .unwrap();
    let result = store.commit_graph_mutation_terminal_v2(
        claimed.claim().clone(),
        current.workspace_revision(),
        current.trace().revision(),
        None,
        TerminalResultV1::Failure(DomainError::InvalidState {
            reason: "Procedure v2 graph mutation failed",
        }),
        operation,
        UnixMillis::new(43),
    );
    panic!("configured v2 graph failure failpoint returned instead of aborting: {result:?}");
}

#[test]
fn v2drw006_graph_mutation_failure_precommit_abort_requeues_and_retry_freezes_one_receipt() {
    let temporary = TempDir::new().unwrap();
    let path = database_path(&temporary);
    let current = artifact_failure_ready_state(850, 860);
    let key = IdempotencyKeyV1::new("v2-failure-precommit-abort").unwrap();
    let job_id = podway_core::JobId::new(uuid(870)).unwrap();
    {
        let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
        store
            .create_graph_session_v2(&identity(), current.clone())
            .unwrap();
        let active = current.trace().active_attempt().unwrap();
        let request = action_runtime_admit_request(
            &current,
            DomainCommand::SessionComplete,
            "session.complete",
            key.as_str(),
            870,
            digest('e'),
            RevisionAttemptItemPreconditionsV1::new(
                Some(current.trace().revision()),
                Some(active.attempt_id().clone()),
                None,
                None,
            )
            .unwrap(),
        );
        store.admit(&identity(), request).unwrap();
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(V2_FAILURE_CRASH_CHILD_TEST)
        .arg("--nocapture")
        .env(V2_FAILURE_CRASH_DATABASE_PATH_ENV, &path)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "v2 graph terminal child must abort at TerminalFailureBeforeCommit: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    #[cfg(unix)]
    assert_eq!(
        output.status.signal(),
        Some(6),
        "v2 graph terminal child must reach AbortProcess and terminate with SIGABRT"
    );
    let raw = Connection::open(&path).unwrap();
    let raw_terminal: (String, Option<String>, Option<String>) = raw
        .query_row(
            "SELECT jobs.state, jobs.terminal_response_json, \
             idempotency_records.terminal_response_json \
             FROM jobs JOIN idempotency_records USING (job_id) WHERE jobs.job_id = ?1",
            [job_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(raw_terminal, ("running".to_owned(), None, None));
    drop(raw);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 44);
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 1);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let recovered = reopened.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(recovered.state(), JobStateV1::Queued);
    assert!(recovered.terminal_receipt().is_none());
    assert!(
        reopened
            .read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );

    let reclaimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("v2-failure-crash-retry-worker").unwrap(),
            UnixMillis::new(45),
        )
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.job().job_id(), &job_id);
    let operation = PersistedGraphTerminalOperationV2::failure(
        PersistedGraphMutationFailureV2::ArtifactChanged,
    )
    .unwrap();
    reopened
        .commit_graph_mutation_terminal_v2(
            reclaimed.claim().clone(),
            current.workspace_revision(),
            current.trace().revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            operation.clone(),
            UnixMillis::new(46),
        )
        .unwrap();
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(current.clone())
    );
    let failed = reopened.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(failed.state(), JobStateV1::Failed);
    let frozen = failed.terminal_receipt().unwrap().clone();
    assert_eq!(
        frozen.graph_session_projection().unwrap().operation(),
        Some(&operation)
    );
    assert_eq!(
        reopened
            .read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&frozen)
    );
    drop(reopened);

    let cold = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 47);
    assert_eq!(cold.startup_recovery_report().requeued_job_count(), 0);
    assert_eq!(
        cold.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    assert_eq!(
        cold.read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&frozen)
    );
    assert_eq!(
        cold.read_idempotency_lookup(&identity(), &key)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&frozen)
    );
}

#[test]
fn graph_mutation_failure_rejects_command_and_revision_mismatches_before_commit() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = graph_state(730, 740, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_complete_and_claim(&store, &current, "v2-invalid-failure", 750);
    let job_id = claimed.job().job_id().clone();

    let item_failure =
        PersistedGraphTerminalOperationV2::failure(PersistedGraphMutationFailureV2::ItemNotFound {
            item_id: ItemId::new("confirm").unwrap(),
        })
        .unwrap();
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                None,
                TerminalResultV1::Failure(podway_core::DomainError::InvalidState {
                    reason: "Procedure v2 graph mutation failed",
                }),
                item_failure,
                UnixMillis::new(33),
            )
            .is_err(),
        "a complete job must not persist an item-only failure"
    );

    let wrong_revision = PersistedGraphTerminalOperationV2::failure(
        PersistedGraphMutationFailureV2::SessionRevisionConflict {
            expected: Revision::new(current.trace().revision().get() + 1),
            actual: current.trace().revision(),
        },
    )
    .unwrap();
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                None,
                TerminalResultV1::Failure(podway_core::DomainError::InvalidState {
                    reason: "Procedure v2 graph mutation failed",
                }),
                wrong_revision,
                UnixMillis::new(34),
            )
            .is_err(),
        "persisted conflict facts must match the admitted precondition and current state"
    );

    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    assert_eq!(
        store
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .state(),
        JobStateV1::Running
    );
}

#[test]
fn graph_item_failure_rejects_unreachable_error_and_false_missing_item() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let current = graph_state(760, 770, 10);
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_uncheck_and_claim(&store, &current, "v2-invalid-item-failure", 780);

    for failure in [
        PersistedGraphMutationFailureV2::ListValueNotFound,
        PersistedGraphMutationFailureV2::ItemNotFound {
            item_id: ItemId::new("done").unwrap(),
        },
    ] {
        let operation = PersistedGraphTerminalOperationV2::failure(failure).unwrap();
        assert!(
            store
                .commit_graph_mutation_terminal_v2(
                    claimed.claim().clone(),
                    current.workspace_revision(),
                    current.trace().revision(),
                    None,
                    TerminalResultV1::Failure(podway_core::DomainError::InvalidState {
                        reason: "Procedure v2 graph mutation failed",
                    }),
                    operation,
                    UnixMillis::new(33),
                )
                .is_err(),
            "an item failure must be reachable for the exact command and current item state"
        );
    }

    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
    assert_eq!(
        store
            .read_job(&identity(), claimed.job().job_id())
            .unwrap()
            .unwrap()
            .state(),
        JobStateV1::Running
    );
}

#[test]
fn artifact_failure_cannot_bypass_an_earlier_completion_gate() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let initial = graph_state_with_required_artifact(790, 800, 10, true);
    let active = initial.trace().active_attempt().unwrap();
    let current = initial
        .mutate_active_item_v2(
            active.attempt_id(),
            &ItemId::new("proof").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Attach {
                value: ArtifactValueV1::local_path("proof.txt", digest('d'), 42, "text/plain")
                    .unwrap(),
            },
            UnixMillis::new(11),
        )
        .unwrap()
        .into_state();
    store
        .create_graph_session_v2(&identity(), current.clone())
        .unwrap();
    let claimed = admit_complete_and_claim(&store, &current, "v2-artifact-precedence", 810);
    let operation = PersistedGraphTerminalOperationV2::failure(
        PersistedGraphMutationFailureV2::ArtifactChanged,
    )
    .unwrap();

    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                current.workspace_revision(),
                current.trace().revision(),
                None,
                TerminalResultV1::Failure(podway_core::DomainError::InvalidState {
                    reason: "Procedure v2 graph mutation failed",
                }),
                operation,
                UnixMillis::new(33),
            )
            .is_err(),
        "artifact failure must not bypass the earlier required-item gate"
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(current)
    );
}
