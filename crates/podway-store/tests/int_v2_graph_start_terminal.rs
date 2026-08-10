//! Atomic Procedure v2 start publication and durable replay.

use std::path::{Path, PathBuf};

use podway_core::{
    ArtifactValueV1, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2,
    CanonicalProcedureJsonV1, CanonicalProcedureSnapshotInputV1, DomainCommand, DomainError,
    DomainResult, GraphNodeId, ItemId, ProcedureSnapshotId, ProcedureSnapshotV1,
    ProcedureSourceLabelV1, ReasonV2, Revision, SessionAggregateV1, SessionAttemptV2, SessionId,
    SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis, WorkspaceId,
    canonicalize_json_v1,
};
use podway_store::codec::encode_persisted_terminal_receipt_v1;
use podway_store::{
    ActiveItemMutationV2, AdmissionSessionIdentityV1, AdmitRequestV1, AttemptMetadataV2,
    CanonicalExecutionJsonV1, DurableWorktreeIdentityV1, GraphNodeCounterV2, GraphSessionStateV2,
    GraphStartCurrentTaskV2, IdempotencyKeyV1, JobStateV1, PersistedGraphMutationFailureV2,
    PersistedGraphTerminalOperationV2, PersistedResponseContextV1, PersistedSessionMutationV1,
    ProcedureSnapshotV2, RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1,
    StateTransitionV1, StoreContractV1, StoreFailpointActionV1, StoreFailpointV1,
    StoreGraphMutationContractV2, StoreGraphStateContractV2, StoreIdempotencyReadContractV1,
    StoreReadContractV1, TerminalResultV1, ValidatedWorkspaceRootV1, WorkerIdV1,
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
            "nodes": [{"id": "work", "use": "work", "skip":{"allowed":true,"reason_required":true}, "terminal": true}]
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
