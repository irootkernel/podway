//! Procedure v2 workflow-memory persistence and immutable-history evidence.

use std::path::{Path, PathBuf};

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
    DurableWorktreeIdentityV1, EvidenceResolutionStateV2, GraphMutationErrorV2, GraphNodeCounterV2,
    GraphSessionStateV2, ItemSlotStateV2, ProcedureSnapshotV2, SqliteStoreOptionsV1, SqliteStoreV1,
    StoreErrorV1, StoreFailpointV1, StoreGraphStateContractV2, StoreIntegrityCheckV1,
    StoreUnavailableReasonV1, StoreValueErrorV1, ValidatedWorkspaceRootV1, WorkflowMemoryStateV2,
    canonical_recorded_items_json_v2,
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
        WorkspaceId::new("00000000-0000-4000-8000-000000000101").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-v2-workflow-memory")).unwrap()
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
    ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000110").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("workflow.yaml").unwrap(),
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

fn active_recorded_state(revision: u64, blocker_state: BlockerState) -> GraphSessionStateV2 {
    state_with_memory(
        revision,
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
        WorkflowMemoryStateV2::new(
            vec![capture_memory(
                true,
                Some((blocker_state, "Waiting for review.")),
            )],
            Vec::new(),
            Vec::new(),
        )
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

fn manual_reactivated_state() -> GraphSessionStateV2 {
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
    let fresh = AttemptWorkflowMemoryV2::new(
        attempt_id(4),
        capture_slots(4, false, false),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let rework = ReworkRecordV2::new(ReworkRecordInputV2 {
        trace: TraceSequenceV2::new(4),
        kind: ReworkKindV2::Manual,
        from_node: node("finish"),
        to_node: node("capture"),
        target_attempt_id: attempt_id(4),
        reason: ReasonV2::new("Requirements changed.").unwrap(),
        reactivated: true,
        actor: Some(ActorAttributionV2::new("operator").unwrap()),
        recorded_at: UnixMillis::new(60),
    })
    .unwrap();
    state_with_memory(
        7,
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
                "finish",
                1,
                3,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
            ),
            attempt(
                4,
                "capture",
                2,
                4,
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
            AttemptMetadataV2::new(
                attempt_id(3),
                UnixMillis::new(40),
                Some(UnixMillis::new(50)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(4), UnixMillis::new(60), None, None).unwrap(),
        ],
        vec![
            GraphNodeCounterV2::new(node("capture"), 2, 1),
            GraphNodeCounterV2::new(node("gate"), 1, 0),
            GraphNodeCounterV2::new(node("finish"), 1, 0),
        ],
        WorkflowMemoryStateV2::new(
            vec![capture.clone(), gate, finish, fresh],
            vec![proceed_decision(&capture)],
            vec![rework],
        )
        .unwrap(),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
fn try_state_with_memory(
    revision: u64,
    lifecycle: SessionLifecycle,
    attempts: Vec<SessionAttemptV2>,
    metadata: Vec<AttemptMetadataV2>,
    counters: Vec<GraphNodeCounterV2>,
    memory: WorkflowMemoryStateV2,
    completed_at: Option<UnixMillis>,
) -> Result<GraphSessionStateV2, StoreValueErrorV1> {
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

fn persist_recorded_capture(store: &SqliteStoreV1) {
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            active_recorded_state(2, BlockerState::Open),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(2),
            Revision::new(2),
            active_recorded_state(3, BlockerState::Resolved),
        )
        .unwrap();
}

fn persist_gate(store: &SqliteStoreV1) {
    persist_recorded_capture(store);
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(3),
            Revision::new(3),
            gate_state(),
        )
        .unwrap();
}

#[test]
fn cursor_stable_items_and_blockers_round_trip_without_moving_the_cursor() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_recorded_capture(&store);
    let loaded = store.read_graph_session_v2(&identity()).unwrap().unwrap();
    assert_eq!(loaded, active_recorded_state(3, BlockerState::Resolved));
    assert_eq!(
        loaded.trace().active_attempt().unwrap().attempt_id(),
        &attempt_id(1)
    );
    assert_eq!(loaded.trace().attempts().len(), 1);
    drop(store);
    assert_eq!(
        open(&temporary, SqliteStoreOptionsV1::new(8).unwrap())
            .read_graph_session_v2(&identity())
            .unwrap(),
        Some(active_recorded_state(3, BlockerState::Resolved))
    );
}

#[test]
fn evidence_digest_uses_all_items_in_author_order_while_readback_honors_selector() {
    let state = gate_state();
    let capture = &state.workflow_memory().attempts()[0];
    let canonical = canonical_recorded_items_json_v2(&capture.recorded_items().unwrap()).unwrap();
    assert_eq!(
        canonical,
        format!(
            r#"[{{"id":"z-summary","value":{{"kind":"text","value":"baseline"}}}},{{"id":"a-count","value":{{"kind":"integer","value":7}}}},{{"id":"m-tags","value":{{"kind":"list","value":["fast","locked"]}}}},{{"id":"c-confirm","value":{{"kind":"confirm"}}}},{{"id":"b-choice","value":{{"kind":"choice","value":"green"}}}},{{"id":"y-artifact","value":{{"digest":"{}","kind":"artifact","location":"reports/result.txt","location_kind":"local_path","media_type":"text/plain","size_bytes":42}}}}]"#,
            digest('c').as_str()
        )
    );
    let reference_digest = state.workflow_memory().attempts()[1].evidence()[0]
        .resolution()
        .snapshot()
        .unwrap()
        .items_digest();
    assert_eq!(reference_digest, &capture.recorded_items_digest().unwrap());
    let readback = state.selected_evidence_readback(&attempt_id(2)).unwrap();
    assert_eq!(readback.len(), 2);
    assert_eq!(readback[0].items().items().len(), 1);
    assert_eq!(readback[0].items().items()[0].id(), &item("z-summary"));
    assert!(readback[1].reference().resolution().is_unresolved());
    assert!(readback[1].items().items().is_empty());

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_recorded_capture(&store);
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(3),
            Revision::new(3),
            state.clone(),
        )
        .unwrap();
    drop(store);
    assert_eq!(
        open(&temporary, SqliteStoreOptionsV1::new(8).unwrap())
            .read_graph_session_v2(&identity())
            .unwrap(),
        Some(state)
    );
}

#[test]
fn retrying_a_decision_re_resolves_evidence_into_clean_attempt_memory() {
    let state = gate_state();
    let prior_attempt = state.trace().attempts()[0].clone();
    let prior_metadata = state.attempt_metadata()[0].clone();
    let prior_memory = state.workflow_memory().attempts()[0].clone();
    let prior_counters = state.counters().to_vec();
    let prior_evidence = state.workflow_memory().attempts()[1].evidence().to_vec();
    let outcome = state
        .retry_active_attempt_v2(
            Revision::new(4),
            &attempt_id(2),
            attempt_id(3),
            ReasonV2::new("Reconsider the decision.").unwrap(),
            UnixMillis::new(40),
        )
        .unwrap();
    let retried = outcome.state();
    let fresh_memory = &retried.workflow_memory().attempts()[2];

    assert_eq!(retried.trace().attempts()[0], prior_attempt);
    assert_eq!(retried.attempt_metadata()[0], prior_metadata);
    assert_eq!(retried.workflow_memory().attempts()[0], prior_memory);
    assert_eq!(retried.counters()[0], prior_counters[0]);
    assert_eq!(retried.counters()[2], prior_counters[2]);
    assert_eq!(
        retried.counters()[1].attempt_count(),
        prior_counters[1].attempt_count() + 1
    );
    assert_eq!(
        retried.counters()[1].rework_traversal_count(),
        prior_counters[1].rework_traversal_count()
    );
    assert_eq!(
        retried.trace().attempts()[0].validity(),
        AttemptValidityV2::Valid
    );
    assert_eq!(
        retried.trace().attempts()[1].lifecycle(),
        AttemptLifecycle::Abandoned
    );
    assert_eq!(
        retried.trace().attempts()[1].validity(),
        AttemptValidityV2::Stale
    );
    assert_eq!(
        retried.trace().active_attempt().unwrap().graph_node_id(),
        &node("gate")
    );
    assert!(fresh_memory.item_slots().is_empty());
    assert!(fresh_memory.blockers().is_empty());
    assert_eq!(fresh_memory.evidence().len(), prior_evidence.len());
    assert_eq!(
        fresh_memory.evidence()[0]
            .resolution()
            .snapshot()
            .unwrap()
            .source_attempt_id(),
        &attempt_id(1)
    );
    assert_eq!(
        fresh_memory.evidence()[0]
            .resolution()
            .snapshot()
            .unwrap()
            .resolved_at(),
        UnixMillis::new(40)
    );
    assert!(fresh_memory.evidence()[1].resolution().snapshot().is_none());
    assert_eq!(
        retried.workflow_memory().attempts()[1].evidence(),
        prior_evidence
    );
    assert!(retried.selected_evidence_readback(&attempt_id(2)).unwrap()[0].stale());
    assert!(!retried.selected_evidence_readback(&attempt_id(3)).unwrap()[0].stale());
    assert!(retried.workflow_memory().decisions().is_empty());
    assert!(retried.workflow_memory().reworks().is_empty());
}

#[test]
fn retry_discards_missing_required_items_and_open_blockers_only_from_the_fresh_attempt() {
    let old_memory = capture_memory(false, Some((BlockerState::Open, "Still blocked.")));
    let state = state_with_memory(
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
        WorkflowMemoryStateV2::new(vec![old_memory.clone()], Vec::new(), Vec::new()).unwrap(),
        None,
    );
    assert!(old_memory.item_slots()[0].value().is_none());
    assert_eq!(old_memory.blockers()[0].state(), BlockerState::Open);

    let retried = state
        .retry_active_attempt_v2(
            Revision::new(1),
            &attempt_id(1),
            attempt_id(2),
            ReasonV2::new("Retry despite incomplete work.").unwrap(),
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    let fresh = &retried.workflow_memory().attempts()[1];

    assert_eq!(retried.workflow_memory().attempts()[0], old_memory);
    assert!(fresh.item_slots().iter().all(|slot| slot.value().is_none()));
    assert!(fresh.blockers().is_empty());
    assert_eq!(retried.counters()[0].attempt_count(), 2);
    assert_eq!(retried.counters()[0].rework_traversal_count(), 0);
    assert_eq!(retried.counters()[1], state.counters()[1]);
    assert_eq!(retried.counters()[2], state.counters()[2]);
}

#[test]
fn declared_rework_keeps_items_evidence_and_decision_history_across_reopen() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_gate(&store);
    let reworked = declared_rework_state(5, "Waiting for review.", false);
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            reworked.clone(),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(reworked.clone())
    );
    drop(store);
    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(reworked)
    );
    let loaded = reopened
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    let readback = loaded.selected_evidence_readback(&attempt_id(2)).unwrap();
    assert!(readback[0].stale());
}

#[test]
fn declarations_required_items_and_activation_evidence_fail_closed() {
    let mut invalid_choice_slots = capture_slots(1, true, false);
    invalid_choice_slots[4] = ItemSlotStateV2::new(
        attempt_id(1),
        item("b-choice"),
        ItemTypeV1::Choice,
        Revision::new(1),
        Some(RecordedItemValueV2::choice("blue").unwrap()),
        UnixMillis::new(10),
        UnixMillis::new(11),
    )
    .unwrap();
    let invalid_choice = try_state_with_memory(
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
        WorkflowMemoryStateV2::new(
            vec![
                AttemptWorkflowMemoryV2::new(
                    attempt_id(1),
                    invalid_choice_slots,
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap(),
            ],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        None,
    );
    assert!(invalid_choice.is_err());

    let empty_capture = capture_memory(false, None);
    let gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        Vec::new(),
        gate_evidence(&empty_capture),
    )
    .unwrap();
    let missing_required = try_state_with_memory(
        2,
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
        WorkflowMemoryStateV2::new(vec![empty_capture, gate], Vec::new(), Vec::new()).unwrap(),
        None,
    );
    assert!(missing_required.is_err());

    let capture = capture_memory(true, Some((BlockerState::Resolved, "Waiting for review.")));
    let wrong_time_reference = EvidenceResolutionStateV2::new(
        0,
        true,
        vec![item("z-summary")],
        ResolvedEvidenceReferenceV2::resolved(
            EvidenceReferenceSnapshotV2::new(
                node("capture"),
                attempt_id(1),
                AttemptNumberV2::FIRST,
                capture.recorded_items_digest().unwrap(),
                UnixMillis::new(29),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let wrong_time_gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        Vec::new(),
        vec![
            wrong_time_reference,
            EvidenceResolutionStateV2::new(
                1,
                false,
                Vec::new(),
                ResolvedEvidenceReferenceV2::unresolved(node("finish")),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let wrong_time = try_state_with_memory(
        4,
        SessionLifecycle::Running,
        gate_state().trace().attempts().to_vec(),
        gate_state().attempt_metadata().to_vec(),
        gate_state().counters().to_vec(),
        WorkflowMemoryStateV2::new(vec![capture, wrong_time_gate], Vec::new(), Vec::new()).unwrap(),
        None,
    );
    assert!(wrong_time.is_err());
}

#[test]
fn optional_evidence_cannot_remain_unresolved_when_a_valid_source_exists() {
    let capture = capture_memory(true, Some((BlockerState::Resolved, "Waiting for review.")));
    let finish =
        AttemptWorkflowMemoryV2::new(attempt_id(3), Vec::new(), Vec::new(), Vec::new()).unwrap();
    let gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        Vec::new(),
        vec![
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
                        UnixMillis::new(40),
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
            EvidenceResolutionStateV2::new(
                1,
                false,
                Vec::new(),
                ResolvedEvidenceReferenceV2::unresolved(node("finish")),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let result = try_state_with_memory(
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
                3,
                "finish",
                1,
                2,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
            ),
            attempt(
                2,
                "gate",
                1,
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
                attempt_id(3),
                UnixMillis::new(30),
                Some(UnixMillis::new(40)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(2), UnixMillis::new(40), None, None).unwrap(),
        ],
        vec![
            GraphNodeCounterV2::new(node("capture"), 1, 0),
            GraphNodeCounterV2::new(node("gate"), 1, 0),
            GraphNodeCounterV2::new(node("finish"), 1, 0),
        ],
        WorkflowMemoryStateV2::new(vec![capture, finish, gate], Vec::new(), Vec::new()).unwrap(),
        None,
    );
    assert!(result.is_err());
}

#[test]
fn rework_cannot_target_a_node_without_an_earlier_attempt() {
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
    let fake_rework = ReworkRecordV2::new(ReworkRecordInputV2 {
        trace: TraceSequenceV2::new(3),
        kind: ReworkKindV2::Manual,
        from_node: node("gate"),
        to_node: node("finish"),
        target_attempt_id: attempt_id(3),
        reason: ReasonV2::new("Pretend this advance was rework.").unwrap(),
        reactivated: false,
        actor: None,
        recorded_at: UnixMillis::new(40),
    })
    .unwrap();
    let result = try_state_with_memory(
        5,
        SessionLifecycle::Running,
        finish_state(5, false).trace().attempts().to_vec(),
        finish_state(5, false).attempt_metadata().to_vec(),
        finish_state(5, false).counters().to_vec(),
        WorkflowMemoryStateV2::new(
            vec![capture.clone(), gate, finish],
            vec![proceed_decision(&capture)],
            vec![fake_rework],
        )
        .unwrap(),
        None,
    );
    assert!(result.is_err());
}

#[test]
fn decision_and_rework_records_are_bound_to_the_exact_successor() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_gate(&store);

    let wrong_revision = declared_rework_state_with_record(
        5,
        "Waiting for review.",
        false,
        Revision::new(4),
        ReworkKindV2::Declared,
        false,
    );
    assert!(matches!(
        store.replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            wrong_revision
        ),
        Err(StoreErrorV1::InvalidStateV1(_))
    ));

    let falsely_reactivated = declared_rework_state_with_record(
        5,
        "Waiting for review.",
        false,
        Revision::new(5),
        ReworkKindV2::Manual,
        true,
    );
    assert!(matches!(
        store.replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            falsely_reactivated
        ),
        Err(StoreErrorV1::InvalidStateV1(_))
    ));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(gate_state())
    );
}

#[test]
fn manual_rework_may_restart_the_current_active_node() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_recorded_capture(&store);

    let old = capture_memory(true, Some((BlockerState::Resolved, "Waiting for review.")));
    let fresh = AttemptWorkflowMemoryV2::new(
        attempt_id(3),
        capture_slots(3, false, false),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let rework = ReworkRecordV2::new(ReworkRecordInputV2 {
        trace: TraceSequenceV2::new(2),
        kind: ReworkKindV2::Manual,
        from_node: node("capture"),
        to_node: node("capture"),
        target_attempt_id: attempt_id(3),
        reason: ReasonV2::new("Restart this placement deliberately.").unwrap(),
        reactivated: false,
        actor: None,
        recorded_at: UnixMillis::new(40),
    })
    .unwrap();
    let next = state_with_memory(
        4,
        SessionLifecycle::Running,
        vec![
            attempt(
                1,
                "capture",
                1,
                1,
                AttemptLifecycle::Abandoned,
                AttemptValidityV2::Stale,
            ),
            attempt(
                3,
                "capture",
                2,
                2,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            ),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(40)),
                Some("Restart this placement deliberately.".to_owned()),
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(3), UnixMillis::new(40), None, None).unwrap(),
        ],
        vec![
            GraphNodeCounterV2::new(node("capture"), 2, 1),
            GraphNodeCounterV2::new(node("gate"), 0, 0),
            GraphNodeCounterV2::new(node("finish"), 0, 0),
        ],
        WorkflowMemoryStateV2::new(vec![old, fresh], Vec::new(), vec![rework]).unwrap(),
        None,
    );
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(3),
            Revision::new(3),
            next.clone(),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
}

#[test]
fn non_rework_successors_cannot_change_rework_counters() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_gate(&store);

    let ordinary = finish_state(5, false);
    let bad_counters = state_with_memory(
        5,
        SessionLifecycle::Running,
        ordinary.trace().attempts().to_vec(),
        ordinary.attempt_metadata().to_vec(),
        vec![
            GraphNodeCounterV2::new(node("capture"), 1, 0),
            GraphNodeCounterV2::new(node("gate"), 1, 1),
            GraphNodeCounterV2::new(node("finish"), 1, 0),
        ],
        ordinary.workflow_memory().clone(),
        None,
    );
    assert!(matches!(
        store.replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            bad_counters
        ),
        Err(StoreErrorV1::InvalidStateV1(_))
    ));
}

#[test]
fn manual_rework_reactivates_completed_session_without_rewriting_history() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_gate(&store);
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            finish_state(5, false),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(5),
            Revision::new(5),
            finish_state(6, true),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(6),
            Revision::new(6),
            manual_reactivated_state(),
        )
        .unwrap();
    drop(store);
    assert_eq!(
        open(&temporary, SqliteStoreOptionsV1::new(8).unwrap())
            .read_graph_session_v2(&identity())
            .unwrap(),
        Some(manual_reactivated_state())
    );
}

#[test]
fn failpoint_history_rewrite_and_digest_corruption_fail_closed_atomically() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_gate(&store);
    drop(store);

    let failing = open(
        &temporary,
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(Some(StoreFailpointV1::V2GraphStateBeforeCommit)),
    );
    let error = failing
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            declared_rework_state(5, "Waiting for review.", false),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        }
    ));
    drop(failing);
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(gate_state())
    );
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            declared_rework_state(5, "Waiting for review.", false),
        )
        .unwrap();
    let rewrite = declared_rework_state(6, "Changed historical blocker.", true);
    let error = store
        .replace_graph_session_v2(&identity(), Revision::new(5), Revision::new(5), rewrite)
        .unwrap_err();
    assert!(matches!(error, StoreErrorV1::InvalidStateV1(_)));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(declared_rework_state(5, "Waiting for review.", false))
    );
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection.execute("PRAGMA foreign_keys = ON", []).unwrap();
    connection.execute("UPDATE v2_resolved_evidence_references SET items_digest = ?1 WHERE attempt_id = ?2 AND source_graph_node_id = 'capture'", [digest('f').as_str(), attempt_id(2).as_str()]).unwrap();
    drop(connection);
    let error = match SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(100),
    ) {
        Ok(_) => panic!("corrupt workflow memory must fail startup integrity"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::SessionCursor
        }
    ));
}

#[test]
fn active_item_mutations_cover_every_type_and_completion_freezes_activation_evidence() {
    let mut state = initial_state();
    let mutations = [
        ("c-confirm", ActiveItemMutationV2::Check),
        (
            "z-summary",
            ActiveItemMutationV2::Set {
                value: "ready".to_owned(),
            },
        ),
        (
            "b-choice",
            ActiveItemMutationV2::Set {
                value: "green".to_owned(),
            },
        ),
        (
            "a-count",
            ActiveItemMutationV2::Set {
                value: "7".to_owned(),
            },
        ),
        (
            "m-tags",
            ActiveItemMutationV2::Add {
                value: "locked".to_owned(),
            },
        ),
        (
            "y-artifact",
            ActiveItemMutationV2::Attach {
                value: ArtifactValueV1::local_path(
                    "reports/result.txt",
                    digest('c'),
                    42,
                    "text/plain",
                )
                .unwrap(),
            },
        ),
    ];
    for (offset, (item_id, mutation)) in mutations.into_iter().enumerate() {
        let outcome = state
            .mutate_active_item_v2(
                &attempt_id(1),
                &item(item_id),
                Revision::ZERO,
                mutation,
                UnixMillis::new(11 + u64::try_from(offset).unwrap()),
            )
            .unwrap();
        assert!(outcome.changed());
        assert_eq!(outcome.item_revision(), Revision::new(1));
        if item_id == "y-artifact" {
            assert_eq!(outcome.value_digest(), Some(&digest('c')));
        } else {
            assert!(outcome.value_digest().is_none());
        }
        state = outcome.into_state();
    }
    assert_eq!(state.workspace_revision(), Revision::new(7));
    assert_eq!(state.trace().revision(), Revision::new(7));
    assert!(
        state.workflow_memory().attempts()[0]
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_some())
    );

    let cleared_list = state
        .mutate_active_item_v2(
            &attempt_id(1),
            &item("m-tags"),
            Revision::new(1),
            ActiveItemMutationV2::Remove {
                value: "locked".to_owned(),
                ignore_missing: false,
            },
            UnixMillis::new(20),
        )
        .unwrap();
    assert!(cleared_list.changed());
    assert!(
        cleared_list.state().workflow_memory().attempts()[0]
            .item_slots()
            .iter()
            .find(|slot| slot.item_id() == &item("m-tags"))
            .unwrap()
            .value()
            .is_none()
    );

    let no_op = state
        .mutate_active_item_v2(
            &attempt_id(1),
            &item("c-confirm"),
            Revision::new(1),
            ActiveItemMutationV2::Check,
            UnixMillis::new(20),
        )
        .unwrap();
    assert!(!no_op.changed());
    assert_eq!(no_op.state(), &state);

    let completed = state
        .complete_active_action_v2(
            Revision::new(7),
            &attempt_id(1),
            Some(attempt_id(2)),
            UnixMillis::new(30),
        )
        .unwrap();
    assert_eq!(completed.to_graph_node_id(), Some(&node("gate")));
    assert_eq!(completed.to_attempt_id(), Some(&attempt_id(2)));
    assert_eq!(completed.state().trace().revision(), Revision::new(8));
    let capture = &completed.state().workflow_memory().attempts()[0];
    let gate = &completed.state().workflow_memory().attempts()[1];
    assert_eq!(
        gate.evidence()[0]
            .resolution()
            .snapshot()
            .unwrap()
            .items_digest(),
        &capture.recorded_items_digest().unwrap()
    );
    let readback = completed
        .state()
        .selected_evidence_readback(&attempt_id(2))
        .unwrap();
    assert_eq!(readback[0].items().items().len(), 1);
    assert_eq!(readback[0].items().items()[0].id(), &item("z-summary"));
}

#[test]
fn action_completion_rejects_an_open_blocker_without_changing_state() {
    let state = active_recorded_state(2, BlockerState::Open);
    let before = state.clone();

    assert_eq!(
        state.complete_active_action_v2(
            Revision::new(2),
            &attempt_id(1),
            Some(attempt_id(2)),
            UnixMillis::new(30),
        ),
        Err(GraphMutationErrorV2::BlockersPresent)
    );
    assert_eq!(state, before);
}

#[test]
fn skipped_optional_evidence_round_trips_as_empty_recorded_state() {
    let document = json!({
        "schema":"podway.procedure/v2",
        "id":"skipped-evidence",
        "version":"1",
        "name":"Skipped evidence",
        "purpose":"Persist an optional skipped reference.",
        "node_definitions":{
            "source-def":{
                "type":"action",
                "title":"Source",
                "intent":"Source.",
                "items":[{"id":"note","type":"text","prompt":"Note","required":false}]
            },
            "consumer-def":{"type":"action","title":"Consumer","intent":"Consumer."}
        },
        "graph":{
            "entry":"source",
            "nodes":[
                {"id":"source","use":"source-def","skip":{"allowed":true,"reason_required":false},"next":"consumer"},
                {"id":"consumer","use":"consumer-def","evidence_from":[{"node":"source","required":false}],"terminal":true}
            ]
        }
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let procedure = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000210").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("skipped.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap();
    let source_memory = AttemptWorkflowMemoryV2::new(
        attempt_id(1),
        vec![
            ItemSlotStateV2::new(
                attempt_id(1),
                item("note"),
                ItemTypeV1::Text,
                Revision::ZERO,
                None,
                UnixMillis::new(10),
                UnixMillis::new(10),
            )
            .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let skipped = EvidenceResolutionStateV2::new(
        0,
        false,
        Vec::new(),
        ResolvedEvidenceReferenceV2::skipped(
            EvidenceReferenceSnapshotV2::new(
                node("source"),
                attempt_id(1),
                AttemptNumberV2::FIRST,
                source_memory.recorded_items_digest().unwrap(),
                UnixMillis::new(20),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let consumer_memory =
        AttemptWorkflowMemoryV2::new(attempt_id(2), Vec::new(), Vec::new(), vec![skipped]).unwrap();
    let state = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        "Skipped evidence",
        procedure.clone(),
        SessionTraceV2::from_parts(
            session_id(),
            SessionLifecycle::Running,
            Revision::new(1),
            vec![
                attempt(
                    1,
                    "source",
                    1,
                    1,
                    AttemptLifecycle::Skipped,
                    AttemptValidityV2::Valid,
                ),
                attempt(
                    2,
                    "consumer",
                    1,
                    2,
                    AttemptLifecycle::Active,
                    AttemptValidityV2::Valid,
                ),
            ],
        )
        .unwrap(),
        vec![
            GraphNodeCounterV2::new(node("source"), 1, 0),
            GraphNodeCounterV2::new(node("consumer"), 1, 0),
        ],
        vec![
            AttemptMetadataV2::new(
                attempt_id(1),
                UnixMillis::new(10),
                Some(UnixMillis::new(20)),
                Some("Not needed.".to_owned()),
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(2), UnixMillis::new(20), None, None).unwrap(),
        ],
        WorkflowMemoryStateV2::new(vec![source_memory, consumer_memory], Vec::new(), Vec::new())
            .unwrap(),
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap();
    let bad_source = AttemptWorkflowMemoryV2::new(
        attempt_id(1),
        vec![
            ItemSlotStateV2::new(
                attempt_id(1),
                item("note"),
                ItemTypeV1::Text,
                Revision::new(1),
                Some(RecordedItemValueV2::text("must not survive skip").unwrap()),
                UnixMillis::new(10),
                UnixMillis::new(11),
            )
            .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let bad_reference = EvidenceResolutionStateV2::new(
        0,
        false,
        Vec::new(),
        ResolvedEvidenceReferenceV2::skipped(
            EvidenceReferenceSnapshotV2::new(
                node("source"),
                attempt_id(1),
                AttemptNumberV2::FIRST,
                bad_source.recorded_items_digest().unwrap(),
                UnixMillis::new(20),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let bad_consumer =
        AttemptWorkflowMemoryV2::new(attempt_id(2), Vec::new(), Vec::new(), vec![bad_reference])
            .unwrap();
    assert!(
        GraphSessionStateV2::new_with_workflow_memory(
            Revision::new(1),
            "Skipped evidence",
            procedure,
            state.trace().clone(),
            state.counters().to_vec(),
            state.attempt_metadata().to_vec(),
            WorkflowMemoryStateV2::new(vec![bad_source, bad_consumer], Vec::new(), Vec::new())
                .unwrap(),
            UnixMillis::new(10),
            None,
            None,
            None,
        )
        .is_err()
    );
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    drop(store);
    let loaded = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap())
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    assert_eq!(loaded, state);
    let readback = loaded.selected_evidence_readback(&attempt_id(2)).unwrap();
    assert!(matches!(
        readback[0].reference().resolution(),
        ResolvedEvidenceReferenceV2::Skipped(_)
    ));
    assert!(readback[0].items().items().is_empty());
}
