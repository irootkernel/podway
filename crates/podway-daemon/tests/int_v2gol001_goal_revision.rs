//! Production vertical coverage for V2GOL-001 goal definition and revision.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, AdvanceTerminalV2, AttemptId, CriterionAssessmentReasonV2,
    CriterionAssessmentResultV2, CriterionId, CriterionStatusV2, DecisionRecordInputV2,
    DecisionRecordV2, GoalAssessmentRecordV2, GoalCriterionV2, GoalDefinitionV2, GoalOutcome,
    GoalRevisionNumberV2, GoalRevisionReasonV2, GoalStatementV2, GraphNodeId, ItemId, ItemTypeV1,
    OptionId, ProcedureSnapshotId, ReasonV2, ResolvedEvidenceSetV2, Revision, SessionId,
    TransitionEffectV2, UnixMillis,
};
use podway_daemon::execution::{
    graph_session_state_from_procedure_v2_snapshot, workspace_procedure_snapshot_from_bytes_v2,
};
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1,
};
use serde_json::{Map, Value, json};

use podway_store::{
    ActiveItemMutationV2, AttemptCriterionAssessmentStateV2, AttemptMetadataV2,
    AttemptWorkflowMemoryV2, CriterionAssessmentStateV2, GoalStateV2, GraphMutationErrorV2,
    GraphNodeCounterV2, GraphSessionStateV2, ItemSlotStateV2, WorkflowMemoryStateV2,
};

const GOAL_PROCEDURE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/equivalent-procedure.yaml");

const COMPLETED_GOAL_PROCEDURE: &str = GOAL_PROCEDURE;

fn typed_request(
    number: u64,
    command: &str,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    mut payload: Map<String, Value>,
    key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
        client: ClientInfoV1::new("v2gol001-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    match command {
        "session.start" | "session.start_replace" => {
            assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Start(_)));
        }
        _ => assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_))),
    }
    (envelope, daemon)
}

fn goal_fields(statement: &str, criterion: &str) -> Map<String, Value> {
    json!({
        "goal": statement,
        "criteria": [{"criterion_id": "verified", "statement": criterion}],
        "actor": "V2GOL-001 test"
    })
    .as_object()
    .unwrap()
    .clone()
}

fn session_identity_preconditions(status: &Map<String, Value>) -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SessionId::new(status["session"]["id"].as_str().unwrap()).unwrap()),
        Some(Revision::new(
            status["session"]["revision"].as_u64().unwrap(),
        )),
        None,
        None,
        None,
        None,
    )
    .unwrap()
}

fn goal_revision_preconditions(status: &Map<String, Value>, goal_revision: u64) -> PreconditionsV1 {
    runtime::session_preconditions(status)
        .with_goal_revision(GoalRevisionNumberV2::new(goal_revision))
        .unwrap()
}

fn assert_public_error(response: ResponseEnvelopeV2, code: &str) -> Value {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{code} must be returned as a public error")
    };
    assert_eq!(error.code().as_str(), code);
    serde_json::to_value(error).unwrap()
}

struct Fixture {
    workspace: support_phase4_workspace::GitWorktreeFixtureV1,
    selector: podway_protocol::WorktreeSelectorWireV1,
    digest: podway_core::Sha256Digest,
    manager: Arc<podway_daemon::runtime_workspace::WorkspaceRuntimeManagerV1>,
}

fn fixture() -> Fixture {
    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(workspace.main().join("goal.yaml"), GOAL_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(GOAL_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = runtime::selector(workspace.main());
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    Fixture {
        workspace,
        selector,
        digest,
        manager,
    }
}

fn initialize(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    number: u64,
) {
    let request = runtime::request(
        number,
        "workspace.init",
        selector,
        Map::new(),
        &format!("v2gol001-init-{number}"),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &request),
        ResponseEnvelopeV2::OutputV2(_)
    ));
}

#[test]
fn v2gol001_initial_goal_is_durable_for_begin_and_replace() {
    let fixture = fixture();
    let production = runtime::dispatcher(Arc::clone(&fixture.manager), "v2gol001-start");
    initialize(&production, &fixture.selector, 101_001);

    let payload = json!({
        "procedure": "goal.yaml",
        "expected_procedure_digest": fixture.digest,
        "task_title": "Initial goal start"
    })
    .as_object()
    .unwrap()
    .clone();
    let start = typed_request(
        101_002,
        "session.start",
        &fixture.selector,
        payload,
        "v2gol001-initial-start",
        PreconditionsV1::default(),
    );
    let started_once = runtime::dispatch(&production, &start);
    let started = runtime::v2_result(started_once.clone(), "session.start");
    assert_eq!(started["goal_defined"], false);
    assert_eq!(started["revision"], 0);
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch(&production, &start)),
        runtime::without_request_id(&started_once),
        "prepared start must replay one durable outcome"
    );

    let session_id = started["session_id"].as_str().unwrap();
    let begin = typed_request(
        101_003,
        "session.begin",
        &fixture.selector,
        goal_fields("Ship the first goal.", "The first goal is persisted."),
        "v2gol001-initial-begin",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let begun_once = runtime::dispatch(&production, &begin);
    let begun = runtime::v2_result(begun_once.clone(), "session.begin");
    assert_eq!(begun["goal_tracking"], true);
    assert_eq!(begun["goal_defined"], true);
    assert_eq!(begun["revision"], 1);
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch(&production, &begin)),
        runtime::without_request_id(&begun_once),
        "goal-bearing begin must replay one durable outcome"
    );

    let status = runtime::status(&production, &fixture.selector, 101_010, session_id);
    let replacement_payload = json!({
        "procedure": "goal.yaml",
        "expected_procedure_digest": fixture.digest,
        "task_title": "Replacement initial goal",
        "confirmed": true,
        "progress_summary": "The isolated test intentionally replaces its running session."
    })
    .as_object()
    .unwrap()
    .clone();
    let replace = typed_request(
        101_011,
        "session.start_replace",
        &fixture.selector,
        replacement_payload,
        "v2gol001-initial-replace",
        session_identity_preconditions(&status),
    );
    let replaced = runtime::v2_result(
        runtime::dispatch(&production, &replace),
        "session.start_replace",
    );
    assert_eq!(replaced["goal_defined"], false);
    assert_eq!(replaced["revision"], 0);
    assert_ne!(replaced["session_id"], started["session_id"]);

    let replacement_session_id = replaced["session_id"].as_str().unwrap();
    let replacement_begin = typed_request(
        101_012,
        "session.begin",
        &fixture.selector,
        goal_fields(
            "Ship the replacement goal.",
            "The replacement owns an immutable first revision.",
        ),
        "v2gol001-replacement-begin",
        PreconditionsV1::new(
            Some(SessionId::new(replacement_session_id).unwrap()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let replacement_begun = runtime::v2_result(
        runtime::dispatch(&production, &replacement_begin),
        "session.begin",
    );
    assert_eq!(replacement_begun["goal_defined"], true);
    assert_eq!(replacement_begun["revision"], 1);
}

#[test]
fn v2gol001_define_and_running_revise_are_fenced_replayable_and_restart_safe() {
    let fixture = fixture();
    let production = runtime::dispatcher(Arc::clone(&fixture.manager), "v2gol001-mutations");
    initialize(&production, &fixture.selector, 102_001);
    let start = runtime::request(
        102_002,
        "session.start",
        &fixture.selector,
        json!({
            "procedure": "goal.yaml",
            "expected_procedure_digest": fixture.digest,
            "task_title": "Late goal definition"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol001-goalless-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&production, &start), "session.start");
    assert_eq!(started["goal_defined"], false);
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    runtime::begin(
        &production,
        &fixture.selector,
        102_003,
        &session_id,
        Map::new(),
        "v2gol001-goalless-begin",
    );
    let before_define = runtime::status(&production, &fixture.selector, 102_010, &session_id);

    let define = typed_request(
        102_011,
        "goal.define",
        &fixture.selector,
        goal_fields(
            "Persist a late goal.",
            "The active attempt is bound to revision one.",
        ),
        "v2gol001-define",
        session_identity_preconditions(&before_define),
    );
    let defined_once = runtime::dispatch(&production, &define);
    let defined = runtime::v2_result(defined_once.clone(), "goal.define");
    assert_eq!(defined["schema"], "podway.goal-definition-result/v1");
    assert_eq!(defined["goal_revision"], 1);
    assert_eq!(
        defined["revision"],
        before_define["session"]["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch(&production, &define)),
        runtime::without_request_id(&defined_once)
    );

    let after_define = runtime::status(&production, &fixture.selector, 102_020, &session_id);
    let duplicate = typed_request(
        102_021,
        "goal.define",
        &fixture.selector,
        goal_fields(
            "A duplicate definition.",
            "It must not replace revision one.",
        ),
        "v2gol001-duplicate-define",
        session_identity_preconditions(&after_define),
    );
    assert_public_error(
        runtime::dispatch(&production, &duplicate),
        "SESSION_GOAL_ALREADY_DEFINED",
    );

    let stale = typed_request(
        102_022,
        "goal.revise",
        &fixture.selector,
        json!({
            "goal": "A stale revision.",
            "criteria": [{"criterion_id": "verified", "statement": "It is rejected."}],
            "target_graph_node_id": "perform",
            "reason": "Exercise the exact goal revision fence."
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol001-stale-revise",
        goal_revision_preconditions(&after_define, 2),
    );
    let stale_error = assert_public_error(
        runtime::dispatch(&production, &stale),
        "GOAL_REVISION_STALE",
    );
    assert_eq!(stale_error["details"]["expected_goal_revision"], 2);
    assert_eq!(stale_error["details"]["actual_goal_revision"], 1);

    let missing_attempt_fence = typed_request(
        1_020_221,
        "goal.revise",
        &fixture.selector,
        json!({
            "goal": "A revision without an attempt fence.",
            "criteria": [{"criterion_id": "verified", "statement": "It is rejected as invalid input."}],
            "target_graph_node_id": "perform",
            "reason": "Exercise the running-session attempt fence."
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol001-missing-attempt-fence",
        session_identity_preconditions(&after_define)
            .with_goal_revision(GoalRevisionNumberV2::FIRST)
            .unwrap(),
    );
    assert_public_error(
        runtime::dispatch(&production, &missing_attempt_fence),
        "REQUEST_INVALID",
    );

    let revise = typed_request(
        102_023,
        "goal.revise",
        &fixture.selector,
        json!({
            "goal": "Persist a revised goal across restart.",
            "criteria": [{"criterion_id": "verified", "statement": "A fresh perform attempt is active."}],
            "target_graph_node_id": "perform",
            "reason": "The desired outcome now includes restart durability.",
            "actor": "V2GOL-001 test"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol001-revise",
        goal_revision_preconditions(&after_define, 1),
    );
    let revised_once = runtime::dispatch(&production, &revise);
    let revised = runtime::v2_result(revised_once.clone(), "goal.revise");
    assert_eq!(revised["schema"], "podway.goal-revision-result/v1");
    assert_eq!(revised["goal_revision"], 2);
    assert_eq!(revised["rework_to"], "perform");
    assert_eq!(revised["reactivated"], false);

    let before_restart = runtime::status(&production, &fixture.selector, 102_030, &session_id);
    let old_attempt = after_define["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap();
    assert_ne!(
        before_restart["current"]["attempt"]["attempt_id"],
        old_attempt
    );
    drop(production);
    drop(fixture.manager);

    let restarted_manager = Arc::new(runtime::manager(fixture.workspace.temporary_path()));
    let restarted = runtime::dispatcher(Arc::clone(&restarted_manager), "v2gol001-restarted");
    let after_restart = runtime::status(&restarted, &fixture.selector, 102_031, &session_id);
    assert_eq!(
        after_restart["current"]["attempt"]["attempt_id"],
        before_restart["current"]["attempt"]["attempt_id"]
    );
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch(&restarted, &revise)),
        runtime::without_request_id(&revised_once),
        "goal revision must replay its sealed result after a cold reopen"
    );
}

fn completed_goal_state() -> GraphSessionStateV2 {
    let created_at = UnixMillis::new(1_700_000_000_000);
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "completed-goal.yaml",
        COMPLETED_GOAL_PROCEDURE.as_bytes(),
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000103001").unwrap(),
        created_at,
    )
    .unwrap();
    let initial_attempt = AttemptId::new("00000000-0000-4000-8000-000000103002").unwrap();
    let state = graph_session_state_from_procedure_v2_snapshot(
        snapshot,
        "Completed goal reactivation",
        SessionId::new("00000000-0000-4000-8000-000000103003").unwrap(),
        initial_attempt.clone(),
        created_at,
    )
    .unwrap()
    .bind_initial_goal_at_start_v2(
        GoalStatementV2::new("Prove completed goal reactivation.").unwrap(),
        GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(
                CriterionId::new("verified").unwrap(),
                "The completed session can be explicitly reactivated.",
            )
            .unwrap(),
        ])
        .unwrap(),
        Some(ActorAttributionV2::new("V2GOL-001 test").unwrap()),
        created_at,
    )
    .unwrap()
    .into_state();
    let state = state
        .mutate_active_item_v2(
            &initial_attempt,
            &ItemId::new("result").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "completed-goal evidence".to_owned(),
            },
            created_at,
        )
        .unwrap()
        .into_state();

    let assess_attempt = AttemptId::new("00000000-0000-4000-8000-000000103004").unwrap();
    let at_assess = state
        .complete_active_action_v2(
            state.trace().revision(),
            &initial_attempt,
            Some(assess_attempt.clone()),
            UnixMillis::new(created_at.get() + 1),
        )
        .unwrap()
        .into_state();
    let active = at_assess.trace().active_attempt().unwrap().clone();
    let finish_attempt = AttemptId::new("00000000-0000-4000-8000-000000103005").unwrap();
    let recorded_at = UnixMillis::new(created_at.get() + 2);
    let result = CriterionAssessmentResultV2::new(
        CriterionId::new("verified").unwrap(),
        CriterionStatusV2::Satisfied,
        CriterionAssessmentReasonV2::new("The reactivation behavior is covered.").unwrap(),
        Vec::new(),
    )
    .unwrap();
    let assessment_state = AttemptCriterionAssessmentStateV2::new(
        assess_attempt.clone(),
        GoalRevisionNumberV2::FIRST,
        vec![CriterionAssessmentStateV2::new(
            result.clone(),
            None,
            recorded_at,
        )],
    )
    .unwrap();

    let mut trace = at_assess.trace().clone();
    trace
        .advance(
            &assess_attempt,
            AdvanceTerminalV2::Completed,
            GraphNodeId::new("finish").unwrap(),
            finish_attempt.clone(),
            Some(GoalRevisionNumberV2::FIRST),
        )
        .unwrap();
    let assess_memory = at_assess
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == &assess_attempt)
        .unwrap();
    let evidence = ResolvedEvidenceSetV2::new(
        assess_memory
            .evidence()
            .iter()
            .map(|reference| reference.resolution().clone())
            .collect(),
    )
    .unwrap();
    let decision = DecisionRecordV2::new(DecisionRecordInputV2 {
        trace: active.trace(),
        session_id: trace.session_id().clone(),
        session_revision: trace.revision(),
        procedure_snapshot_id: at_assess.snapshot().snapshot_id().clone(),
        procedure_digest: at_assess.snapshot().digest().clone(),
        graph_node_id: GraphNodeId::new("decide").unwrap(),
        node_definition_id: podway_core::NodeDefinitionId::new("assess").unwrap(),
        attempt_id: assess_attempt.clone(),
        attempt_number: active.number(),
        goal_revision: Some(GoalRevisionNumberV2::FIRST),
        selected_option: OptionId::new("achieved").unwrap(),
        route_effect: TransitionEffectV2::Advance,
        route_target: GraphNodeId::new("finish").unwrap(),
        reason: ReasonV2::new("The criterion is satisfied.").unwrap(),
        evidence: evidence.clone(),
        actor: None,
        recorded_at,
    })
    .unwrap();
    let assessment = GoalAssessmentRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        GoalOutcome::Achieved,
        vec![result],
        evidence,
        None,
        assess_attempt.clone(),
        GraphNodeId::new("decide").unwrap(),
        active.trace(),
        recorded_at,
    )
    .unwrap();
    let goal_state = GoalStateV2::new(
        Some(GoalRevisionNumberV2::FIRST),
        at_assess.goal_state().revisions().to_vec(),
        vec![assessment_state],
        vec![assessment],
    )
    .unwrap();
    let mut memories = at_assess.workflow_memory().attempts().to_vec();
    memories.push(
        AttemptWorkflowMemoryV2::new(
            finish_attempt.clone(),
            vec![
                ItemSlotStateV2::new(
                    finish_attempt.clone(),
                    ItemId::new("result").unwrap(),
                    ItemTypeV1::Text,
                    Revision::ZERO,
                    None,
                    recorded_at,
                    recorded_at,
                )
                .unwrap(),
            ],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
    );
    let workflow = WorkflowMemoryStateV2::new(
        memories,
        vec![decision],
        at_assess.workflow_memory().reworks().to_vec(),
    )
    .unwrap();
    let mut metadata = at_assess.attempt_metadata().to_vec();
    let assess_metadata = metadata
        .iter_mut()
        .find(|value| value.attempt_id() == &assess_attempt)
        .unwrap();
    *assess_metadata = AttemptMetadataV2::new(
        assess_attempt,
        assess_metadata.started_at(),
        Some(recorded_at),
        None,
    )
    .unwrap();
    metadata.push(AttemptMetadataV2::new(finish_attempt.clone(), recorded_at, None, None).unwrap());
    let counters = at_assess
        .counters()
        .iter()
        .map(|counter| {
            GraphNodeCounterV2::new(
                counter.graph_node_id().clone(),
                counter.attempt_count() + u64::from(counter.graph_node_id().as_str() == "finish"),
                counter.rework_traversal_count(),
            )
        })
        .collect();
    let at_finish = GraphSessionStateV2::new_with_goal_state(
        at_assess.workspace_revision().checked_next().unwrap(),
        at_assess.task_title(),
        at_assess.snapshot().clone(),
        trace,
        counters,
        metadata,
        workflow,
        goal_state,
        at_assess.created_at(),
        None,
        None,
        None,
    )
    .unwrap();
    let at_finish = at_finish
        .mutate_active_item_v2(
            &finish_attempt,
            &ItemId::new("result").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "terminal evidence".to_owned(),
            },
            UnixMillis::new(created_at.get() + 3),
        )
        .unwrap()
        .into_state();
    at_finish
        .complete_active_action_v2(
            at_finish.trace().revision(),
            &finish_attempt,
            None,
            UnixMillis::new(created_at.get() + 4),
        )
        .unwrap()
        .into_state()
}

#[test]
fn v2gol001_completed_revision_requires_explicit_reactivation_and_creates_fresh_cursor() {
    let completed = completed_goal_state();
    assert_eq!(
        completed.trace().lifecycle(),
        podway_core::SessionLifecycle::Completed
    );
    let target = GraphNodeId::new("perform").unwrap();
    let reason = GoalRevisionReasonV2::new("The completed goal now includes recovery.").unwrap();
    let without_flag = completed
        .revise_goal_v2(
            completed.trace().revision(),
            None,
            GoalRevisionNumberV2::FIRST,
            GoalStatementV2::new("Prove completed recovery and reactivation.").unwrap(),
            GoalDefinitionV2::new(vec![
                GoalCriterionV2::new(
                    CriterionId::new("verified").unwrap(),
                    "The reactivated session owns a fresh cursor.",
                )
                .unwrap(),
            ])
            .unwrap(),
            target.clone(),
            AttemptId::new("00000000-0000-4000-8000-000000103006").unwrap(),
            reason.clone(),
            None,
            false,
            UnixMillis::new(1_700_000_000_004),
        )
        .unwrap_err();
    assert_eq!(without_flag, GraphMutationErrorV2::ReactivationFlagRequired);

    let prior_trace_length = completed.trace().attempts().len();
    let fresh_attempt = AttemptId::new("00000000-0000-4000-8000-000000103007").unwrap();
    let revised = completed
        .revise_goal_v2(
            completed.trace().revision(),
            None,
            GoalRevisionNumberV2::FIRST,
            GoalStatementV2::new("Prove completed recovery and reactivation.").unwrap(),
            GoalDefinitionV2::new(vec![
                GoalCriterionV2::new(
                    CriterionId::new("verified").unwrap(),
                    "The reactivated session owns a fresh cursor.",
                )
                .unwrap(),
            ])
            .unwrap(),
            target,
            fresh_attempt.clone(),
            reason,
            None,
            true,
            UnixMillis::new(1_700_000_000_004),
        )
        .unwrap();
    assert_eq!(revised.revision().revision().get(), 2);
    assert!(revised.revision().reactivated());
    assert_eq!(
        revised.state().trace().lifecycle(),
        podway_core::SessionLifecycle::Running
    );
    assert_eq!(
        revised
            .state()
            .trace()
            .active_attempt()
            .unwrap()
            .attempt_id(),
        &fresh_attempt
    );
    assert_eq!(
        revised.state().trace().attempts().len(),
        prior_trace_length + 1
    );
    assert_eq!(revised.state().completed_at(), None);
}
