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

fn skip_snapshot() -> ProcedureSnapshotV2 {
    let mut document: serde_json::Value =
        serde_json::from_str(snapshot().canonical_json().as_str()).unwrap();
    let nodes = document["graph"]["nodes"].as_array_mut().unwrap();
    nodes[0]["skip"] = json!({"allowed": true, "reason_required": true});
    nodes[1]["evidence_from"][0]["required"] = json!(false);
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000111").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("skip-procedure.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

fn required_gate_item_snapshot() -> ProcedureSnapshotV2 {
    let mut document: serde_json::Value =
        serde_json::from_str(snapshot().canonical_json().as_str()).unwrap();
    document["node_definitions"]["gate-def"]["items"] = json!([
        {"id":"decision-note","type":"text","prompt":"Decision note","required":true}
    ]);
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000112").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("required-gate-item.yaml").unwrap(),
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
    ProcedureSnapshotV2::new(
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
fn v2drw001_decide_advance_records_the_route_and_activates_one_fresh_target() {
    let state = gate_state();
    let outcome = state
        .decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("proceed").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Proceed to finish.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap();

    assert_eq!(outcome.from_graph_node_id(), &node("gate"));
    assert_eq!(outcome.from_attempt_id(), &attempt_id(2));
    assert_eq!(outcome.to_graph_node_id(), &node("finish"));
    assert_eq!(outcome.to_attempt_id(), &attempt_id(3));
    assert!(outcome.declared_rework_record().is_none());
    let record = outcome.decision_record();
    assert_eq!(record.selected_option(), &OptionId::new("proceed").unwrap());
    assert_eq!(record.route_effect(), TransitionEffectV2::Advance);
    assert_eq!(record.session_revision(), Revision::new(5));
    assert_eq!(record.actor().unwrap().as_str(), "reviewer");
    assert_eq!(record.evidence().references().len(), 2);

    let next = outcome.into_state();
    assert_eq!(next.workspace_revision(), Revision::new(5));
    assert_eq!(next.trace().revision(), Revision::new(5));
    assert_eq!(
        next.trace().active_attempt().unwrap().graph_node_id(),
        &node("finish")
    );
    assert_eq!(next.trace().attempts().len(), 3);
    assert_eq!(next.workflow_memory().decisions().len(), 1);
    assert!(next.workflow_memory().reworks().is_empty());
    assert_eq!(next.counters()[2].attempt_count(), 1);
    assert_eq!(next.counters()[2].rework_traversal_count(), 0);
}

#[test]
fn v2drw001_decide_declared_rework_stales_the_suffix_and_records_one_reentry() {
    let state = gate_state();
    let outcome = state
        .decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("redo").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Redo the capture.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap();

    assert_eq!(outcome.to_graph_node_id(), &node("capture"));
    let rework = outcome.declared_rework_record().unwrap();
    assert_eq!(rework.kind(), ReworkKindV2::Declared);
    assert_eq!(rework.trace(), TraceSequenceV2::new(3));
    assert_eq!(rework.target_attempt_id(), &attempt_id(3));
    assert!(!rework.reactivated());
    let next = outcome.into_state();
    assert_eq!(
        next.trace()
            .attempts()
            .iter()
            .map(SessionAttemptV2::validity)
            .collect::<Vec<_>>(),
        vec![
            AttemptValidityV2::Stale,
            AttemptValidityV2::Stale,
            AttemptValidityV2::Valid,
        ]
    );
    assert_eq!(
        next.trace().active_attempt().unwrap().attempt_id(),
        &attempt_id(3)
    );
    assert_eq!(next.workflow_memory().decisions().len(), 1);
    assert_eq!(next.workflow_memory().reworks().len(), 1);
    assert_eq!(next.counters()[0].attempt_count(), 2);
    assert_eq!(next.counters()[0].rework_traversal_count(), 1);
}

#[test]
fn v2drw002_advance_decision_preserves_complete_provenance_across_sqlite_reopen() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_gate(&store);
    let outcome = gate_state()
        .decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("proceed").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Proceed to finish.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap();
    let expected_record = outcome.decision_record().clone();
    let next = outcome.into_state();

    let record = &next.workflow_memory().decisions()[0];
    assert_eq!(record.trace(), TraceSequenceV2::new(2));
    assert_eq!(record.session_id(), &session_id());
    assert_eq!(record.session_revision(), Revision::new(5));
    assert_eq!(record.procedure_snapshot_id(), snapshot().snapshot_id());
    assert_eq!(record.procedure_digest(), snapshot().digest());
    assert_eq!(record.graph_node_id(), &node("gate"));
    assert_eq!(
        record.node_definition_id(),
        &NodeDefinitionId::new("gate-def").unwrap()
    );
    assert_eq!(record.attempt_id(), &attempt_id(2));
    assert_eq!(record.attempt_number(), AttemptNumberV2::FIRST);
    assert_eq!(record.goal_revision(), None);
    assert_eq!(record.selected_option(), &OptionId::new("proceed").unwrap());
    assert_eq!(record.route_effect(), TransitionEffectV2::Advance);
    assert_eq!(record.route_target(), &node("finish"));
    assert_eq!(record.reason().as_str(), "Proceed to finish.");
    assert_eq!(record.actor().unwrap().as_str(), "reviewer");
    assert_eq!(record.recorded_at(), UnixMillis::new(40));
    assert_eq!(record.evidence().references().len(), 2);
    let resolved = record.evidence().references()[0].snapshot().unwrap();
    assert_eq!(resolved.source_node(), &node("capture"));
    assert_eq!(resolved.source_attempt_id(), &attempt_id(1));
    assert_eq!(resolved.source_attempt_number(), AttemptNumberV2::FIRST);
    assert_eq!(
        resolved.items_digest(),
        &next.workflow_memory().attempts()[0]
            .recorded_items_digest()
            .unwrap()
    );
    assert_eq!(resolved.resolved_at(), UnixMillis::new(30));
    assert!(matches!(
        &record.evidence().references()[1],
        ResolvedEvidenceReferenceV2::Unresolved { source_node }
            if source_node == &node("finish")
    ));

    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            next.clone(),
        )
        .unwrap();
    drop(store);
    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    let loaded = reopened
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    assert_eq!(loaded, next);
    assert_eq!(loaded.workflow_memory().decisions(), &[expected_record]);
}

#[test]
fn v2drw002_declared_rework_keeps_the_exact_decision_after_its_attempt_becomes_stale() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_gate(&store);
    let outcome = gate_state()
        .decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("redo").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Redo the capture.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap();
    let expected_record = outcome.decision_record().clone();
    let next = outcome.into_state();

    assert_eq!(
        next.trace().attempts()[1].validity(),
        AttemptValidityV2::Stale
    );
    assert_eq!(
        next.workflow_memory().decisions(),
        &[expected_record.clone()]
    );
    assert_eq!(
        next.workflow_memory().decisions()[0]
            .evidence()
            .references(),
        expected_record.evidence().references()
    );
    assert!(next.selected_evidence_readback(&attempt_id(2)).unwrap()[0].stale());

    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            next.clone(),
        )
        .unwrap();
    drop(store);
    let loaded = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap())
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    assert_eq!(loaded, next);
    assert_eq!(loaded.workflow_memory().decisions(), &[expected_record]);
    assert_eq!(
        loaded.trace().attempts()[1].validity(),
        AttemptValidityV2::Stale
    );
    assert!(loaded.selected_evidence_readback(&attempt_id(2)).unwrap()[0].stale());
}

#[test]
fn v2drw001_decide_rejects_stale_fences_option_and_missing_reason_without_mutation() {
    let state = gate_state();
    let before = state.clone();
    assert_eq!(
        state.decide_active_route_v2(
            Revision::new(3),
            &attempt_id(2),
            OptionId::new("proceed").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Proceed.").unwrap()),
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::SessionRevisionConflict {
            expected: Revision::new(3),
            actual: Revision::new(4),
        })
    );
    assert_eq!(state, before);
    assert_eq!(
        state.decide_active_route_v2(
            Revision::new(4),
            &attempt_id(99),
            OptionId::new("proceed").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Proceed.").unwrap()),
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::AttemptNotCurrent {
            expected: attempt_id(99),
            actual: Some(attempt_id(2)),
        })
    );
    assert_eq!(state, before);
    let unknown = OptionId::new("unknown").unwrap();
    assert_eq!(
        state.decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            unknown.clone(),
            attempt_id(3),
            Some(ReasonV2::new("Proceed.").unwrap()),
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::OptionNotAllowed {
            graph_node_id: node("gate"),
            option_id: unknown,
            allowed_option_ids: vec![
                OptionId::new("proceed").unwrap(),
                OptionId::new("redo").unwrap(),
            ],
        })
    );
    assert_eq!(state, before);
    assert_eq!(
        state.decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("proceed").unwrap(),
            attempt_id(3),
            None,
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::DecisionReasonMissing {
            graph_node_id: node("gate"),
        })
    );
    assert_eq!(state, before);
}

#[test]
fn v2drw001_decide_rejects_wrong_node_missing_local_item_and_open_blocker() {
    let action = initial_state();
    assert_eq!(
        action.decide_active_route_v2(
            Revision::new(1),
            &attempt_id(1),
            OptionId::new("proceed").unwrap(),
            attempt_id(2),
            Some(ReasonV2::new("Proceed.").unwrap()),
            None,
            UnixMillis::new(20),
        ),
        Err(GraphMutationErrorV2::GraphNodeTypeMismatch {
            graph_node_id: node("capture"),
            actual: podway_core::NodeKindV2::Action,
        })
    );

    let capture = capture_memory(true, Some((BlockerState::Resolved, "Waiting for review.")));
    let required_slot = ItemSlotStateV2::new(
        attempt_id(2),
        item("decision-note"),
        ItemTypeV1::Text,
        Revision::ZERO,
        None,
        UnixMillis::new(30),
        UnixMillis::new(30),
    )
    .unwrap();
    let required_gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        vec![required_slot],
        Vec::new(),
        gate_evidence(&capture),
    )
    .unwrap();
    let required_state = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(4),
        "Required decision item",
        required_gate_item_snapshot(),
        gate_state().trace().clone(),
        gate_state().counters().to_vec(),
        gate_state().attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(vec![capture.clone(), required_gate], Vec::new(), Vec::new())
            .unwrap(),
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        required_state.decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("proceed").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Proceed.").unwrap()),
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::RequiredItemsMissing {
            item_ids: vec![item("decision-note")],
        })
    );

    let blocker = BlockerStateV2::new(
        BlockerId::new("00000000-0000-4000-8000-000000000098").unwrap(),
        attempt_id(2),
        "Decision blocked.",
        BlockerState::Open,
        UnixMillis::new(35),
        None,
    )
    .unwrap();
    let blocked_gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        vec![blocker],
        gate_evidence(&capture),
    )
    .unwrap();
    let blocked_state = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(4),
        "Blocked decision",
        snapshot(),
        gate_state().trace().clone(),
        gate_state().counters().to_vec(),
        gate_state().attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(vec![capture, blocked_gate], Vec::new(), Vec::new()).unwrap(),
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        blocked_state.decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("proceed").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Proceed.").unwrap()),
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::BlockersPresent)
    );
}

#[test]
fn v2drw001_unresolved_and_stale_required_evidence_fail_before_decision_admission() {
    let capture = capture_memory(true, Some((BlockerState::Resolved, "Waiting for review.")));
    let unresolved_gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        Vec::new(),
        vec![
            EvidenceResolutionStateV2::new(
                0,
                false,
                vec![item("z-summary")],
                ResolvedEvidenceReferenceV2::unresolved(node("capture")),
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
    assert!(
        GraphSessionStateV2::new_with_workflow_memory(
            Revision::new(4),
            "Unresolved required evidence",
            snapshot(),
            gate_state().trace().clone(),
            gate_state().counters().to_vec(),
            gate_state().attempt_metadata().to_vec(),
            WorkflowMemoryStateV2::new(
                vec![capture.clone(), unresolved_gate],
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
            UnixMillis::new(10),
            None,
            None,
            None,
        )
        .is_err()
    );

    let stale_trace = SessionTraceV2::from_parts(
        session_id(),
        SessionLifecycle::Running,
        Revision::new(4),
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
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            ),
        ],
    )
    .unwrap();
    let stale_gate = AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        Vec::new(),
        Vec::new(),
        gate_evidence(&capture),
    )
    .unwrap();
    assert!(
        GraphSessionStateV2::new_with_workflow_memory(
            Revision::new(4),
            "Stale required evidence",
            snapshot(),
            stale_trace,
            gate_state().counters().to_vec(),
            gate_state().attempt_metadata().to_vec(),
            WorkflowMemoryStateV2::new(vec![capture, stale_gate], Vec::new(), Vec::new()).unwrap(),
            UnixMillis::new(10),
            None,
            None,
            None,
        )
        .is_err()
    );
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
fn block_unblock_and_cancel_are_revision_checked_history_preserving_transitions() {
    let initial = initial_state();
    let blocker = BlockerId::new("00000000-0000-4000-8000-000000000099").unwrap();
    let blocked = initial
        .block_active_attempt_v2(
            Revision::new(1),
            &attempt_id(1),
            blocker.clone(),
            "Waiting for review.",
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    assert_eq!(blocked.trace().revision(), Revision::new(2));
    assert_eq!(
        blocked.trace().active_attempt(),
        initial.trace().active_attempt()
    );
    assert_eq!(blocked.counters(), initial.counters());
    assert_eq!(blocked.goal_state(), initial.goal_state());
    assert_eq!(
        blocked.workflow_memory().attempts()[0].blockers()[0].blocker_id(),
        &blocker
    );

    let outcome = blocked
        .unblock_active_attempt_v2(
            Revision::new(2),
            &attempt_id(1),
            Some(&blocker),
            false,
            UnixMillis::new(30),
        )
        .unwrap();
    assert_eq!(outcome.blocker_ids(), &[blocker]);
    let unblocked = outcome.into_state();
    assert_eq!(unblocked.trace().revision(), Revision::new(3));
    assert_eq!(
        unblocked.workflow_memory().attempts()[0].blockers()[0].state(),
        BlockerState::Resolved
    );
    assert_eq!(
        unblocked.workflow_memory().attempts()[0].item_slots(),
        initial.workflow_memory().attempts()[0].item_slots()
    );

    let cancelled = unblocked
        .cancel_active_session_v2(
            Revision::new(3),
            &attempt_id(1),
            ReasonV2::new("No longer needed.").unwrap(),
            UnixMillis::new(40),
        )
        .unwrap()
        .into_state();
    assert_eq!(cancelled.trace().lifecycle(), SessionLifecycle::Cancelled);
    assert!(cancelled.trace().active_attempt().is_none());
    assert_eq!(cancelled.workflow_memory(), unblocked.workflow_memory());
    assert_eq!(cancelled.counters(), unblocked.counters());
    assert_eq!(cancelled.cancelled_at(), Some(UnixMillis::new(40)));
    assert_eq!(cancelled.cancel_reason(), Some("No longer needed."));
}

#[test]
fn block_enforces_the_v2_open_limit_and_session_global_id_uniqueness() {
    let mut state = initial_state();
    for number in 1..=64 {
        let blocker = BlockerId::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap();
        state = state
            .block_active_attempt_v2(
                Revision::new(number),
                &attempt_id(1),
                blocker,
                "Open blocker.",
                UnixMillis::new(20 + number),
            )
            .unwrap()
            .into_state();
    }
    let before = state.clone();
    assert_eq!(
        state.block_active_attempt_v2(
            Revision::new(65),
            &attempt_id(1),
            BlockerId::new("00000000-0000-4000-8000-000000000099").unwrap(),
            "One too many.",
            UnixMillis::new(100),
        ),
        Err(GraphMutationErrorV2::TooManyOpenBlockers { maximum: 64 })
    );
    assert_eq!(state, before);
}

#[test]
fn same_millisecond_blocks_are_canonically_ordered_by_frozen_id_and_reopen() {
    let initial = initial_state();
    let higher = BlockerId::new("00000000-0000-4000-8000-0000000000ff").unwrap();
    let lower = BlockerId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let first = initial
        .block_active_attempt_v2(
            Revision::new(1),
            &attempt_id(1),
            higher.clone(),
            "Higher identifier.",
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    let second = first
        .block_active_attempt_v2(
            Revision::new(2),
            &attempt_id(1),
            lower.clone(),
            "Lower identifier.",
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    assert_eq!(
        second.workflow_memory().attempts()[0]
            .blockers()
            .iter()
            .map(BlockerStateV2::blocker_id)
            .collect::<Vec<_>>(),
        vec![&lower, &higher]
    );
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(&identity(), second.clone())
        .unwrap();
    drop(store);
    assert_eq!(
        open(&temporary, SqliteStoreOptionsV1::new(8).unwrap())
            .read_graph_session_v2(&identity())
            .unwrap(),
        Some(second)
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
fn skip_clears_values_preserves_blockers_and_resolves_optional_source_as_empty() {
    assert_eq!(
        initial_state().skip_active_action_v2(
            Revision::new(1),
            &attempt_id(1),
            Some(attempt_id(2)),
            Some(ReasonV2::new("Not allowed.").unwrap()),
            UnixMillis::new(20),
        ),
        Err(GraphMutationErrorV2::SkipNotAllowed {
            graph_node_id: node("capture")
        })
    );
    assert_eq!(
        gate_state().skip_active_action_v2(
            Revision::new(4),
            &attempt_id(2),
            Some(attempt_id(3)),
            Some(ReasonV2::new("Decisions cannot skip.").unwrap()),
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::GraphNodeTypeMismatch {
            graph_node_id: node("gate"),
            actual: podway_core::NodeKindV2::Decision,
        })
    );
    let old_memory = capture_memory(true, Some((BlockerState::Open, "Still blocked.")));
    let state = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        "Skip recorded action",
        skip_snapshot(),
        SessionTraceV2::from_parts(
            session_id(),
            SessionLifecycle::Running,
            Revision::new(1),
            vec![attempt(
                1,
                "capture",
                1,
                1,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
            )],
        )
        .unwrap(),
        vec![
            GraphNodeCounterV2::new(node("capture"), 1, 0),
            GraphNodeCounterV2::new(node("gate"), 0, 0),
            GraphNodeCounterV2::new(node("finish"), 0, 0),
        ],
        vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
        WorkflowMemoryStateV2::new(vec![old_memory.clone()], Vec::new(), Vec::new()).unwrap(),
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        state.skip_active_action_v2(
            Revision::new(1),
            &attempt_id(1),
            Some(attempt_id(2)),
            None,
            UnixMillis::new(20),
        ),
        Err(GraphMutationErrorV2::SkipReasonRequired {
            graph_node_id: node("capture")
        })
    );

    let outcome = state
        .skip_active_action_v2(
            Revision::new(1),
            &attempt_id(1),
            Some(attempt_id(2)),
            Some(ReasonV2::new("Not needed for this task.").unwrap()),
            UnixMillis::new(20),
        )
        .unwrap();
    let skipped = outcome.state();
    let old_after = &skipped.workflow_memory().attempts()[0];
    let fresh = &skipped.workflow_memory().attempts()[1];

    assert_eq!(
        skipped.trace().attempts()[0].lifecycle(),
        AttemptLifecycle::Skipped
    );
    assert_eq!(old_after.blockers(), old_memory.blockers());
    assert_eq!(old_after.evidence(), old_memory.evidence());
    assert!(
        old_after
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
    for (before, after) in old_memory.item_slots().iter().zip(old_after.item_slots()) {
        assert_eq!(after.revision(), before.revision().checked_next().unwrap());
        assert_eq!(after.updated_at(), UnixMillis::new(20));
    }
    assert!(fresh.item_slots().is_empty());
    assert!(fresh.blockers().is_empty());
    assert!(matches!(
        fresh.evidence()[0].resolution(),
        ResolvedEvidenceReferenceV2::Skipped(_)
    ));
    let readback = skipped.selected_evidence_readback(&attempt_id(2)).unwrap();
    assert!(readback[0].items().items().is_empty());
    assert_eq!(
        fresh.evidence()[0]
            .resolution()
            .snapshot()
            .unwrap()
            .items_digest(),
        &old_after.recorded_items_digest().unwrap()
    );
    assert!(fresh.evidence()[1].resolution().snapshot().is_none());
    assert_eq!(skipped.counters()[0].attempt_count(), 1);
    assert_eq!(skipped.counters()[1].attempt_count(), 1);
    assert_eq!(skipped.counters()[2].attempt_count(), 0);

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store.create_graph_session_v2(&identity(), state).unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            outcome.into_state(),
        )
        .unwrap();
    drop(store);
    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    let loaded = reopened
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.trace().attempts()[0].lifecycle(),
        AttemptLifecycle::Skipped
    );
    assert!(
        loaded.workflow_memory().attempts()[0]
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
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
    let bad_counters = try_state_with_memory(
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
    assert!(bad_counters.is_err());
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(gate_state())
    );
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
fn v2drw003_manual_rework_reenters_running_and_completed_sessions() {
    let running = gate_state();
    let running_outcome = running
        .manual_rework_v2(
            Revision::new(4),
            Some(&attempt_id(2)),
            node("capture"),
            attempt_id(3),
            ReasonV2::new("Revisit the capture.").unwrap(),
            Some(ActorAttributionV2::new("operator").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap();
    assert!(!running_outcome.reactivated());
    assert_eq!(running_outcome.from_graph_node_id(), &node("gate"));
    assert_eq!(running_outcome.to_graph_node_id(), &node("capture"));
    assert_eq!(running_outcome.to_attempt_id(), &attempt_id(3));
    assert_eq!(running_outcome.record().kind(), ReworkKindV2::Manual);
    assert_eq!(
        running_outcome.record().actor().unwrap().as_str(),
        "operator"
    );
    let reentered = running_outcome.into_state();
    assert_eq!(reentered.trace().lifecycle(), SessionLifecycle::Running);
    assert_eq!(
        reentered.trace().active_attempt().unwrap().attempt_id(),
        &attempt_id(3)
    );
    assert_eq!(reentered.workflow_memory().reworks().len(), 1);
    assert_eq!(reentered.counters()[0].attempt_count(), 2);
    assert_eq!(reentered.counters()[0].rework_traversal_count(), 1);

    let completed = finish_state(6, true);
    let completed_outcome = completed
        .manual_rework_v2(
            Revision::new(6),
            None,
            node("capture"),
            attempt_id(4),
            ReasonV2::new("Requirements changed.").unwrap(),
            Some(ActorAttributionV2::new("operator").unwrap()),
            UnixMillis::new(60),
        )
        .unwrap();
    assert!(completed_outcome.reactivated());
    assert_eq!(completed_outcome.from_graph_node_id(), &node("finish"));
    let reactivated = completed_outcome.into_state();
    assert_eq!(reactivated.trace().revision(), Revision::new(7));
    assert_eq!(reactivated.trace().lifecycle(), SessionLifecycle::Running);
    assert!(reactivated.completed_at().is_none());
    assert_eq!(
        reactivated.trace().active_attempt().unwrap().attempt_id(),
        &attempt_id(4)
    );
    assert_eq!(reactivated, manual_reactivated_state());
}

#[test]
fn v2drw004_rework_successors_preserve_history_and_stale_exact_suffixes() {
    struct Case {
        name: &'static str,
        before: GraphSessionStateV2,
        after: GraphSessionStateV2,
        target: GraphNodeId,
        fresh_attempt_id: AttemptId,
        expected_source_lifecycle: AttemptLifecycle,
        expected_gate_evidence_stale: bool,
        appended_decision: bool,
    }

    let declared_before = gate_state();
    let declared_after = declared_before
        .decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("redo").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Redo the capture.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap()
        .into_state();

    let running_manual_before = gate_state();
    let running_manual_after = running_manual_before
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

    let same_node_before = finish_state(5, false);
    let same_node_after = same_node_before
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

    let completed_before = finish_state(6, true);
    let completed_after = completed_before
        .manual_rework_v2(
            Revision::new(6),
            None,
            node("capture"),
            attempt_id(4),
            ReasonV2::new("Requirements changed.").unwrap(),
            Some(ActorAttributionV2::new("operator").unwrap()),
            UnixMillis::new(60),
        )
        .unwrap()
        .into_state();

    let cases = [
        Case {
            name: "declared gate to capture",
            before: declared_before,
            after: declared_after,
            target: node("capture"),
            fresh_attempt_id: attempt_id(3),
            expected_source_lifecycle: AttemptLifecycle::Completed,
            expected_gate_evidence_stale: true,
            appended_decision: true,
        },
        Case {
            name: "running manual gate to capture",
            before: running_manual_before,
            after: running_manual_after,
            target: node("capture"),
            fresh_attempt_id: attempt_id(3),
            expected_source_lifecycle: AttemptLifecycle::Abandoned,
            expected_gate_evidence_stale: true,
            appended_decision: false,
        },
        Case {
            name: "same-node manual finish to finish",
            before: same_node_before,
            after: same_node_after,
            target: node("finish"),
            fresh_attempt_id: attempt_id(4),
            expected_source_lifecycle: AttemptLifecycle::Abandoned,
            expected_gate_evidence_stale: false,
            appended_decision: false,
        },
        Case {
            name: "completed manual finish to capture",
            before: completed_before,
            after: completed_after,
            target: node("capture"),
            fresh_attempt_id: attempt_id(4),
            expected_source_lifecycle: AttemptLifecycle::Completed,
            expected_gate_evidence_stale: true,
            appended_decision: false,
        },
    ];

    for case in cases {
        let old_attempt_count = case.before.trace().attempts().len();
        let target_trace = case
            .before
            .trace()
            .attempts()
            .iter()
            .find(|attempt| {
                attempt.graph_node_id() == &case.target
                    && attempt.validity() == AttemptValidityV2::Valid
            })
            .unwrap_or_else(|| panic!("{}: valid target is absent", case.name))
            .trace();
        let old_source = case.before.trace().attempts().last().unwrap();

        assert_eq!(
            case.after.workspace_revision(),
            case.before.workspace_revision().checked_next().unwrap(),
            "{}: workspace revision",
            case.name
        );
        assert_eq!(
            case.after.trace().revision(),
            case.before.trace().revision().checked_next().unwrap(),
            "{}: session revision",
            case.name
        );
        assert_eq!(
            case.after.trace().attempts().len(),
            old_attempt_count + 1,
            "{}: one fresh attempt",
            case.name
        );
        assert_eq!(
            &case.after.workflow_memory().attempts()[..old_attempt_count],
            case.before.workflow_memory().attempts(),
            "{}: old workflow memory",
            case.name
        );
        assert_eq!(
            &case.after.workflow_memory().decisions()
                [..case.before.workflow_memory().decisions().len()],
            case.before.workflow_memory().decisions(),
            "{}: decision prefix",
            case.name
        );
        assert_eq!(
            &case.after.workflow_memory().reworks()
                [..case.before.workflow_memory().reworks().len()],
            case.before.workflow_memory().reworks(),
            "{}: rework prefix",
            case.name
        );
        assert_eq!(
            case.after.workflow_memory().decisions().len(),
            case.before.workflow_memory().decisions().len() + usize::from(case.appended_decision),
            "{}: decision append count",
            case.name
        );
        assert_eq!(
            case.after.workflow_memory().reworks().len(),
            case.before.workflow_memory().reworks().len() + 1,
            "{}: rework append count",
            case.name
        );

        for (old, new) in case
            .before
            .trace()
            .attempts()
            .iter()
            .zip(case.after.trace().attempts())
        {
            assert_eq!(
                new.attempt_id(),
                old.attempt_id(),
                "{}: attempt id",
                case.name
            );
            assert_eq!(
                new.graph_node_id(),
                old.graph_node_id(),
                "{}: graph node id",
                case.name
            );
            assert_eq!(new.number(), old.number(), "{}: attempt number", case.name);
            assert_eq!(new.trace(), old.trace(), "{}: trace identity", case.name);
            assert_eq!(
                new.goal_revision(),
                old.goal_revision(),
                "{}: goal binding",
                case.name
            );
            let expected_validity =
                if old.validity() == AttemptValidityV2::Valid && old.trace() >= target_trace {
                    AttemptValidityV2::Stale
                } else {
                    old.validity()
                };
            assert_eq!(
                new.validity(),
                expected_validity,
                "{}: exact suffix validity at trace {}",
                case.name,
                old.trace().get()
            );
            let expected_lifecycle = if old.attempt_id() == old_source.attempt_id() {
                case.expected_source_lifecycle
            } else {
                old.lifecycle()
            };
            assert_eq!(
                new.lifecycle(),
                expected_lifecycle,
                "{}: lifecycle at trace {}",
                case.name,
                old.trace().get()
            );
        }

        let fresh = case.after.trace().attempts().last().unwrap();
        assert_eq!(
            fresh.attempt_id(),
            &case.fresh_attempt_id,
            "{}: fresh id",
            case.name
        );
        assert_eq!(
            fresh.graph_node_id(),
            &case.target,
            "{}: fresh target",
            case.name
        );
        assert_eq!(
            fresh.lifecycle(),
            AttemptLifecycle::Active,
            "{}: fresh lifecycle",
            case.name
        );
        assert_eq!(
            fresh.validity(),
            AttemptValidityV2::Valid,
            "{}: fresh validity",
            case.name
        );
        assert_eq!(
            fresh.trace(),
            TraceSequenceV2::new(old_attempt_count as u64 + 1),
            "{}: fresh trace",
            case.name
        );
        assert_eq!(
            case.after.trace().active_attempt(),
            Some(fresh),
            "{}: authoritative cursor",
            case.name
        );
        assert_eq!(
            case.after
                .trace()
                .attempts()
                .iter()
                .filter(|attempt| attempt.lifecycle() == AttemptLifecycle::Active)
                .count(),
            1,
            "{}: sole cursor",
            case.name
        );
        let fresh_memory = case.after.workflow_memory().attempts().last().unwrap();
        assert_eq!(
            fresh_memory.attempt_id(),
            fresh.attempt_id(),
            "{}: fresh memory",
            case.name
        );
        assert!(
            fresh_memory
                .item_slots()
                .iter()
                .all(|slot| { slot.revision() == Revision::ZERO && slot.value().is_none() }),
            "{}: fresh item memory",
            case.name
        );
        assert!(
            fresh_memory.blockers().is_empty(),
            "{}: fresh blockers",
            case.name
        );
        assert!(
            fresh_memory.evidence().is_empty(),
            "{}: fresh evidence",
            case.name
        );

        let target_counter_before = case
            .before
            .counters()
            .iter()
            .find(|counter| counter.graph_node_id() == &case.target)
            .unwrap();
        for counter_before in case.before.counters() {
            let counter_after = case
                .after
                .counters()
                .iter()
                .find(|counter| counter.graph_node_id() == counter_before.graph_node_id())
                .unwrap();
            let target_increment = u64::from(counter_before.graph_node_id() == &case.target);
            assert_eq!(
                counter_after.attempt_count(),
                counter_before.attempt_count() + target_increment,
                "{}: attempt counter for {}",
                case.name,
                counter_before.graph_node_id()
            );
            assert_eq!(
                counter_after.rework_traversal_count(),
                counter_before.rework_traversal_count() + target_increment,
                "{}: rework counter for {}",
                case.name,
                counter_before.graph_node_id()
            );
        }
        assert_eq!(
            fresh.number().get(),
            target_counter_before.attempt_count() + 1,
            "{}: fresh node attempt number",
            case.name
        );

        let gate_readback = case
            .after
            .selected_evidence_readback(&attempt_id(2))
            .unwrap();
        assert_eq!(
            gate_readback[0].stale(),
            case.expected_gate_evidence_stale,
            "{}: evidence staleness",
            case.name
        );
        assert_eq!(
            gate_readback[0].reference(),
            &case.before.workflow_memory().attempts()[1].evidence()[0],
            "{}: evidence snapshot identity",
            case.name
        );
        if case.appended_decision {
            let decision = case.after.workflow_memory().decisions().last().unwrap();
            assert_eq!(decision.attempt_id(), &attempt_id(2));
            assert_eq!(decision.route_effect(), TransitionEffectV2::Rework);
            assert_eq!(decision.route_target(), &node("capture"));
            assert_eq!(
                case.after.trace().attempts()[1].validity(),
                AttemptValidityV2::Stale,
                "{}: routing decision belongs to suffix",
                case.name
            );
            assert_eq!(gate_readback[0].decision(), None);
        } else if !case.before.workflow_memory().decisions().is_empty() {
            assert_eq!(
                case.after.workflow_memory().decisions(),
                case.before.workflow_memory().decisions(),
                "{}: retained decision record",
                case.name
            );
        }
    }
}

#[test]
fn v2drw004_fresh_reentry_reresolves_evidence_without_rewriting_stale_history() {
    let reworked = gate_state()
        .decide_active_route_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("redo").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Redo the capture.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(40),
        )
        .unwrap()
        .into_state();
    let stale_gate_memory = reworked.workflow_memory().attempts()[1].clone();
    let stale_decision = reworked.workflow_memory().decisions()[0].clone();

    let recorded = reworked
        .mutate_active_item_v2(
            &attempt_id(3),
            &item("z-summary"),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "reworked proof".to_owned(),
            },
            UnixMillis::new(41),
        )
        .unwrap()
        .into_state();
    let advanced = recorded
        .complete_active_action_v2(
            Revision::new(6),
            &attempt_id(3),
            Some(attempt_id(4)),
            UnixMillis::new(42),
        )
        .unwrap()
        .into_state();

    assert_eq!(advanced.workflow_memory().attempts()[1], stale_gate_memory);
    assert_eq!(advanced.workflow_memory().decisions()[0], stale_decision);
    assert!(advanced.selected_evidence_readback(&attempt_id(2)).unwrap()[0].stale());

    let fresh_capture = &advanced.workflow_memory().attempts()[2];
    let fresh_gate = &advanced.workflow_memory().attempts()[3];
    let fresh_reference = fresh_gate.evidence()[0].resolution().snapshot().unwrap();
    assert_eq!(fresh_reference.source_attempt_id(), &attempt_id(3));
    assert_eq!(
        fresh_reference.source_attempt_number(),
        AttemptNumberV2::new(2)
    );
    assert_eq!(fresh_reference.resolved_at(), UnixMillis::new(42));
    assert_eq!(
        fresh_reference.items_digest(),
        &fresh_capture.recorded_items_digest().unwrap()
    );
    assert!(!advanced.selected_evidence_readback(&attempt_id(4)).unwrap()[0].stale());
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
    let temporary = TempDir::new().unwrap();
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
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store.create_graph_session_v2(&identity(), state).unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
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

    let error = match SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(100),
    ) {
        Ok(_) => panic!("resurrected obsolete attempt validity must fail startup integrity"),
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
fn v2drw004_sqlite_reopen_rejects_fresh_reentry_staling_without_resurrection() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(&identity(), persisted_reentry_state())
        .unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE v2_attempts SET validity = 'stale' WHERE attempt_id = ?1",
            [attempt_id(3).as_str()],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(100),
    ) {
        Ok(_) => panic!("staled fresh reentry must fail startup integrity"),
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
fn v2drw_declared_rework_actor_tamper_fails_cold_load() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(
            &identity(),
            declared_rework_state(5, "Waiting for review.", false),
        )
        .unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE v2_rework_records SET actor = 'different-reviewer' WHERE kind = 'declared'",
            [],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(100),
    ) {
        Ok(_) => panic!("declared decision/rework actor mismatch must fail startup integrity"),
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
fn v2drw_declared_rework_timestamp_tamper_fails_cold_load() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(
            &identity(),
            declared_rework_state(5, "Waiting for review.", false),
        )
        .unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE v2_rework_records SET recorded_at_ms = recorded_at_ms + 1 WHERE kind = 'declared'",
            [],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(100),
    ) {
        Ok(_) => panic!("declared decision/rework timestamp mismatch must fail startup integrity"),
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
fn v2drw004_sqlite_reopen_rejects_running_manual_reactivation_bit_tamper() {
    let temporary = TempDir::new().unwrap();
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
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store.create_graph_session_v2(&identity(), state).unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE v2_rework_records SET reactivated = 1 WHERE trace_sequence = 3",
            [],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(100),
    ) {
        Ok(_) => panic!("running manual reactivation tamper must fail startup integrity"),
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
fn v2drw004_sqlite_reopen_rejects_deleted_same_node_manual_rework_record() {
    let temporary = TempDir::new().unwrap();
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
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store.create_graph_session_v2(&identity(), state).unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute("DELETE FROM v2_rework_records WHERE trace_sequence = 4", [])
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        database_path(&temporary),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(100),
    ) {
        Ok(_) => panic!("deleted same-node manual rework record must fail startup integrity"),
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
fn v2drw003_manual_rework_allows_the_current_node_and_rejects_invalid_policy_state() {
    let active_finish = finish_state(5, false);
    let same_node = active_finish
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
    assert_eq!(
        same_node.trace().attempts()[2].validity(),
        AttemptValidityV2::Stale
    );
    assert_eq!(
        same_node.trace().active_attempt().unwrap().graph_node_id(),
        &node("finish")
    );
    assert_eq!(same_node.counters()[2].attempt_count(), 2);
    assert_eq!(same_node.counters()[2].rework_traversal_count(), 1);

    let state = gate_state();
    let before = state.clone();
    assert_eq!(
        state.manual_rework_v2(
            Revision::new(3),
            Some(&attempt_id(2)),
            node("capture"),
            attempt_id(3),
            ReasonV2::new("Revisit.").unwrap(),
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::SessionRevisionConflict {
            expected: Revision::new(3),
            actual: Revision::new(4),
        })
    );
    assert_eq!(state, before);
    assert_eq!(
        state.manual_rework_v2(
            Revision::new(4),
            Some(&attempt_id(2)),
            node("gate"),
            attempt_id(3),
            ReasonV2::new("Revisit.").unwrap(),
            None,
            UnixMillis::new(40),
        ),
        Err(GraphMutationErrorV2::ManualReworkTargetNotAllowed {
            target_graph_node_id: node("gate"),
        })
    );
    assert_eq!(state, before);
    assert_eq!(
        initial_state().manual_rework_v2(
            Revision::new(1),
            Some(&attempt_id(1)),
            node("finish"),
            attempt_id(2),
            ReasonV2::new("Revisit.").unwrap(),
            None,
            UnixMillis::new(20),
        ),
        Err(GraphMutationErrorV2::ManualReworkTargetNotOnTrace {
            target_graph_node_id: node("finish"),
        })
    );

    let completed = finish_state(6, true);
    assert_eq!(
        completed.manual_rework_v2(
            Revision::new(6),
            Some(&attempt_id(3)),
            node("capture"),
            attempt_id(4),
            ReasonV2::new("Revisit.").unwrap(),
            None,
            UnixMillis::new(60),
        ),
        Err(GraphMutationErrorV2::AttemptNotCurrent {
            expected: attempt_id(3),
            actual: None,
        })
    );

    let cancelled = gate_state()
        .cancel_active_session_v2(
            Revision::new(4),
            &attempt_id(2),
            ReasonV2::new("Cancelled.").unwrap(),
            UnixMillis::new(40),
        )
        .unwrap()
        .into_state();
    assert_eq!(
        cancelled.manual_rework_v2(
            Revision::new(5),
            None,
            node("capture"),
            attempt_id(3),
            ReasonV2::new("Revisit.").unwrap(),
            None,
            UnixMillis::new(50),
        ),
        Err(GraphMutationErrorV2::SessionCancelled)
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
