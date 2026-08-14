//! Production acceptance closure for cross-task V2GOL invariants.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{GoalRevisionNumberV2, Revision, SessionId};
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1, WorktreeSelectorWireV1,
};
use serde_json::{Map, Value, json};

const GOAL_PROCEDURE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/equivalent-procedure.yaml");

const GOAL_FREE_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol-epic-goal-free
version: "1"
name: Goal-free acceptance fixture
purpose: Prove that goal mutations remain inapplicable without explicit opt-in.
node_definitions:
  work:
    type: action
    title: Work
    intent: Keep one running goal-free cursor.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
"#;

struct Fixture {
    workspace: support_phase4_workspace::GitWorktreeFixtureV1,
    selector: WorktreeSelectorWireV1,
    digest: podway_core::Sha256Digest,
    manager: Arc<podway_daemon::runtime_workspace::WorkspaceRuntimeManagerV1>,
}

fn fixture(procedure: &str, file: &str) -> Fixture {
    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(workspace.main().join(file), procedure).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(procedure.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
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
    selector: &WorktreeSelectorWireV1,
    number: u64,
) {
    let request = runtime::request(
        number,
        "workspace.init",
        selector,
        Map::new(),
        &format!("v2gol-epic-init-{number}"),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &request),
        ResponseEnvelopeV2::OutputV2(_)
    ));
}

fn typed_mutation(
    number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
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
        client: ClientInfoV1::new("v2gol-epic-acceptance", "1", 1).unwrap(),
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

fn start(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &Fixture,
    number: u64,
    file: &str,
    initial_goal: bool,
) -> String {
    let mut payload = json!({
        "procedure": file,
        "expected_procedure_digest": fixture.digest,
        "task_title": "V2GOL epic acceptance"
    })
    .as_object()
    .unwrap()
    .clone();
    if initial_goal {
        payload.extend(
            json!({
                "goal": "Preserve running revision semantics.",
                "criteria": [{
                    "criterion_id": "verified",
                    "statement": "The running revision remains an ordinary rework."
                }],
                "actor": "V2GOL epic acceptance"
            })
            .as_object()
            .unwrap()
            .clone(),
        );
    }
    let request = if initial_goal {
        typed_mutation(
            number,
            "session.start",
            &fixture.selector,
            payload,
            &format!("v2gol-epic-start-{number}"),
            PreconditionsV1::default(),
        )
    } else {
        runtime::request(
            number,
            "session.start",
            &fixture.selector,
            payload,
            &format!("v2gol-epic-start-{number}"),
            PreconditionsV1::default(),
        )
    };
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.start")["session_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn verbose_status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: u64,
) -> Map<String, Value> {
    let request = runtime::request(
        number,
        "session.status",
        selector,
        json!({"verbose": true}).as_object().unwrap().clone(),
        "unused-v2gol-epic-status",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.status")
}

fn session_state(status: &Map<String, Value>) -> Value {
    let mut value = status.clone();
    value.remove("queue");
    Value::Object(value)
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

fn goal_preconditions(status: &Map<String, Value>) -> PreconditionsV1 {
    runtime::session_preconditions(status)
        .with_goal_revision(GoalRevisionNumberV2::FIRST)
        .unwrap()
}

fn assert_goal_tracking_disabled(response: &ResponseEnvelopeV2) {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("a goal mutation without opt-in must fail")
    };
    assert_eq!(error.code().as_str(), "GOAL_TRACKING_NOT_ENABLED");
    assert_eq!(error.details()["admission"]["admitted"], true);
    assert!(error.details()["admission"]["job_id"].is_string());
    assert!(error.details()["admission"]["workspace_sequence"].is_number());
}

#[test]
fn v2gol_epic_goal_mutations_without_opt_in_fail_atomically_and_replay_after_restart() {
    let fixture = fixture(GOAL_FREE_PROCEDURE, "goal-free.yaml");
    let production = runtime::dispatcher(Arc::clone(&fixture.manager), "v2gol-epic-goal-free");
    initialize(&production, &fixture.selector, 108_001);
    let session_id = start(&production, &fixture, 108_002, "goal-free.yaml", false);
    let baseline = verbose_status(&production, &fixture.selector, &session_id, 108_003);

    let requests = [
        typed_mutation(
            108_010,
            "goal.define",
            &fixture.selector,
            json!({
                "goal": "This goal must remain absent.",
                "criteria": [{"criterion_id": "verified", "statement": "Opt-in is required."}]
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2gol-epic-goal-free-define",
            session_identity_preconditions(&baseline),
        ),
        typed_mutation(
            108_020,
            "goal.revise",
            &fixture.selector,
            json!({
                "goal": "This revision must remain absent.",
                "criteria": [{"criterion_id": "verified", "statement": "Opt-in is required."}],
                "target_graph_node_id": "work",
                "reason": "Exercise the opt-in guard.",
                "reactivate": false
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2gol-epic-goal-free-revise",
            goal_preconditions(&baseline),
        ),
        typed_mutation(
            108_030,
            "goal.assess_criterion",
            &fixture.selector,
            json!({
                "criterion_id": "verified",
                "status": "satisfied",
                "reason": "Exercise the opt-in guard.",
                "evidence": [],
                "items": []
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2gol-epic-goal-free-assess",
            goal_preconditions(&baseline),
        ),
    ];

    let mut sealed = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        assert!(matches!(request.1, DaemonRequestV1::ProcedureV2Mutation(_)));
        let response = runtime::dispatch(&production, request);
        assert_goal_tracking_disabled(&response);
        assert_eq!(
            runtime::without_request_id(&runtime::dispatch(&production, request)),
            runtime::without_request_id(&response),
            "the admitted goal-free failure must replay exactly"
        );
        let after = verbose_status(
            &production,
            &fixture.selector,
            &session_id,
            108_011 + index as u64 * 10,
        );
        assert_eq!(
            session_state(&after),
            session_state(&baseline),
            "a rejected goal mutation changed session state"
        );
        sealed.push(response);
    }

    drop(production);
    drop(fixture.manager);
    let reopened_manager = Arc::new(runtime::manager(fixture.workspace.temporary_path()));
    let reopened = runtime::dispatcher(reopened_manager, "v2gol-epic-goal-free-reopen");
    for (request, response) in requests.iter().zip(&sealed) {
        assert_eq!(
            runtime::without_request_id(&runtime::dispatch(&reopened, request)),
            runtime::without_request_id(response),
            "the goal-free failure receipt changed after cold reopen"
        );
    }
    let cold = verbose_status(&reopened, &fixture.selector, &session_id, 108_099);
    assert_eq!(session_state(&cold), session_state(&baseline));
}

fn running_revision_projection(status: &Map<String, Value>) -> Value {
    let latest_goal_outcome = status
        .get("latest_goal_outcome")
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "session_lifecycle": status["session"]["lifecycle"],
        "session_revision": status["session"]["revision"],
        "current_node": status["current"]["node"],
        "current_attempt_number": status["current"]["attempt"]["attempt_number"],
        "current_readiness": status["current"]["readiness"],
        "goal_revision": status["goal_revision"],
        "goal": status["goal"],
        "latest_goal_outcome": latest_goal_outcome,
        "trace_length": status["trace_length"],
        "counters": status["counters"],
        "allowed_manual_rework_targets": status["allowed_manual_rework_targets"],
    })
}

fn revise_running(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &Fixture,
    session_id: &str,
    number: u64,
    reactivate: bool,
) -> (
    (podway_protocol::RequestEnvelopeV1, DaemonRequestV1),
    ResponseEnvelopeV2,
    Map<String, Value>,
) {
    let before = verbose_status(dispatcher, &fixture.selector, session_id, number);
    let request = typed_mutation(
        number + 1,
        "goal.revise",
        &fixture.selector,
        json!({
            "goal": "Preserve equivalent running revision semantics.",
            "criteria": [{
                "criterion_id": "verified",
                "statement": "The running revision remains an ordinary rework."
            }],
            "target_graph_node_id": "perform",
            "reason": "Compare the running reactivate flag branches.",
            "actor": "V2GOL epic acceptance",
            "reactivate": reactivate
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2gol-epic-running-reactivate-{reactivate}"),
        goal_preconditions(&before),
    );
    let response = runtime::dispatch(dispatcher, &request);
    let result = runtime::v2_result(response.clone(), "goal.revise");
    assert_eq!(result["goal_revision"], 2);
    assert_eq!(result["rework_to"], "perform");
    assert_eq!(result["reactivated"], false);
    let after = verbose_status(dispatcher, &fixture.selector, session_id, number + 2);
    assert_eq!(after["session"]["lifecycle"], "running");
    (request, response, after)
}

#[test]
fn v2gol_epic_running_reactivate_true_is_inert_and_cold_replayable() {
    let true_fixture = fixture(GOAL_PROCEDURE, "goal.yaml");
    let true_dispatcher = runtime::dispatcher(
        Arc::clone(&true_fixture.manager),
        "v2gol-epic-reactivate-true",
    );
    initialize(&true_dispatcher, &true_fixture.selector, 109_001);
    let true_session = start(&true_dispatcher, &true_fixture, 109_002, "goal.yaml", true);

    let false_fixture = fixture(GOAL_PROCEDURE, "goal.yaml");
    let false_dispatcher = runtime::dispatcher(
        Arc::clone(&false_fixture.manager),
        "v2gol-epic-reactivate-false",
    );
    initialize(&false_dispatcher, &false_fixture.selector, 109_101);
    let false_session = start(
        &false_dispatcher,
        &false_fixture,
        109_102,
        "goal.yaml",
        true,
    );

    let (true_request, true_response, true_after) = revise_running(
        &true_dispatcher,
        &true_fixture,
        &true_session,
        109_010,
        true,
    );
    let (_, _, false_after) = revise_running(
        &false_dispatcher,
        &false_fixture,
        &false_session,
        109_110,
        false,
    );
    assert_eq!(
        running_revision_projection(&true_after),
        running_revision_projection(&false_after),
        "reactivate=true changed ordinary running revision semantics"
    );

    let warm_projection = running_revision_projection(&true_after);
    drop(true_dispatcher);
    drop(true_fixture.manager);
    let reopened_manager = Arc::new(runtime::manager(true_fixture.workspace.temporary_path()));
    let reopened = runtime::dispatcher(reopened_manager, "v2gol-epic-reactivate-true-reopen");
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch(&reopened, &true_request)),
        runtime::without_request_id(&true_response),
        "the running reactivate=true result changed after cold reopen"
    );
    let cold = verbose_status(&reopened, &true_fixture.selector, &true_session, 109_099);
    assert_eq!(running_revision_projection(&cold), warm_projection);
}
