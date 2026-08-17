//! Production state-table coverage for V2GOL-003 goal outcomes and progression gates.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    AttemptId, DomainCommand, DomainError, GoalRevisionNumberV2, GraphNodeId, JobId, Sha256Digest,
    canonicalize_json_v1,
};
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
    ProcedureV2MutationRequestV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1,
    RequestOptionsV1, ResponseEnvelopeV2, WorkspaceContextV1,
    canonical_procedure_v2_mutation_identity_v1,
};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1, CanonicalExecutionJsonV1,
    IdempotencyKeyV1 as StoreIdempotencyKeyV1, PersistedGraphMutationFailureV2,
    PersistedGraphTerminalOperationV2, PersistedResponseContextV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreGraphStateContractV2, StoreReadContractV1, TerminalResultV1, WorkerIdV1,
    WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const GOAL_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol003-outcomes
version: "2"
name: Goal outcome runtime
purpose: Exercise goal outcome derivation and terminal progression gates.
goal_tracking: true
node_definitions:
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
    objective: Select the outcome determined by the recorded criteria.
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
  entry: perform
  nodes:
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
      skip:
        allowed: true
        reason_required: true
manual_rework:
  allowed_targets:
    - perform
"#;

const GENERAL_DECISION_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol003-general-decision
version: "2"
name: General decision regression
purpose: Prove that an ordinary decision never creates a goal assessment.
goal_tracking: true
node_definitions:
  choose:
    type: decision
    title: Choose
    objective: Select the ordinary route.
    prompt: Continue?
    options:
      - id: continue
        label: Continue
    reason:
      required: true
  assess:
    type: decision
    title: Assess the goal
    objective: Select the recorded goal outcome.
    prompt: Which outcome applies?
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
    intent: Finish the ordinary decision fixture.
graph:
  entry: choose
  nodes:
    - id: choose
      use: choose
      routes:
        continue:
          to: assess
          effect: advance
    - id: assess
      use: assess
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
"#;

struct GoalFixture {
    workspace: support_phase4_workspace::GitWorktreeFixtureV1,
    selector: podway_protocol::WorktreeSelectorWireV1,
    session_id: String,
}

fn procedure_digest(source: &str, name: &str) -> Sha256Digest {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("{name} must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(name, source, ProcedureDocumentFormat::Yaml);
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(diagnostics.is_empty(), "invalid {name}: {diagnostics:?}");
    validated.digest().clone()
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
        "v2gol003-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &request),
        ResponseEnvelopeV2::OutputV2(_)
    ));
}

fn goal_start_request(
    selector: &podway_protocol::WorktreeSelectorWireV1,
    mut payload: Map<String, Value>,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000103002").unwrap(),
        client: ClientInfoV1::new("v2gol003-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.start").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new("v2gol003-start").unwrap()),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Start(_)));
    (envelope, daemon)
}

fn goal_fixture(worker_id: &str) -> (GoalFixture, impl RequestDispatcherV1) {
    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(workspace.main().join("goal.yaml"), GOAL_PROCEDURE).unwrap();
    let digest = procedure_digest(GOAL_PROCEDURE, "goal.yaml");
    let selector = runtime::selector(workspace.main());
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let dispatcher = runtime::dispatcher(manager, worker_id);
    initialize(&dispatcher, &selector, 103_001);
    let start = goal_start_request(
        &selector,
        json!({
            "procedure": "goal.yaml",
            "expected_procedure_digest": digest,
            "task_title": "Derive goal outcomes"
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    runtime::begin(
        &dispatcher,
        &selector,
        103_099,
        &session_id,
        json!({
            "goal": "Ship a correct and tested result.",
            "criteria": [
                {"criterion_id": "correct", "statement": "The result is correct."},
                {"criterion_id": "tested", "statement": "The result is tested."}
            ],
            "actor": "goal author"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol003-begin",
    );
    runtime::mutate_item(
        &dispatcher,
        &selector,
        103_003,
        &session_id,
        "item.set",
        "result",
        json!({"value": "recorded evidence"})
            .as_object()
            .unwrap()
            .clone(),
        "v2gol003-set-result",
    );
    let before = runtime::status(&dispatcher, &selector, 103_004, &session_id);
    let complete = runtime::request(
        103_005,
        "session.complete",
        &selector,
        Map::new(),
        "v2gol003-complete-work",
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(
        runtime::dispatch(&dispatcher, &complete),
        "session.complete",
    );
    let at_decision = runtime::status(&dispatcher, &selector, 103_006, &session_id);
    assert_eq!(at_decision["current"]["node"]["graph_node_id"], "decide");
    (
        GoalFixture {
            workspace,
            selector,
            session_id,
        },
        dispatcher,
    )
}

fn general_decision_fixture(worker_id: &str) -> (GoalFixture, impl RequestDispatcherV1) {
    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(
        workspace.main().join("general.yaml"),
        GENERAL_DECISION_PROCEDURE,
    )
    .unwrap();
    let digest = procedure_digest(GENERAL_DECISION_PROCEDURE, "general.yaml");
    let selector = runtime::selector(workspace.main());
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let dispatcher = runtime::dispatcher(manager, worker_id);
    initialize(&dispatcher, &selector, 103_201);
    let start = goal_start_request(
        &selector,
        json!({
            "procedure": "general.yaml",
            "expected_procedure_digest": digest,
            "task_title": "General decision regression"
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    runtime::begin(
        &dispatcher,
        &selector,
        103_299,
        &session_id,
        json!({
            "goal": "Reach a recorded outcome.",
            "criteria": [{"criterion_id": "verified", "statement": "The result is verified."}]
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol003-general-begin",
    );
    (
        GoalFixture {
            workspace,
            selector,
            session_id,
        },
        dispatcher,
    )
}

fn mutation_preconditions(status: &Map<String, Value>) -> PreconditionsV1 {
    runtime::session_preconditions(status)
        .with_goal_revision(GoalRevisionNumberV2::FIRST)
        .unwrap()
}

fn assess(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &GoalFixture,
    number: u64,
    criterion_id: &str,
    status_name: &str,
    key: &str,
) -> Map<String, Value> {
    let status = runtime::status(dispatcher, &fixture.selector, number, &fixture.session_id);
    let mut payload = json!({
        "criterion_id": criterion_id,
        "status": status_name,
        "reason": format!("The actor records {status_name} without semantic judgment."),
        "evidence": [],
        "items": [],
        "actor": "V2GOL-003 reviewer"
    })
    .as_object()
    .unwrap()
    .clone();
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(&fixture.selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{:012x}", number + 1))
            .unwrap(),
        client: ClientInfoV1::new("v2gol003-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("goal.assess_criterion").unwrap(),
        workspace: Some(WorkspaceContextV1::new(fixture.selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        preconditions: mutation_preconditions(&status),
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    let request = (envelope, daemon);
    runtime::v2_result(
        runtime::dispatch(dispatcher, &request),
        "goal.assess_criterion",
    )
}

fn decide_request(
    number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    option_id: &str,
    key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
        client: ClientInfoV1::new("v2gol003-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.decide").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload: json!({
            "selector": selector,
            "option_id": option_id,
            "reason": "Select the outcome determined by the recorded criterion states.",
            "actor": "V2GOL-003 reviewer"
        })
        .as_object()
        .unwrap()
        .clone(),
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    (envelope, daemon)
}

fn error(response: ResponseEnvelopeV2, code: &str) -> Value {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{code} must be returned as a public error")
    };
    assert_eq!(error.code().as_str(), code);
    serde_json::to_value(error).unwrap()
}

fn stable_state(status: &Map<String, Value>) -> Value {
    json!({
        "revision": status["session"]["revision"],
        "trace_length": status["trace_length"],
        "current": status["current"],
        "goal": status.get("goal").cloned().unwrap_or(Value::Null),
    })
}

fn complete_assessment(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &GoalFixture,
    statuses: [&str; 2],
    option_id: &str,
    outcome: &str,
) -> (
    (RequestEnvelopeV1, DaemonRequestV1),
    ResponseEnvelopeV2,
    Map<String, Value>,
) {
    assess(
        dispatcher,
        fixture,
        103_010,
        "correct",
        statuses[0],
        "v2gol003-assess-correct",
    );
    assess(
        dispatcher,
        fixture,
        103_020,
        "tested",
        statuses[1],
        "v2gol003-assess-tested",
    );
    let ready = runtime::status(dispatcher, &fixture.selector, 103_030, &fixture.session_id);
    let request = decide_request(
        103_031,
        &fixture.selector,
        option_id,
        "v2gol003-decide",
        mutation_preconditions(&ready),
    );
    let response = runtime::dispatch(dispatcher, &request);
    let result = runtime::v2_result(response.clone(), "session.decide");
    assert_eq!(result["record"]["assessment"], "session_goal");
    assert_eq!(result["record"]["goal_outcome"], outcome);
    assert_eq!(
        result["record"]["assessment_mode"],
        if outcome == "superseded" {
            "applicability"
        } else {
            "assessment"
        }
    );
    assert_eq!(
        result["record"]["criterion_results"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        result["record"]["criterion_results"][0]["criterion_id"],
        "correct"
    );
    assert_eq!(
        result["record"]["criterion_results"][0]["status"],
        statuses[0]
    );
    assert_eq!(
        result["record"]["criterion_results"][1]["criterion_id"],
        "tested"
    );
    assert_eq!(
        result["record"]["criterion_results"][1]["status"],
        statuses[1]
    );
    (request, response, result)
}

#[test]
fn v2gol003_outcome_state_table_is_actor_claimed_and_replayable() {
    let cases = [
        (["satisfied", "satisfied"], "achieved", "achieved"),
        (["satisfied", "unsatisfied"], "not-achieved", "not_achieved"),
        (
            ["not_applicable", "not_applicable"],
            "superseded",
            "superseded",
        ),
    ];
    for (index, (statuses, option_id, outcome)) in cases.into_iter().enumerate() {
        let worker_id = format!("v2gol003-table-{index}");
        let (fixture, dispatcher) = goal_fixture(&worker_id);
        let (request, response, result) =
            complete_assessment(&dispatcher, &fixture, statuses, option_id, outcome);
        assert_eq!(result["option_id"], option_id);
        assert_eq!(result["target_graph_node_id"], "finish");
        assert_eq!(
            runtime::without_request_id(&runtime::dispatch(&dispatcher, &request)),
            runtime::without_request_id(&response),
            "the paired decision and assessment must replay as one sealed result"
        );
    }
}

#[test]
fn v2gol003_incomplete_and_mismatched_outcomes_fail_atomically() {
    let (fixture, dispatcher) = goal_fixture("v2gol003-invalid");
    assess(
        &dispatcher,
        &fixture,
        103_110,
        "correct",
        "satisfied",
        "v2gol003-invalid-first",
    );
    let incomplete = runtime::status(&dispatcher, &fixture.selector, 103_120, &fixture.session_id);
    let baseline = stable_state(&incomplete);
    let missing_fence_request = decide_request(
        103_121,
        &fixture.selector,
        "achieved",
        "v2gol003-missing-goal-fence",
        runtime::session_preconditions(&incomplete),
    );
    error(
        runtime::dispatch(&dispatcher, &missing_fence_request),
        "REQUEST_INVALID",
    );
    let after_missing_fence =
        runtime::status(&dispatcher, &fixture.selector, 103_122, &fixture.session_id);
    assert_eq!(stable_state(&after_missing_fence), baseline);

    let stale_preconditions = runtime::session_preconditions(&incomplete)
        .with_goal_revision(GoalRevisionNumberV2::new(2))
        .unwrap();
    let stale_fence_request = decide_request(
        103_123,
        &fixture.selector,
        "achieved",
        "v2gol003-stale-goal-fence",
        stale_preconditions,
    );
    let stale = error(
        runtime::dispatch(&dispatcher, &stale_fence_request),
        "GOAL_REVISION_STALE",
    );
    assert_eq!(stale["details"]["expected_goal_revision"], 2);
    assert_eq!(stale["details"]["actual_goal_revision"], 1);
    let after_stale_fence =
        runtime::status(&dispatcher, &fixture.selector, 103_124, &fixture.session_id);
    assert_eq!(stable_state(&after_stale_fence), baseline);

    let incomplete_request = decide_request(
        103_125,
        &fixture.selector,
        "achieved",
        "v2gol003-incomplete",
        mutation_preconditions(&incomplete),
    );
    let missing = error(
        runtime::dispatch(&dispatcher, &incomplete_request),
        "CRITERION_RESULT_MISSING",
    );
    assert_eq!(
        missing["details"]["missing_criterion_ids"],
        json!(["tested"])
    );
    let after_incomplete =
        runtime::status(&dispatcher, &fixture.selector, 103_126, &fixture.session_id);
    assert_eq!(stable_state(&after_incomplete), baseline);

    assess(
        &dispatcher,
        &fixture,
        103_130,
        "tested",
        "satisfied",
        "v2gol003-invalid-second",
    );
    let complete = runtime::status(&dispatcher, &fixture.selector, 103_140, &fixture.session_id);
    let baseline = stable_state(&complete);
    let mismatch_request = decide_request(
        103_141,
        &fixture.selector,
        "not-achieved",
        "v2gol003-mismatch",
        mutation_preconditions(&complete),
    );
    let mismatch = error(
        runtime::dispatch(&dispatcher, &mismatch_request),
        "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED",
    );
    assert_eq!(mismatch["details"]["option_id"], "not-achieved");
    assert_eq!(mismatch["details"]["determined_outcome"], "achieved");
    assert_eq!(
        mismatch["details"]["allowed_option_ids"],
        json!(["achieved"])
    );
    let after_mismatch =
        runtime::status(&dispatcher, &fixture.selector, 103_142, &fixture.session_id);
    assert_eq!(stable_state(&after_mismatch), baseline);
}

#[test]
fn v2gol003_general_decision_never_creates_goal_assessment_state() {
    let (fixture, dispatcher) = general_decision_fixture("v2gol003-general");
    let before = runtime::status(&dispatcher, &fixture.selector, 103_203, &fixture.session_id);
    let stale = decide_request(
        103_204,
        &fixture.selector,
        "continue",
        "v2gol003-general-stale",
        runtime::session_preconditions(&before)
            .with_goal_revision(GoalRevisionNumberV2::new(2))
            .unwrap(),
    );
    let stale_response = runtime::dispatch(&dispatcher, &stale);
    error(stale_response.clone(), "GOAL_REVISION_STALE");
    let unchanged = runtime::status(&dispatcher, &fixture.selector, 103_205, &fixture.session_id);
    assert_eq!(stable_state(&unchanged), stable_state(&before));
    let decide = decide_request(
        103_206,
        &fixture.selector,
        "continue",
        "v2gol003-general-decide",
        runtime::session_preconditions(&before)
            .with_goal_revision(GoalRevisionNumberV2::FIRST)
            .unwrap(),
    );
    let decided = runtime::dispatch(&dispatcher, &decide);
    let result = runtime::v2_result(decided.clone(), "session.decide");
    assert_eq!(result["record"]["goal_revision"], 1);
    drop(dispatcher);
    let restarted = runtime::dispatcher(
        Arc::new(runtime::manager(fixture.workspace.temporary_path())),
        "v2gol003-general-restart",
    );
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch_after_cold_reopen(&restarted, &decide)),
        runtime::without_request_id(&decided)
    );
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch_after_cold_reopen(&restarted, &stale)),
        runtime::without_request_id(&stale_response),
        "the stale V14 failure must remain sealed after restart"
    );
    for field in [
        "assessment",
        "assessment_mode",
        "goal_outcome",
        "criterion_results",
    ] {
        assert!(result["record"].get(field).is_none(), "unexpected {field}");
    }
}

#[test]
fn v2gol003_goal_assessment_wrong_verb_on_general_decision_is_not_admitted_and_changes_no_state() {
    let (fixture, dispatcher) = general_decision_fixture("v2gol003-wrong-assessment-verb");
    let before = runtime::status(&dispatcher, &fixture.selector, 103_301, &fixture.session_id);
    let mut payload = json!({
        "criterion_id": "verified",
        "status": "satisfied",
        "reason": "An ordinary decision cannot accept a criterion assessment.",
        "evidence": [],
        "items": [],
        "actor": "V2GOL-003 reviewer"
    })
    .as_object()
    .unwrap()
    .clone();
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(&fixture.selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000103302").unwrap(),
        client: ClientInfoV1::new("v2gol003-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("goal.assess_criterion").unwrap(),
        workspace: Some(WorkspaceContextV1::new(fixture.selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new("v2gol003-wrong-assessment-verb").unwrap()),
        preconditions: mutation_preconditions(&before),
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));

    let rejected = error(
        runtime::dispatch(&dispatcher, &(envelope, daemon)),
        "REQUEST_INVALID",
    );
    assert_eq!(rejected["details"]["admission"]["admitted"], false);

    let after = runtime::status(&dispatcher, &fixture.selector, 103_303, &fixture.session_id);
    assert_eq!(stable_state(&after), stable_state(&before));
    assert_eq!(after["queue"], before["queue"]);
}

#[test]
fn v2gol003_queued_v9_goal_assessment_fails_and_replays_after_cold_reopen() {
    let (fixture, dispatcher) = goal_fixture("v2gol003-legacy-v9");
    let before = runtime::status(&dispatcher, &fixture.selector, 103_401, &fixture.session_id);
    let legacy_request = decide_request(
        103_402,
        &fixture.selector,
        "achieved",
        "v2gol003-legacy-v9-decision",
        runtime::session_preconditions(&before),
    );
    let DaemonRequestV1::ProcedureV2Mutation(typed_request) = &legacy_request.1 else {
        panic!("the legacy decision fixture must use the typed Procedure v2 route")
    };
    let typed_request: &ProcedureV2MutationRequestV1 = typed_request;
    drop(dispatcher);

    let options = SqliteStoreOptionsV1::new(8).unwrap();
    let resolved = WorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        SqliteWorkspaceBindingInspectorV1::new(options.clone()),
    )
    .resolve_existing(
        support_phase4_workspace::selector(fixture.workspace.main()),
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
    let state = store
        .read_graph_session_v2(binding.identity())
        .unwrap()
        .unwrap();
    let active = state.trace().active_attempt().unwrap();
    let fresh_attempt_id = AttemptId::new("00000000-0000-4000-8000-000000103403").unwrap();
    let document = canonicalize_json_v1(&json!({
        "command": "session.decide",
        "execution_version": 9,
        "fresh_attempt_id": fresh_attempt_id,
        "payload": {
            "actor": "V2GOL-003 legacy reviewer",
            "option_id": "achieved",
            "reason": "Replay the historically admitted assessment decision.",
        },
        "preconditions": {
            "attempt_id": active.attempt_id(),
            "session_id": state.trace().session_id(),
            "session_revision": state.trace().revision(),
        },
        "selector": fixture.selector,
        "workspace_id": binding.identity().workspace_uuid(),
    }))
    .unwrap();
    let canonical_identity = canonical_procedure_v2_mutation_identity_v1(
        typed_request,
        binding.identity().workspace_uuid(),
    )
    .unwrap();
    let request_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_identity.as_bytes())
    ))
    .unwrap();
    let store_key = StoreIdempotencyKeyV1::new("v2gol003-legacy-v9-decision").unwrap();
    let response_context = PersistedResponseContextV1::new(
        "00000000-0000-4000-8000-000000103402",
        "session.decide",
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
    let durable = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::SessionDecide,
        store_key,
        JobId::new("00000000-0000-4000-8000-000000103404").unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(active.attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
        request_digest,
        now,
        CanonicalExecutionJsonV1::new(document).unwrap(),
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(response_context);
    assert!(matches!(
        store.admit(binding.identity(), durable),
        Ok(AdmitOutcomeV1::New(_))
    ));
    let engine = DaemonExecutionEngineV1::new(
        Arc::clone(&store),
        NativeExecutionIdSourceV1,
        WallUtcExecutionClockV1,
        NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
        NativeArtifactVerifierV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
        NativeWorkspaceRevalidatorV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
    );
    let terminal = engine
        .execute_next_with_graph_v2(
            &binding,
            WorkerIdV1::new("v2gol003-legacy-v9-worker").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        terminal.result(),
        &TerminalResultV1::Failure(DomainError::InvalidState {
            reason: "Procedure v2 graph mutation failed",
        })
    );
    let persisted = store
        .read_job(binding.identity(), terminal.job().job_id())
        .unwrap()
        .unwrap();
    assert!(matches!(
        persisted
            .terminal_receipt()
            .unwrap()
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(PersistedGraphTerminalOperationV2::Failure {
            error: PersistedGraphMutationFailureV2::GoalAssessmentDecisionRequiresAssessment {
                graph_node_id,
            },
        }) if graph_node_id == &GraphNodeId::new("decide").unwrap()
    ));
    let unchanged = store
        .read_graph_session_v2(binding.identity())
        .unwrap()
        .unwrap();
    assert_eq!(unchanged, state);
    drop(engine);
    drop(store);

    let first_manager = Arc::new(runtime::manager(fixture.workspace.temporary_path()));
    let first_replay = runtime::dispatcher(first_manager, "v2gol003-legacy-v9-replay-one");
    let first = runtime::dispatch_after_cold_reopen(&first_replay, &legacy_request);
    error(first.clone(), "REQUEST_INVALID");
    drop(first_replay);
    let second_manager = Arc::new(runtime::manager(fixture.workspace.temporary_path()));
    let second_replay = runtime::dispatcher(second_manager, "v2gol003-legacy-v9-replay-two");
    let second = runtime::dispatch_after_cold_reopen(&second_replay, &legacy_request);
    assert_eq!(
        runtime::without_request_id(&second),
        runtime::without_request_id(&first),
        "the queued V9 assessment failure must remain sealed after cold reopen"
    );
}

#[test]
fn v2gol003_fresh_assessment_allows_terminal_complete_and_skip_after_restart() {
    for (index, transition) in ["session.complete", "session.skip"].into_iter().enumerate() {
        let worker_id = format!("v2gol003-terminal-{index}");
        let (fixture, dispatcher) = goal_fixture(&worker_id);
        let (decide, decided, _) = complete_assessment(
            &dispatcher,
            &fixture,
            ["satisfied", "unsatisfied"],
            "not-achieved",
            "not_achieved",
        );
        drop(dispatcher);
        let manager = Arc::new(runtime::manager(fixture.workspace.temporary_path()));
        let restart_worker_id = format!("v2gol003-restart-{index}");
        let restarted = runtime::dispatcher(manager, &restart_worker_id);
        assert_eq!(
            runtime::without_request_id(&runtime::dispatch_after_cold_reopen(&restarted, &decide)),
            runtime::without_request_id(&decided)
        );
        let at_terminal =
            runtime::status(&restarted, &fixture.selector, 103_301, &fixture.session_id);
        let payload = if transition == "session.skip" {
            json!({"reason": "The terminal placement is intentionally skipped."})
                .as_object()
                .unwrap()
                .clone()
        } else {
            Map::new()
        };
        let terminal_key = format!("v2gol003-terminal-{index}");
        let terminal = runtime::request(
            103_302,
            transition,
            &fixture.selector,
            payload,
            &terminal_key,
            runtime::session_preconditions(&at_terminal),
        );
        let result = runtime::v2_result(runtime::dispatch(&restarted, &terminal), transition);
        assert_eq!(result["session_state"], "completed");
        let completed =
            runtime::status(&restarted, &fixture.selector, 103_303, &fixture.session_id);
        assert_eq!(completed["session"]["lifecycle"], "completed");
        assert!(completed["current"].is_null());
    }
}
