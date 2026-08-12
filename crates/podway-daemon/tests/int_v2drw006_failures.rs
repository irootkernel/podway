//! Production failure, replay, and concurrency closure for V2DRW decisions and rework.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{Revision, SessionId};
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, MAX_FRAME_PAYLOAD_BYTES_V1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV2, WorkspaceContextV1, WorktreeSelectorWireV1, decode_response_payload_v2,
    decode_single_frame_v1, encode_frame_v1, encode_response_payload_v2,
    validate_frame_payload_length,
};
use serde_json::{Map, Value, json};

const FAILURE_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: decision-failure-closure
version: "2"
name: Decision failure closure
purpose: Exercise deterministic decision and rework failures through production dispatch.
node_definitions:
  work:
    type: action
    title: Record work
    intent: Produce evidence for review.
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
    objective: Accept or reject the recorded work.
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
    intent: Finish the failure fixture.
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
manual_rework:
  allowed_targets:
    - review
"#;

struct ReadyFixture {
    selector: WorktreeSelectorWireV1,
    session_id: String,
}

fn raw_request(
    number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    mut payload: Map<String, Value>,
    key: &str,
    preconditions: PreconditionsV1,
) -> RequestEnvelopeV1 {
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
        client: ClientInfoV1::new("v2drw006-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap()
}

fn typed_request(
    number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    payload: Map<String, Value>,
    key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let envelope = raw_request(number, command, selector, payload, key, preconditions);
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    (envelope, daemon)
}

fn decide_request(
    number: u64,
    selector: &WorktreeSelectorWireV1,
    option_id: &str,
    reason: &str,
    key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    typed_request(
        number,
        "session.decide",
        selector,
        json!({
            "option_id": option_id,
            "reason": reason,
            "actor": "V2DRW-006 reviewer"
        })
        .as_object()
        .unwrap()
        .clone(),
        key,
        preconditions,
    )
}

fn rework_request(
    number: u64,
    selector: &WorktreeSelectorWireV1,
    target: &str,
    reason: &str,
    key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    typed_request(
        number,
        "session.rework",
        selector,
        json!({
            "target_graph_node_id": target,
            "reason": reason,
            "actor": "V2DRW-006 reviewer"
        })
        .as_object()
        .unwrap()
        .clone(),
        key,
        preconditions,
    )
}

fn assert_error(response: &ResponseEnvelopeV2, code: &str, admitted: bool) -> Value {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{code} must be returned as a public error: {response:?}")
    };
    assert_eq!(error.code().as_str(), code);
    assert_eq!(error.details()["admission"]["admitted"], admitted);
    serde_json::to_value(error).unwrap()
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
            "cold-reopen lifecycle maintenance did not release within 5 seconds: {response:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn status(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &ReadyFixture,
    number: u64,
    verbose: bool,
) -> Map<String, Value> {
    let request = runtime::request(
        number,
        "session.status",
        &fixture.selector,
        if verbose {
            json!({"verbose": true}).as_object().unwrap().clone()
        } else {
            Map::new()
        },
        "unused-v2drw006-status-key",
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
    let response = runtime::dispatch(dispatcher, &request);
    assert!(encode_response_payload_v2(&response).unwrap().len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    runtime::v2_result(response, "session.status")
}

fn verbose_status_before(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &ReadyFixture,
    number: u64,
    history_before: u64,
) -> Map<String, Value> {
    let request = runtime::request(
        number,
        "session.status",
        &fixture.selector,
        json!({"verbose": true, "history_before": history_before})
            .as_object()
            .unwrap()
            .clone(),
        "unused-v2drw006-paged-status-key",
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
    let response = runtime::dispatch(dispatcher, &request);
    assert!(encode_response_payload_v2(&response).unwrap().len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    runtime::v2_result(response, "session.status")
}

fn status_after_cold_reopen(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &ReadyFixture,
    number: u64,
) -> Map<String, Value> {
    let request = runtime::request(
        number,
        "session.status",
        &fixture.selector,
        json!({"verbose": true}).as_object().unwrap().clone(),
        "unused-v2drw006-reopened-status-key",
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
    let response = dispatch_after_cold_reopen(dispatcher, &request);
    assert!(encode_response_payload_v2(&response).unwrap().len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    runtime::v2_result(response, "session.status")
}

fn counter<'a>(value: &'a Map<String, Value>, graph_node_id: &str) -> &'a Value {
    value["counters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|counter| counter["graph_node_id"] == graph_node_id)
        .unwrap()
}

fn bounded_query(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &ReadyFixture,
    number: &mut u64,
    command: &str,
    payload: Map<String, Value>,
) -> Map<String, Value> {
    let request = runtime::request(
        *number,
        command,
        &fixture.selector,
        payload,
        "unused-v2drw006-bounded-query-key",
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
    *number += 1;
    let response = runtime::dispatch(dispatcher, &request);
    let encoded = encode_response_payload_v2(&response).unwrap();
    validate_frame_payload_length(encoded.len()).unwrap();
    assert!(encoded.len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    let frame = encode_frame_v1(&encoded).unwrap();
    let framed_payload = decode_single_frame_v1(&frame).unwrap();
    assert_eq!(framed_payload, encoded);
    let decoded = decode_response_payload_v2(framed_payload).unwrap();
    assert_eq!(decoded, response);
    runtime::v2_result(decoded, command)
}

fn assert_history_window(value: &Value, total: u64, maximum: usize) {
    let entries = value["entries"].as_array().unwrap();
    assert_eq!(entries.len(), usize::try_from(total).unwrap().min(maximum));
    assert_eq!(value["trace_truncated"], total > maximum as u64);
    if entries.is_empty() {
        assert!(value["trace_window"].is_null());
    } else {
        let sequences = entries
            .iter()
            .map(|entry| entry["trace_sequence"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            value["trace_window"]["first_sequence"],
            sequences.iter().min().copied().unwrap()
        );
        assert_eq!(
            value["trace_window"]["last_sequence"],
            sequences.iter().max().copied().unwrap()
        );
    }
}

fn set_text_item(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &ReadyFixture,
    number: &mut u64,
    item_id: &str,
    value: &str,
) {
    runtime::mutate_item(
        dispatcher,
        &fixture.selector,
        *number,
        &fixture.session_id,
        "item.set",
        item_id,
        json!({"value": value}).as_object().unwrap().clone(),
        &format!("v2drw006-cycle-set-{item_id}-{number}"),
    );
    *number += 2;
}

fn complete_current(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &ReadyFixture,
    number: &mut u64,
) -> Map<String, Value> {
    let before = status(dispatcher, fixture, *number, false);
    let request = runtime::request(
        *number + 1,
        "session.complete",
        &fixture.selector,
        Map::new(),
        &format!("v2drw006-cycle-complete-{number}"),
        runtime::session_preconditions(&before),
    );
    *number += 2;
    let response = runtime::dispatch(dispatcher, &request);
    assert!(encode_response_payload_v2(&response).unwrap().len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    runtime::v2_result(response, "session.complete")
}

fn decide_current(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &ReadyFixture,
    number: &mut u64,
    option_id: &str,
) -> Map<String, Value> {
    let before = status(dispatcher, fixture, *number, false);
    let request = decide_request(
        *number + 1,
        &fixture.selector,
        option_id,
        &format!("Valid repeated {option_id} decision at request {number}."),
        &format!("v2drw006-cycle-decide-{number}"),
        runtime::session_preconditions(&before),
    );
    *number += 2;
    let response = runtime::dispatch(dispatcher, &request);
    assert!(encode_response_payload_v2(&response).unwrap().len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    runtime::v2_result(response, "session.decide")
}

fn start_ready(
    dispatcher: &impl RequestDispatcherV1,
    root: &std::path::Path,
    number: u64,
) -> ReadyFixture {
    runtime::make_runtime_private(root);
    fs::write(root.join("v2drw006.yaml"), FAILURE_PROCEDURE).unwrap();
    let selector = runtime::selector(root);
    let initialize = runtime::request(
        number,
        "workspace.init",
        &selector,
        Map::new(),
        &format!("v2drw006-init-{number}"),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(FAILURE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let start = runtime::request(
        number + 1,
        "session.start",
        &selector,
        json!({
            "procedure": "v2drw006.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2DRW-006 production failure closure"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2drw006-start-{number}"),
        PreconditionsV1::default(),
    );
    let session_id =
        runtime::v2_result(runtime::dispatch(dispatcher, &start), "session.start")["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
    runtime::mutate_item(
        dispatcher,
        &selector,
        number + 10,
        &session_id,
        "item.set",
        "result",
        json!({"value": "failure matrix work evidence"})
            .as_object()
            .unwrap()
            .clone(),
        &format!("v2drw006-set-result-{number}"),
    );
    let work = runtime::status(dispatcher, &selector, number + 20, &session_id);
    let complete = runtime::request(
        number + 21,
        "session.complete",
        &selector,
        Map::new(),
        &format!("v2drw006-complete-work-{number}"),
        runtime::session_preconditions(&work),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &complete), "session.complete");
    runtime::mutate_item(
        dispatcher,
        &selector,
        number + 30,
        &session_id,
        "item.set",
        "note",
        json!({"value": "reviewed before decision"})
            .as_object()
            .unwrap()
            .clone(),
        &format!("v2drw006-set-note-{number}"),
    );
    ReadyFixture {
        selector,
        session_id,
    }
}

fn assert_cursor_state_unchanged(before: &Map<String, Value>, after: &Map<String, Value>) {
    assert_eq!(after["session"]["revision"], before["session"]["revision"]);
    assert_eq!(after["current"], before["current"]);
    assert_eq!(after["trace_length"], before["trace_length"]);
    assert_eq!(after["counters"], before["counters"]);
    assert_eq!(after["decision_history"], before["decision_history"]);
    assert_eq!(after["rework_history"], before["rework_history"]);
    assert_eq!(after["queue"]["queued_count"], 0);
    assert!(after["queue"]["running_job_id"].is_null());
}

#[test]
fn v2drw006_failure_receipts_are_atomic_and_cold_replayable() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager_root = workspace.temporary_path().to_path_buf();
    let first_manager = Arc::new(runtime::manager(&manager_root));
    let production = runtime::dispatcher(Arc::clone(&first_manager), "v2drw006-failures");
    let fixture = start_ready(&production, workspace.main(), 96_000);
    let before = status(&production, &fixture, 96_040, true);
    assert_eq!(before["current"]["readiness"]["can_advance"], true);

    for (offset, command, payload) in [
        (1, "session.decide", json!({"option_id": "accept"})),
        (
            2,
            "session.decide",
            json!({"option_id": "accept", "reason": " \t\n"}),
        ),
        (
            3,
            "session.rework",
            json!({"target_graph_node_id": "review"}),
        ),
        (
            4,
            "session.rework",
            json!({"target_graph_node_id": "review", "reason": " \t\n"}),
        ),
    ] {
        let envelope = raw_request(
            96_040 + offset,
            command,
            &fixture.selector,
            payload.as_object().unwrap().clone(),
            &format!("v2drw006-malformed-{offset}"),
            runtime::session_preconditions(&before),
        );
        assert!(
            DaemonRequestV1::from_envelope(&envelope).is_err(),
            "the public daemon transport maps malformed {command} to REQUEST_INVALID"
        );
    }
    assert_cursor_state_unchanged(&before, &status(&production, &fixture, 96_050, true));

    let invalid_option = decide_request(
        96_051,
        &fixture.selector,
        "unknown",
        "Exercise a durable admitted decision failure.",
        "v2drw006-invalid-option",
        runtime::session_preconditions(&before),
    );
    let first_failure = runtime::dispatch(&production, &invalid_option);
    assert_error(&first_failure, "OPTION_NOT_ALLOWED", true);
    let after_failure = status(&production, &fixture, 96_052, true);
    assert_cursor_state_unchanged(&before, &after_failure);

    drop(production);
    drop(first_manager);
    let second_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(Arc::clone(&second_manager), "v2drw006-replay");
    let replay = decide_request(
        96_053,
        &fixture.selector,
        "unknown",
        "Exercise a durable admitted decision failure.",
        "v2drw006-invalid-option",
        runtime::session_preconditions(&before),
    );
    let replayed = dispatch_after_cold_reopen(&reopened, &replay);
    assert_eq!(
        runtime::without_request_id(&replayed),
        runtime::without_request_id(&first_failure),
        "an admitted failure receipt must survive a genuine manager reopen"
    );
    let drift = decide_request(
        96_054,
        &fixture.selector,
        "accept",
        "Reuse the failure key with different payload bytes.",
        "v2drw006-invalid-option",
        runtime::session_preconditions(&before),
    );
    assert_error(
        &runtime::dispatch(&reopened, &drift),
        "IDEMPOTENCY_KEY_REUSED",
        false,
    );
    assert_cursor_state_unchanged(&before, &status(&reopened, &fixture, 96_055, true));

    let ready = status(&reopened, &fixture, 96_056, false);
    let cancel = runtime::request(
        96_057,
        "session.cancel",
        &fixture.selector,
        json!({"reason": "Cancel before manual rework failure replay."})
            .as_object()
            .unwrap()
            .clone(),
        "v2drw006-cancel",
        runtime::session_preconditions(&ready),
    );
    let cancelled = runtime::v2_result(runtime::dispatch(&reopened, &cancel), "session.cancel");
    let cancelled_revision = cancelled["revision"].as_u64().unwrap();
    let cancelled_preconditions = PreconditionsV1::new(
        Some(SessionId::new(&fixture.session_id).unwrap()),
        Some(Revision::new(cancelled_revision)),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let cancelled_rework = rework_request(
        96_058,
        &fixture.selector,
        "review",
        "A cancelled session must remain terminal.",
        "v2drw006-cancelled-rework",
        cancelled_preconditions.clone(),
    );
    let cancelled_failure = runtime::dispatch(&reopened, &cancelled_rework);
    assert_error(&cancelled_failure, "SESSION_CANCELLED", true);

    drop(reopened);
    drop(second_manager);
    let third_manager = Arc::new(runtime::manager(&manager_root));
    let reopened_again = runtime::dispatcher(third_manager, "v2drw006-cancelled-replay");
    let cancelled_replay = rework_request(
        96_059,
        &fixture.selector,
        "review",
        "A cancelled session must remain terminal.",
        "v2drw006-cancelled-rework",
        cancelled_preconditions,
    );
    let replayed_cancelled = dispatch_after_cold_reopen(&reopened_again, &cancelled_replay);
    assert_eq!(
        runtime::without_request_id(&replayed_cancelled),
        runtime::without_request_id(&cancelled_failure)
    );
    let final_status = status(&reopened_again, &fixture, 96_060, true);
    assert_eq!(final_status["session"]["lifecycle"], "cancelled");
    assert!(final_status["current"].is_null());
    assert_eq!(final_status["decision_history"]["entries"], json!([]));
    assert_eq!(final_status["rework_history"]["entries"], json!([]));
    assert_eq!(final_status["queue"]["queued_count"], 0);
    assert!(final_status["queue"]["running_job_id"].is_null());
}

#[test]
fn v2drw006_concurrent_decisions_keep_one_cursor_and_stale_history_non_satisfying() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager_root = workspace.temporary_path().to_path_buf();
    let manager = Arc::new(runtime::manager(&manager_root));
    let production = Arc::new(runtime::dispatcher(
        Arc::clone(&manager),
        "v2drw006-concurrent",
    ));
    let fixture = start_ready(production.as_ref(), workspace.main(), 97_000);
    let before = status(production.as_ref(), &fixture, 97_040, false);
    let original_attempt_id = before["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let left = decide_request(
        97_041,
        &fixture.selector,
        "accept",
        "Concurrent acceptance from the left caller.",
        "v2drw006-concurrent-left",
        runtime::session_preconditions(&before),
    );
    let right = decide_request(
        97_042,
        &fixture.selector,
        "accept",
        "Concurrent acceptance from the right caller.",
        "v2drw006-concurrent-right",
        runtime::session_preconditions(&before),
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
    for response in &responses {
        assert!(encode_response_payload_v2(response).unwrap().len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    }
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(response, ResponseEnvelopeV2::OutputV2(_)))
            .count(),
        1
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| {
                matches!(
                    response,
                    ResponseEnvelopeV2::Error(error)
                        if matches!(error.code().as_str(), "SESSION_REVISION_CONFLICT" | "ATTEMPT_NOT_CURRENT")
                )
            })
            .count(),
        1,
        "the same cursor fence must have exactly one deterministic conflict loser"
    );

    let after = status(production.as_ref(), &fixture, 97_050, true);
    assert_eq!(after["current"]["node"]["graph_node_id"], "finish");
    assert_eq!(
        after["trace_length"],
        before["trace_length"].as_u64().unwrap() + 1
    );
    assert_eq!(
        after["decision_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        after["decision_history"]["entries"][0]["attempt_id"],
        original_attempt_id
    );
    let immutable_decision = after["decision_history"]["entries"][0].clone();
    assert_eq!(after["rework_history"]["entries"], json!([]));
    assert_eq!(after["queue"]["queued_count"], 0);
    assert!(after["queue"]["running_job_id"].is_null());

    let rework = rework_request(
        97_051,
        &fixture.selector,
        "review",
        "Re-enter review after the accepted decision.",
        "v2drw006-reenter-review",
        runtime::session_preconditions(&after),
    );
    let reworked = runtime::v2_result(
        runtime::dispatch(production.as_ref(), &rework),
        "session.rework",
    );
    assert_eq!(reworked["to_graph_node_id"], "review");
    let fresh = status(production.as_ref(), &fixture, 97_052, true);
    assert_eq!(fresh["current"]["node"]["graph_node_id"], "review");
    assert_ne!(
        fresh["current"]["attempt"]["attempt_id"],
        original_attempt_id
    );
    assert_eq!(fresh["current"]["readiness"]["items_satisfied"], false);
    assert_eq!(fresh["current"]["readiness"]["can_advance"], false);
    assert_eq!(fresh["decision_history"]["entries"][0], immutable_decision);
    let newest_stale = &fresh["stale_attempt_history"]["entries"][0];
    let older_stale = verbose_status_before(
        production.as_ref(),
        &fixture,
        97_053,
        newest_stale["trace_sequence"].as_u64().unwrap(),
    );
    assert_eq!(
        older_stale["stale_attempt_history"]["entries"][0]["attempt_id"], original_attempt_id,
        "the decision's old attempt must remain pageable as stale after manual re-entry"
    );
    assert_eq!(
        fresh["rework_history"]["entries"].as_array().unwrap().len(),
        1
    );

    let delayed = decide_request(
        97_054,
        &fixture.selector,
        "accept",
        "A delayed caller still carries the obsolete cursor fence.",
        "v2drw006-delayed-old-decision",
        runtime::session_preconditions(&before),
    );
    let delayed_response = runtime::dispatch(production.as_ref(), &delayed);
    let ResponseEnvelopeV2::Error(delayed_error) = delayed_response else {
        panic!("an obsolete decision fence must not mutate the fresh review attempt")
    };
    assert!(matches!(
        delayed_error.code().as_str(),
        "SESSION_REVISION_CONFLICT" | "ATTEMPT_NOT_CURRENT"
    ));
    assert_eq!(delayed_error.details()["admission"]["admitted"], false);

    drop(production);
    drop(manager);
    let reopened_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(reopened_manager, "v2drw006-concurrent-reopen");
    let reopened_status = status_after_cold_reopen(&reopened, &fixture, 97_055);
    assert_eq!(reopened_status["session"], fresh["session"]);
    assert_eq!(reopened_status["current"], fresh["current"]);
    assert_eq!(
        reopened_status["decision_history"],
        fresh["decision_history"]
    );
    assert_eq!(reopened_status["rework_history"], fresh["rework_history"]);
    assert_eq!(reopened_status["queue"]["queued_count"], 0);
    assert!(reopened_status["queue"]["running_job_id"].is_null());
}

#[test]
fn v2drw006_repeated_declared_and_manual_cycles_survive_midrun_cold_reopen() {
    const DECLARED_CYCLES: u64 = 6;

    let workspace = support_phase4_workspace::git_worktrees();
    let manager_root = workspace.temporary_path().to_path_buf();
    let mut manager = Arc::new(runtime::manager(&manager_root));
    let mut production = runtime::dispatcher(Arc::clone(&manager), "v2drw006-cycles-first");
    let fixture = start_ready(&production, workspace.main(), 98_000);
    let initial = status(&production, &fixture, 98_040, true);
    let initial_revision = initial["session"]["revision"].as_u64().unwrap();
    let initial_trace_length = initial["trace_length"].as_u64().unwrap();
    assert_eq!(initial_trace_length, 2);
    assert_eq!(counter(&initial, "work")["attempt_count"], 1);
    assert_eq!(counter(&initial, "review")["attempt_count"], 1);

    let mut number = 98_100;
    let mut previous_revision = initial_revision;
    let mut previous_trace_length = initial_trace_length;
    for cycle in 0..DECLARED_CYCLES {
        let rejected = decide_current(&production, &fixture, &mut number, "reject");
        assert_eq!(rejected["target_graph_node_id"], "work");
        set_text_item(
            &production,
            &fixture,
            &mut number,
            "result",
            &format!("corrected work for declared cycle {cycle}"),
        );
        let completed = complete_current(&production, &fixture, &mut number);
        assert_eq!(completed["to_graph_node_id"], "review");
        set_text_item(
            &production,
            &fixture,
            &mut number,
            "note",
            &format!("review note for declared cycle {cycle}"),
        );
        let after_cycle = status(&production, &fixture, number, true);
        number += 1;
        let revision = after_cycle["session"]["revision"].as_u64().unwrap();
        let trace_length = after_cycle["trace_length"].as_u64().unwrap();
        assert!(revision > previous_revision);
        assert_eq!(trace_length, previous_trace_length + 2);
        assert_eq!(counter(&after_cycle, "work")["attempt_count"], cycle + 2);
        assert_eq!(
            counter(&after_cycle, "work")["rework_traversal_count"],
            cycle + 1
        );
        assert_eq!(counter(&after_cycle, "review")["attempt_count"], cycle + 2);
        assert_eq!(counter(&after_cycle, "review")["rework_traversal_count"], 0);
        assert_eq!(after_cycle["current"]["node"]["graph_node_id"], "review");
        assert_eq!(after_cycle["queue"]["queued_count"], 0);
        assert!(after_cycle["queue"]["running_job_id"].is_null());
        previous_revision = revision;
        previous_trace_length = trace_length;

        if cycle == 2 {
            let before_reopen = after_cycle;
            drop(production);
            drop(manager);
            manager = Arc::new(runtime::manager(&manager_root));
            production = runtime::dispatcher(Arc::clone(&manager), "v2drw006-cycles-reopened");
            let reopened = status_after_cold_reopen(&production, &fixture, number);
            number += 1;
            assert_eq!(reopened["session"], before_reopen["session"]);
            assert_eq!(reopened["current"], before_reopen["current"]);
            assert_eq!(reopened["trace_length"], before_reopen["trace_length"]);
            assert_eq!(reopened["counters"], before_reopen["counters"]);
            assert_eq!(
                reopened["decision_history"],
                before_reopen["decision_history"]
            );
            assert_eq!(reopened["rework_history"], before_reopen["rework_history"]);
        }
    }

    let accepted = decide_current(&production, &fixture, &mut number, "accept");
    assert_eq!(accepted["target_graph_node_id"], "finish");
    let accepted_attempt_id = accepted["attempt_id"].as_str().unwrap().to_owned();
    let at_finish = status(&production, &fixture, number, true);
    number += 1;
    assert!(at_finish["session"]["revision"].as_u64().unwrap() > previous_revision);
    assert_eq!(at_finish["trace_length"], previous_trace_length + 1);
    let manual = rework_request(
        number,
        &fixture.selector,
        "review",
        "Return to review after the repeated accepted decision.",
        "v2drw006-cycle-manual-rework",
        runtime::session_preconditions(&at_finish),
    );
    number += 1;
    let manual_response = runtime::dispatch(&production, &manual);
    assert!(
        encode_response_payload_v2(&manual_response).unwrap().len() <= MAX_FRAME_PAYLOAD_BYTES_V1
    );
    let reworked = runtime::v2_result(manual_response, "session.rework");
    assert_eq!(reworked["to_graph_node_id"], "review");
    assert_eq!(reworked["reactivated"], false);

    let final_status = status(&production, &fixture, number, true);
    assert!(
        final_status["session"]["revision"].as_u64().unwrap()
            > at_finish["session"]["revision"].as_u64().unwrap()
    );
    assert_eq!(final_status["trace_length"], initial_trace_length + 14);
    assert_eq!(counter(&final_status, "work")["attempt_count"], 7);
    assert_eq!(counter(&final_status, "work")["rework_traversal_count"], 6);
    assert_eq!(counter(&final_status, "review")["attempt_count"], 8);
    assert_eq!(
        counter(&final_status, "review")["rework_traversal_count"],
        1
    );
    assert_eq!(counter(&final_status, "finish")["attempt_count"], 1);
    assert_eq!(final_status["current"]["node"]["graph_node_id"], "review");
    assert_eq!(
        final_status["current"]["readiness"]["items_satisfied"],
        false
    );
    assert_eq!(final_status["current"]["readiness"]["can_advance"], false);
    assert_eq!(
        final_status["decision_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(final_status["decision_history"]["trace_truncated"], true);
    assert_eq!(
        final_status["rework_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert_eq!(final_status["rework_history"]["trace_truncated"], true);
    assert_eq!(final_status["queue"]["queued_count"], 0);
    assert!(final_status["queue"]["running_job_id"].is_null());

    let newest_stale = &final_status["stale_attempt_history"]["entries"][0];
    let older_stale = verbose_status_before(
        &production,
        &fixture,
        number + 1,
        newest_stale["trace_sequence"].as_u64().unwrap(),
    );
    assert_eq!(
        older_stale["stale_attempt_history"]["entries"][0]["attempt_id"],
        accepted_attempt_id
    );
}

#[test]
fn v2drw006_generated_cycle_counts_have_no_runtime_traversal_limit_and_stay_bounded() {
    const MAX_GENERATED_CYCLES: u64 = 12;

    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2drw006-generated-cycle-counts");
    let fixture = start_ready(&production, workspace.main(), 99_000);
    let mut number = 99_100;

    // Exercise every count in the bounded family rather than treating any sampled count as a
    // product limit. Procedure/domain validation remains the only authority over valid routes.
    for completed_cycles in 0..=MAX_GENERATED_CYCLES {
        let compact = bounded_query(
            &production,
            &fixture,
            &mut number,
            "session.status",
            json!({"compact": true, "wait_for_idle": true})
                .as_object()
                .unwrap()
                .clone(),
        );
        let standard = bounded_query(
            &production,
            &fixture,
            &mut number,
            "session.status",
            Map::new(),
        );
        let verbose = bounded_query(
            &production,
            &fixture,
            &mut number,
            "session.status",
            json!({"verbose": true}).as_object().unwrap().clone(),
        );
        let next = bounded_query(
            &production,
            &fixture,
            &mut number,
            "session.next",
            Map::new(),
        );

        let expected_trace_length = 2 + completed_cycles * 2;
        for projection in [&compact, &standard, &verbose, &next] {
            assert_eq!(projection["trace_length"], expected_trace_length);
            assert_eq!(
                counter(projection, "work")["attempt_count"],
                completed_cycles + 1
            );
            assert_eq!(
                counter(projection, "work")["rework_traversal_count"],
                completed_cycles
            );
            assert_eq!(
                counter(projection, "review")["attempt_count"],
                completed_cycles + 1
            );
            assert_eq!(counter(projection, "review")["rework_traversal_count"], 0);
            assert_eq!(counter(projection, "finish")["attempt_count"], 0);
        }
        assert_eq!(compact["schema"], "podway.compact-status-result/v2");
        assert_eq!(standard["tier"], "standard");
        assert_eq!(verbose["tier"], "verbose");
        assert_eq!(next["schema"], "podway.next-result/v2");
        assert_history_window(&verbose["decision_history"], completed_cycles, 1);
        assert_history_window(&verbose["rework_history"], completed_cycles, 6);

        if completed_cycles == MAX_GENERATED_CYCLES {
            break;
        }
        let rejected = decide_current(&production, &fixture, &mut number, "reject");
        assert_eq!(rejected["target_graph_node_id"], "work");
        set_text_item(
            &production,
            &fixture,
            &mut number,
            "result",
            &format!("generated correction {completed_cycles}"),
        );
        let completed = complete_current(&production, &fixture, &mut number);
        assert_eq!(completed["to_graph_node_id"], "review");
        set_text_item(
            &production,
            &fixture,
            &mut number,
            "note",
            &format!("generated review note {completed_cycles}"),
        );
    }
}

#[test]
fn v2drw006_failure_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(FAILURE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2DRW-006 fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "v2drw006.yaml",
        FAILURE_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2DRW-006 fixture must pass structural vetting: {diagnostics:?}"
    );
}
