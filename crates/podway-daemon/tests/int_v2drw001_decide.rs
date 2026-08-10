//! Production vertical coverage for V2DRW-001 typed decision execution.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{AttemptId, Revision, SessionId};
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1,
};
use serde_json::{Map, Value, json};

const DECISION_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: decision-runtime
version: "2"
name: Decision runtime
purpose: Exercise a fully fenced decision with required items and resolved evidence.
node_definitions:
  work:
    type: action
    title: Record work
    intent: Produce evidence for the decision.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        min_length: 1
        max_length: 200
        multiline: false
  review:
    type: decision
    title: Review work
    objective: Choose whether to accept the recorded work.
    prompt: Accept the work?
    items:
      - id: note
        type: text
        prompt: Record the decision note.
        required: true
        min_length: 1
        max_length: 200
        multiline: false
    options:
      - id: accept
        label: Accept
      - id: reject
        label: Reject
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the decision fixture.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: review
    - id: review
      use: review
      evidence_from:
        - node: work
          required: true
          items:
            - result
      routes:
        accept:
          to: finish
          effect: advance
        reject:
          to: work
          effect: rework
    - id: finish
      use: finish
      terminal: true
"#;

fn decide_request(
    request_number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    option_id: &str,
    reason: &str,
    actor: Option<&str>,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let mut payload = json!({
        "selector": selector,
        "option_id": option_id,
        "reason": reason,
    })
    .as_object()
    .unwrap()
    .clone();
    if let Some(actor) = actor {
        payload.insert("actor".to_owned(), json!(actor));
    }
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .unwrap(),
        client: ClientInfoV1::new("v2drw001-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.decide").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(idempotency_key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    (envelope, daemon)
}

fn assert_error(response: ResponseEnvelopeV2, code: &str, admitted: bool) -> Value {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{code} must be returned as a public error")
    };
    assert_eq!(error.code().as_str(), code);
    assert_eq!(
        error.details()["admission"]["admitted"],
        admitted,
        "unexpected error details: {}",
        serde_json::to_string(error.details()).unwrap()
    );
    serde_json::to_value(error).unwrap()
}

fn start_at_decision(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
) -> String {
    let initialize = runtime::request(
        91_001,
        "workspace.init",
        selector,
        Map::new(),
        "v2drw001-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(DECISION_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let start = runtime::request(
        91_002,
        "session.start",
        selector,
        json!({
            "procedure": "decision.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2DRW-001 production decision"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2drw001-start",
        PreconditionsV1::default(),
    );
    let session_id =
        runtime::v2_result(runtime::dispatch(dispatcher, &start), "session.start")["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
    let action = runtime::status(dispatcher, selector, 91_005, &session_id);
    let wrong_node_type = decide_request(
        91_006,
        selector,
        "accept",
        "A decision cannot run on an action placement.",
        None,
        "v2drw001-action-decide",
        runtime::session_preconditions(&action),
    );
    let wrong_type = assert_error(
        runtime::dispatch(dispatcher, &wrong_node_type),
        "GRAPH_NODE_TYPE_MISMATCH",
        true,
    );
    assert_eq!(wrong_type["details"]["graph_node_id"], "work");
    assert_eq!(wrong_type["details"]["expected_node_type"], "decision");
    assert_eq!(wrong_type["details"]["actual_node_type"], "action");
    runtime::mutate_item(
        dispatcher,
        selector,
        91_010,
        &session_id,
        "item.set",
        "result",
        json!({"value": "focused tests passed"})
            .as_object()
            .unwrap()
            .clone(),
        "v2drw001-set-result",
    );
    let before_complete = runtime::status(dispatcher, selector, 91_020, &session_id);
    let complete = runtime::request(
        91_021,
        "session.complete",
        selector,
        Map::new(),
        "v2drw001-complete-work",
        runtime::session_preconditions(&before_complete),
    );
    let completed =
        runtime::v2_result(runtime::dispatch(dispatcher, &complete), "session.complete");
    assert_eq!(completed["to_graph_node_id"], "review");
    session_id
}

#[test]
fn v2drw001_decision_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(DECISION_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2DRW-001 decision fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "decision.yaml",
        DECISION_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2DRW-001 decision fixture must pass structural vetting: {diagnostics:?}"
    );
}

#[test]
fn v2drw001_decide_validates_fences_and_rules_then_persists_exact_replay() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("decision.yaml"), DECISION_PROCEDURE).unwrap();
    let selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2drw001-production");
    let session_id = start_at_decision(&production, &selector);
    let before = runtime::status(&production, &selector, 91_030, &session_id);
    assert_eq!(before["current"]["node"]["graph_node_id"], "review");
    assert_eq!(before["current"]["readiness"]["items_satisfied"], false);
    assert_eq!(before["references"][0]["state"], "resolved");

    let missing_item = decide_request(
        91_031,
        &selector,
        "accept",
        "Evidence and review support acceptance.",
        None,
        "v2drw001-missing-item",
        runtime::session_preconditions(&before),
    );
    assert_error(
        runtime::dispatch(&production, &missing_item),
        "REQUIRED_ITEMS_MISSING",
        true,
    );

    let stale_revision = decide_request(
        91_032,
        &selector,
        "accept",
        "Evidence and review support acceptance.",
        None,
        "v2drw001-stale-revision",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(Revision::new(
                before["session"]["revision"].as_u64().unwrap() + 1,
            )),
            Some(
                AttemptId::new(before["current"]["attempt"]["attempt_id"].as_str().unwrap())
                    .unwrap(),
            ),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_error(
        runtime::dispatch(&production, &stale_revision),
        "SESSION_REVISION_CONFLICT",
        false,
    );

    let stale_attempt = decide_request(
        91_033,
        &selector,
        "accept",
        "Evidence and review support acceptance.",
        None,
        "v2drw001-stale-attempt",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(Revision::new(
                before["session"]["revision"].as_u64().unwrap(),
            )),
            Some(AttemptId::new("00000000-0000-4000-8000-000000009199").unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_error(
        runtime::dispatch(&production, &stale_attempt),
        "ATTEMPT_NOT_CURRENT",
        false,
    );

    runtime::mutate_item(
        &production,
        &selector,
        91_040,
        &session_id,
        "item.set",
        "note",
        json!({"value": "reviewed locally"})
            .as_object()
            .unwrap()
            .clone(),
        "v2drw001-set-note",
    );
    let ready = runtime::status(&production, &selector, 91_050, &session_id);
    assert_eq!(ready["current"]["readiness"]["can_advance"], true);
    let invalid_option = decide_request(
        91_051,
        &selector,
        "unknown",
        "Attempt an unsupported route.",
        None,
        "v2drw001-invalid-option",
        runtime::session_preconditions(&ready),
    );
    let invalid = assert_error(
        runtime::dispatch(&production, &invalid_option),
        "OPTION_NOT_ALLOWED",
        true,
    );
    assert_eq!(
        invalid["details"]["allowed_option_ids"],
        json!(["accept", "reject"])
    );

    let decide = decide_request(
        91_060,
        &selector,
        "accept",
        "Evidence and review support acceptance.",
        Some("V2DRW reviewer"),
        "v2drw001-decide",
        runtime::session_preconditions(&ready),
    );
    let first_response = runtime::dispatch(&production, &decide);
    let result = runtime::v2_result(first_response.clone(), "session.decide");
    let keys = result.keys().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "admission",
            "attempt_id",
            "attempt_number",
            "effect",
            "graph_node_id",
            "option_id",
            "record",
            "revision",
            "schema",
            "session_state",
            "target_attempt_id",
            "target_graph_node_id",
        ]
    );
    assert_eq!(result["schema"], "podway.decision-result/v1");
    assert_eq!(result["admission"]["admitted"], true);
    assert_eq!(result["graph_node_id"], "review");
    assert_eq!(
        result["attempt_id"],
        ready["current"]["attempt"]["attempt_id"]
    );
    assert_eq!(result["attempt_number"], 1);
    assert_eq!(result["option_id"], "accept");
    assert_eq!(result["effect"], "advance");
    assert_eq!(result["target_graph_node_id"], "finish");
    assert_eq!(
        result["revision"],
        ready["session"]["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(result["session_state"], "running");
    assert_eq!(result["record"]["session_id"], session_id);
    assert_eq!(result["record"]["session_revision"], result["revision"]);
    assert_eq!(result["record"]["target_graph_node_id"], "finish");
    assert_eq!(
        result["record"]["reason"],
        "Evidence and review support acceptance."
    );
    assert_eq!(result["record"]["actor"], "V2DRW reviewer");
    assert_eq!(result["record"]["references"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["record"]["references"][0]["source_graph_node_id"],
        "work"
    );
    assert_eq!(result["record"]["references"][0]["state"], "resolved");

    drop(production);
    let restarted = runtime::dispatcher(Arc::clone(&manager), "v2drw001-restarted");
    let replay = decide_request(
        91_061,
        &selector,
        "accept",
        "Evidence and review support acceptance.",
        Some("V2DRW reviewer"),
        "v2drw001-decide",
        runtime::session_preconditions(&ready),
    );
    let replayed = runtime::dispatch(&restarted, &replay);
    assert_eq!(
        runtime::without_request_id(&replayed),
        runtime::without_request_id(&first_response),
        "exact idempotency replay after restart must preserve the frozen public decision output"
    );

    let after = runtime::status(&restarted, &selector, 91_070, &session_id);
    assert_eq!(after["current"]["node"]["graph_node_id"], "finish");
    assert_eq!(
        after["current"]["attempt"]["attempt_id"],
        result["target_attempt_id"]
    );
    assert_eq!(after["session"]["revision"], result["revision"]);
    assert_eq!(after["queue"]["queued_count"], 0);
    assert_eq!(after["queue"]["pending_mutations"], false);

    let verbose_request = runtime::request(
        91_071,
        "session.status",
        &selector,
        json!({"verbose": true}).as_object().unwrap().clone(),
        "unused-v2drw001-status-key",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let verbose = runtime::v2_result(
        runtime::dispatch(&restarted, &verbose_request),
        "session.status",
    );
    assert_eq!(
        verbose["decision_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(verbose["decision_history"]["entries"][0], result["record"]);
}
