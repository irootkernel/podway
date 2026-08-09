//! Atomic Procedure v2 start publication and durable replay.

use std::path::{Path, PathBuf};

use podway_core::{
    ArtifactValueV1, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2,
    CanonicalProcedureJsonV1, CanonicalProcedureSnapshotInputV1, DomainCommand, DomainResult,
    GraphNodeId, ItemId, ProcedureSnapshotId, ProcedureSnapshotV1, ProcedureSourceLabelV1,
    Revision, SessionAggregateV1, SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2,
    Sha256Digest, TraceSequenceV2, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{
    ActiveItemMutationV2, AdmissionSessionIdentityV1, AdmitRequestV1, AttemptMetadataV2,
    CanonicalExecutionJsonV1, DurableWorktreeIdentityV1, GraphNodeCounterV2, GraphSessionStateV2,
    GraphStartCurrentTaskV2, IdempotencyKeyV1, JobStateV1, PersistedGraphMutationFailureV2,
    PersistedGraphTerminalOperationV2, PersistedResponseContextV1, PersistedSessionMutationV1,
    ProcedureSnapshotV2, RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1,
    StateTransitionV1, StoreContractV1, StoreFailpointActionV1, StoreFailpointV1,
    StoreGraphMutationContractV2, StoreGraphStateContractV2, StoreReadContractV1, TerminalResultV1,
    ValidatedWorkspaceRootV1, WorkerIdV1,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

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
    let mut items = vec![json!({
        "id":"done","type":"confirm","prompt":"Done","required":required_artifact
    })];
    if required_artifact {
        items.push(json!({
            "id":"proof","type":"artifact","prompt":"Proof","required":true
        }));
    }
    let document = json!({
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
            "nodes": [{"id": "work", "use": "work", "terminal": true}]
        }
    });
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

fn legacy_aggregate() -> SessionAggregateV1 {
    let authored = json!({
        "schema": "podway.procedure/v1",
        "id": "legacy-before-v2",
        "version": "1",
        "name": "Legacy before v2",
        "stages": [{
            "id": "first",
            "title": "First",
            "instructions": [],
            "items": [{"type": "confirm", "id": "done", "prompt": "Done", "required": false}]
        }],
        "rework": {"allow_return_to": "any_previous"}
    });
    let canonical = canonicalize_json_v1(&authored).unwrap();
    let snapshot = ProcedureSnapshotV1::from_canonical_json(CanonicalProcedureSnapshotInputV1 {
        snapshot_id: ProcedureSnapshotId::new(uuid(420)).unwrap(),
        schema_id: "podway.procedure/v1".to_owned(),
        procedure_id: "legacy-before-v2".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Legacy before v2".to_owned(),
        source_label: ProcedureSourceLabelV1::file("legacy.yaml").unwrap(),
        canonical_json: CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
        digest: Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
            .unwrap(),
        created_at: UnixMillis::new(5),
    })
    .unwrap();
    SessionAggregateV1::start(
        SessionId::new(uuid(421)).unwrap(),
        "Legacy task",
        snapshot,
        AttemptId::new(uuid(422)).unwrap(),
        UnixMillis::new(6),
    )
    .unwrap()
}

fn seed_legacy_session(store: &SqliteStoreV1) -> SessionAggregateV1 {
    let aggregate = legacy_aggregate();
    let request = AdmitRequestV1::new(
        DomainCommand::SessionStart,
        IdempotencyKeyV1::new("legacy-seed").unwrap(),
        podway_core::JobId::new(uuid(423)).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(7),
    )
    .with_admitted_procedure_snapshot(aggregate.snapshot().clone());
    store.admit(&identity(), request).unwrap();
    let claim = store
        .claim_next(
            &identity(),
            WorkerIdV1::new("legacy-seed-worker").unwrap(),
            UnixMillis::new(8),
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
            UnixMillis::new(9),
        )
        .unwrap();
    aggregate
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
fn graph_start_replace_atomically_replaces_a_legacy_task() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap(), 1);
    let previous = seed_legacy_session(&store);
    let next = graph_state(430, 440, 22);
    let claimed = admit_and_claim(
        &store,
        DomainCommand::SessionStartReplace,
        "legacy-to-v2-replace",
        450,
        AdmissionSessionIdentityV1::Exact(previous.session_id().clone()),
        Some(previous.revision()),
    );
    store
        .commit_graph_start_terminal_v2(
            claimed.claim().clone(),
            GraphStartCurrentTaskV2::Exact {
                session_id: previous.session_id().clone(),
                session_revision: previous.revision(),
            },
            next.clone(),
            UnixMillis::new(23),
        )
        .unwrap();
    assert!(store.read_session_aggregate(&identity()).unwrap().is_none());
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
