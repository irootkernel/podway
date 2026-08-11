//! Production read-back coverage for V2GOL-004 goal revision and assessment history.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{
    fs,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{GoalRevisionNumberV2, Revision, SessionId};
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1, WorktreeSelectorWireV1, encode_response_payload_v2,
};
use serde_json::{Map, Value, json};

const PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol004-readback
version: "2"
name: Goal history read-back
purpose: Exercise current and stale goal history through production dispatch.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Produce evidence
    intent: Record evidence for the current goal revision.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        max_length: 1000
  assess:
    type: decision
    title: Assess the goal
    objective: Select the outcome determined by the current criteria.
    prompt: Which goal outcome applies?
    items:
      - id: note-one
        type: text
        prompt: Record the first local citation target.
        required: false
        max_length: 100
      - id: note-two
        type: text
        prompt: Record the second local citation target.
        required: false
        max_length: 100
      - id: note-three
        type: text
        prompt: Record the third local citation target.
        required: false
        max_length: 100
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
    intent: Read the assessment back before terminal completion.
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
      evidence_from:
        - node: decide
          required: true
      terminal: true
manual_rework:
  allowed_targets:
    - perform
"#;

const HISTORY_KEYS: [&str; 6] = [
    "current_trace_history",
    "stale_attempt_history",
    "decision_history",
    "rework_history",
    "stale_goal_revision_history",
    "stale_goal_assessment_history",
];

fn next_number(number: &mut u64) -> u64 {
    let current = *number;
    *number += 1;
    current
}

fn mutation_request(
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
        client: ClientInfoV1::new("v2gol004-test", "1", 1).unwrap(),
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
    (envelope, daemon)
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
            ResponseEnvelopeV2::Error(error) if error.code().as_str() == "WORKSPACE_MAINTENANCE"
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

fn query_request(
    number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    payload: Map<String, Value>,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    runtime::request(
        number,
        command,
        selector,
        payload,
        "unused-v2gol004-query-key",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    )
}

fn query(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    command: &str,
    payload: Map<String, Value>,
) -> Map<String, Value> {
    let request = query_request(next_number(number), command, selector, session_id, payload);
    runtime::v2_result(runtime::dispatch(dispatcher, &request), command)
}

fn cold_query(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    command: &str,
    payload: Map<String, Value>,
) -> Map<String, Value> {
    let request = query_request(next_number(number), command, selector, session_id, payload);
    runtime::v2_result(dispatch_after_cold_reopen(dispatcher, &request), command)
}

fn status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
) -> Map<String, Value> {
    query(
        dispatcher,
        selector,
        session_id,
        number,
        "session.status",
        Map::new(),
    )
}

fn compact_status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
) -> Map<String, Value> {
    query(
        dispatcher,
        selector,
        session_id,
        number,
        "session.status",
        json!({"compact": true, "wait_for_idle": true})
            .as_object()
            .unwrap()
            .clone(),
    )
}

fn verbose_status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    history_before: Option<u64>,
) -> Map<String, Value> {
    let mut payload = json!({"verbose": true}).as_object().unwrap().clone();
    if let Some(cursor) = history_before {
        payload.insert("history_before".to_owned(), json!(cursor));
    }
    let status = query(
        dispatcher,
        selector,
        session_id,
        number,
        "session.status",
        payload,
    );
    for history in HISTORY_KEYS {
        let entries = status[history]["entries"].as_array().unwrap();
        assert!(serde_json::to_vec(&status[history]).unwrap().len() <= 65_536);
        assert!(entries.windows(2).all(|pair| {
            pair[0]["trace_sequence"].as_u64().unwrap()
                > pair[1]["trace_sequence"].as_u64().unwrap()
        }));
        if let Some(cursor) = history_before {
            assert!(
                entries
                    .iter()
                    .all(|entry| entry["trace_sequence"].as_u64().unwrap() < cursor)
            );
        }
    }
    status
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

fn goal_preconditions(status: &Map<String, Value>, revision: u64) -> PreconditionsV1 {
    runtime::session_preconditions(status)
        .with_goal_revision(GoalRevisionNumberV2::new(revision))
        .unwrap()
}

fn set_result(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    revision: u64,
) {
    let request_number = next_number(number);
    runtime::mutate_item(
        dispatcher,
        selector,
        request_number,
        session_id,
        "item.set",
        "result",
        json!({"value": format!("evidence for goal revision {revision}")})
            .as_object()
            .unwrap()
            .clone(),
        &format!("v2gol004-set-{revision}"),
    );
}

fn complete_action(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    key: &str,
) -> Map<String, Value> {
    let before = status(dispatcher, selector, session_id, number);
    let request = runtime::request(
        next_number(number),
        "session.complete",
        selector,
        Map::new(),
        key,
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.complete")
}

fn assess(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    revision: u64,
    criterion_id: &str,
    criterion_status: &str,
) {
    let before = status(dispatcher, selector, session_id, number);
    let citations = if criterion_status == "not_applicable" {
        Vec::new()
    } else {
        vec![json!("perform")]
    };
    let request_number = next_number(number);
    let request = mutation_request(
        request_number,
        "goal.assess_criterion",
        selector,
        json!({
            "criterion_id": criterion_id,
            "status": criterion_status,
            "reason": format!("revision {revision} {criterion_id} is {criterion_status}"),
            "evidence": citations,
            "items": [],
            "actor": format!("revision {revision} assessor")
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2gol004-assess-{revision}-{criterion_id}"),
        goal_preconditions(&before, revision),
    );
    runtime::v2_result(
        runtime::dispatch(dispatcher, &request),
        "goal.assess_criterion",
    );
}

fn decide(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    revision: u64,
    option_id: &str,
) -> Value {
    let before = status(dispatcher, selector, session_id, number);
    let request_number = next_number(number);
    let request = mutation_request(
        request_number,
        "session.decide",
        selector,
        json!({
            "option_id": option_id,
            "reason": format!("select the recorded outcome for revision {revision}"),
            "actor": format!("revision {revision} decider")
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2gol004-decide-{revision}"),
        goal_preconditions(&before, revision),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.decide")["record"].clone()
}

fn revise(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    prior_revision: u64,
    revision: u64,
) {
    let before = status(dispatcher, selector, session_id, number);
    let request_number = next_number(number);
    let request = mutation_request(
        request_number,
        "goal.revise",
        selector,
        json!({
            "goal": format!("Deliver goal revision {revision}."),
            "criteria": [
                {"criterion_id": "correct", "statement": format!("Revision {revision} is correct.")},
                {"criterion_id": "tested", "statement": format!("Revision {revision} is tested.")}
            ],
            "target_graph_node_id": "perform",
            "reason": format!("replace revision {prior_revision} with revision {revision}"),
            "actor": format!("revision {revision} author")
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2gol004-revise-{revision}"),
        goal_preconditions(&before, prior_revision),
    );
    let result = runtime::v2_result(runtime::dispatch(dispatcher, &request), "goal.revise");
    assert_eq!(result["goal_revision"], revision);
    assert_eq!(result["rework_to"], "perform");
}

fn run_revision(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    revision: u64,
    statuses: [&str; 2],
    option_id: &str,
) -> Value {
    set_result(dispatcher, selector, session_id, number, revision);
    complete_action(
        dispatcher,
        selector,
        session_id,
        number,
        &format!("v2gol004-complete-work-{revision}"),
    );
    assess(
        dispatcher,
        selector,
        session_id,
        number,
        revision,
        "correct",
        statuses[0],
    );
    assess(
        dispatcher,
        selector,
        session_id,
        number,
        revision,
        "tested",
        statuses[1],
    );
    decide(
        dispatcher, selector, session_id, number, revision, option_id,
    )
}

fn read_fingerprint(status: &Map<String, Value>) -> Value {
    json!({
        "session": status["session"],
        "current": status["current"],
        "goal_revision": status["goal_revision"],
        "latest_goal_outcome": status["latest_goal_outcome"],
        "trace_length": status["trace_length"],
        "counters": status["counters"],
        "queue": status["queue"],
    })
}

fn assert_no_histories(status: &Map<String, Value>) {
    for history in HISTORY_KEYS {
        assert!(!status.contains_key(history), "status leaked {history}");
    }
}

fn ordinary_decision_record(enriched: &Value) -> Value {
    let mut record = enriched.as_object().unwrap().clone();
    for field in [
        "assessment",
        "assessment_mode",
        "goal_outcome",
        "criterion_results",
    ] {
        record.remove(field);
    }
    Value::Object(record)
}

fn assert_assessment_summary(summary: &Value, decision: &Value, outcome: &str) {
    assert_eq!(summary["trace_sequence"], decision["trace_sequence"]);
    assert_eq!(summary["session_id"], decision["session_id"]);
    assert_eq!(summary["session_revision"], decision["session_revision"]);
    assert_eq!(
        summary["procedure_snapshot_id"],
        decision["procedure_snapshot_id"]
    );
    assert_eq!(summary["procedure_digest"], decision["procedure_digest"]);
    assert_eq!(summary["graph_node_id"], decision["graph_node_id"]);
    assert_eq!(
        summary["node_definition_id"],
        decision["node_definition_id"]
    );
    assert_eq!(summary["attempt_id"], decision["attempt_id"]);
    assert_eq!(summary["attempt_number"], decision["attempt_number"]);
    assert_eq!(summary["goal_revision"], decision["goal_revision"]);
    assert_eq!(summary["option_id"], decision["option_id"]);
    assert_eq!(summary["effect"], decision["effect"]);
    assert_eq!(
        summary["target_graph_node_id"],
        decision["target_graph_node_id"]
    );
    assert_eq!(summary["mode"], decision["assessment_mode"]);
    assert_eq!(summary["outcome"], outcome);
    assert_eq!(
        summary["criterion_statuses"],
        Value::Array(
            decision["criterion_results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|result| {
                    json!({
                        "criterion_id": result["criterion_id"],
                        "status": result["status"],
                        "citations": result["citations"],
                    })
                })
                .collect(),
        )
    );
    assert_eq!(summary["references"], decision["references"]);
    assert_eq!(summary["actor"], decision["actor"]);
    assert_eq!(summary["recorded_at"], decision["recorded_at"]);
    let digest = summary["record_digest"].as_str().unwrap();
    assert_eq!(digest.len(), 71);
    assert!(digest.starts_with("sha256:"));
}

#[test]
fn v2gol004_command_generated_goal_history_is_complete_pageable_and_cold_stable() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("V2GOL-004 fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let diagnostics = vet_procedure_v2(
        &validated,
        &AuthoringContext::new(
            "goal-readback.yaml",
            PROCEDURE,
            ProcedureDocumentFormat::Yaml,
        ),
    );
    assert!(diagnostics.is_empty(), "invalid fixture: {diagnostics:?}");

    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(workspace.main().join("goal-readback.yaml"), PROCEDURE).unwrap();
    let selector = runtime::selector(workspace.main());
    let manager_root = workspace.temporary_path().to_path_buf();
    let manager = Arc::new(runtime::manager(&manager_root));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2gol004-production");
    let mut number = 104_000;

    let initialize = runtime::request(
        next_number(&mut number),
        "workspace.init",
        &selector,
        Map::new(),
        "v2gol004-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(&dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let start = runtime::request(
        next_number(&mut number),
        "session.start",
        &selector,
        json!({
            "procedure": "goal-readback.yaml",
            "expected_procedure_digest": validated.digest(),
            "task_title": "V2GOL-004 production goal history"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol004-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();

    let before_define = status(&dispatcher, &selector, &session_id, &mut number);
    let define = mutation_request(
        next_number(&mut number),
        "goal.define",
        &selector,
        json!({
            "goal": "Deliver goal revision 1.",
            "criteria": [
                {"criterion_id": "correct", "statement": "Revision 1 is correct."},
                {"criterion_id": "tested", "statement": "Revision 1 is tested."}
            ],
            "actor": "revision 1 author"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol004-define",
        session_identity_preconditions(&before_define),
    );
    let defined = runtime::v2_result(runtime::dispatch(&dispatcher, &define), "goal.define");
    assert_eq!(defined["goal_revision"], 1);

    let revision_one = run_revision(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        1,
        ["satisfied", "satisfied"],
        "achieved",
    );
    revise(&dispatcher, &selector, &session_id, &mut number, 1, 2);
    let revision_two = run_revision(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        2,
        ["satisfied", "unsatisfied"],
        "not-achieved",
    );
    revise(&dispatcher, &selector, &session_id, &mut number, 2, 3);
    let revision_three = run_revision(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        3,
        ["not_applicable", "not_applicable"],
        "superseded",
    );

    let standard = status(&dispatcher, &selector, &session_id, &mut number);
    let compact = compact_status(&dispatcher, &selector, &session_id, &mut number);
    assert_no_histories(&standard);
    assert_no_histories(&compact);
    assert_eq!(standard["goal_revision"], 3);
    assert_eq!(standard["latest_goal_outcome"], "superseded");
    assert_eq!(standard["session"]["lifecycle"], "running");
    assert_eq!(
        standard["goal"]["criteria"],
        json!([
            {"criterion_id":"correct", "statement":"Revision 3 is correct.", "status":"not_applicable"},
            {"criterion_id":"tested", "statement":"Revision 3 is tested.", "status":"not_applicable"}
        ])
    );

    let next = query(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        "session.next",
        Map::new(),
    );
    assert_eq!(next["readback"].as_array().unwrap().len(), 1);
    assert_eq!(
        next["readback"][0]["decision_record"], revision_three,
        "next must replay the complete assessment decision record"
    );
    assert_eq!(
        next["readback"][0]["decision_record"]["criterion_results"][0],
        json!({
            "criterion_id":"correct",
            "status":"not_applicable",
            "reason":"revision 3 correct is not_applicable",
            "citations": []
        })
    );

    let verbose = verbose_status(&dispatcher, &selector, &session_id, &mut number, None);
    assert_eq!(
        verbose["decision_history"]["entries"][0],
        ordinary_decision_record(&revision_three)
    );
    for assessment_field in [
        "assessment",
        "assessment_mode",
        "goal_outcome",
        "criterion_results",
    ] {
        assert!(
            verbose["decision_history"]["entries"][0]
                .get(assessment_field)
                .is_none(),
            "decision history leaked assessment-only field {assessment_field}"
        );
    }
    assert_eq!(verbose["decision_history"]["trace_truncated"], true);
    let revision_three_trace = revision_three["trace_sequence"].as_u64().unwrap();
    let prior_decision = verbose_status(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        Some(revision_three_trace),
    );
    assert_eq!(
        prior_decision["decision_history"]["entries"][0],
        ordinary_decision_record(&revision_two)
    );
    assert_eq!(prior_decision["decision_history"]["trace_truncated"], true);
    let revision_two_trace = revision_two["trace_sequence"].as_u64().unwrap();
    let first_decision = verbose_status(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        Some(revision_two_trace),
    );
    assert_eq!(
        first_decision["decision_history"]["entries"][0],
        ordinary_decision_record(&revision_one)
    );
    assert_eq!(first_decision["decision_history"]["trace_truncated"], false);
    let stale_revisions = verbose["stale_goal_revision_history"]["entries"]
        .as_array()
        .unwrap();
    let stale_assessments = verbose["stale_goal_assessment_history"]["entries"]
        .as_array()
        .unwrap();
    assert_eq!(stale_revisions.len(), 1);
    assert_eq!(stale_assessments.len(), 1);
    assert_eq!(
        verbose["stale_goal_revision_history"]["trace_truncated"],
        true
    );
    assert_eq!(
        verbose["stale_goal_assessment_history"]["trace_truncated"],
        true
    );
    assert_eq!(stale_revisions[0]["revision"], 2);
    assert_eq!(
        stale_revisions[0]["criteria"],
        json!([
            {"criterion_id":"correct", "statement":"Revision 2 is correct.", "status":"satisfied"},
            {"criterion_id":"tested", "statement":"Revision 2 is tested.", "status":"unsatisfied"}
        ])
    );
    assert_assessment_summary(&stale_assessments[0], &revision_two, "not_achieved");

    let revision_cursor = stale_revisions[0]["trace_sequence"].as_u64().unwrap();
    let earlier_revision = verbose_status(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        Some(revision_cursor),
    );
    assert_eq!(
        earlier_revision["stale_goal_revision_history"]["entries"][0]["revision"],
        1
    );
    assert_eq!(
        earlier_revision["stale_goal_revision_history"]["entries"][0]["criteria"],
        json!([
            {"criterion_id":"correct", "statement":"Revision 1 is correct.", "status":"satisfied"},
            {"criterion_id":"tested", "statement":"Revision 1 is tested.", "status":"satisfied"}
        ])
    );
    assert_eq!(
        earlier_revision["stale_goal_revision_history"]["trace_truncated"],
        false
    );

    let assessment_cursor = stale_assessments[0]["trace_sequence"].as_u64().unwrap();
    let earlier_assessment = verbose_status(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        Some(assessment_cursor),
    );
    let earlier_assessment = &earlier_assessment["stale_goal_assessment_history"]["entries"][0];
    assert_assessment_summary(earlier_assessment, &revision_one, "achieved");
    assert_ne!(
        earlier_assessment["record_digest"],
        stale_assessments[0]["record_digest"]
    );
    assert!(
        !verbose["stale_goal_assessment_history"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["goal_revision"] == 3)
    );

    let fingerprint = read_fingerprint(&standard);
    let after_reads = status(&dispatcher, &selector, &session_id, &mut number);
    assert_eq!(read_fingerprint(&after_reads), fingerprint);

    drop(dispatcher);
    drop(manager);
    let reopened_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(Arc::clone(&reopened_manager), "v2gol004-reopened");
    assert_eq!(
        cold_query(
            &reopened,
            &selector,
            &session_id,
            &mut number,
            "session.status",
            json!({"verbose": true}).as_object().unwrap().clone(),
        ),
        verbose
    );
    assert_eq!(
        cold_query(
            &reopened,
            &selector,
            &session_id,
            &mut number,
            "session.next",
            Map::new(),
        ),
        next
    );

    let terminal = complete_action(
        &reopened,
        &selector,
        &session_id,
        &mut number,
        "v2gol004-complete-terminal",
    );
    assert_eq!(terminal["session_state"], "completed");
    let completed_standard = status(&reopened, &selector, &session_id, &mut number);
    let completed_verbose = verbose_status(&reopened, &selector, &session_id, &mut number, None);
    assert_eq!(completed_standard["session"]["lifecycle"], "completed");
    assert_eq!(completed_standard["latest_goal_outcome"], "superseded");
    assert_eq!(
        completed_verbose["decision_history"]["entries"][0],
        ordinary_decision_record(&revision_three)
    );

    drop(reopened);
    drop(reopened_manager);
    let final_manager = Arc::new(runtime::manager(&manager_root));
    let final_dispatcher = runtime::dispatcher(final_manager, "v2gol004-final-reopen");
    assert_eq!(
        cold_query(
            &final_dispatcher,
            &selector,
            &session_id,
            &mut number,
            "session.status",
            Map::new(),
        ),
        completed_standard
    );
    assert_eq!(
        cold_query(
            &final_dispatcher,
            &selector,
            &session_id,
            &mut number,
            "session.status",
            json!({"verbose": true}).as_object().unwrap().clone(),
        ),
        completed_verbose
    );
}

#[test]
fn v2gol004_max_escaped_assessment_stays_out_of_history_but_survives_next_readback() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("V2GOL-004 fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let diagnostics = vet_procedure_v2(
        &validated,
        &AuthoringContext::new(
            "goal-readback.yaml",
            PROCEDURE,
            ProcedureDocumentFormat::Yaml,
        ),
    );
    assert!(diagnostics.is_empty(), "invalid fixture: {diagnostics:?}");

    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(workspace.main().join("goal-readback.yaml"), PROCEDURE).unwrap();
    let selector = runtime::selector(workspace.main());
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let dispatcher = runtime::dispatcher(manager, "v2gol004-max-production");
    let mut number = 104_500;

    let initialize = runtime::request(
        next_number(&mut number),
        "workspace.init",
        &selector,
        Map::new(),
        "v2gol004-max-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(&dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let start = runtime::request(
        next_number(&mut number),
        "session.start",
        &selector,
        json!({
            "procedure": "goal-readback.yaml",
            "expected_procedure_digest": validated.digest(),
            "task_title": "V2GOL-004 maximum escaped goal readback"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol004-max-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();

    let criteria = (0..16)
        .map(|index| {
            json!({
                "criterion_id": format!("criterion-{index:02}"),
                "statement": format!("Maximum criterion {index:02} is satisfied."),
            })
        })
        .collect::<Vec<_>>();
    let before_define = status(&dispatcher, &selector, &session_id, &mut number);
    let define = mutation_request(
        next_number(&mut number),
        "goal.define",
        &selector,
        json!({
            "goal": "Preserve the maximum escaped assessment readback.",
            "criteria": criteria,
            "actor": "maximum goal author"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol004-max-define",
        session_identity_preconditions(&before_define),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &define), "goal.define");

    set_result(&dispatcher, &selector, &session_id, &mut number, 1);
    complete_action(
        &dispatcher,
        &selector,
        &session_id,
        &mut number,
        "v2gol004-max-complete-work",
    );
    for item_id in ["note-one", "note-two", "note-three"] {
        let request_number = next_number(&mut number);
        runtime::mutate_item(
            &dispatcher,
            &selector,
            request_number,
            &session_id,
            "item.set",
            item_id,
            json!({"value": format!("persisted {item_id}")})
                .as_object()
                .unwrap()
                .clone(),
            &format!("v2gol004-max-set-{item_id}"),
        );
    }

    let maximum_reason = "\u{1}".repeat(2_000);
    let maximum_actor = "a".repeat(256);
    for index in 0..16 {
        let before = status(&dispatcher, &selector, &session_id, &mut number);
        let criterion_id = format!("criterion-{index:02}");
        let request_number = next_number(&mut number);
        let request = mutation_request(
            request_number,
            "goal.assess_criterion",
            &selector,
            json!({
                "criterion_id": criterion_id,
                "status": "satisfied",
                "reason": maximum_reason,
                "evidence": ["perform"],
                "items": ["note-one", "note-two", "note-three"],
                "actor": maximum_actor,
            })
            .as_object()
            .unwrap()
            .clone(),
            &format!("v2gol004-max-assess-{index:02}"),
            goal_preconditions(&before, 1),
        );
        runtime::v2_result(
            runtime::dispatch(&dispatcher, &request),
            "goal.assess_criterion",
        );
    }

    let before_decide = status(&dispatcher, &selector, &session_id, &mut number);
    let decide = mutation_request(
        next_number(&mut number),
        "session.decide",
        &selector,
        json!({
            "option_id": "achieved",
            "reason": maximum_reason,
            "actor": maximum_actor,
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2gol004-max-decide",
        goal_preconditions(&before_decide, 1),
    );
    let decision =
        runtime::v2_result(runtime::dispatch(&dispatcher, &decide), "session.decide")["record"]
            .clone();
    assert_eq!(decision["reason"], maximum_reason);
    assert_eq!(decision["actor"], maximum_actor);
    assert_eq!(decision["criterion_results"].as_array().unwrap().len(), 16);

    let next_request = query_request(
        next_number(&mut number),
        "session.next",
        &selector,
        &session_id,
        Map::new(),
    );
    let next_response = runtime::dispatch(&dispatcher, &next_request);
    let next_encoded = encode_response_payload_v2(&next_response).unwrap();
    assert!(next_encoded.len() <= 1_048_576);
    let next_decoded: ResponseEnvelopeV2 = serde_json::from_slice(&next_encoded).unwrap();
    assert_eq!(next_decoded, next_response);
    let next = runtime::v2_result(next_response, "session.next");
    let readback = &next["readback"][0]["decision_record"];
    assert_eq!(readback, &decision);
    for result in readback["criterion_results"].as_array().unwrap() {
        assert_eq!(result["reason"], maximum_reason);
        assert_eq!(result["citations"].as_array().unwrap().len(), 4);
        assert_eq!(
            result["citations"],
            json!([
                {"reference_graph_node_id": "perform"},
                {"local_item_id": "note-one"},
                {"local_item_id": "note-two"},
                {"local_item_id": "note-three"}
            ])
        );
    }

    revise(&dispatcher, &selector, &session_id, &mut number, 1, 2);
    let cursor = decision["trace_sequence"].as_u64().unwrap() + 1;
    let mut payload = json!({"verbose": true, "history_before": cursor})
        .as_object()
        .unwrap()
        .clone();
    let verbose_request = query_request(
        next_number(&mut number),
        "session.status",
        &selector,
        &session_id,
        std::mem::take(&mut payload),
    );
    let verbose_response = runtime::dispatch(&dispatcher, &verbose_request);
    let verbose_encoded = encode_response_payload_v2(&verbose_response).unwrap();
    assert!(verbose_encoded.len() <= 1_048_576);
    let verbose_decoded: ResponseEnvelopeV2 = serde_json::from_slice(&verbose_encoded).unwrap();
    assert_eq!(verbose_decoded, verbose_response);
    let verbose = runtime::v2_result(verbose_response, "session.status");
    let ordinary = &verbose["decision_history"]["entries"][0];
    assert_eq!(ordinary, &ordinary_decision_record(&decision));
    for assessment_field in [
        "assessment",
        "assessment_mode",
        "goal_outcome",
        "criterion_results",
    ] {
        assert!(ordinary.get(assessment_field).is_none());
    }
    assert!(
        serde_json::to_vec(&verbose["decision_history"])
            .unwrap()
            .len()
            <= 65_536
    );
    let assessment = &verbose["stale_goal_assessment_history"]["entries"][0];
    assert_assessment_summary(assessment, &decision, "achieved");
    assert_eq!(
        assessment["criterion_statuses"].as_array().unwrap().len(),
        16
    );
    assert!(
        serde_json::to_vec(&verbose["stale_goal_assessment_history"])
            .unwrap()
            .len()
            <= 65_536
    );
}
