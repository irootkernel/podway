//! Current Procedure v2 semantics and direct persistence regression evidence.
use super::int_v2dfc008_persistence::{fixture, reject_corruption, seed};
use rusqlite::Connection;

use podway_core::{
    ActorAttributionV2, ArtifactValueV1, AttemptId, AttemptLifecycle, AttemptNumberV2,
    AttemptValidityV2, BlockerId, BlockerState, CanonicalProcedureJsonV1, DecisionRecordInputV2,
    DecisionRecordV2, EvidenceReferenceSnapshotV2, GraphNodeId, ItemId, ItemTypeV1,
    NodeDefinitionId, OptionId, ProcedureSnapshotId, ProcedureSourceLabelV1, ReasonV2,
    RecordedItemValueV2, ResolvedEvidenceReferenceV2, ResolvedEvidenceSetV2, Revision,
    ReworkKindV2, ReworkRecordInputV2, ReworkRecordV2, SessionAttemptV2, SessionId,
    SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, TransitionEffectV2,
    UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{
    ActiveItemMutationV2, AttemptMetadataV2, AttemptWorkflowMemoryV2, BlockerStateV2,
    DurableWorktreeIdentityV1, EvidenceResolutionStateV2, GraphNodeCounterV2, GraphSessionStateV2,
    ItemSlotStateV2, ProcedureSnapshotV2, WorkflowMemoryStateV2,
};
use serde_json::json;
use sha2::{Digest, Sha256};

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

fn node(value: &str) -> GraphNodeId {
    GraphNodeId::new(value).unwrap()
}

fn item(value: &str) -> ItemId {
    ItemId::new(value).unwrap()
}

fn attempt_id(number: u64) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap()
}

fn blocker_id() -> BlockerId {
    BlockerId::new("00000000-0000-4000-8000-000000000099").unwrap()
}

fn session_id() -> SessionId {
    SessionId::new("00000000-0000-4000-8000-000000000120").unwrap()
}

fn snapshot() -> ProcedureSnapshotV2 {
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "workflow-memory",
        "version": "1",
        "name": "Workflow memory",
        "purpose": "Prove durable item, evidence, decision, and rework history.",
        "node_definitions": {
            "capture-def": {
                "type": "action",
                "title": "Capture",
                "intent": "Capture bounded values.",
                "items": [
                    {"id":"z-summary","type":"text","prompt":"Summary","required":true},
                    {"id":"a-count","type":"integer","prompt":"Count","required":false},
                    {"id":"m-tags","type":"list","prompt":"Tags","required":false,"min_items":1},
                    {"id":"c-confirm","type":"confirm","prompt":"Confirm","required":false},
                    {"id":"b-choice","type":"choice","prompt":"Choice","required":false,"choices":["green","red"]},
                    {"id":"y-artifact","type":"artifact","prompt":"Artifact","required":false,"allowed_media_types":["text/plain"]}
                ]
            },
            "gate-def": {
                "type": "decision",
                "title": "Gate",
                "objective": "Choose the next route.",
                "prompt": "Proceed or redo?",
                "options": [
                    {"id":"proceed","label":"Proceed"},
                    {"id":"redo","label":"Redo"}
                ],
                "reason": {"required":true}
            },
            "finish-def": {"type":"action","title":"Finish","intent":"Finish."}
        },
        "graph": {
            "entry": "capture",
            "nodes": [
                {"id":"capture","use":"capture-def","next":"gate"},
                {
                    "id":"gate",
                    "use":"gate-def",
                    "evidence_from":[
                        {"node":"capture","required":true,"items":["z-summary"]},
                        {"node":"finish","required":false}
                    ],
                    "routes": {
                        "proceed":{"to":"finish","effect":"advance"},
                        "redo":{"to":"capture","effect":"rework"}
                    }
                },
                {"id":"finish","use":"finish-def","terminal":true}
            ]
        },
        "manual_rework":{"allowed_targets":["capture","finish"]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000110").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("workflow.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

fn validity_replay_snapshot() -> ProcedureSnapshotV2 {
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "validity-replay",
        "version": "1",
        "name": "Validity replay",
        "purpose": "Prove cold-load validity follows append-only transition causes.",
        "node_definitions": {
            "work-def": {"type":"action","title":"Work","intent":"Do the work."},
            "finish-def": {"type":"action","title":"Finish","intent":"Finish."}
        },
        "graph": {
            "entry": "work",
            "nodes": [
                {"id":"work","use":"work-def","next":"finish"},
                {"id":"finish","use":"finish-def","terminal":true}
            ]
        },
        "manual_rework": {"allowed_targets":["work"]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000113").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("validity-replay.yaml").unwrap(),
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

fn capture_slots(
    attempt_number: u64,
    recorded: bool,
    changed_summary: bool,
) -> Vec<ItemSlotStateV2> {
    let attempt_id = attempt_id(attempt_number);
    let started_at = match attempt_number {
        1 => 10,
        3 => 40,
        4 => 60,
        _ => 40,
    };
    let values = if recorded {
        vec![
            Some(
                RecordedItemValueV2::text(if changed_summary {
                    "rewritten"
                } else {
                    "baseline"
                })
                .unwrap(),
            ),
            Some(RecordedItemValueV2::integer(7)),
            Some(RecordedItemValueV2::list(vec!["fast".to_owned(), "locked".to_owned()]).unwrap()),
            Some(RecordedItemValueV2::confirm()),
            Some(RecordedItemValueV2::choice("green").unwrap()),
            Some(RecordedItemValueV2::artifact(
                ArtifactValueV1::local_path("reports/result.txt", digest('c'), 42, "text/plain")
                    .unwrap(),
            )),
        ]
    } else {
        vec![None, None, None, None, None, None]
    };
    [
        ("z-summary", ItemTypeV1::Text),
        ("a-count", ItemTypeV1::Integer),
        ("m-tags", ItemTypeV1::List),
        ("c-confirm", ItemTypeV1::Confirm),
        ("b-choice", ItemTypeV1::Choice),
        ("y-artifact", ItemTypeV1::Artifact),
    ]
    .into_iter()
    .zip(values)
    .map(|((id, item_type), value)| {
        ItemSlotStateV2::new(
            attempt_id.clone(),
            item(id),
            item_type,
            if recorded {
                Revision::new(1)
            } else {
                Revision::ZERO
            },
            value,
            UnixMillis::new(started_at),
            UnixMillis::new(if recorded { started_at + 1 } else { started_at }),
        )
        .unwrap()
    })
    .collect()
}

fn capture_memory(
    recorded: bool,
    blocker: Option<(BlockerState, &str)>,
) -> AttemptWorkflowMemoryV2 {
    let blockers = blocker.map_or_else(Vec::new, |(state, reason)| {
        vec![
            BlockerStateV2::new(
                blocker_id(),
                attempt_id(1),
                reason,
                state,
                UnixMillis::new(12),
                (state == BlockerState::Resolved).then_some(UnixMillis::new(13)),
            )
            .unwrap(),
        ]
    });
    AttemptWorkflowMemoryV2::new(
        attempt_id(1),
        capture_slots(1, recorded, false),
        blockers,
        Vec::new(),
    )
    .unwrap()
}

fn initial_state() -> GraphSessionStateV2 {
    state_with_memory(
        1,
        SessionLifecycle::Running,
        vec![attempt(
            1,
            "capture",
            1,
            1,
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
        )],
        vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
        vec![
            GraphNodeCounterV2::new(node("capture"), 1, 0),
            GraphNodeCounterV2::new(node("gate"), 0, 0),
            GraphNodeCounterV2::new(node("finish"), 0, 0),
        ],
        WorkflowMemoryStateV2::new(vec![capture_memory(false, None)], Vec::new(), Vec::new())
            .unwrap(),
        None,
    )
}

fn evidence(capture: &AttemptWorkflowMemoryV2) -> EvidenceResolutionStateV2 {
    EvidenceResolutionStateV2::new(
        0,
        true,
        vec![item("z-summary")],
        ResolvedEvidenceReferenceV2::resolved(
            EvidenceReferenceSnapshotV2::new(
                node("capture"),
                attempt_id(1),
                AttemptNumberV2::FIRST,
                capture.recorded_items_digest().unwrap(),
                UnixMillis::new(30),
            )
            .unwrap(),
        ),
    )
    .unwrap()
}

fn gate_evidence(capture: &AttemptWorkflowMemoryV2) -> Vec<EvidenceResolutionStateV2> {
    vec![
        evidence(capture),
        EvidenceResolutionStateV2::new(
            1,
            false,
            Vec::new(),
            ResolvedEvidenceReferenceV2::unresolved(node("finish")),
        )
        .unwrap(),
    ]
}

fn gate_state() -> GraphSessionStateV2 {
    let capture = capture_memory(true, Some((BlockerState::Resolved, "Waiting for review.")));
    let gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        Vec::new(),
        gate_evidence(&capture),
    )
    .unwrap();
    state_with_memory(
        4,
        SessionLifecycle::Running,
        vec![
            attempt(
                1,
                "capture",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
            ),
            attempt(
                2,
                "gate",
                1,
                2,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            ),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(30)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(2), UnixMillis::new(30), None, None).unwrap(),
        ],
        vec![
            GraphNodeCounterV2::new(node("capture"), 1, 0),
            GraphNodeCounterV2::new(node("gate"), 1, 0),
            GraphNodeCounterV2::new(node("finish"), 0, 0),
        ],
        WorkflowMemoryStateV2::new(vec![capture, gate], Vec::new(), Vec::new()).unwrap(),
        None,
    )
}

fn declared_rework_state(
    revision: u64,
    old_blocker_reason: &str,
    fresh_recorded: bool,
) -> GraphSessionStateV2 {
    declared_rework_state_with_record(
        revision,
        old_blocker_reason,
        fresh_recorded,
        Revision::new(5),
        ReworkKindV2::Declared,
        false,
    )
}

fn declared_rework_state_with_record(
    revision: u64,
    old_blocker_reason: &str,
    fresh_recorded: bool,
    decision_revision: Revision,
    rework_kind: ReworkKindV2,
    reactivated: bool,
) -> GraphSessionStateV2 {
    let capture = capture_memory(true, Some((BlockerState::Resolved, old_blocker_reason)));
    let gate_evidence = gate_evidence(&capture);
    let gate =
        AttemptWorkflowMemoryV2::new(attempt_id(2), Vec::new(), Vec::new(), gate_evidence.clone())
            .unwrap();
    let fresh = AttemptWorkflowMemoryV2::new(
        attempt_id(3),
        capture_slots(3, fresh_recorded, false),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let evidence_set = ResolvedEvidenceSetV2::new(
        gate_evidence
            .iter()
            .map(|reference| reference.resolution().clone())
            .collect(),
    )
    .unwrap();
    let decision = DecisionRecordV2::new(DecisionRecordInputV2 {
        trace: TraceSequenceV2::new(2),
        session_id: session_id(),
        session_revision: decision_revision,
        procedure_snapshot_id: snapshot().snapshot_id().clone(),
        procedure_digest: snapshot().digest().clone(),
        graph_node_id: node("gate"),
        node_definition_id: NodeDefinitionId::new("gate-def").unwrap(),
        attempt_id: attempt_id(2),
        attempt_number: AttemptNumberV2::FIRST,
        goal_revision: None,
        selected_option: OptionId::new("redo").unwrap(),
        route_effect: TransitionEffectV2::Rework,
        route_target: node("capture"),
        reason: ReasonV2::new("Redo the capture.").unwrap(),
        evidence: evidence_set,
        actor: Some(ActorAttributionV2::new("reviewer").unwrap()),
        recorded_at: UnixMillis::new(40),
    })
    .unwrap();
    let rework = ReworkRecordV2::new(ReworkRecordInputV2 {
        trace: TraceSequenceV2::new(3),
        kind: rework_kind,
        from_node: node("gate"),
        to_node: node("capture"),
        target_attempt_id: attempt_id(3),
        reason: ReasonV2::new("Redo the capture.").unwrap(),
        reactivated,
        actor: Some(ActorAttributionV2::new("reviewer").unwrap()),
        recorded_at: UnixMillis::new(40),
    })
    .unwrap();
    state_with_memory(
        revision,
        SessionLifecycle::Running,
        vec![
            attempt(
                1,
                "capture",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
            ),
            attempt(
                2,
                "gate",
                1,
                2,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
            ),
            attempt(
                3,
                "capture",
                2,
                3,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            ),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(30)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                attempt_id(2),
                UnixMillis::new(30),
                Some(UnixMillis::new(40)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(3), UnixMillis::new(40), None, None).unwrap(),
        ],
        vec![
            GraphNodeCounterV2::new(node("capture"), 2, 1),
            GraphNodeCounterV2::new(node("gate"), 1, 0),
            GraphNodeCounterV2::new(node("finish"), 0, 0),
        ],
        WorkflowMemoryStateV2::new(vec![capture, gate, fresh], vec![decision], vec![rework])
            .unwrap(),
        None,
    )
}

fn proceed_decision(capture: &AttemptWorkflowMemoryV2) -> DecisionRecordV2 {
    let gate_evidence = gate_evidence(capture);
    DecisionRecordV2::new(DecisionRecordInputV2 {
        trace: TraceSequenceV2::new(2),
        session_id: session_id(),
        session_revision: Revision::new(5),
        procedure_snapshot_id: snapshot().snapshot_id().clone(),
        procedure_digest: snapshot().digest().clone(),
        graph_node_id: node("gate"),
        node_definition_id: NodeDefinitionId::new("gate-def").unwrap(),
        attempt_id: attempt_id(2),
        attempt_number: AttemptNumberV2::FIRST,
        goal_revision: None,
        selected_option: OptionId::new("proceed").unwrap(),
        route_effect: TransitionEffectV2::Advance,
        route_target: node("finish"),
        reason: ReasonV2::new("Proceed to finish.").unwrap(),
        evidence: ResolvedEvidenceSetV2::new(
            gate_evidence
                .iter()
                .map(|reference| reference.resolution().clone())
                .collect(),
        )
        .unwrap(),
        actor: None,
        recorded_at: UnixMillis::new(40),
    })
    .unwrap()
}

fn finish_state(revision: u64, completed: bool) -> GraphSessionStateV2 {
    let capture = capture_memory(true, Some((BlockerState::Resolved, "Waiting for review.")));
    let gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        Vec::new(),
        gate_evidence(&capture),
    )
    .unwrap();
    let finish =
        AttemptWorkflowMemoryV2::new(attempt_id(3), Vec::new(), Vec::new(), Vec::new()).unwrap();
    state_with_memory(
        revision,
        if completed {
            SessionLifecycle::Completed
        } else {
            SessionLifecycle::Running
        },
        vec![
            attempt(
                1,
                "capture",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
            ),
            attempt(
                2,
                "gate",
                1,
                2,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
            ),
            attempt(
                3,
                "finish",
                1,
                3,
                if completed {
                    AttemptLifecycle::Completed
                } else {
                    AttemptLifecycle::Active
                },
                AttemptValidityV2::Valid,
            ),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(30)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                attempt_id(2),
                UnixMillis::new(30),
                Some(UnixMillis::new(40)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                attempt_id(3),
                UnixMillis::new(40),
                completed.then_some(UnixMillis::new(50)),
                None,
            )
            .unwrap(),
        ],
        vec![
            GraphNodeCounterV2::new(node("capture"), 1, 0),
            GraphNodeCounterV2::new(node("gate"), 1, 0),
            GraphNodeCounterV2::new(node("finish"), 1, 0),
        ],
        WorkflowMemoryStateV2::new(
            vec![capture.clone(), gate, finish],
            vec![proceed_decision(&capture)],
            Vec::new(),
        )
        .unwrap(),
        completed.then_some(UnixMillis::new(50)),
    )
}

fn state_with_memory(
    revision: u64,
    lifecycle: SessionLifecycle,
    attempts: Vec<SessionAttemptV2>,
    metadata: Vec<AttemptMetadataV2>,
    counters: Vec<GraphNodeCounterV2>,
    memory: WorkflowMemoryStateV2,
    completed_at: Option<UnixMillis>,
) -> GraphSessionStateV2 {
    try_state_with_memory(
        revision,
        lifecycle,
        attempts,
        metadata,
        counters,
        memory,
        completed_at,
    )
    .unwrap()
}

fn try_state_with_memory(
    revision: u64,
    lifecycle: SessionLifecycle,
    attempts: Vec<SessionAttemptV2>,
    metadata: Vec<AttemptMetadataV2>,
    counters: Vec<GraphNodeCounterV2>,
    memory: WorkflowMemoryStateV2,
    completed_at: Option<UnixMillis>,
) -> Result<GraphSessionStateV2, podway_core::ProcedureStateErrorV2> {
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(revision),
        "Persist workflow memory",
        snapshot(),
        SessionTraceV2::from_parts(session_id(), lifecycle, Revision::new(revision), attempts)
            .unwrap(),
        counters,
        metadata,
        memory,
        UnixMillis::new(10),
        completed_at,
        None,
        None,
    )
}

fn persisted_reentry_state() -> GraphSessionStateV2 {
    let snapshot = validity_replay_snapshot();
    let initial = GraphSessionStateV2::new(
        Revision::new(1),
        "Replay attempt validity",
        snapshot,
        SessionTraceV2::from_parts(
            session_id(),
            SessionLifecycle::Running,
            Revision::new(1),
            vec![attempt(
                1,
                "work",
                1,
                1,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            )],
        )
        .unwrap(),
        vec![
            GraphNodeCounterV2::new(node("work"), 1, 0),
            GraphNodeCounterV2::new(node("finish"), 0, 0),
        ],
        vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap();
    let first_finish = initial
        .complete_active_action_v2(
            Revision::new(1),
            &attempt_id(1),
            Some(attempt_id(2)),
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    let reentered = first_finish
        .manual_rework_v2(
            Revision::new(2),
            Some(&attempt_id(2)),
            node("work"),
            attempt_id(3),
            ReasonV2::new("Revisit the work.").unwrap(),
            None,
            UnixMillis::new(30),
        )
        .unwrap()
        .into_state();
    reentered
        .complete_active_action_v2(
            Revision::new(3),
            &attempt_id(3),
            Some(attempt_id(4)),
            UnixMillis::new(40),
        )
        .unwrap()
        .into_state()
}

#[test]
fn v2drw004_sqlite_reopen_rejects_resurrected_obsolete_attempt_validity() {
    let (_temporary, store) = fixture(&identity());
    let state = persisted_reentry_state();
    assert_eq!(
        state
            .trace()
            .attempts()
            .iter()
            .map(SessionAttemptV2::validity)
            .collect::<Vec<_>>(),
        vec![
            AttemptValidityV2::Stale,
            AttemptValidityV2::Stale,
            AttemptValidityV2::Valid,
            AttemptValidityV2::Valid,
        ]
    );
    seed(&store, &(state));

    let connection = Connection::open(store.layout().database()).unwrap();
    // The partial unique index permits only one valid attempt per node. Stale the legitimate
    // successor first so resurrecting the obsolete same-node attempt remains a schema-valid DB
    // witness and reaches startup semantic validation.
    connection
        .execute(
            "UPDATE v2_attempts SET validity = 'stale' WHERE attempt_id = ?1",
            [attempt_id(3).as_str()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE v2_attempts SET validity = 'valid' WHERE attempt_id = ?1",
            [attempt_id(1).as_str()],
        )
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn v2drw004_sqlite_reopen_rejects_fresh_reentry_staling_without_resurrection() {
    let (_temporary, store) = fixture(&identity());
    seed(&store, &(persisted_reentry_state()));

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_attempts SET validity = 'stale' WHERE attempt_id = ?1",
            [attempt_id(3).as_str()],
        )
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn v2drw_declared_rework_actor_tamper_fails_cold_load() {
    let (_temporary, store) = fixture(&identity());
    seed(
        &store,
        &(declared_rework_state(5, "Waiting for review.", false)),
    );

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_rework_records SET actor = 'different-reviewer' WHERE kind = 'declared'",
            [],
        )
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn v2drw_declared_rework_timestamp_tamper_fails_cold_load() {
    let (_temporary, store) = fixture(&identity());
    seed(
        &store,
        &(declared_rework_state(5, "Waiting for review.", false)),
    );

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_rework_records SET recorded_at_ms = recorded_at_ms + 1 WHERE kind = 'declared'",
            [],
        )
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn v2drw004_sqlite_reopen_rejects_running_manual_reactivation_bit_tamper() {
    let (_temporary, store) = fixture(&identity());
    let state = gate_state()
        .manual_rework_v2(
            Revision::new(4),
            Some(&attempt_id(2)),
            node("capture"),
            attempt_id(3),
            ReasonV2::new("Revisit the capture.").unwrap(),
            Some(ActorAttributionV2::new("operator").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap()
        .into_state();
    seed(&store, &(state));

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_rework_records SET reactivated = 1 WHERE trace_sequence = 3",
            [],
        )
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn v2drw004_sqlite_reopen_rejects_deleted_same_node_manual_rework_record() {
    let (_temporary, store) = fixture(&identity());
    let state = finish_state(5, false)
        .manual_rework_v2(
            Revision::new(5),
            Some(&attempt_id(3)),
            node("finish"),
            attempt_id(4),
            ReasonV2::new("Repeat final checks.").unwrap(),
            None,
            UnixMillis::new(50),
        )
        .unwrap()
        .into_state();
    seed(&store, &(state));

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute("DELETE FROM v2_rework_records WHERE trace_sequence = 4", [])
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn list_snapshot_without_explicit_total_bound_reconstructs_with_identical_canonical_bytes() {
    use super::int_v2dfc005_direct_store::{deadline, store as cold_store};
    let expected = initial_state();
    let declaration: serde_json::Value =
        serde_json::from_str(expected.snapshot().canonical_json().as_str()).unwrap();
    assert_eq!(
        declaration["node_definitions"]["capture-def"]["items"][2]["type"],
        json!("list")
    );
    assert!(
        declaration["node_definitions"]["capture-def"]["items"][2]
            .get("max_total_length")
            .is_none()
    );
    let (_temporary, direct) = fixture(&identity());
    seed(&direct, &expected);
    let cold = cold_store(direct.layout().root().to_path_buf().as_path());
    let actual = cold
        .read(deadline(), |snapshot| snapshot.current_graph())
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        actual.snapshot().canonical_json().as_str(),
        expected.snapshot().canonical_json().as_str()
    );
}

#[test]
fn workflow_history_rewrite_and_evidence_digest_corruption_fail_closed() {
    use super::int_v2dfc005_direct_store::{deadline, request, success};
    let (_temporary, direct) = fixture(&identity());
    let prior = declared_rework_state(5, "Waiting for review.", false);
    seed(&direct, &prior);
    let rewrite = declared_rework_state(6, "Changed historical blocker.", true);
    let change = request("rewrite-workflow-history");
    let sequence = direct
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    let result = direct.mutate(
        &change,
        deadline(),
        || UnixMillis::new(200),
        |transaction| {
            transaction.now();
            transaction.replace_graph(
                prior.workspace_revision(),
                prior.trace().revision(),
                &rewrite,
            )?;
            Ok(success(&change, json!({"changed":true}), transaction.now()))
        },
    );
    assert!(matches!(
        result,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::InvalidStateV1(_)
        ))
    ));
    direct
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.current_graph()?, Some(prior));
            assert_eq!(snapshot.committed_sequence()?, sequence);
            assert!(snapshot.outcome(change.key())?.is_none());
            Ok(())
        })
        .unwrap();
    let connection = Connection::open(direct.layout().database()).unwrap();
    connection.execute("UPDATE v2_resolved_evidence_references SET items_digest=?1 WHERE attempt_id=?2 AND source_graph_node_id='capture'",[digest('f').as_str(),attempt_id(2).as_str()]).unwrap();
    drop(connection);
    reject_corruption(&direct, true);
}

#[test]
fn cold_item_decode_rejects_noncanonical_and_mismatched_values_without_repair() {
    let state = initial_state()
        .mutate_active_item_v2(
            &attempt_id(1),
            &item("z-summary"),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "Summary".to_owned(),
            },
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    for encoded in [
        r#" {"kind":"text","value":"Summary"}"#,
        r#"{"value":"Summary","kind":"text"}"#,
        r#"{"kind":"text","value":"\u0053ummary"}"#,
        r#"{"kind":"text","kind":"text","value":"Summary"}"#,
        r#"{"kind":"text","unknown":true,"value":"Summary"}"#,
        r#"{"kind":"integer","value":1}"#,
        r#"{"kind":"text","value":1}"#,
    ] {
        let (_temporary, direct) = fixture(&identity());
        seed(&direct, &state);
        let connection = Connection::open(direct.layout().database()).unwrap();
        assert_eq!(
            connection
                .execute(
                    "UPDATE v2_item_slots SET value_json=?1 WHERE item_id='z-summary'",
                    [encoded],
                )
                .unwrap(),
            1,
        );
        drop(connection);
        reject_corruption(&direct, true);
    }
}
