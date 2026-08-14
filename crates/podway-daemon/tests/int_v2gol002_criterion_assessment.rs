//! Production vertical coverage for V2GOL-002 criterion assessment.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AttemptId, GoalRevisionNumberV2, Revision, SessionId, UnixMillis};
use podway_daemon::runtime_workspace::WorkspaceRuntimeObservationV1;
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    Rfc3339MillisV1, WorkspaceContextV1,
};
use podway_store::{SqliteStoreV1, StoreGraphStateContractV2};
use serde_json::{Map, Value, json};

const ASSESSMENT_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol002-assessment
version: "2"
name: Criterion assessment
purpose: Exercise durable criterion assessment.
description: A focused V2GOL-002 runtime fixture.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Produce evidence
    intent: Record evidence for assessment.
    instructions:
      - Record the evidence value.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        max_length: 1000
  assess:
    type: decision
    title: Assess the goal
    objective: Record every criterion result.
    prompt: Which goal outcome applies?
    items:
      - id: assessment-note
        type: text
        prompt: Record a local assessment note.
        required: false
        max_length: 1000
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
      prompt: Explain the assessment.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Finish the workflow.
    instructions:
      - Finish outside Podway.
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
manual_rework:
  allowed_targets:
    - perform
"#;

struct Fixture {
    workspace: support_phase4_workspace::GitWorktreeFixtureV1,
    selector: podway_protocol::WorktreeSelectorWireV1,
    manager: Arc<podway_daemon::runtime_workspace::WorkspaceRuntimeManagerV1>,
    session_id: String,
}

fn initialize(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
) {
    let request = runtime::request(
        102_001,
        "workspace.init",
        selector,
        Map::new(),
        "v2gol002-init",
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
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000102002").unwrap(),
        client: ClientInfoV1::new("v2gol002-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.start").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new("v2gol002-start").unwrap()),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, runtime::TEST_WAIT_TIMEOUT_MILLIS).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Start(_)));
    (envelope, daemon)
}

fn fixture(worker_id: &str) -> (Fixture, impl RequestDispatcherV1) {
    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(
        workspace.main().join("assessment.yaml"),
        ASSESSMENT_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ASSESSMENT_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = runtime::selector(workspace.main());
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), worker_id);
    initialize(&dispatcher, &selector);

    let start = goal_start_request(
        &selector,
        json!({
            "procedure": "assessment.yaml",
            "expected_procedure_digest": digest,
            "task_title": "Assess the goal",
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
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();

    runtime::mutate_item(
        &dispatcher,
        &selector,
        102_010,
        &session_id,
        "item.set",
        "result",
        json!({"value": "focused assessment evidence"})
            .as_object()
            .unwrap()
            .clone(),
        "v2gol002-set-source",
    );
    let before_complete = runtime::status(&dispatcher, &selector, 102_012, &session_id);
    let complete = runtime::request(
        102_013,
        "session.complete",
        &selector,
        Map::new(),
        "v2gol002-complete-source",
        runtime::session_preconditions(&before_complete),
    );
    runtime::v2_result(
        runtime::dispatch(&dispatcher, &complete),
        "session.complete",
    );
    let at_decision = runtime::status(&dispatcher, &selector, 102_014, &session_id);
    assert_eq!(at_decision["current"]["node"]["graph_node_id"], "decide");

    (
        Fixture {
            workspace,
            selector,
            manager,
            session_id,
        },
        dispatcher,
    )
}

fn assessment_preconditions(status: &Map<String, Value>) -> PreconditionsV1 {
    runtime::session_preconditions(status)
        .with_goal_revision(GoalRevisionNumberV2::FIRST)
        .unwrap()
}

fn goal_mutation_request(
    number: u64,
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
        client: ClientInfoV1::new("v2gol002-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("goal.assess_criterion").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, runtime::TEST_WAIT_TIMEOUT_MILLIS).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    (envelope, daemon)
}

#[allow(clippy::too_many_arguments)]
fn assessment_request(
    number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    status: &Map<String, Value>,
    criterion_id: &str,
    criterion_status: &str,
    reason: &str,
    evidence: Vec<&str>,
    items: Vec<&str>,
    actor: Option<&str>,
    key: &str,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let mut payload = json!({
        "criterion_id": criterion_id,
        "status": criterion_status,
        "reason": reason,
        "evidence": evidence,
        "items": items,
    })
    .as_object()
    .expect("assessment payload is an object")
    .clone();
    if let Some(actor) = actor {
        payload.insert("actor".to_owned(), json!(actor));
    }
    goal_mutation_request(
        number,
        selector,
        payload,
        key,
        assessment_preconditions(status),
    )
}

fn error_value(response: ResponseEnvelopeV2, expected_code: &str) -> Value {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{expected_code} must be returned as a public error")
    };
    assert_eq!(error.code().as_str(), expected_code);
    serde_json::to_value(error).unwrap()
}

fn goal_projection(status: &Map<String, Value>) -> Value {
    json!({
        "revision": status["session"]["revision"],
        "current": status["current"],
        "goal": status["goal"],
    })
}

#[test]
fn v2gol002_assessment_mode_records_citations_attribution_replay_and_restart() {
    let (fixture, dispatcher) = fixture("v2gol002-assessment");
    runtime::mutate_item(
        &dispatcher,
        &fixture.selector,
        102_020,
        &fixture.session_id,
        "item.set",
        "assessment-note",
        json!({"value": "decision-local note"})
            .as_object()
            .unwrap()
            .clone(),
        "v2gol002-set-local",
    );
    let before_first =
        runtime::status(&dispatcher, &fixture.selector, 102_022, &fixture.session_id);
    let first = assessment_request(
        102_023,
        &fixture.selector,
        &before_first,
        "correct",
        "satisfied",
        "The source and local note support correctness.",
        vec!["perform"],
        vec!["assessment-note"],
        Some("reviewer"),
        "v2gol002-correct",
    );
    let first_response = runtime::dispatch(&dispatcher, &first);
    let first_result = runtime::v2_result(first_response.clone(), "goal.assess_criterion");
    assert_eq!(
        first_result["schema"],
        "podway.criterion-assessment-result/v1"
    );
    assert_eq!(first_result["graph_node_id"], "decide");
    assert_eq!(
        first_result["attempt_id"],
        before_first["current"]["attempt"]["attempt_id"]
    );
    assert_eq!(first_result["goal_revision"], 1);
    assert_eq!(first_result["mode"], "assessment");
    assert_eq!(first_result["result"]["criterion_id"], "correct");
    assert_eq!(first_result["result"]["status"], "satisfied");
    assert_eq!(
        first_result["result"]["reason"],
        "The source and local note support correctness."
    );
    assert_eq!(
        first_result["result"]["citations"],
        json!([
            {"reference_graph_node_id": "perform"},
            {"local_item_id": "assessment-note"}
        ])
    );
    assert_eq!(first_result["complete"], false);
    assert!(first_result.get("determined_outcome").is_none());
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch(&dispatcher, &first)),
        runtime::without_request_id(&first_response),
        "a criterion assessment must replay its sealed durable result"
    );

    let before_second =
        runtime::status(&dispatcher, &fixture.selector, 102_024, &fixture.session_id);
    assert_eq!(
        before_second["current"]["attempt"]["attempt_id"],
        before_first["current"]["attempt"]["attempt_id"],
        "assessment must not move the graph cursor"
    );
    let second = assessment_request(
        102_025,
        &fixture.selector,
        &before_second,
        "tested",
        "unsatisfied",
        "The recorded run does not cover the final edge case.",
        vec![],
        vec![],
        None,
        "v2gol002-tested",
    );
    let second_response = runtime::dispatch(&dispatcher, &second);
    let second_result = runtime::v2_result(second_response.clone(), "goal.assess_criterion");
    assert_eq!(second_result["mode"], "assessment");
    assert_eq!(second_result["complete"], true);
    assert_eq!(second_result["determined_outcome"], "not_achieved");
    assert_eq!(second_result["result"]["status"], "unsatisfied");

    drop(dispatcher);
    let restarted = runtime::dispatcher(Arc::clone(&fixture.manager), "v2gol002-restart");
    let status_request = runtime::request(
        102_026,
        "session.status",
        &fixture.selector,
        Map::new(),
        "unused-v2gol002-restart-status-key",
        PreconditionsV1::new(
            Some(SessionId::new(&fixture.session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let after_restart = runtime::v2_result(
        runtime::dispatch_after_cold_reopen(&restarted, &status_request),
        "session.status",
    );
    assert_eq!(
        after_restart["current"]["attempt"]["attempt_id"],
        before_first["current"]["attempt"]["attempt_id"]
    );
    assert_eq!(after_restart["goal"]["determined_outcome"], "not_achieved");
    assert_eq!(
        after_restart["goal"]["criteria"],
        json!([
            {"criterion_id": "correct", "statement": "The result is correct.", "status": "satisfied"},
            {"criterion_id": "tested", "statement": "The result is tested.", "status": "unsatisfied"}
        ])
    );
    assert_eq!(
        runtime::without_request_id(&runtime::dispatch_after_cold_reopen(&restarted, &second)),
        runtime::without_request_id(&second_response),
        "the final criterion result must replay after a cold reopen"
    );

    let scheduler = fixture
        .manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.workspace.main()),
            None,
            WorkspaceRuntimeObservationV1::new(
                UnixMillis::new(1_700_000_100_000),
                Rfc3339MillisV1::new("2026-08-11T00:00:00.000Z").unwrap(),
            ),
        )
        .unwrap();
    let context = scheduler.context_snapshot();
    let store = SqliteStoreV1::open(
        context.database_path(),
        context.workspace_root(),
        context.binding().identity().clone(),
        context.store_options().clone(),
        UnixMillis::new(1_700_000_100_001),
    )
    .unwrap();
    let persisted = store
        .read_graph_session_v2(context.binding().identity())
        .unwrap()
        .unwrap();
    let results = persisted.goal_state().attempt_assessments()[0].results();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].actor().unwrap().as_str(), "reviewer");
    assert_eq!(results[1].actor(), None);
}

#[test]
fn v2gol002_applicability_mode_accepts_only_uncited_not_applicable_results() {
    let (fixture, dispatcher) = fixture("v2gol002-applicability");
    let before_first =
        runtime::status(&dispatcher, &fixture.selector, 102_030, &fixture.session_id);
    let first = assessment_request(
        102_031,
        &fixture.selector,
        &before_first,
        "correct",
        "not_applicable",
        "The goal is being superseded.",
        vec![],
        vec![],
        Some("goal owner"),
        "v2gol002-na-correct",
    );
    let first = runtime::v2_result(
        runtime::dispatch(&dispatcher, &first),
        "goal.assess_criterion",
    );
    assert_eq!(first["mode"], "applicability");
    assert_eq!(first["result"]["citations"], json!([]));
    assert_eq!(first["complete"], false);

    let before_second =
        runtime::status(&dispatcher, &fixture.selector, 102_032, &fixture.session_id);
    let second = assessment_request(
        102_033,
        &fixture.selector,
        &before_second,
        "tested",
        "not_applicable",
        "The superseding goal replaces this criterion too.",
        vec![],
        vec![],
        Some("goal owner"),
        "v2gol002-na-tested",
    );
    let second = runtime::v2_result(
        runtime::dispatch(&dispatcher, &second),
        "goal.assess_criterion",
    );
    assert_eq!(second["mode"], "applicability");
    assert_eq!(second["complete"], true);
    assert_eq!(second["determined_outcome"], "superseded");
}

#[test]
fn v2gol002_invalid_runtime_assessments_are_atomic() {
    let (fixture, dispatcher) = fixture("v2gol002-invalid");
    let before = runtime::status(&dispatcher, &fixture.selector, 102_040, &fixture.session_id);
    let valid = assessment_request(
        102_041,
        &fixture.selector,
        &before,
        "correct",
        "satisfied",
        "The evidence supports correctness.",
        vec!["perform"],
        vec![],
        Some("reviewer"),
        "v2gol002-valid-first",
    );
    runtime::v2_result(
        runtime::dispatch(&dispatcher, &valid),
        "goal.assess_criterion",
    );
    let baseline = runtime::status(&dispatcher, &fixture.selector, 102_042, &fixture.session_id);
    let baseline_projection = goal_projection(&baseline);

    let cases = [
        (
            "mixed",
            "tested",
            "not_applicable",
            vec![],
            vec![],
            assessment_preconditions(&baseline),
            "CRITERION_MODE_MIXED",
        ),
        (
            "duplicate-result",
            "correct",
            "unsatisfied",
            vec![],
            vec![],
            assessment_preconditions(&baseline),
            "REQUEST_INVALID",
        ),
        (
            "unknown-criterion",
            "missing",
            "satisfied",
            vec![],
            vec![],
            assessment_preconditions(&baseline),
            "CRITERION_NOT_FOUND",
        ),
        (
            "unknown-evidence",
            "tested",
            "satisfied",
            vec!["finish"],
            vec![],
            assessment_preconditions(&baseline),
            "CRITERION_CITATION_INVALID",
        ),
        (
            "unknown-item",
            "tested",
            "satisfied",
            vec![],
            vec!["result"],
            assessment_preconditions(&baseline),
            "CRITERION_CITATION_INVALID",
        ),
        (
            "stale-goal",
            "tested",
            "satisfied",
            vec![],
            vec![],
            runtime::session_preconditions(&baseline)
                .with_goal_revision(GoalRevisionNumberV2::new(2))
                .unwrap(),
            "GOAL_REVISION_STALE",
        ),
        (
            "stale-session",
            "tested",
            "satisfied",
            vec![],
            vec![],
            PreconditionsV1::new(
                Some(SessionId::new(fixture.session_id.clone()).unwrap()),
                Some(Revision::new(
                    baseline["session"]["revision"].as_u64().unwrap() - 1,
                )),
                Some(
                    AttemptId::new(
                        baseline["current"]["attempt"]["attempt_id"]
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
            .with_goal_revision(GoalRevisionNumberV2::FIRST)
            .unwrap(),
            "SESSION_REVISION_CONFLICT",
        ),
        (
            "wrong-attempt",
            "tested",
            "satisfied",
            vec![],
            vec![],
            PreconditionsV1::new(
                Some(SessionId::new(fixture.session_id.clone()).unwrap()),
                Some(Revision::new(
                    baseline["session"]["revision"].as_u64().unwrap(),
                )),
                Some(AttemptId::new("00000000-0000-4000-8000-000000102099").unwrap()),
                None,
                None,
                None,
            )
            .unwrap()
            .with_goal_revision(GoalRevisionNumberV2::FIRST)
            .unwrap(),
            "ATTEMPT_NOT_CURRENT",
        ),
    ];

    for (offset, (label, criterion, status, evidence, items, preconditions, code)) in
        cases.into_iter().enumerate()
    {
        let request = goal_mutation_request(
            102_050 + offset as u64 * 2,
            &fixture.selector,
            json!({
                "criterion_id": criterion,
                "status": status,
                "reason": format!("Reject {label} atomically."),
                "evidence": evidence,
                "items": items,
                "actor": "reviewer"
            })
            .as_object()
            .unwrap()
            .clone(),
            &format!("v2gol002-{label}"),
            preconditions,
        );
        error_value(runtime::dispatch(&dispatcher, &request), code);
        let after = runtime::status(
            &dispatcher,
            &fixture.selector,
            102_051 + offset as u64 * 2,
            &fixture.session_id,
        );
        assert_eq!(
            goal_projection(&after),
            baseline_projection,
            "{label} changed durable state"
        );
    }
}

#[test]
fn v2gol002_duplicate_citations_fail_during_closed_request_decode_without_mutation() {
    let (fixture, dispatcher) = fixture("v2gol002-duplicate-citation");
    let before = runtime::status(&dispatcher, &fixture.selector, 102_080, &fixture.session_id);
    let mut payload = json!({
        "criterion_id": "correct",
        "status": "satisfied",
        "reason": "Duplicate citations are invalid.",
        "evidence": ["perform", "perform"],
        "items": [],
        "actor": "reviewer",
    })
    .as_object()
    .unwrap()
    .clone();
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(&fixture.selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000102081").unwrap(),
        client: ClientInfoV1::new("v2gol002-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("goal.assess_criterion").unwrap(),
        workspace: Some(WorkspaceContextV1::new(fixture.selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new("v2gol002-duplicate-citations").unwrap()),
        preconditions: assessment_preconditions(&before),
        options: RequestOptionsV1::new(false, runtime::TEST_WAIT_TIMEOUT_MILLIS).unwrap(),
        payload,
    })
    .unwrap();
    assert!(DaemonRequestV1::from_envelope(&envelope).is_err());
    let after = runtime::status(&dispatcher, &fixture.selector, 102_082, &fixture.session_id);
    assert_eq!(goal_projection(&after), goal_projection(&before));
}
