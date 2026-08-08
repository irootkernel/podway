//! Procedure v2 goal revision and assessment persistence evidence.

use std::path::{Path, PathBuf};

use podway_core::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2,
    CanonicalProcedureJsonV1, CriterionAssessmentReasonV2, CriterionAssessmentResultV2,
    CriterionCitationV2, CriterionId, CriterionStatusV2, DecisionRecordInputV2, DecisionRecordV2,
    DomainCommand, EvidenceReferenceSnapshotV2, GoalAssessmentRecordV2, GoalCriterionV2,
    GoalDefinitionV2, GoalOutcome, GoalRevisionNumberV2, GoalRevisionReasonV2,
    GoalRevisionRecordV2, GoalStatementV2, GraphNodeId, ItemId, ItemTypeV1, JobId,
    NodeDefinitionId, OptionId, ProcedureSnapshotId, ProcedureSourceLabelV1, ReasonV2,
    RecordedItemValueV2, ResolvedEvidenceReferenceV2, ResolvedEvidenceSetV2, Revision,
    SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2,
    TransitionEffectV2, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, AttemptCriterionAssessmentStateV2, AttemptMetadataV2,
    AttemptWorkflowMemoryV2, CriterionAssessmentStateV2, DurableWorktreeIdentityV1,
    EvidenceResolutionStateV2, GoalStateV2, GraphNodeCounterV2, GraphSessionStateV2,
    IdempotencyKeyV1, ItemSlotStateV2, ProcedureSnapshotV2, RevisionAttemptItemPreconditionsV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1, StoreErrorV1, StoreFailpointV1,
    StoreGraphStateContractV2, StoreIntegrityCheckV1, StoreUnavailableReasonV1,
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
        WorkspaceId::new("00000000-0000-4000-8000-000000000401").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-v2-goal-state")).unwrap()
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

const V2_TABLES: [&str; 16] = [
    "v2_attempts",
    "v2_blockers",
    "v2_criterion_assessment_results",
    "v2_criterion_citations",
    "v2_decision_records",
    "v2_goal_assessments",
    "v2_goal_criteria",
    "v2_goal_revisions",
    "v2_graph_node_counters",
    "v2_graph_nodes",
    "v2_item_slots",
    "v2_procedure_snapshots",
    "v2_resolved_evidence_references",
    "v2_rework_records",
    "v2_task_sessions",
    "v2_workspace_state",
];

fn v2_table_counts(path: &Path) -> Vec<(String, i64)> {
    let connection = Connection::open(path).unwrap();
    V2_TABLES
        .iter()
        .map(|table| {
            let count = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            ((*table).to_owned(), count)
        })
        .collect()
}

fn base_store_identity(
    path: &Path,
) -> (
    (String, String, String, String, i64, i64, i64),
    Vec<(i64, String, String, i64)>,
) {
    let connection = Connection::open(path).unwrap();
    let workspace = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
                    last_validated_root, next_workspace_sequence, created_at_ms, updated_at_ms \
             FROM workspace_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();
    let migrations = connection
        .prepare(
            "SELECT version, name, checksum, applied_at_ms \
             FROM schema_migrations ORDER BY version",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    (workspace, migrations)
}

fn seed_running_workspace_job(store: &SqliteStoreV1, number: u64) {
    let job_id = JobId::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap();
    let request = AdmitRequestV1::new(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new(format!("v2-recovery-{number}")).unwrap(),
        job_id.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('d'),
        UnixMillis::new(20),
    );
    assert!(matches!(
        store.admit(&identity(), request),
        Ok(AdmitOutcomeV1::New(_))
    ));
    let claimed = store
        .claim_next(
            &identity(),
            WorkerIdV1::new(format!("v2-recovery-worker-{number}")).unwrap(),
            UnixMillis::new(21),
        )
        .unwrap()
        .unwrap();
    assert_eq!(claimed.job().job_id(), &job_id);
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
                {"id":"finish","use":"finish-def","terminal":true}
            ]
        },
        "manual_rework":{"allowed_targets":["clarify","assess"]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    ProcedureSnapshotV2::new(
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
    ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000411").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("no-goal.yaml").unwrap(),
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
    ProcedureSnapshotV2::new(
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

fn reverse_partial_criterion_state() -> AttemptCriterionAssessmentStateV2 {
    AttemptCriterionAssessmentStateV2::new(
        attempt_id(2),
        GoalRevisionNumberV2::FIRST,
        vec![criterion_result("a-safety", Vec::new())],
    )
    .unwrap()
}

fn reverse_complete_criterion_state() -> AttemptCriterionAssessmentStateV2 {
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
) -> Result<GraphSessionStateV2, podway_store::StoreValueErrorV1> {
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

fn initial_state() -> GraphSessionStateV2 {
    state(
        1,
        SessionLifecycle::Running,
        vec![attempt(
            1,
            "clarify",
            1,
            1,
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
            None,
        )],
        vec![AttemptMetadataV2::new(attempt_id(1), UnixMillis::new(10), None, None).unwrap()],
        counters(1, 0, 0, 0),
        WorkflowMemoryStateV2::new(vec![clarify_memory(1, false)], Vec::new(), Vec::new()).unwrap(),
        GoalStateV2::empty(),
        None,
    )
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

fn persist_through_assessment(store: &SqliteStoreV1) {
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            defined_state(2, false),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(2),
            Revision::new(2),
            defined_state(3, true),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(3),
            Revision::new(3),
            assessment_state(4, false, Vec::new()),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            assessment_state(5, true, Vec::new()),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(5),
            Revision::new(5),
            assessment_state(6, true, vec![partial_criterion_state()]),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(6),
            Revision::new(6),
            assessment_state(7, true, vec![complete_criterion_state()]),
        )
        .unwrap();
}

fn persist_through_completed(store: &SqliteStoreV1) {
    persist_through_assessment(store);
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(7),
            Revision::new(7),
            decided_state(8, false),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(8),
            Revision::new(8),
            decided_state(9, true),
        )
        .unwrap();
}

#[test]
fn admission_goal_and_late_definition_round_trip_in_criterion_order() {
    let admission = TempDir::new().unwrap();
    let admission_store = open(&admission, SqliteStoreOptionsV1::new(8).unwrap());
    let admission_state = defined_state(1, false);
    admission_store
        .create_graph_session_v2(&identity(), admission_state.clone())
        .unwrap();
    drop(admission_store);
    let loaded = open(&admission, SqliteStoreOptionsV1::new(8).unwrap())
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    assert_eq!(loaded, admission_state);
    assert_eq!(
        loaded.goal_state().revisions()[0]
            .criteria()
            .criteria()
            .iter()
            .map(|criterion| criterion.id().as_str())
            .collect::<Vec<_>>(),
        vec!["z-proof", "a-safety"]
    );

    let late = TempDir::new().unwrap();
    let late_store = open(&late, SqliteStoreOptionsV1::new(8).unwrap());
    late_store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    late_store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            defined_state(2, false),
        )
        .unwrap();
    let loaded = late_store
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    assert_eq!(loaded.workspace_revision(), Revision::new(2));
    assert_eq!(loaded.trace().revision(), Revision::new(2));
    assert_eq!(loaded.trace().attempts().len(), 1);
    assert_eq!(
        loaded.trace().active_attempt().unwrap().goal_revision(),
        Some(GoalRevisionNumberV2::FIRST)
    );
    assert_eq!(loaded.counters(), counters(1, 0, 0, 0));
}

#[test]
fn criterion_citations_final_assessment_and_reactivation_survive_reopen() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_through_completed(&store);
    let completed = store.read_graph_session_v2(&identity()).unwrap().unwrap();
    assert_eq!(completed, decided_state(9, true));
    assert_eq!(
        completed
            .goal_state()
            .latest_fresh_assessment(completed.trace()),
        completed.goal_state().assessments().first()
    );
    let result = &completed.goal_state().attempt_assessments()[0].results()[0];
    assert_eq!(result.result().citations().len(), 2);
    assert_eq!(
        result.result().citations()[0],
        CriterionCitationV2::Evidence(node("clarify"))
    );
    assert_eq!(
        result.result().citations()[1],
        CriterionCitationV2::Item(item("assessment-note"))
    );
    assert_eq!(completed.counters(), counters(1, 1, 1, 0));

    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(9),
            Revision::new(9),
            revised_state(),
        )
        .unwrap();
    drop(store);
    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    let revised = reopened
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    assert_eq!(revised, revised_state());
    assert_eq!(revised.trace().attempts().len(), 4);
    assert_eq!(revised.counters(), counters(2, 1, 1, 1));
    assert!(
        revised
            .goal_state()
            .latest_fresh_assessment(revised.trace())
            .is_none()
    );
    assert_eq!(revised.goal_state().revisions().len(), 2);
    assert_eq!(revised.goal_state().assessments().len(), 1);
    assert_eq!(revised.goal_state().attempt_assessments().len(), 1);
}

#[test]
fn goal_history_is_append_only_and_rewrite_failure_preserves_prior_state() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            defined_state(2, false),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(2),
            Revision::new(2),
            defined_state(3, true),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(3),
            Revision::new(3),
            assessment_state(4, false, Vec::new()),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            assessment_state(5, true, Vec::new()),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(5),
            Revision::new(5),
            assessment_state(6, true, vec![partial_criterion_state()]),
        )
        .unwrap();

    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(6),
            Revision::new(6),
            assessment_state(7, true, vec![complete_criterion_state()]),
        )
        .unwrap();

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
    assert!(matches!(
        store.replace_graph_session_v2(&identity(), Revision::new(7), Revision::new(7), rewritten),
        Err(StoreErrorV1::InvalidStateV1(_))
    ));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(assessment_state(7, true, vec![complete_criterion_state()]))
    );
}

#[test]
fn goal_revision_failpoint_rolls_back_and_corruption_fails_reopen() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_through_completed(&store);
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
            Revision::new(9),
            Revision::new(9),
            revised_state(),
        )
        .unwrap_err();
    assert_eq!(
        error,
        StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        }
    );
    drop(failing);
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(decided_state(9, true))
    );
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(9),
            Revision::new(9),
            revised_state(),
        )
        .unwrap();
    drop(store);

    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE v2_task_sessions SET current_goal_revision = 1 WHERE singleton = 1",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(
        SqliteStoreV1::open(
            database_path(&temporary),
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(100),
        )
        .is_err()
    );
}

#[test]
fn startup_recovery_preserves_rich_v2_state_across_recovery_commit_boundaries() {
    for (number, failpoint, expected_requeued_after_retry) in [
        (501, StoreFailpointV1::RecoveryBeforeCommit, 1),
        (502, StoreFailpointV1::RecoveryAfterCommitBeforeReturn, 0),
    ] {
        let temporary = TempDir::new().unwrap();
        let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
        persist_through_completed(&store);
        seed_running_workspace_job(&store, number);
        let expected = decided_state(9, true);
        assert_eq!(
            store.read_graph_session_v2(&identity()).unwrap(),
            Some(expected.clone())
        );
        drop(store);

        let failed = SqliteStoreV1::open(
            database_path(&temporary),
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8)
                .unwrap()
                .with_failpoint(Some(failpoint)),
            UnixMillis::new(22),
        );
        assert!(matches!(
            failed,
            Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Recovery,
            })
        ));

        let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
        assert_eq!(
            reopened.startup_recovery_report().requeued_job_count(),
            expected_requeued_after_retry
        );
        assert_eq!(
            reopened.read_graph_session_v2(&identity()).unwrap(),
            Some(expected.clone())
        );
        let view = reopened.read_workspace_view(&identity()).unwrap();
        assert_eq!(view.queued_job_count(), 1);
        assert!(view.running_job_id().is_none());
        drop(reopened);

        let reopened_again = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
        assert_eq!(
            reopened_again
                .startup_recovery_report()
                .requeued_job_count(),
            0
        );
        assert_eq!(
            reopened_again.read_graph_session_v2(&identity()).unwrap(),
            Some(expected)
        );
    }
}

#[test]
fn v2_session_reset_is_revision_checked_atomic_and_complete() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_through_completed(&store);
    let expected = decided_state(9, true);
    let path = database_path(&temporary);
    let counts_before = v2_table_counts(&path);
    let base_before = base_store_identity(&path);

    assert_eq!(
        store.clear_graph_session_v2(&identity(), Revision::new(8), Revision::new(9)),
        Err(StoreErrorV1::PreconditionConflictV1 {
            expected: Some(Revision::new(8)),
            actual: Some(Revision::new(9)),
        })
    );
    assert_eq!(v2_table_counts(&path), counts_before);
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(expected.clone())
    );

    assert_eq!(
        store.clear_graph_session_v2(&identity(), Revision::new(9), Revision::new(8)),
        Err(StoreErrorV1::PreconditionConflictV1 {
            expected: Some(Revision::new(8)),
            actual: Some(Revision::new(9)),
        })
    );
    assert_eq!(v2_table_counts(&path), counts_before);
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(expected.clone())
    );
    drop(store);

    let failing = open(
        &temporary,
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(Some(StoreFailpointV1::V2GraphStateBeforeCommit)),
    );
    assert_eq!(
        failing.clear_graph_session_v2(&identity(), Revision::new(9), Revision::new(9)),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery,
        })
    );
    drop(failing);
    assert_eq!(v2_table_counts(&path), counts_before);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(expected)
    );
    reopened
        .clear_graph_session_v2(&identity(), Revision::new(9), Revision::new(9))
        .unwrap();
    assert_eq!(reopened.read_graph_session_v2(&identity()).unwrap(), None);
    assert_eq!(
        v2_table_counts(&path),
        V2_TABLES
            .iter()
            .map(|table| ((*table).to_owned(), 0))
            .collect::<Vec<_>>()
    );
    assert_eq!(base_store_identity(&path), base_before);
    drop(reopened);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    assert_eq!(reopened.read_graph_session_v2(&identity()).unwrap(), None);
    reopened
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(initial_state())
    );
}

#[test]
fn populated_newer_schema_and_downgrade_stamp_fail_without_changing_v2_state() {
    let newer = TempDir::new().unwrap();
    let store = open(&newer, SqliteStoreOptionsV1::new(8).unwrap());
    persist_through_completed(&store);
    drop(store);
    let newer_path = database_path(&newer);
    let newer_counts = v2_table_counts(&newer_path);
    let newer_base = base_store_identity(&newer_path);
    let connection = Connection::open(&newer_path).unwrap();
    connection.pragma_update(None, "user_version", 4).unwrap();
    drop(connection);

    let newer_error = match SqliteStoreV1::open(
        newer_path.clone(),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(30),
    ) {
        Ok(_) => panic!("a populated newer schema must not open"),
        Err(error) => error,
    };
    assert_eq!(
        newer_error,
        StoreErrorV1::NewerStateV1 {
            found_schema_version: 4,
            supported_schema_version: 3,
        }
    );
    assert_eq!(v2_table_counts(&newer_path), newer_counts);
    assert_eq!(base_store_identity(&newer_path), newer_base);
    let version: i64 = Connection::open(&newer_path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);

    let downgrade = TempDir::new().unwrap();
    let store = open(&downgrade, SqliteStoreOptionsV1::new(8).unwrap());
    persist_through_completed(&store);
    drop(store);
    let downgrade_path = database_path(&downgrade);
    let downgrade_counts = v2_table_counts(&downgrade_path);
    let downgrade_base = base_store_identity(&downgrade_path);
    let connection = Connection::open(&downgrade_path).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);

    let downgrade_error = match SqliteStoreV1::open(
        downgrade_path.clone(),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(30),
    ) {
        Ok(_) => panic!("a v3 database stamped as v2 must not be downgraded or reopened"),
        Err(error) => error,
    };
    assert_eq!(
        downgrade_error,
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::RequiredSchemaObjects,
        }
    );
    assert_eq!(v2_table_counts(&downgrade_path), downgrade_counts);
    assert_eq!(base_store_identity(&downgrade_path), downgrade_base);
    let version: i64 = Connection::open(&downgrade_path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
}

#[test]
fn reverse_recording_order_reopens_in_goal_definition_order() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(&identity(), initial_state())
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(1),
            Revision::new(1),
            defined_state(2, false),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(2),
            Revision::new(2),
            defined_state(3, true),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(3),
            Revision::new(3),
            assessment_state(4, false, Vec::new()),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(4),
            Revision::new(4),
            assessment_state(5, true, Vec::new()),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(5),
            Revision::new(5),
            assessment_state(6, true, vec![reverse_partial_criterion_state()]),
        )
        .unwrap();
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(6),
            Revision::new(6),
            assessment_state(7, true, vec![reverse_complete_criterion_state()]),
        )
        .unwrap();
    drop(store);

    let reopened = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    let loaded = reopened
        .read_graph_session_v2(&identity())
        .unwrap()
        .unwrap();
    let ids = loaded.goal_state().attempt_assessments()[0]
        .results()
        .iter()
        .map(|state| state.result().criterion_id().as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["z-proof", "a-safety"]);
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
fn decision_assessment_failpoint_is_atomic() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    persist_through_assessment(&store);
    drop(store);
    let failing = open(
        &temporary,
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(Some(StoreFailpointV1::V2GraphStateBeforeCommit)),
    );
    assert!(matches!(
        failing.replace_graph_session_v2(
            &identity(),
            Revision::new(7),
            Revision::new(7),
            decided_state(8, false),
        ),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));
    drop(failing);
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(assessment_state(7, true, vec![complete_criterion_state()]))
    );
    store
        .replace_graph_session_v2(
            &identity(),
            Revision::new(7),
            Revision::new(7),
            decided_state(8, false),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(decided_state(8, false))
    );
}

#[test]
fn fk_off_orphan_goal_rows_fail_reopen() {
    for orphan in ["criterion", "citation"] {
        let temporary = TempDir::new().unwrap();
        let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
        store
            .create_graph_session_v2(&identity(), defined_state(1, false))
            .unwrap();
        drop(store);
        let connection = Connection::open(database_path(&temporary)).unwrap();
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
        assert!(
            SqliteStoreV1::open(
                database_path(&temporary),
                &root(),
                identity(),
                SqliteStoreOptionsV1::new(8).unwrap(),
                UnixMillis::new(100),
            )
            .is_err(),
            "{orphan} orphan must fail startup integrity"
        );
    }
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
        try_state(
            9,
            SessionLifecycle::Completed,
            attempts,
            base.attempt_metadata().to_vec(),
            base.counters().to_vec(),
            base.workflow_memory().clone(),
            base.goal_state().clone(),
            Some(UnixMillis::new(70)),
        )
        .is_err()
    );
}

#[test]
fn assessment_digest_tamper_fails_reopen() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, SqliteStoreOptionsV1::new(8).unwrap());
    store
        .create_graph_session_v2(&identity(), decided_state(9, true))
        .unwrap();
    drop(store);
    let connection = Connection::open(database_path(&temporary)).unwrap();
    connection
        .execute(
            "UPDATE v2_goal_assessments SET record_digest = ?1 WHERE decision_attempt_id = ?2",
            [digest('f').as_str(), attempt_id(2).as_str()],
        )
        .unwrap();
    drop(connection);
    assert!(
        SqliteStoreV1::open(
            database_path(&temporary),
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(100),
        )
        .is_err()
    );
}
