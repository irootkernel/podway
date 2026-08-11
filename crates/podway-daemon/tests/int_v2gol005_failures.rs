//! Production failure, recovery, and concurrency closure for V2GOL-005.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AttemptId, GoalRevisionNumberV2, Revision, SessionId};
use podway_daemon::{
    execution::{DaemonExecutionEngineV1, ExecutionClockV1},
    native_execution::{
        NativeArtifactVerifierV1, NativeExecutionIdSourceV1, NativeProcedureProviderV1,
        NativeWorkspaceRevalidatorV1, WallUtcExecutionClockV1,
    },
    server::{DaemonRequestV1, RequestDispatcherV1},
    workspace::{SqliteWorkspaceBindingInspectorV1, WorkspaceResolverV1},
};
use podway_git::NativeGitResolverV1;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_store::{
    AdmitOutcomeV1, IdempotencyKeyV1 as StoreIdempotencyKeyV1, JobStateV1,
    PersistedResponseContextV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreGraphStateContractV2,
    StoreReadContractV1, WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};

const FAILURE_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol005-failure-closure
version: "2"
name: Goal failure closure
purpose: Exercise deterministic goal failure and recovery behavior.
goal_tracking: true
node_definitions:
  prelude:
    type: action
    title: Prepare
    intent: Establish an earlier valid trace placement.
  work:
    type: action
    title: Produce evidence
    intent: Record the evidence considered by the goal assessment.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        max_length: 1000
  assess:
    type: decision
    title: Assess the goal
    objective: Select the outcome determined by the criterion results.
    prompt: Which goal outcome applies?
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Finish after a fresh goal assessment.
graph:
  entry: prelude
  nodes:
    - id: prelude
      use: prelude
      next: perform
    - id: perform
      use: work
      next: decide
    - id: decide
      use: assess
      evidence_from:
        - node: perform
          required: true
          items:
            - result
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - perform
    - decide
    - finish
"#;

struct Fixture {
    _workspace: support_phase4_workspace::GitWorktreeFixtureV1,
    selector: WorktreeSelectorWireV1,
    session_id: String,
}

fn request(
    number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    mut payload: Map<String, Value>,
    key: &str,
    preconditions: PreconditionsV1,
    detach: bool,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    let operation = match command {
        "session.status" | "job.lookup" | "job.status" => OperationV1::Query,
        _ => OperationV1::Mutate,
    };
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
        client: ClientInfoV1::new("v2gol005-test", "1", 1).unwrap(),
        operation,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: matches!(operation, OperationV1::Mutate)
            .then(|| IdempotencyKeyV1::new(key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(detach, if detach { 0 } else { 5_000 }).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    (envelope, daemon)
}

fn mutation(
    number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    payload: Value,
    key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let request = request(
        number,
        command,
        selector,
        payload.as_object().unwrap().clone(),
        key,
        preconditions,
        false,
    );
    assert!(matches!(request.1, DaemonRequestV1::ProcedureV2Mutation(_)));
    request
}

fn status(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &Fixture,
    number: u64,
) -> Map<String, Value> {
    let query = request(
        number,
        "session.status",
        &fixture.selector,
        json!({"verbose": true}).as_object().unwrap().clone(),
        "unused-v2gol005-status-key",
        PreconditionsV1::new(
            Some(SessionId::new(&fixture.session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
        false,
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &query), "session.status")
}

fn dispatch_after_cold_reopen(
    dispatcher: &impl RequestDispatcherV1,
    request: &(RequestEnvelopeV1, DaemonRequestV1),
) -> ResponseEnvelopeV2 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = runtime::dispatch(dispatcher, request);
        if !matches!(
            &response,
            ResponseEnvelopeV2::Error(error)
                if error.code().as_str() == "WORKSPACE_MAINTENANCE"
        ) {
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "cold reopen remained in maintenance"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn public_error(response: &ResponseEnvelopeV2, code: &str, admitted: bool) -> Value {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{code} must be returned as a public error: {response:?}")
    };
    assert_eq!(error.code().as_str(), code);
    assert_eq!(error.details()["admission"]["admitted"], admitted);
    let value = serde_json::to_value(error).unwrap();
    if let Some(kind) = value["details"]["kind"].as_str() {
        assert_eq!(kind, code);
    }
    value
}

fn graph_fingerprint(status: &Map<String, Value>) -> Value {
    let mut fingerprint = status.clone();
    fingerprint.remove("queue");
    Value::Object(fingerprint)
}

fn assert_failure_unchanged(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &Fixture,
    before: &Map<String, Value>,
    request: &(RequestEnvelopeV1, DaemonRequestV1),
    code: &str,
    admitted: bool,
    status_number: u64,
) -> ResponseEnvelopeV2 {
    let response = runtime::dispatch(dispatcher, request);
    public_error(&response, code, admitted);
    let after = status(dispatcher, fixture, status_number);
    assert_eq!(graph_fingerprint(&after), graph_fingerprint(before));
    assert_eq!(after["queue"]["queued_count"], 0);
    assert!(after["queue"]["running_job_id"].is_null());
    response
}

fn goal_preconditions(status: &Map<String, Value>, goal_revision: u64) -> PreconditionsV1 {
    runtime::session_preconditions(status)
        .with_goal_revision(GoalRevisionNumberV2::new(goal_revision))
        .unwrap()
}

fn completed_preconditions(status: &Map<String, Value>, goal_revision: u64) -> PreconditionsV1 {
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
    .with_goal_revision(GoalRevisionNumberV2::new(goal_revision))
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn assess_request(
    number: u64,
    fixture: &Fixture,
    status: &Map<String, Value>,
    criterion_id: &str,
    criterion_status: &str,
    evidence: Vec<&str>,
    key: &str,
    detach: bool,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let goal_revision = status["goal"]["revision"].as_u64().unwrap();
    let request = request(
        number,
        "goal.assess_criterion",
        &fixture.selector,
        json!({
            "criterion_id": criterion_id,
            "status": criterion_status,
            "reason": format!("Record {criterion_status} for {criterion_id}."),
            "evidence": evidence,
            "items": [],
            "actor": "V2GOL-005 reviewer"
        })
        .as_object()
        .unwrap()
        .clone(),
        key,
        goal_preconditions(status, goal_revision),
        detach,
    );
    assert!(matches!(request.1, DaemonRequestV1::ProcedureV2Mutation(_)));
    request
}

fn revise_request(
    number: u64,
    fixture: &Fixture,
    status: &Map<String, Value>,
    target: &str,
    reactivate: bool,
    key: &str,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let goal_revision = status["goal"]["revision"].as_u64().unwrap();
    let preconditions = if status["current"].is_null() {
        completed_preconditions(status, goal_revision)
    } else {
        goal_preconditions(status, goal_revision)
    };
    mutation(
        number,
        "goal.revise",
        &fixture.selector,
        json!({
            "goal": format!("Revised goal for {key}."),
            "criteria": [
                {"criterion_id": "correct", "statement": "The result remains correct."},
                {"criterion_id": "tested", "statement": "The result remains tested."}
            ],
            "target_graph_node_id": target,
            "reason": format!("Exercise {key}."),
            "actor": "V2GOL-005 reviewer",
            "reactivate": reactivate
        }),
        key,
        preconditions,
    )
}

fn complete_action(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &Fixture,
    number: u64,
    key: &str,
) -> Map<String, Value> {
    let before = status(dispatcher, fixture, number);
    let request = runtime::request(
        number + 1,
        "session.complete",
        &fixture.selector,
        Map::new(),
        key,
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.complete")
}

fn enter_decision(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &Fixture,
    number: u64,
) -> Map<String, Value> {
    let at_node = status(dispatcher, fixture, number);
    if at_node["current"]["node"]["graph_node_id"] == "prelude" {
        complete_action(dispatcher, fixture, number + 1, "v2gol005-complete-prelude");
    }
    let at_work = status(dispatcher, fixture, number + 3);
    assert_eq!(at_work["current"]["node"]["graph_node_id"], "perform");
    runtime::mutate_item(
        dispatcher,
        &fixture.selector,
        number + 4,
        &fixture.session_id,
        "item.set",
        "result",
        json!({"value": format!("goal evidence at {number}")})
            .as_object()
            .unwrap()
            .clone(),
        &format!("v2gol005-set-result-{number}"),
    );
    complete_action(
        dispatcher,
        fixture,
        number + 6,
        &format!("v2gol005-complete-work-{number}"),
    );
    let at_decision = status(dispatcher, fixture, number + 8);
    assert_eq!(at_decision["current"]["node"]["graph_node_id"], "decide");
    at_decision
}

fn start_fixture(
    dispatcher: &impl RequestDispatcherV1,
    workspace: support_phase4_workspace::GitWorktreeFixtureV1,
    number: u64,
) -> Fixture {
    runtime::make_runtime_private(workspace.main());
    fs::write(workspace.main().join("v2gol005.yaml"), FAILURE_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(FAILURE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = runtime::selector(workspace.main());
    let initialize = runtime::request(
        number,
        "workspace.init",
        &selector,
        Map::new(),
        &format!("v2gol005-init-{number}"),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let start = request(
        number + 1,
        "session.start",
        &selector,
        json!({
            "procedure": "v2gol005.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2GOL-005 failure closure",
            "goal": "Ship a correct and tested result.",
            "criteria": [
                {"criterion_id": "correct", "statement": "The result is correct."},
                {"criterion_id": "tested", "statement": "The result is tested."}
            ],
            "actor": "V2GOL-005 goal author"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2gol005-start-{number}"),
        PreconditionsV1::default(),
        false,
    );
    let started = runtime::v2_result(runtime::dispatch(dispatcher, &start), "session.start");
    Fixture {
        _workspace: workspace,
        selector,
        session_id: started["session_id"].as_str().unwrap().to_owned(),
    }
}

fn assess_success(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &Fixture,
    number: u64,
    criterion_id: &str,
    criterion_status: &str,
    key: &str,
) -> Map<String, Value> {
    let before = status(dispatcher, fixture, number);
    let request = assess_request(
        number + 1,
        fixture,
        &before,
        criterion_id,
        criterion_status,
        vec![],
        key,
        false,
    );
    runtime::v2_result(
        runtime::dispatch(dispatcher, &request),
        "goal.assess_criterion",
    )
}

fn retry(dispatcher: &impl RequestDispatcherV1, fixture: &Fixture, number: u64, key: &str) {
    let before = status(dispatcher, fixture, number);
    let request = runtime::request(
        number + 1,
        "session.retry",
        &fixture.selector,
        json!({"reason": format!("Switch assessment mode for {key}.")})
            .as_object()
            .unwrap()
            .clone(),
        key,
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.retry");
}

#[test]
fn v2gol005_failure_matrix_is_atomic_causal_and_cold_replayable() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager_root = workspace.temporary_path().to_path_buf();
    let manager = Arc::new(runtime::manager(&manager_root));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2gol005-failures");
    let fixture = start_fixture(&production, workspace, 105_000);
    let first_decision = enter_decision(&production, &fixture, 105_010);

    assess_success(
        &production,
        &fixture,
        105_020,
        "correct",
        "satisfied",
        "v2gol005-mode-assessment",
    );
    let assessment_mode = status(&production, &fixture, 105_022);
    let mixed = assess_request(
        105_023,
        &fixture,
        &assessment_mode,
        "tested",
        "not_applicable",
        vec![],
        "v2gol005-mode-mixed",
        false,
    );
    assert_failure_unchanged(
        &production,
        &fixture,
        &assessment_mode,
        &mixed,
        "CRITERION_MODE_MIXED",
        true,
        105_024,
    );
    retry(
        &production,
        &fixture,
        105_025,
        "v2gol005-retry-applicability",
    );
    assess_success(
        &production,
        &fixture,
        105_027,
        "correct",
        "not_applicable",
        "v2gol005-mode-applicability",
    );
    let applicability_mode = status(&production, &fixture, 105_029);
    let reverse_mixed = assess_request(
        105_030,
        &fixture,
        &applicability_mode,
        "tested",
        "satisfied",
        vec![],
        "v2gol005-mode-reverse-mixed",
        false,
    );
    assert_failure_unchanged(
        &production,
        &fixture,
        &applicability_mode,
        &reverse_mixed,
        "CRITERION_MODE_MIXED",
        true,
        105_031,
    );
    retry(&production, &fixture, 105_032, "v2gol005-retry-assessment");

    let before_revision = status(&production, &fixture, 105_034);
    let delayed_revision = revise_request(
        105_035,
        &fixture,
        &before_revision,
        "perform",
        false,
        "v2gol005-delayed-revision-a",
    );
    let revision_b = revise_request(
        105_036,
        &fixture,
        &before_revision,
        "perform",
        false,
        "v2gol005-revision-b",
    );
    let revised = runtime::v2_result(runtime::dispatch(&production, &revision_b), "goal.revise");
    assert_eq!(revised["goal_revision"], 2);
    let after_revision = status(&production, &fixture, 105_037);
    let delayed = assert_failure_unchanged(
        &production,
        &fixture,
        &after_revision,
        &delayed_revision,
        "SESSION_REVISION_CONFLICT",
        false,
        105_038,
    );
    let delayed_error = public_error(&delayed, "SESSION_REVISION_CONFLICT", false);
    assert_eq!(
        delayed_error["details"]["expected_revision"],
        before_revision["session"]["revision"]
    );
    assert_eq!(
        delayed_error["details"]["current_revision"],
        after_revision["session"]["revision"]
    );

    let old_goal_revision = mutation(
        105_381,
        "goal.assess_criterion",
        &fixture.selector,
        json!({
            "criterion_id": "correct",
            "status": "satisfied",
            "reason": "A current cursor cannot write against the prior goal revision.",
            "evidence": [],
            "items": []
        }),
        "v2gol005-old-goal-revision",
        runtime::session_preconditions(&after_revision)
            .with_goal_revision(GoalRevisionNumberV2::FIRST)
            .unwrap(),
    );
    let old_goal_response = assert_failure_unchanged(
        &production,
        &fixture,
        &after_revision,
        &old_goal_revision,
        "GOAL_REVISION_STALE",
        true,
        105_382,
    );
    let old_goal_error = public_error(&old_goal_response, "GOAL_REVISION_STALE", true);
    assert_eq!(old_goal_error["details"]["expected_goal_revision"], 1);
    assert_eq!(old_goal_error["details"]["actual_goal_revision"], 2);

    let stale_attempt_preconditions = PreconditionsV1::new(
        Some(SessionId::new(&fixture.session_id).unwrap()),
        Some(Revision::new(
            after_revision["session"]["revision"].as_u64().unwrap(),
        )),
        Some(
            AttemptId::new(
                first_decision["current"]["attempt"]["attempt_id"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        ),
        None,
        None,
        None,
    )
    .unwrap()
    .with_goal_revision(GoalRevisionNumberV2::new(2))
    .unwrap();
    let stale_attempt = mutation(
        105_039,
        "goal.assess_criterion",
        &fixture.selector,
        json!({
            "criterion_id": "correct",
            "status": "satisfied",
            "reason": "The stale attempt must not accept a result.",
            "evidence": [],
            "items": []
        }),
        "v2gol005-stale-attempt",
        stale_attempt_preconditions,
    );
    assert_failure_unchanged(
        &production,
        &fixture,
        &after_revision,
        &stale_attempt,
        "ATTEMPT_NOT_CURRENT",
        false,
        105_040,
    );

    let forbidden = revise_request(
        105_041,
        &fixture,
        &after_revision,
        "prelude",
        false,
        "v2gol005-forbidden-target",
    );
    let forbidden_response = assert_failure_unchanged(
        &production,
        &fixture,
        &after_revision,
        &forbidden,
        "GOAL_REVISION_TARGET_NOT_ALLOWED",
        true,
        105_042,
    );
    assert_eq!(
        public_error(
            &forbidden_response,
            "GOAL_REVISION_TARGET_NOT_ALLOWED",
            true
        )["details"]["target_graph_node_id"],
        "prelude"
    );
    let off_trace = revise_request(
        105_043,
        &fixture,
        &after_revision,
        "decide",
        false,
        "v2gol005-off-trace-target",
    );
    assert_failure_unchanged(
        &production,
        &fixture,
        &after_revision,
        &off_trace,
        "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
        true,
        105_044,
    );

    enter_decision(&production, &fixture, 105_050);
    assess_success(
        &production,
        &fixture,
        105_060,
        "correct",
        "satisfied",
        "v2gol005-final-correct",
    );
    assess_success(
        &production,
        &fixture,
        105_062,
        "tested",
        "satisfied",
        "v2gol005-final-tested",
    );
    let ready = status(&production, &fixture, 105_064);
    assert_eq!(ready["goal"]["determined_outcome"], "achieved");
    assert_eq!(
        ready["goal"]["criteria"],
        json!([
            {"criterion_id": "correct", "statement": "The result remains correct.", "status": "satisfied"},
            {"criterion_id": "tested", "statement": "The result remains tested.", "status": "satisfied"}
        ])
    );
    assert_eq!(ready["current"]["readiness"]["can_advance"], true);
    let decide = mutation(
        105_065,
        "session.decide",
        &fixture.selector,
        json!({
            "option_id": "achieved",
            "reason": "Select the outcome determined by the recorded criterion states."
        }),
        "v2gol005-final-decision",
        runtime::session_preconditions(&ready)
            .with_goal_revision(GoalRevisionNumberV2::new(2))
            .unwrap(),
    );
    runtime::v2_result(runtime::dispatch(&production, &decide), "session.decide");
    complete_action(&production, &fixture, 105_066, "v2gol005-complete-terminal");
    let completed = status(&production, &fixture, 105_068);
    assert_eq!(completed["session"]["lifecycle"], "completed");

    let missing_reactivate = revise_request(
        105_069,
        &fixture,
        &completed,
        "perform",
        false,
        "v2gol005-missing-reactivate",
    );
    let missing_response = assert_failure_unchanged(
        &production,
        &fixture,
        &completed,
        &missing_reactivate,
        "REACTIVATION_FLAG_REQUIRED",
        true,
        105_070,
    );
    let unsafe_target = revise_request(
        105_071,
        &fixture,
        &completed,
        "finish",
        true,
        "v2gol005-unsafe-target",
    );
    let unsafe_response = assert_failure_unchanged(
        &production,
        &fixture,
        &completed,
        &unsafe_target,
        "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
        true,
        105_072,
    );
    assert_eq!(
        public_error(
            &unsafe_response,
            "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
            true
        )["details"]["target_graph_node_id"],
        "finish"
    );

    let reactivate = revise_request(
        105_073,
        &fixture,
        &completed,
        "perform",
        true,
        "v2gol005-reactivate",
    );
    runtime::v2_result(runtime::dispatch(&production, &reactivate), "goal.revise");
    let running = status(&production, &fixture, 105_074);
    let cancel = runtime::request(
        105_075,
        "session.cancel",
        &fixture.selector,
        json!({"reason": "Cancelled sessions cannot revise their goal."})
            .as_object()
            .unwrap()
            .clone(),
        "v2gol005-cancel",
        runtime::session_preconditions(&running),
    );
    runtime::v2_result(runtime::dispatch(&production, &cancel), "session.cancel");
    let cancelled = status(&production, &fixture, 105_076);
    let cancelled_revise = revise_request(
        105_077,
        &fixture,
        &cancelled,
        "perform",
        true,
        "v2gol005-cancelled-revise",
    );
    assert_failure_unchanged(
        &production,
        &fixture,
        &cancelled,
        &cancelled_revise,
        "SESSION_CANCELLED",
        true,
        105_078,
    );

    drop(production);
    drop(manager);
    let reopened_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(reopened_manager, "v2gol005-failure-replay");
    let replay = dispatch_after_cold_reopen(&reopened, &missing_reactivate);
    assert_eq!(
        runtime::without_request_id(&replay),
        runtime::without_request_id(&missing_response),
        "an admitted lifecycle failure must replay from its sealed cold receipt"
    );
    let cold_cancelled = status(&reopened, &fixture, 105_079);
    assert_eq!(
        graph_fingerprint(&cold_cancelled),
        graph_fingerprint(&cancelled)
    );
}

#[test]
fn v2gol005_queued_assessment_executes_after_cold_reopen_and_replays_exactly() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager_root = workspace.temporary_path().to_path_buf();
    let manager = Arc::new(runtime::manager(&manager_root));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2gol005-detached-admit");
    let fixture = start_fixture(&production, workspace, 106_000);
    let at_decision = enter_decision(&production, &fixture, 106_010);
    let synchronous = assess_request(
        106_020,
        &fixture,
        &at_decision,
        "correct",
        "satisfied",
        vec!["perform"],
        "v2gol005-detached-assessment",
        false,
    );
    let DaemonRequestV1::ProcedureV2Mutation(typed_request) = &synchronous.1 else {
        unreachable!()
    };
    drop(production);
    drop(manager);

    let options = SqliteStoreOptionsV1::new(8).unwrap();
    let resolved = WorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        SqliteWorkspaceBindingInspectorV1::new(options.clone()),
    )
    .resolve_existing(
        support_phase4_workspace::selector(fixture._workspace.main()),
        None,
    )
    .unwrap();
    let binding = WorkspaceBindingV1::new(
        resolved.store_identity().clone(),
        resolved.workspace_root().clone(),
    );
    let now = WallUtcExecutionClockV1.now();
    let store = Arc::new(
        SqliteStoreV1::open(
            resolved.database_path(),
            resolved.workspace_root(),
            resolved.store_identity().clone(),
            options.clone(),
            now,
        )
        .unwrap(),
    );
    let before_admission = store
        .read_graph_session_v2(binding.identity())
        .unwrap()
        .unwrap();
    let engine = DaemonExecutionEngineV1::new(
        Arc::clone(&store),
        NativeExecutionIdSourceV1,
        WallUtcExecutionClockV1,
        NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
        NativeArtifactVerifierV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
        NativeWorkspaceRevalidatorV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
    );
    let response_context = PersistedResponseContextV1::new(
        synchronous.0.request_id().as_str(),
        "goal.assess_criterion",
        binding.identity().workspace_uuid().clone(),
        binding
            .last_validated_root()
            .to_path_buf()
            .display()
            .to_string(),
        0,
    )
    .unwrap()
    .with_frozen_public_terminal_envelope();
    let admitted = engine
        .admit_procedure_v2_typed_mutation_for_workspace_with_response_context(
            &binding,
            typed_request,
            StoreIdempotencyKeyV1::new("v2gol005-detached-assessment").unwrap(),
            Some(response_context),
        )
        .unwrap()
        .unwrap();
    let AdmitOutcomeV1::New(receipt) = admitted else {
        panic!("the direct Store admission must create one new queued job")
    };
    let job_id = receipt.job_id().as_str().to_owned();
    let queued_job = store
        .read_job(binding.identity(), receipt.job_id())
        .unwrap()
        .unwrap();
    assert_eq!(queued_job.state(), JobStateV1::Queued);
    assert_eq!(
        store
            .read_graph_session_v2(binding.identity())
            .unwrap()
            .unwrap(),
        before_admission,
        "durable admission alone must not execute or mutate graph state"
    );
    drop(engine);
    drop(store);

    let reopened_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(Arc::clone(&reopened_manager), "v2gol005-detached-run");
    let terminal_response = dispatch_after_cold_reopen(&reopened, &synchronous);
    let terminal = runtime::v2_result(terminal_response.clone(), "goal.assess_criterion");
    assert_eq!(terminal["result"]["criterion_id"], "correct");
    assert_eq!(terminal["result"]["status"], "satisfied");

    let lookup = request(
        106_023,
        "job.lookup",
        &fixture.selector,
        json!({"idempotency_key": "v2gol005-detached-assessment"})
            .as_object()
            .unwrap()
            .clone(),
        "unused-v2gol005-lookup",
        PreconditionsV1::default(),
        false,
    );
    let lookup = runtime::v2_result(runtime::dispatch(&reopened, &lookup), "job.lookup");
    assert_eq!(lookup["found"], true);
    assert_eq!(lookup["job"]["id"], job_id);
    assert_eq!(lookup["job"]["command"], "goal.assess_criterion");
    assert_eq!(lookup["job"]["state"], "succeeded");
    let after = status(&reopened, &fixture, 106_024);
    assert_eq!(after["queue"]["queued_count"], 0);
    assert_eq!(after["goal"]["criteria"][0]["status"], "satisfied");
    assert_eq!(
        after["current"]["attempt"]["attempt_id"],
        at_decision["current"]["attempt"]["attempt_id"]
    );

    let drift = request(
        106_025,
        "goal.assess_criterion",
        &fixture.selector,
        json!({
            "criterion_id": "correct",
            "status": "unsatisfied",
            "reason": "A different canonical payload must not reuse the key.",
            "evidence": [],
            "items": []
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol005-detached-assessment",
        goal_preconditions(&at_decision, 1),
        false,
    );
    public_error(
        &runtime::dispatch(&reopened, &drift),
        "IDEMPOTENCY_KEY_REUSED",
        false,
    );
    drop(reopened);
    drop(reopened_manager);

    let final_manager = Arc::new(runtime::manager(&manager_root));
    let final_dispatcher = runtime::dispatcher(final_manager, "v2gol005-detached-replay");
    let replay = dispatch_after_cold_reopen(&final_dispatcher, &synchronous);
    assert_eq!(
        runtime::without_request_id(&replay),
        runtime::without_request_id(&terminal_response)
    );
    let final_status = status(&final_dispatcher, &fixture, 106_026);
    assert_eq!(graph_fingerprint(&final_status), graph_fingerprint(&after));
}

#[test]
fn v2gol005_concurrent_distinct_criteria_commit_exactly_one_result() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager_root = workspace.temporary_path().to_path_buf();
    let manager = Arc::new(runtime::manager(&manager_root));
    let production = Arc::new(runtime::dispatcher(
        Arc::clone(&manager),
        "v2gol005-concurrent",
    ));
    let fixture = start_fixture(production.as_ref(), workspace, 107_000);
    let before = enter_decision(production.as_ref(), &fixture, 107_010);
    let attempt_id = before["current"]["attempt"]["attempt_id"].clone();
    let left = assess_request(
        107_020,
        &fixture,
        &before,
        "correct",
        "satisfied",
        vec![],
        "v2gol005-concurrent-correct",
        false,
    );
    let right = assess_request(
        107_021,
        &fixture,
        &before,
        "tested",
        "satisfied",
        vec![],
        "v2gol005-concurrent-tested",
        false,
    );
    let barrier = Arc::new(Barrier::new(3));
    let left_dispatcher = Arc::clone(&production);
    let left_barrier = Arc::clone(&barrier);
    let left_thread = thread::spawn(move || {
        left_barrier.wait();
        runtime::dispatch(left_dispatcher.as_ref(), &left)
    });
    let right_dispatcher = Arc::clone(&production);
    let right_barrier = Arc::clone(&barrier);
    let right_thread = thread::spawn(move || {
        right_barrier.wait();
        runtime::dispatch(right_dispatcher.as_ref(), &right)
    });
    barrier.wait();
    let responses = [left_thread.join().unwrap(), right_thread.join().unwrap()];
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(response, ResponseEnvelopeV2::OutputV2(_)))
            .count(),
        1
    );
    let loser = responses
        .iter()
        .find(|response| matches!(response, ResponseEnvelopeV2::Error(_)))
        .unwrap();
    public_error(loser, "SESSION_REVISION_CONFLICT", true);

    let after = status(production.as_ref(), &fixture, 107_022);
    assert_eq!(after["current"]["attempt"]["attempt_id"], attempt_id);
    assert_eq!(
        after["goal"]["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|criterion| criterion["status"] == "satisfied")
            .count(),
        1
    );
    assert_eq!(after["goal"]["determined_outcome"], Value::Null);
    assert_eq!(after["queue"]["queued_count"], 0);
    assert_eq!(after["trace_length"], before["trace_length"]);

    drop(production);
    drop(manager);
    let reopened_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(reopened_manager, "v2gol005-concurrent-reopen");
    let cold = status(&reopened, &fixture, 107_023);
    assert_eq!(graph_fingerprint(&cold), graph_fingerprint(&after));
}
