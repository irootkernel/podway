//! Pure goal cases migrated from Store int_v2_goal_state; original names retained.

use crate::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2,
    CanonicalProcedureJsonV1, CriterionAssessmentReasonV2, CriterionAssessmentResultV2,
    CriterionCitationV2, CriterionId, CriterionStatusV2, DecisionRecordInputV2, DecisionRecordV2,
    EvidenceReferenceSnapshotV2, GoalAssessmentRecordV2, GoalCriterionV2, GoalDefinitionV2,
    GoalOutcome, GoalRevisionNumberV2, GoalRevisionReasonV2, GoalRevisionRecordV2, GoalStatementV2,
    GraphNodeId, ItemId, ItemTypeV1, NodeDefinitionId, OptionId, ProcedureSnapshotId,
    ProcedureSourceLabelV1, ReasonV2, RecordedItemValueV2, ResolvedEvidenceReferenceV2,
    ResolvedEvidenceSetV2, Revision, SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2,
    Sha256Digest, TraceSequenceV2, TransitionEffectV2, UnixMillis, canonicalize_json_v1,
};
use crate::{
    AttemptCriterionAssessmentStateV2, AttemptMetadataV2, AttemptWorkflowMemoryV2,
    CriterionAssessmentStateV2, EvidenceResolutionStateV2, GoalStateV2, GraphMutationErrorV2,
    GraphNodeCounterV2, GraphSessionStateV2, ItemSlotStateV2, ProcedureSnapshotV2,
    WorkflowMemoryStateV2,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn snapshot_from_canonical(
    id: crate::ProcedureSnapshotId,
    canonical: crate::CanonicalProcedureJsonV1,
    digest: crate::Sha256Digest,
    source: crate::ProcedureSourceLabelV1,
    now: crate::UnixMillis,
) -> Result<crate::ProcedureSnapshotV2, crate::ProcedureStateErrorV2> {
    crate::ProcedureSnapshotV2::from_admitted_document(
        id,
        crate::CanonicalProcedureDocumentV2::new(canonical, digest)?,
        source,
        now,
    )
}

fn node(value: &str) -> GraphNodeId {
    GraphNodeId::new(value).unwrap()
}

fn item(value: &str) -> ItemId {
    ItemId::new(value).unwrap()
}

fn criterion(value: &str) -> CriterionId {
    CriterionId::new(value).unwrap()
}

fn attempt_id(number: u64) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap()
}

fn session_id() -> SessionId {
    SessionId::new("00000000-0000-4000-8000-000000000420").unwrap()
}

fn snapshot() -> ProcedureSnapshotV2 {
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "goal-state",
        "version": "1",
        "name": "Goal state",
        "purpose": "Prove durable goal revision and assessment history.",
        "goal_tracking": true,
        "node_definitions": {
            "clarify-def": {
                "type": "action",
                "title": "Clarify",
                "intent": "Record supporting evidence.",
                "items": [
                    {"id":"proof","type":"text","prompt":"Proof","required":true}
                ]
            },
            "assess-def": {
                "type": "decision",
                "title": "Assess",
                "objective": "Assess the current session goal.",
                "prompt": "What is the goal outcome?",
                "items": [
                    {"id":"assessment-note","type":"text","prompt":"Note","required":false}
                ],
                "options": [
                    {"id":"achieved","label":"Achieved"},
                    {"id":"not-achieved","label":"Not achieved"},
                    {"id":"superseded","label":"Superseded"}
                ],
                "assessment": {
                    "target":"session_goal",
                    "outcomes": {
                        "achieved":"achieved",
                        "not-achieved":"not_achieved",
                        "superseded":"superseded"
                    }
                },
                "reason":{"required":true}
            },
            "finish-def": {"type":"action","title":"Finish","intent":"Finish."}
        },
        "graph": {
            "entry":"clarify",
            "nodes": [
                {"id":"clarify","use":"clarify-def","next":"assess"},
                {
                    "id":"assess",
                    "use":"assess-def",
                    "evidence_from":[{"node":"clarify","required":true}],
                    "routes": {
                        "achieved":{"to":"finish","effect":"advance"},
                        "not-achieved":{"to":"finish","effect":"advance"},
                        "superseded":{"to":"finish","effect":"advance"}
                    }
                },
                {"id":"finish","use":"finish-def","skip":{"allowed":true,"reason_required":false},"terminal":true}
            ]
        },
        "manual_rework":{"allowed_targets":["clarify","assess"]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    snapshot_from_canonical(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000410").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("goal.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

fn opt_out_snapshot() -> ProcedureSnapshotV2 {
    let document = json!({
        "schema":"podway.procedure/v2",
        "id":"no-goal",
        "version":"1",
        "name":"No goal",
        "purpose":"Prove goal state requires opt-in.",
        "node_definitions": {
            "work-def":{"type":"action","title":"Work","intent":"Work."}
        },
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work-def","terminal":true}]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    snapshot_from_canonical(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000411").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("no-goal.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

fn terminal_goal_snapshot() -> ProcedureSnapshotV2 {
    let document = json!({
        "schema":"podway.procedure/v2",
        "id":"terminal-goal-gate",
        "version":"1",
        "name":"Terminal goal gate",
        "purpose":"Prove terminal completion requires a fresh goal assessment.",
        "goal_tracking":true,
        "node_definitions": {
            "finish-def":{"type":"action","title":"Finish","intent":"Finish."}
        },
        "graph":{
            "entry":"finish",
            "nodes":[{"id":"finish","use":"finish-def","skip":{"allowed":true,"reason_required":false},"terminal":true}]
        }
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    snapshot_from_canonical(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000413").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("terminal-goal.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

fn stale_assessment_terminal_snapshot() -> ProcedureSnapshotV2 {
    let document = json!({
        "schema":"podway.procedure/v2",
        "id":"stale-assessment-terminal-gate",
        "version":"1",
        "name":"Stale assessment terminal gate",
        "purpose":"Prove stale goal assessment history cannot satisfy terminal readiness.",
        "goal_tracking":true,
        "node_definitions": {
            "hub-def":{
                "type":"decision",
                "title":"Hub",
                "objective":"Choose the next step.",
                "prompt":"Where next?",
                "options":[
                    {"id":"assess","label":"Assess"},
                    {"id":"finish","label":"Finish"}
                ],
                "reason":{"required":true}
            },
            "assess-def":{
                "type":"decision",
                "title":"Assess",
                "objective":"Assess the current session goal.",
                "prompt":"What is the goal outcome?",
                "options":[
                    {"id":"achieved","label":"Achieved"},
                    {"id":"not-achieved","label":"Not achieved"},
                    {"id":"superseded","label":"Superseded"}
                ],
                "assessment":{"target":"session_goal","outcomes":{
                    "achieved":"achieved",
                    "not-achieved":"not_achieved",
                    "superseded":"superseded"
                }},
                "reason":{"required":true}
            },
            "finish-def":{"type":"action","title":"Finish","intent":"Finish."}
        },
        "graph":{
            "entry":"hub",
            "nodes":[
                {"id":"hub","use":"hub-def","routes":{
                    "assess":{"to":"assess","effect":"advance"},
                    "finish":{"to":"finish","effect":"advance"}
                }},
                {"id":"assess","use":"assess-def","routes":{
                    "achieved":{"to":"finish","effect":"advance"},
                    "not-achieved":{"to":"finish","effect":"advance"},
                    "superseded":{"to":"finish","effect":"advance"}
                }},
                {"id":"finish","use":"finish-def","skip":{"allowed":true,"reason_required":false},"terminal":true}
            ]
        },
        "manual_rework":{"allowed_targets":["hub"]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    snapshot_from_canonical(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000414").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("stale-assessment-terminal.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

fn unsafe_rework_route_snapshot() -> ProcedureSnapshotV2 {
    let assessment = |title: &str| {
        json!({
            "type":"decision","title":title,"objective":"Assess.","prompt":"Outcome?",
            "options":[
                {"id":"achieved","label":"Achieved"},
                {"id":"not-achieved","label":"Not achieved"},
                {"id":"superseded","label":"Superseded"}
            ],
            "assessment":{"target":"session_goal","outcomes":{
                "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"
            }},
            "reason":{"required":true}
        })
    };
    let document = json!({
        "schema":"podway.procedure/v2","id":"unsafe-revision-route","version":"1",
        "name":"Unsafe revision route","purpose":"A rework route can bypass a fresh assessment.",
        "goal_tracking":true,
        "node_definitions":{
            "initial-assess-def":assessment("Initial assessment"),
            "hub-def":{"type":"decision","title":"Hub","objective":"Choose.","prompt":"Route?","options":[{"id":"work","label":"Work"},{"id":"finish","label":"Finish"}],"reason":{"required":true}},
            "target-def":{"type":"action","title":"Target","intent":"Revise here."},
            "router-def":{"type":"decision","title":"Router","objective":"Choose.","prompt":"Route?","options":[{"id":"forward","label":"Forward"},{"id":"redo","label":"Redo"}],"reason":{"required":true}},
            "final-assess-def":assessment("Final assessment"),
            "terminal-def":{"type":"action","title":"Terminal","intent":"Finish."}
        },
        "graph":{"entry":"initial-assess","nodes":[
            {"id":"initial-assess","use":"initial-assess-def","routes":{
                "achieved":{"to":"hub","effect":"advance"},"not-achieved":{"to":"hub","effect":"advance"},"superseded":{"to":"hub","effect":"advance"}}},
            {"id":"hub","use":"hub-def","routes":{"work":{"to":"target","effect":"advance"},"finish":{"to":"terminal","effect":"advance"}}},
            {"id":"target","use":"target-def","next":"router"},
            {"id":"router","use":"router-def","routes":{"forward":{"to":"final-assess","effect":"advance"},"redo":{"to":"hub","effect":"rework"}}},
            {"id":"final-assess","use":"final-assess-def","routes":{
                "achieved":{"to":"terminal","effect":"advance"},"not-achieved":{"to":"terminal","effect":"advance"},"superseded":{"to":"terminal","effect":"advance"}}},
            {"id":"terminal","use":"terminal-def","terminal":true}
        ]},
        "manual_rework":{"allowed_targets":["target"]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    snapshot_from_canonical(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000412").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("unsafe-route.yaml").unwrap(),
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
    goal_revision: Option<u64>,
) -> SessionAttemptV2 {
    SessionAttemptV2::new(
        attempt_id(number),
        node(graph_node),
        AttemptNumberV2::new(node_attempt),
        TraceSequenceV2::new(trace),
        lifecycle,
        validity,
        goal_revision.map(crate::GoalRevisionNumberV2::new),
    )
    .unwrap()
}

fn clarify_memory(attempt_number: u64, recorded: bool) -> AttemptWorkflowMemoryV2 {
    let started_at = if attempt_number == 1 { 10 } else { 80 };
    AttemptWorkflowMemoryV2::new(
        attempt_id(attempt_number),
        vec![
            ItemSlotStateV2::new(
                attempt_id(attempt_number),
                item("proof"),
                ItemTypeV1::Text,
                if recorded {
                    Revision::new(1)
                } else {
                    Revision::ZERO
                },
                recorded.then(|| RecordedItemValueV2::text("restart survives").unwrap()),
                UnixMillis::new(started_at),
                UnixMillis::new(if recorded { started_at + 1 } else { started_at }),
            )
            .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

fn assess_memory(
    clarify: &AttemptWorkflowMemoryV2,
    note_recorded: bool,
) -> AttemptWorkflowMemoryV2 {
    AttemptWorkflowMemoryV2::new(
        attempt_id(2),
        vec![
            ItemSlotStateV2::new(
                attempt_id(2),
                item("assessment-note"),
                ItemTypeV1::Text,
                if note_recorded {
                    Revision::new(1)
                } else {
                    Revision::ZERO
                },
                note_recorded.then(|| RecordedItemValueV2::text("reviewed evidence").unwrap()),
                UnixMillis::new(30),
                UnixMillis::new(if note_recorded { 31 } else { 30 }),
            )
            .unwrap(),
        ],
        Vec::new(),
        vec![
            EvidenceResolutionStateV2::new(
                0,
                true,
                Vec::new(),
                ResolvedEvidenceReferenceV2::resolved(
                    EvidenceReferenceSnapshotV2::new(
                        node("clarify"),
                        attempt_id(1),
                        AttemptNumberV2::FIRST,
                        clarify.recorded_items_digest().unwrap(),
                        UnixMillis::new(30),
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

fn assessment_decision(clarify: &AttemptWorkflowMemoryV2) -> DecisionRecordV2 {
    DecisionRecordV2::new(DecisionRecordInputV2 {
        trace: TraceSequenceV2::new(2),
        session_id: session_id(),
        session_revision: Revision::new(8),
        procedure_snapshot_id: snapshot().snapshot_id().clone(),
        procedure_digest: snapshot().digest().clone(),
        graph_node_id: node("assess"),
        node_definition_id: NodeDefinitionId::new("assess-def").unwrap(),
        attempt_id: attempt_id(2),
        attempt_number: AttemptNumberV2::FIRST,
        goal_revision: Some(crate::GoalRevisionNumberV2::FIRST),
        selected_option: OptionId::new("achieved").unwrap(),
        route_effect: TransitionEffectV2::Advance,
        route_target: node("finish"),
        reason: ReasonV2::new("Every criterion is satisfied.").unwrap(),
        evidence: ResolvedEvidenceSetV2::new(
            assess_memory(clarify, true)
                .evidence()
                .iter()
                .map(|reference| reference.resolution().clone())
                .collect(),
        )
        .unwrap(),
        actor: Some(ActorAttributionV2::new("reviewer").unwrap()),
        recorded_at: UnixMillis::new(60),
    })
    .unwrap()
}

fn criteria(version: u64) -> GoalDefinitionV2 {
    let safety = if version == 1 {
        "Restart preserves the acknowledged outcome."
    } else {
        "Restart preserves the new goal outcome."
    };
    GoalDefinitionV2::new(vec![
        GoalCriterionV2::new(criterion("z-proof"), "Repeated requests have one outcome.").unwrap(),
        GoalCriterionV2::new(criterion("a-safety"), safety).unwrap(),
    ])
    .unwrap()
}

fn goal_revision_one() -> GoalRevisionRecordV2 {
    GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        None,
        GoalStatementV2::new("Cancellation is deterministic.").unwrap(),
        criteria(1),
        None,
        None,
        false,
        Some(ActorAttributionV2::new("planner").unwrap()),
        TraceSequenceV2::FIRST,
        UnixMillis::new(20),
    )
    .unwrap()
}

fn terminal_goal_state(goal_revision: Option<u64>, goal: GoalStateV2) -> GraphSessionStateV2 {
    let active = attempt(
        9,
        "finish",
        1,
        1,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        goal_revision,
    );
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(1),
        "Terminal goal gate",
        terminal_goal_snapshot(),
        SessionTraceV2::from_parts(
            session_id(),
            SessionLifecycle::Running,
            Revision::new(1),
            vec![active],
        )
        .unwrap(),
        vec![GraphNodeCounterV2::new(node("finish"), 1, 0)],
        vec![AttemptMetadataV2::new(attempt_id(9), UnixMillis::new(20), None, None).unwrap()],
        WorkflowMemoryStateV2::new(
            vec![
                AttemptWorkflowMemoryV2::new(attempt_id(9), Vec::new(), Vec::new(), Vec::new())
                    .unwrap(),
            ],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        goal,
        UnixMillis::new(20),
        None,
        None,
        None,
    )
    .unwrap()
}

fn criterion_result(id: &str, citations: Vec<CriterionCitationV2>) -> CriterionAssessmentStateV2 {
    criterion_result_at(
        id,
        CriterionStatusV2::Satisfied,
        citations,
        if id == "z-proof" { 40 } else { 50 },
    )
}

fn criterion_result_at(
    id: &str,
    status: CriterionStatusV2,
    citations: Vec<CriterionCitationV2>,
    recorded_at: u64,
) -> CriterionAssessmentStateV2 {
    CriterionAssessmentStateV2::new(
        CriterionAssessmentResultV2::new(
            criterion(id),
            status,
            CriterionAssessmentReasonV2::new("The recorded evidence supports this criterion.")
                .unwrap(),
            citations,
        )
        .unwrap(),
        Some(ActorAttributionV2::new("reviewer").unwrap()),
        UnixMillis::new(recorded_at),
    )
}

fn complete_criterion_state() -> AttemptCriterionAssessmentStateV2 {
    AttemptCriterionAssessmentStateV2::new(
        attempt_id(2),
        GoalRevisionNumberV2::FIRST,
        vec![
            criterion_result(
                "z-proof",
                vec![
                    CriterionCitationV2::Evidence(node("clarify")),
                    CriterionCitationV2::Item(item("assessment-note")),
                ],
            ),
            criterion_result("a-safety", Vec::new()),
        ],
    )
    .unwrap()
}

fn goal_assessment(clarify: &AttemptWorkflowMemoryV2) -> GoalAssessmentRecordV2 {
    GoalAssessmentRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        GoalOutcome::Achieved,
        complete_criterion_state()
            .results()
            .iter()
            .map(|state| state.result().clone())
            .collect(),
        ResolvedEvidenceSetV2::new(
            assess_memory(clarify, true)
                .evidence()
                .iter()
                .map(|reference| reference.resolution().clone())
                .collect(),
        )
        .unwrap(),
        Some(ActorAttributionV2::new("reviewer").unwrap()),
        attempt_id(2),
        node("assess"),
        TraceSequenceV2::new(2),
        UnixMillis::new(60),
    )
    .unwrap()
}

fn partial_criterion_state() -> AttemptCriterionAssessmentStateV2 {
    AttemptCriterionAssessmentStateV2::new(
        attempt_id(2),
        GoalRevisionNumberV2::FIRST,
        vec![criterion_result(
            "z-proof",
            vec![
                CriterionCitationV2::Evidence(node("clarify")),
                CriterionCitationV2::Item(item("assessment-note")),
            ],
        )],
    )
    .unwrap()
}

fn state(
    revision: u64,
    lifecycle: SessionLifecycle,
    attempts: Vec<SessionAttemptV2>,
    metadata: Vec<AttemptMetadataV2>,
    counters: Vec<GraphNodeCounterV2>,
    workflow: WorkflowMemoryStateV2,
    goal: GoalStateV2,
    completed_at: Option<UnixMillis>,
) -> GraphSessionStateV2 {
    try_state(
        revision,
        lifecycle,
        attempts,
        metadata,
        counters,
        workflow,
        goal,
        completed_at,
    )
    .unwrap()
}

fn try_state(
    revision: u64,
    lifecycle: SessionLifecycle,
    attempts: Vec<SessionAttemptV2>,
    metadata: Vec<AttemptMetadataV2>,
    counters: Vec<GraphNodeCounterV2>,
    workflow: WorkflowMemoryStateV2,
    goal: GoalStateV2,
    completed_at: Option<UnixMillis>,
) -> Result<GraphSessionStateV2, crate::ProcedureStateErrorV2> {
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(revision),
        "Persist goal state",
        snapshot(),
        SessionTraceV2::from_parts(session_id(), lifecycle, Revision::new(revision), attempts)
            .unwrap(),
        counters,
        metadata,
        workflow,
        goal,
        UnixMillis::new(10),
        completed_at,
        None,
        None,
    )
}

fn counters(
    clarify: u64,
    assess: u64,
    finish: u64,
    clarify_reworks: u64,
) -> Vec<GraphNodeCounterV2> {
    vec![
        GraphNodeCounterV2::new(node("clarify"), clarify, clarify_reworks),
        GraphNodeCounterV2::new(node("assess"), assess, 0),
        GraphNodeCounterV2::new(node("finish"), finish, 0),
    ]
}

fn assessment_state(
    revision: u64,
    note_recorded: bool,
    results: Vec<AttemptCriterionAssessmentStateV2>,
) -> GraphSessionStateV2 {
    let clarify = clarify_memory(1, true);
    let assess = assess_memory(&clarify, note_recorded);
    state(
        revision,
        SessionLifecycle::Running,
        vec![
            attempt(
                1,
                "clarify",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                Some(1),
            ),
            attempt(
                2,
                "assess",
                1,
                2,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                Some(1),
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
        counters(1, 1, 0, 0),
        WorkflowMemoryStateV2::new(vec![clarify, assess], Vec::new(), Vec::new()).unwrap(),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![goal_revision_one()],
            results,
            Vec::new(),
        )
        .unwrap(),
        None,
    )
}

fn decided_state(revision: u64, completed: bool) -> GraphSessionStateV2 {
    let clarify = clarify_memory(1, true);
    let assess = assess_memory(&clarify, true);
    let finish =
        AttemptWorkflowMemoryV2::new(attempt_id(3), Vec::new(), Vec::new(), Vec::new()).unwrap();
    state(
        revision,
        if completed {
            SessionLifecycle::Completed
        } else {
            SessionLifecycle::Running
        },
        vec![
            attempt(
                1,
                "clarify",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                Some(1),
            ),
            attempt(
                2,
                "assess",
                1,
                2,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                Some(1),
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
                Some(1),
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
                Some(UnixMillis::new(60)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                attempt_id(3),
                UnixMillis::new(60),
                completed.then_some(UnixMillis::new(70)),
                None,
            )
            .unwrap(),
        ],
        counters(1, 1, 1, 0),
        WorkflowMemoryStateV2::new(
            vec![clarify.clone(), assess, finish],
            vec![assessment_decision(&clarify)],
            Vec::new(),
        )
        .unwrap(),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![goal_revision_one()],
            vec![complete_criterion_state()],
            vec![goal_assessment(&clarify)],
        )
        .unwrap(),
        completed.then_some(UnixMillis::new(70)),
    )
}

#[test]
fn criterion_mutation_is_cursor_stable_and_normalizes_reverse_call_order() {
    let initial = assessment_state(5, true, Vec::new());
    let first_result =
        criterion_result_at("a-safety", CriterionStatusV2::Satisfied, Vec::new(), 40);
    let first = initial
        .assess_goal_criterion_v2(
            Revision::new(5),
            &attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            first_result.result().clone(),
            first_result.actor().cloned(),
            first_result.recorded_at(),
        )
        .unwrap();
    assert!(!first.complete());
    assert_eq!(first.determined_outcome(), None);
    assert_eq!(first.state().trace().attempts(), initial.trace().attempts());
    assert_eq!(first.state().trace().revision(), Revision::new(6));

    let mixed = CriterionAssessmentResultV2::new(
        criterion("z-proof"),
        CriterionStatusV2::NotApplicable,
        CriterionAssessmentReasonV2::new("The goal is being superseded.").unwrap(),
        Vec::new(),
    )
    .unwrap();
    assert!(matches!(
        first.state().assess_goal_criterion_v2(
            Revision::new(6),
            &attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            mixed,
            None,
            UnixMillis::new(41),
        ),
        Err(GraphMutationErrorV2::CriterionModeMixed { .. })
    ));

    let second_result = criterion_result_at(
        "z-proof",
        CriterionStatusV2::Satisfied,
        vec![
            CriterionCitationV2::Evidence(node("clarify")),
            CriterionCitationV2::Item(item("assessment-note")),
        ],
        41,
    );
    let second = first
        .state()
        .assess_goal_criterion_v2(
            Revision::new(6),
            &attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            second_result.result().clone(),
            second_result.actor().cloned(),
            second_result.recorded_at(),
        )
        .unwrap();
    assert!(second.complete());
    assert_eq!(second.determined_outcome(), Some(GoalOutcome::Achieved));
    assert_eq!(
        second.state().trace().attempts(),
        initial.trace().attempts()
    );
    let ids = second.state().goal_state().attempt_assessments()[0]
        .results()
        .iter()
        .map(|state| state.result().criterion_id().as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["z-proof", "a-safety"]);
    assert!(matches!(
        second.state().assess_goal_criterion_v2(
            Revision::new(7),
            &attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            second_result.result().clone(),
            None,
            UnixMillis::new(42),
        ),
        Err(GraphMutationErrorV2::CriterionResultAlreadyRecorded { .. })
    ));
}

#[test]
fn goal_assessment_decision_requires_complete_results_and_the_derived_outcome() {
    let incomplete = assessment_state(6, true, vec![partial_criterion_state()]);
    assert_eq!(
        incomplete.decide_active_route_with_goal_revision_v2(
            Revision::new(6),
            &attempt_id(2),
            OptionId::new("achieved").unwrap(),
            attempt_id(3),
            Some(GoalRevisionNumberV2::FIRST),
            Some(ReasonV2::new("Every criterion is satisfied.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(60),
        ),
        Err(GraphMutationErrorV2::CriterionResultMissing {
            missing_criterion_ids: vec![criterion("a-safety")],
        })
    );

    let complete = assessment_state(7, true, vec![complete_criterion_state()]);
    assert!(matches!(
        complete.decide_active_route_v2(
            Revision::new(7),
            &attempt_id(2),
            OptionId::new("achieved").unwrap(),
            attempt_id(3),
            Some(ReasonV2::new("Every criterion is satisfied.").unwrap()),
            None,
            UnixMillis::new(60),
        ),
        Err(GraphMutationErrorV2::GoalAssessmentDecisionRequiresAssessment {
            graph_node_id
        }) if graph_node_id == node("assess")
    ));
    assert!(matches!(
        complete.decide_active_route_with_goal_revision_v2(
            Revision::new(7),
            &attempt_id(2),
            OptionId::new("achieved").unwrap(),
            attempt_id(3),
            None,
            Some(ReasonV2::new("Every criterion is satisfied.").unwrap()),
            None,
            UnixMillis::new(60),
        ),
        Err(GraphMutationErrorV2::InvalidState(_))
    ));
    assert_eq!(
        complete.decide_active_route_with_goal_revision_v2(
            Revision::new(7),
            &attempt_id(2),
            OptionId::new("achieved").unwrap(),
            attempt_id(3),
            Some(GoalRevisionNumberV2::new(2)),
            Some(ReasonV2::new("Every criterion is satisfied.").unwrap()),
            None,
            UnixMillis::new(60),
        ),
        Err(GraphMutationErrorV2::GoalRevisionStale {
            expected: GoalRevisionNumberV2::new(2),
            actual: GoalRevisionNumberV2::FIRST,
        })
    );
    assert_eq!(
        complete.decide_active_route_with_goal_revision_v2(
            Revision::new(7),
            &attempt_id(2),
            OptionId::new("not-achieved").unwrap(),
            attempt_id(3),
            Some(GoalRevisionNumberV2::FIRST),
            Some(ReasonV2::new("Every criterion is satisfied.").unwrap()),
            Some(ActorAttributionV2::new("reviewer").unwrap()),
            UnixMillis::new(60),
        ),
        Err(GraphMutationErrorV2::GoalAssessmentOutcomeNotAllowed {
            option_id: OptionId::new("not-achieved").unwrap(),
            determined_outcome: GoalOutcome::Achieved,
            allowed_option_ids: vec![OptionId::new("achieved").unwrap()],
        })
    );
}

#[test]
fn goal_assessment_decision_derives_all_three_outcomes_from_homogeneous_results() {
    let cases = [
        (
            CriterionStatusV2::Satisfied,
            CriterionStatusV2::Unsatisfied,
            "not-achieved",
            GoalOutcome::NotAchieved,
        ),
        (
            CriterionStatusV2::NotApplicable,
            CriterionStatusV2::NotApplicable,
            "superseded",
            GoalOutcome::Superseded,
        ),
    ];
    for (first_status, second_status, option, expected_outcome) in cases {
        let criterion_state = AttemptCriterionAssessmentStateV2::new(
            attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            vec![
                criterion_result_at("z-proof", first_status, Vec::new(), 40),
                criterion_result_at("a-safety", second_status, Vec::new(), 50),
            ],
        )
        .unwrap();
        let state = assessment_state(7, true, vec![criterion_state]);
        let outcome = state
            .decide_active_route_with_goal_revision_v2(
                Revision::new(7),
                &attempt_id(2),
                OptionId::new(option).unwrap(),
                attempt_id(3),
                Some(GoalRevisionNumberV2::FIRST),
                Some(ReasonV2::new("The complete assessment determines this outcome.").unwrap()),
                Some(ActorAttributionV2::new("reviewer").unwrap()),
                UnixMillis::new(60),
            )
            .unwrap();
        assert_eq!(
            outcome.goal_assessment_record().unwrap().outcome(),
            expected_outcome
        );
    }
}

#[test]
fn malformed_goal_owners_opt_out_and_timestamp_bounds_are_rejected() {
    assert!(
        AttemptCriterionAssessmentStateV2::new(
            attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            Vec::new(),
        )
        .is_err()
    );

    let earlier = goal_assessment(&clarify_memory(1, true));
    let later = GoalAssessmentRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        GoalOutcome::Achieved,
        complete_criterion_state()
            .results()
            .iter()
            .map(|state| state.result().clone())
            .collect(),
        earlier.evidence().clone(),
        earlier.actor().cloned(),
        attempt_id(4),
        node("assess"),
        TraceSequenceV2::new(4),
        UnixMillis::new(90),
    )
    .unwrap();
    assert!(
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![goal_revision_one()],
            Vec::new(),
            vec![later, earlier],
        )
        .is_err()
    );

    let no_goal_attempt = SessionAttemptV2::new(
        attempt_id(1),
        node("work"),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        Some(GoalRevisionNumberV2::FIRST),
    )
    .unwrap();
    assert!(
        GraphSessionStateV2::new_with_goal_state(
            Revision::new(1),
            "Opt-out",
            opt_out_snapshot(),
            SessionTraceV2::from_parts(
                session_id(),
                SessionLifecycle::Running,
                Revision::new(1),
                vec![no_goal_attempt],
            )
            .unwrap(),
            vec![GraphNodeCounterV2::new(node("work"), 1, 0)],
            vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
            WorkflowMemoryStateV2::new(
                vec![
                    AttemptWorkflowMemoryV2::new(attempt_id(1), Vec::new(), Vec::new(), Vec::new())
                        .unwrap()
                ],
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
            GoalStateV2::new(
                Some(GoalRevisionNumberV2::FIRST),
                vec![goal_revision_one()],
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

    let base = assessment_state(5, true, Vec::new());
    let too_early = AttemptCriterionAssessmentStateV2::new(
        attempt_id(2),
        GoalRevisionNumberV2::FIRST,
        vec![criterion_result_at(
            "z-proof",
            CriterionStatusV2::Satisfied,
            Vec::new(),
            29,
        )],
    )
    .unwrap();
    assert!(
        try_state(
            5,
            SessionLifecycle::Running,
            base.trace().attempts().to_vec(),
            base.attempt_metadata().to_vec(),
            base.counters().to_vec(),
            base.workflow_memory().clone(),
            GoalStateV2::new(
                Some(GoalRevisionNumberV2::FIRST),
                vec![goal_revision_one()],
                vec![too_early],
                Vec::new(),
            )
            .unwrap(),
            None,
        )
        .is_err()
    );
}

#[test]
fn retrying_a_goal_decision_retains_history_but_starts_without_criterion_state() {
    let state = assessment_state(6, true, vec![partial_criterion_state()]);
    let historical = state.goal_state().attempt_assessments()[0].clone();
    let outcome = state
        .retry_active_attempt_v2(
            Revision::new(6),
            &attempt_id(2),
            attempt_id(3),
            ReasonV2::new("Reassess the current goal decision.").unwrap(),
            UnixMillis::new(50),
        )
        .unwrap();
    let retried = outcome.state();

    assert_eq!(
        retried.goal_state().current_revision(),
        Some(GoalRevisionNumberV2::FIRST)
    );
    assert_eq!(retried.goal_state().attempt_assessments(), &[historical]);
    assert!(
        retried
            .goal_state()
            .attempt_assessments()
            .iter()
            .all(|assessment| assessment.attempt_id() != &attempt_id(3))
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
        retried.trace().active_attempt().unwrap().goal_revision(),
        Some(GoalRevisionNumberV2::FIRST)
    );
    assert!(
        retried.workflow_memory().attempts()[2].item_slots()[0]
            .value()
            .is_none()
    );
}

#[test]
fn revision_safety_follows_declared_rework_routes() {
    let snapshot = unsafe_rework_route_snapshot();
    let first = GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        None,
        GoalStatementV2::new("Original goal.").unwrap(),
        criteria(1),
        None,
        None,
        false,
        None,
        TraceSequenceV2::FIRST,
        UnixMillis::new(10),
    )
    .unwrap();
    let second = GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::new(2),
        Some(GoalRevisionNumberV2::FIRST),
        GoalStatementV2::new("Revised goal.").unwrap(),
        criteria(2),
        Some(GoalRevisionReasonV2::new("Requirements changed.").unwrap()),
        Some(node("target")),
        false,
        None,
        TraceSequenceV2::new(2),
        UnixMillis::new(20),
    )
    .unwrap();
    let attempts = vec![
        attempt(
            1,
            "target",
            1,
            1,
            AttemptLifecycle::Completed,
            AttemptValidityV2::Stale,
            Some(1),
        ),
        attempt(
            2,
            "target",
            2,
            2,
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
            Some(2),
        ),
    ];
    let metadata = vec![
        AttemptMetadataV2::new(
            attempt_id(1),
            UnixMillis::new(10),
            Some(UnixMillis::new(20)),
            None,
        )
        .unwrap(),
        AttemptMetadataV2::new(attempt_id(2), UnixMillis::new(20), None, None).unwrap(),
    ];
    let counters = snapshot
        .graph_nodes()
        .iter()
        .map(|placement| {
            if placement.graph_node_id() == &node("target") {
                GraphNodeCounterV2::new(node("target"), 2, 1)
            } else {
                GraphNodeCounterV2::new(placement.graph_node_id().clone(), 0, 0)
            }
        })
        .collect();
    let workflow = WorkflowMemoryStateV2::new(
        vec![
            AttemptWorkflowMemoryV2::new(attempt_id(1), Vec::new(), Vec::new(), Vec::new())
                .unwrap(),
            AttemptWorkflowMemoryV2::new(attempt_id(2), Vec::new(), Vec::new(), Vec::new())
                .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    assert!(
        GraphSessionStateV2::new_with_goal_state(
            Revision::new(2),
            "Unsafe revision target",
            snapshot,
            SessionTraceV2::from_parts(
                session_id(),
                SessionLifecycle::Running,
                Revision::new(2),
                attempts,
            )
            .unwrap(),
            counters,
            metadata,
            workflow,
            GoalStateV2::new(
                Some(GoalRevisionNumberV2::new(2)),
                vec![first, second],
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
}

#[test]
fn stale_goal_assessment_cannot_satisfy_terminal_completion() {
    let base = decided_state(9, true);
    let attempts = base
        .trace()
        .attempts()
        .iter()
        .map(|attempt| {
            SessionAttemptV2::new(
                attempt.attempt_id().clone(),
                attempt.graph_node_id().clone(),
                attempt.number(),
                attempt.trace(),
                attempt.lifecycle(),
                AttemptValidityV2::Stale,
                attempt.goal_revision(),
            )
            .unwrap()
        })
        .collect();
    assert!(
        SessionTraceV2::from_parts(
            session_id(),
            SessionLifecycle::Completed,
            Revision::new(9),
            attempts,
        )
        .is_err()
    );
}

#[test]
fn stale_goal_assessment_history_cannot_complete_or_skip_a_terminal_cursor() {
    let initial = GraphSessionStateV2::new_with_goal_state(
        Revision::new(1),
        "Stale assessment terminal gate",
        stale_assessment_terminal_snapshot(),
        SessionTraceV2::from_parts(
            session_id(),
            SessionLifecycle::Running,
            Revision::new(1),
            vec![attempt(
                1,
                "hub",
                1,
                1,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                Some(1),
            )],
        )
        .unwrap(),
        vec![
            GraphNodeCounterV2::new(node("hub"), 1, 0),
            GraphNodeCounterV2::new(node("assess"), 0, 0),
            GraphNodeCounterV2::new(node("finish"), 0, 0),
        ],
        vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
        WorkflowMemoryStateV2::new(
            vec![
                AttemptWorkflowMemoryV2::new(attempt_id(1), Vec::new(), Vec::new(), Vec::new())
                    .unwrap(),
            ],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![goal_revision_one()],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        UnixMillis::new(10),
        None,
        None,
        None,
    )
    .unwrap();
    let assessment = initial
        .decide_active_route_v2(
            Revision::new(1),
            &attempt_id(1),
            OptionId::new("assess").unwrap(),
            attempt_id(2),
            Some(ReasonV2::new("Assess the current goal.").unwrap()),
            None,
            UnixMillis::new(30),
        )
        .unwrap()
        .into_state();
    let assessment = assessment
        .assess_goal_criterion_v2(
            Revision::new(2),
            &attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            criterion_result_at("z-proof", CriterionStatusV2::Satisfied, Vec::new(), 31)
                .result()
                .clone(),
            None,
            UnixMillis::new(31),
        )
        .unwrap()
        .into_state();
    let assessment = assessment
        .assess_goal_criterion_v2(
            Revision::new(3),
            &attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            criterion_result_at("a-safety", CriterionStatusV2::Satisfied, Vec::new(), 32)
                .result()
                .clone(),
            None,
            UnixMillis::new(32),
        )
        .unwrap()
        .into_state();
    let terminal = assessment
        .decide_active_route_with_goal_revision_v2(
            Revision::new(4),
            &attempt_id(2),
            OptionId::new("achieved").unwrap(),
            attempt_id(3),
            Some(GoalRevisionNumberV2::FIRST),
            Some(ReasonV2::new("Every criterion is satisfied.").unwrap()),
            None,
            UnixMillis::new(40),
        )
        .unwrap()
        .into_state();
    let reworked = terminal
        .manual_rework_v2(
            Revision::new(5),
            Some(&attempt_id(3)),
            node("hub"),
            attempt_id(4),
            ReasonV2::new("Reconsider the route.").unwrap(),
            None,
            UnixMillis::new(50),
        )
        .unwrap()
        .into_state();
    let stale_only_terminal = reworked
        .decide_active_route_v2(
            Revision::new(6),
            &attempt_id(4),
            OptionId::new("finish").unwrap(),
            attempt_id(5),
            Some(ReasonV2::new("Proceed to the terminal action.").unwrap()),
            None,
            UnixMillis::new(60),
        )
        .unwrap()
        .into_state();

    assert_eq!(
        stale_only_terminal
            .trace()
            .active_attempt()
            .unwrap()
            .graph_node_id(),
        &node("finish")
    );
    assert_eq!(stale_only_terminal.goal_state().assessments().len(), 1);
    assert_eq!(
        stale_only_terminal.trace().attempts()[1].validity(),
        AttemptValidityV2::Stale
    );
    assert_eq!(stale_only_terminal.workflow_memory().reworks().len(), 1);
    assert!(
        stale_only_terminal
            .goal_state()
            .latest_fresh_assessment(stale_only_terminal.trace())
            .is_none()
    );

    let before = stale_only_terminal.clone();
    assert_eq!(
        stale_only_terminal.complete_active_action_v2(
            Revision::new(7),
            &attempt_id(5),
            None,
            UnixMillis::new(70),
        ),
        Err(GraphMutationErrorV2::FreshGoalAssessmentMissing {
            goal_revision: GoalRevisionNumberV2::FIRST,
        })
    );
    assert_eq!(stale_only_terminal, before);
    assert_eq!(
        stale_only_terminal.skip_active_action_v2(
            Revision::new(7),
            &attempt_id(5),
            None,
            None,
            UnixMillis::new(70),
        ),
        Err(GraphMutationErrorV2::FreshGoalAssessmentMissing {
            goal_revision: GoalRevisionNumberV2::FIRST,
        })
    );
    assert_eq!(stale_only_terminal, before);
}

#[test]
fn terminal_action_completion_requires_goal_and_fresh_assessment_without_changing_state() {
    let missing_goal = terminal_goal_state(None, GoalStateV2::empty());
    let missing_goal_before = missing_goal.clone();
    assert_eq!(
        missing_goal.complete_active_action_v2(
            Revision::new(1),
            &attempt_id(9),
            None,
            UnixMillis::new(30),
        ),
        Err(GraphMutationErrorV2::SessionGoalMissing)
    );
    assert_eq!(missing_goal, missing_goal_before);
    assert_eq!(
        missing_goal.skip_active_action_v2(
            Revision::new(1),
            &attempt_id(9),
            None,
            None,
            UnixMillis::new(30),
        ),
        Err(GraphMutationErrorV2::SessionGoalMissing)
    );

    let missing_assessment = terminal_goal_state(
        Some(1),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![goal_revision_one()],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
    );
    let missing_assessment_before = missing_assessment.clone();
    assert_eq!(
        missing_assessment.complete_active_action_v2(
            Revision::new(1),
            &attempt_id(9),
            None,
            UnixMillis::new(30),
        ),
        Err(GraphMutationErrorV2::FreshGoalAssessmentMissing {
            goal_revision: GoalRevisionNumberV2::FIRST,
        })
    );
    assert_eq!(missing_assessment, missing_assessment_before);
    assert_eq!(
        missing_assessment.skip_active_action_v2(
            Revision::new(1),
            &attempt_id(9),
            None,
            None,
            UnixMillis::new(30),
        ),
        Err(GraphMutationErrorV2::FreshGoalAssessmentMissing {
            goal_revision: GoalRevisionNumberV2::FIRST,
        })
    );
}

#[test]
fn terminal_skip_with_a_fresh_goal_assessment_completes_without_rewriting_goal_history() {
    let ready = decided_state(8, false);
    let goal_before = ready.goal_state().clone();
    let workflow_before = ready.workflow_memory().clone();
    let skipped = ready
        .skip_active_action_v2(
            Revision::new(8),
            &attempt_id(3),
            None,
            None,
            UnixMillis::new(70),
        )
        .unwrap()
        .into_state();

    assert_eq!(skipped.trace().lifecycle(), SessionLifecycle::Completed);
    assert!(skipped.trace().active_attempt().is_none());
    assert_eq!(
        skipped.trace().attempts()[2].lifecycle(),
        AttemptLifecycle::Skipped
    );
    assert_eq!(skipped.completed_at(), Some(UnixMillis::new(70)));
    assert_eq!(skipped.goal_state(), &goal_before);
    assert_eq!(skipped.workflow_memory(), &workflow_before);
}

#[test]
fn assessment_cannot_be_staled_without_a_causal_transition() {
    let ready = decided_state(8, false);
    let mut stale_attempts = ready.trace().attempts().to_vec();
    let assessment = &stale_attempts[1];
    stale_attempts[1] = SessionAttemptV2::new(
        assessment.attempt_id().clone(),
        assessment.graph_node_id().clone(),
        assessment.number(),
        assessment.trace(),
        assessment.lifecycle(),
        AttemptValidityV2::Stale,
        assessment.goal_revision(),
    )
    .unwrap();
    assert_eq!(
        try_state(
            8,
            SessionLifecycle::Running,
            stale_attempts,
            ready.attempt_metadata().to_vec(),
            ready.counters().to_vec(),
            ready.workflow_memory().clone(),
            ready.goal_state().clone(),
            None,
        ),
        Err(crate::ProcedureStateErrorV2::new(
            "Procedure v2 attempt validity is inconsistent with transition history"
        ))
    );
}

#[test]
fn bound003_goal_interpretation_and_deferred_memory_admission_remain_independent() {
    fn changed_snapshot(document: serde_json::Value) -> ProcedureSnapshotV2 {
        let original = snapshot();
        let canonical = canonicalize_json_v1(&document).unwrap();
        let digest =
            Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
                .unwrap();
        snapshot_from_canonical(
            original.snapshot_id().clone(),
            CanonicalProcedureJsonV1::new(canonical).unwrap(),
            digest,
            original.source().clone(),
            original.created_at(),
        )
        .unwrap()
    }
    let original = snapshot();
    let mut document: serde_json::Value =
        serde_json::from_str(original.canonical_json().as_str()).unwrap();
    document["node_definitions"]["clarify-def"]["items"][0]["min_length"] = json!(2);
    document["node_definitions"]["clarify-def"]["items"][0]["max_length"] = json!(1);
    let memory_invalid = changed_snapshot(document);
    assert!(memory_invalid.validate_memory_declarations_v2().is_err());
    assert!(super::GoalSnapshotModelV2::parse(&memory_invalid).is_ok());
    assert!(
        memory_invalid
            .graph_node(&node("clarify"))
            .unwrap()
            .action_outcome()
            .is_ok()
    );

    let mut document: serde_json::Value =
        serde_json::from_str(original.canonical_json().as_str()).unwrap();
    document["node_definitions"]["assess-def"]["assessment"]
        .as_object_mut()
        .unwrap()
        .remove("outcomes");
    let goal_invalid = changed_snapshot(document);
    assert!(goal_invalid.validate_memory_declarations_v2().is_ok());
    assert!(super::GoalSnapshotModelV2::parse(&goal_invalid).is_err());
}
