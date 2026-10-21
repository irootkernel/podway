//! Current Procedure v2 semantics and direct persistence regression evidence.
use super::int_v2dfc008_persistence::{fixture, reject_corruption, seed};
use rusqlite::Connection;

use podway_core::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2,
    CanonicalProcedureJsonV1, CriterionAssessmentReasonV2, CriterionAssessmentResultV2,
    CriterionCitationV2, CriterionId, CriterionStatusV2, DecisionRecordInputV2, DecisionRecordV2,
    EvidenceReferenceSnapshotV2, GoalAssessmentRecordV2, GoalCriterionV2, GoalDefinitionV2,
    GoalOutcome, GoalRevisionNumberV2, GoalRevisionReasonV2, GoalRevisionRecordV2, GoalStatementV2,
    GraphNodeId, ItemId, ItemTypeV1, NodeDefinitionId, OptionId, ProcedureSnapshotId,
    ProcedureSourceLabelV1, ReasonV2, RecordedItemValueV2, ResolvedEvidenceReferenceV2,
    ResolvedEvidenceSetV2, Revision, SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2,
    Sha256Digest, TraceSequenceV2, TransitionEffectV2, UnixMillis, WorkspaceId,
    canonicalize_json_v1,
};
use podway_store::{
    AttemptCriterionAssessmentStateV2, AttemptMetadataV2, AttemptWorkflowMemoryV2,
    CriterionAssessmentStateV2, DurableWorktreeIdentityV1, EvidenceResolutionStateV2, GoalStateV2,
    GraphNodeCounterV2, GraphSessionStateV2, ItemSlotStateV2, ProcedureSnapshotV2,
    WorkflowMemoryStateV2,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000401").unwrap(),
        digest('b'),
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
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000410").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("goal.yaml").unwrap(),
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
        goal_revision.map(podway_core::GoalRevisionNumberV2::new),
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
        goal_revision: Some(podway_core::GoalRevisionNumberV2::FIRST),
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

fn goal_revision_two() -> GoalRevisionRecordV2 {
    GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::new(2),
        Some(GoalRevisionNumberV2::FIRST),
        GoalStatementV2::new("Cancellation is deterministic across restart.").unwrap(),
        criteria(2),
        Some(GoalRevisionReasonV2::new("Restart is now explicit.").unwrap()),
        Some(node("clarify")),
        true,
        Some(ActorAttributionV2::new("planner").unwrap()),
        TraceSequenceV2::new(4),
        UnixMillis::new(80),
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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
fn try_state(
    revision: u64,
    lifecycle: SessionLifecycle,
    attempts: Vec<SessionAttemptV2>,
    metadata: Vec<AttemptMetadataV2>,
    counters: Vec<GraphNodeCounterV2>,
    workflow: WorkflowMemoryStateV2,
    goal: GoalStateV2,
    completed_at: Option<UnixMillis>,
) -> Result<GraphSessionStateV2, podway_core::ProcedureStateErrorV2> {
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

fn defined_state(revision: u64, recorded: bool) -> GraphSessionStateV2 {
    state(
        revision,
        SessionLifecycle::Running,
        vec![attempt(
            1,
            "clarify",
            1,
            1,
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
            Some(1),
        )],
        vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
        counters(1, 0, 0, 0),
        WorkflowMemoryStateV2::new(vec![clarify_memory(1, recorded)], Vec::new(), Vec::new())
            .unwrap(),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![goal_revision_one()],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        None,
    )
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

fn revised_state() -> GraphSessionStateV2 {
    let clarify = clarify_memory(1, true);
    let assess = assess_memory(&clarify, true);
    let finish =
        AttemptWorkflowMemoryV2::new(attempt_id(3), Vec::new(), Vec::new(), Vec::new()).unwrap();
    let fresh = clarify_memory(4, false);
    state(
        10,
        SessionLifecycle::Running,
        vec![
            attempt(
                1,
                "clarify",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
                Some(1),
            ),
            attempt(
                2,
                "assess",
                1,
                2,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
                Some(1),
            ),
            attempt(
                3,
                "finish",
                1,
                3,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Stale,
                Some(1),
            ),
            attempt(
                4,
                "clarify",
                2,
                4,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                Some(2),
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
                Some(UnixMillis::new(70)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(attempt_id(4), UnixMillis::new(80), None, None).unwrap(),
        ],
        counters(2, 1, 1, 1),
        WorkflowMemoryStateV2::new(
            vec![clarify.clone(), assess, finish, fresh],
            vec![assessment_decision(&clarify)],
            Vec::new(),
        )
        .unwrap(),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::new(2)),
            vec![goal_revision_one(), goal_revision_two()],
            vec![complete_criterion_state()],
            vec![goal_assessment(&clarify)],
        )
        .unwrap(),
        None,
    )
}

#[test]
fn v2drw004_completed_goal_reactivation_bit_flip_fails_reopen() {
    let (_temporary, store) = fixture(&identity());
    seed(&store, &(revised_state()));

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_goal_revisions SET reactivated = 0 WHERE goal_revision = 2",
            [],
        )
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn fk_off_orphan_goal_rows_fail_reopen() {
    for orphan in ["criterion", "citation"] {
        let (_temporary, store) = fixture(&identity());
        seed(&store, &(defined_state(1, false)));

        let connection = Connection::open(store.layout().database()).unwrap();
        connection.execute("PRAGMA foreign_keys = OFF", []).unwrap();
        if orphan == "criterion" {
            connection.execute(
                "INSERT INTO v2_goal_criteria (session_id, goal_revision, criterion_id, criterion_ordinal, statement) VALUES (?1, 99, 'orphan', 0, 'Orphan criterion.')",
                [session_id().as_str()],
            ).unwrap();
        } else {
            connection.execute(
                "INSERT INTO v2_criterion_citations (attempt_id, criterion_id, citation_ordinal, citation_kind, source_graph_node_id, item_id) VALUES (?1, 'orphan', 0, 'item', NULL, 'proof')",
                [attempt_id(99).as_str()],
            ).unwrap();
        }
        drop(connection);
        reject_corruption(&store, false);
    }
}

#[test]
fn duplicate_persisted_criterion_citation_values_fail_reopen() {
    let (_temporary, store) = fixture(&identity());
    seed(
        &store,
        &(assessment_state(6, true, vec![partial_criterion_state()])),
    );

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_criterion_citations SET citation_kind = 'evidence', \
             source_graph_node_id = 'clarify', item_id = NULL \
             WHERE attempt_id = ?1 AND criterion_id = 'z-proof' AND citation_ordinal = 1",
            [attempt_id(2).as_str()],
        )
        .unwrap();
    drop(connection);

    reject_corruption(&store, true);
}

#[test]
fn assessment_digest_tamper_fails_reopen() {
    let (_temporary, store) = fixture(&identity());
    seed(&store, &(decided_state(9, true)));

    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_goal_assessments SET record_digest = ?1 WHERE decision_attempt_id = ?2",
            [digest('f').as_str(), attempt_id(2).as_str()],
        )
        .unwrap();
    drop(connection);
    reject_corruption(&store, true);
}

#[test]
fn goal_revision_corruption_fails_cold_read_without_rewriting_history() {
    let (_temporary, store) = fixture(&identity());
    seed(&store, &revised_state());
    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_task_sessions SET current_goal_revision=1 WHERE activity='current'",
            [],
        )
        .unwrap();
    drop(connection);
    reject_corruption(&store, true);
}

#[test]
fn archived_goal_history_does_not_corrupt_a_goal_enabled_successor_after_cold_reopen() {
    use super::int_v2dfc005_direct_store::{deadline, store as cold_store};
    use super::int_v2dfc008_persistence::archive;
    let (_temporary, direct) = fixture(&identity());
    let completed = decided_state(9, true);
    seed(&direct, &completed);
    archive(&direct, &completed, UnixMillis::new(200));
    let successor = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Goal-enabled successor",
        podway_store::admit_procedure_snapshot_v2(
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000498").unwrap(),
            completed.snapshot().canonical_json().clone(),
            completed.snapshot().digest().clone(),
            ProcedureSourceLabelV1::file("goal-successor.yaml").unwrap(),
            UnixMillis::new(300),
        )
        .unwrap(),
        SessionId::new("00000000-0000-4000-8000-000000000499").unwrap(),
        UnixMillis::new(300),
    )
    .unwrap();
    seed(&direct, &successor);
    let cold = cold_store(direct.layout().root().to_path_buf().as_path());
    assert_eq!(
        cold.read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap(),
        Some(successor)
    );
    assert_eq!(
        cold.read(deadline(), |snapshot| snapshot
            .archived_graph(completed.trace().session_id()))
            .unwrap(),
        Some(completed)
    );
    cold.verify_integrity(deadline()).unwrap();
}

#[test]
fn goal_history_is_append_only_and_rewrite_failure_preserves_prior_state() {
    use super::int_v2dfc005_direct_store::{deadline, request, success};
    let (_temporary, direct) = fixture(&identity());
    let prior = assessment_state(7, true, vec![complete_criterion_state()]);
    seed(&direct, &prior);
    let base = assessment_state(8, true, vec![complete_criterion_state()]);
    let rewritten_revision = GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        None,
        GoalStatementV2::new("A rewritten historical goal.").unwrap(),
        criteria(1),
        None,
        None,
        false,
        Some(ActorAttributionV2::new("planner").unwrap()),
        TraceSequenceV2::FIRST,
        UnixMillis::new(20),
    )
    .unwrap();
    let rewritten = state(
        8,
        SessionLifecycle::Running,
        base.trace().attempts().to_vec(),
        base.attempt_metadata().to_vec(),
        base.counters().to_vec(),
        base.workflow_memory().clone(),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![rewritten_revision],
            vec![complete_criterion_state()],
            Vec::new(),
        )
        .unwrap(),
        None,
    );
    let change = request("rewrite-goal-history");
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
                &rewritten,
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
    direct.verify_integrity(deadline()).unwrap();
}
