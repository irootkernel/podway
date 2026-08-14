//! Production read-back coverage for immutable V2DRW decision and rework history.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::SessionId;
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, MAX_FRAME_PAYLOAD_BYTES_V1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV2, WorkspaceContextV1, WorktreeSelectorWireV1, encode_response_payload_v2,
};
use serde_json::{Map, Value, json};

const READBACK_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: decision-readback
version: "2"
name: Decision read-back
purpose: Exercise bounded immutable decision and rework history through production dispatch.
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
    intent: Finish the read-back fixture.
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

fn assert_bounded_response(response: &ResponseEnvelopeV2) {
    let payload = encode_response_payload_v2(response).unwrap();
    assert!(
        payload.len() <= MAX_FRAME_PAYLOAD_BYTES_V1,
        "response exceeded the 1 MiB frame payload: {} bytes",
        payload.len()
    );
}

fn result(response: ResponseEnvelopeV2, command: &str) -> Map<String, Value> {
    assert_bounded_response(&response);
    runtime::v2_result(response, command)
}

fn mutation_request(
    number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    mut payload: Map<String, Value>,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
        client: ClientInfoV1::new("v2drw005-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(idempotency_key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, runtime::TEST_WAIT_TIMEOUT_MILLIS).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    (envelope, daemon)
}

fn query(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    command: &str,
    payload: Map<String, Value>,
) -> Map<String, Value> {
    let request = runtime::request(
        next_number(number),
        command,
        selector,
        payload,
        "unused-v2drw005-query-key",
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
    result(runtime::dispatch(dispatcher, &request), command)
}

fn query_after_cold_reopen(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    command: &str,
    payload: Map<String, Value>,
) -> Map<String, Value> {
    let request = runtime::request(
        next_number(number),
        command,
        selector,
        payload,
        "unused-v2drw005-reopen-query-key",
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
    result(
        runtime::dispatch_after_cold_reopen(dispatcher, &request),
        command,
    )
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
    let value = query(
        dispatcher,
        selector,
        session_id,
        number,
        "session.status",
        payload,
    );
    for history in HISTORY_KEYS {
        let encoded = serde_json::to_vec(&value[history]).unwrap();
        assert!(
            encoded.len() <= 65_536,
            "{history} exceeded its 64 KiB history budget: {} bytes",
            encoded.len()
        );
        let entries = value[history]["entries"].as_array().unwrap();
        if entries.is_empty() {
            assert!(value[history]["trace_window"].is_null());
        } else {
            assert_newest_first(entries);
            assert_eq!(
                value[history]["trace_window"]["first_sequence"],
                entries.last().unwrap()["trace_sequence"]
            );
            assert_eq!(
                value[history]["trace_window"]["last_sequence"],
                entries.first().unwrap()["trace_sequence"]
            );
        }
        if let Some(cursor) = history_before {
            assert!(
                entries
                    .iter()
                    .all(|entry| entry["trace_sequence"].as_u64().unwrap() < cursor),
                "{history} did not apply the shared exclusive cursor"
            );
        }
    }
    value
}

fn set_item(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    item_id: &str,
    value: &str,
) {
    let request_number = next_number(number);
    *number += 1;
    runtime::mutate_item(
        dispatcher,
        selector,
        request_number,
        session_id,
        "item.set",
        item_id,
        json!({"value": value}).as_object().unwrap().clone(),
        &format!("v2drw005-set-{item_id}-{request_number}"),
    );
}

fn complete(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
) -> Map<String, Value> {
    let before = status(dispatcher, selector, session_id, number);
    let request_number = next_number(number);
    let request = runtime::request(
        request_number,
        "session.complete",
        selector,
        Map::new(),
        &format!("v2drw005-complete-{request_number}"),
        runtime::session_preconditions(&before),
    );
    result(runtime::dispatch(dispatcher, &request), "session.complete")
}

fn decide(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
    option_id: &str,
    reason: &str,
) -> Map<String, Value> {
    let before = status(dispatcher, selector, session_id, number);
    let request_number = next_number(number);
    let request = mutation_request(
        request_number,
        "session.decide",
        selector,
        json!({
            "option_id": option_id,
            "reason": reason,
            "actor": "V2DRW-005 reviewer"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2drw005-decide-{request_number}"),
        runtime::session_preconditions(&before),
    );
    result(runtime::dispatch(dispatcher, &request), "session.decide")
}

fn rework(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    session_id: &str,
    number: &mut u64,
) -> Map<String, Value> {
    let before = status(dispatcher, selector, session_id, number);
    let request_number = next_number(number);
    let request = mutation_request(
        request_number,
        "session.rework",
        selector,
        json!({
            "target_graph_node_id": "review",
            "reason": "Re-open the accepted decision after a new finding.",
            "actor": "V2DRW-005 reviewer"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2drw005-rework-{request_number}"),
        runtime::session_preconditions(&before),
    );
    result(runtime::dispatch(dispatcher, &request), "session.rework")
}

fn history_entries<'a>(status: &'a Map<String, Value>, history: &str) -> &'a [Value] {
    status[history]["entries"].as_array().unwrap()
}

fn assert_newest_first(entries: &[Value]) {
    assert!(entries.windows(2).all(|pair| {
        pair[0]["trace_sequence"].as_u64().unwrap() > pair[1]["trace_sequence"].as_u64().unwrap()
    }));
}

fn read_fingerprint(status: &Map<String, Value>) -> Value {
    json!({
        "revision": status["session"]["revision"],
        "lifecycle": status["session"]["lifecycle"],
        "current": status["current"],
        "trace_length": status["trace_length"],
        "counters": status["counters"],
        "queue": status["queue"],
    })
}

#[test]
fn v2drw005_readback_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(READBACK_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2DRW-005 read-back fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "decision-readback.yaml",
        READBACK_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2DRW-005 read-back fixture must pass structural vetting: {diagnostics:?}"
    );
}

#[test]
fn v2drw005_production_readback_is_bounded_immutable_pageable_and_cold_stable() {
    let workspace = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(workspace.main());
    fs::write(
        workspace.main().join("decision-readback.yaml"),
        READBACK_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(READBACK_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = runtime::selector(workspace.main());
    let manager_root = workspace.temporary_path().to_path_buf();
    let manager = Arc::new(runtime::manager(&manager_root));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2drw005-production");
    let mut number = 95_500;

    let initialize = runtime::request(
        next_number(&mut number),
        "workspace.init",
        &selector,
        Map::new(),
        "v2drw005-init",
        PreconditionsV1::default(),
    );
    let initialized = runtime::dispatch(&production, &initialize);
    assert_bounded_response(&initialized);
    assert!(matches!(initialized, ResponseEnvelopeV2::OutputV2(_)));
    let start = runtime::request(
        next_number(&mut number),
        "session.start",
        &selector,
        json!({
            "procedure": "decision-readback.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2DRW-005 production history read-back"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2drw005-start",
        PreconditionsV1::default(),
    );
    let session_id = result(runtime::dispatch(&production, &start), "session.start")["session_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut expected_decisions = Vec::new();
    let mut expected_reworks = Vec::new();
    for cycle in 1..=7 {
        set_item(
            &production,
            &selector,
            &session_id,
            &mut number,
            "result",
            &format!("work result {cycle}"),
        );
        let completed = complete(&production, &selector, &session_id, &mut number);
        assert_eq!(completed["to_graph_node_id"], "review");
        set_item(
            &production,
            &selector,
            &session_id,
            &mut number,
            "note",
            &format!("reject note {cycle}"),
        );
        let rejected = decide(
            &production,
            &selector,
            &session_id,
            &mut number,
            "reject",
            &format!("Reject cycle {cycle} for another recorded attempt."),
        );
        expected_decisions.push(rejected["record"].clone());
        let captured = verbose_status(&production, &selector, &session_id, &mut number, None);
        expected_reworks.push(captured["rework_history"]["entries"][0].clone());
    }

    set_item(
        &production,
        &selector,
        &session_id,
        &mut number,
        "result",
        "final accepted work result",
    );
    let final_work = status(&production, &selector, &session_id, &mut number);
    let final_work_attempt_id = final_work["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    complete(&production, &selector, &session_id, &mut number);
    set_item(
        &production,
        &selector,
        &session_id,
        &mut number,
        "note",
        "accept note",
    );
    let accepted = decide(
        &production,
        &selector,
        &session_id,
        &mut number,
        "accept",
        "Accept the final recorded attempt.",
    );
    expected_decisions.push(accepted["record"].clone());
    let accepted_attempt_id = accepted["attempt_id"].as_str().unwrap().to_owned();
    assert_eq!(accepted["target_graph_node_id"], "finish");

    let manual = rework(&production, &selector, &session_id, &mut number);
    assert_eq!(manual["from_graph_node_id"], "finish");
    assert_eq!(manual["to_graph_node_id"], "review");
    let fresh_attempt_id = manual["target_attempt_id"].as_str().unwrap().to_owned();
    let fresh = verbose_status(&production, &selector, &session_id, &mut number, None);
    expected_reworks.push(fresh["rework_history"]["entries"][0].clone());

    assert_eq!(history_entries(&fresh, "decision_history").len(), 1);
    assert_eq!(fresh["decision_history"]["entries"][0], accepted["record"]);
    assert_eq!(fresh["decision_history"]["trace_truncated"], true);
    assert_eq!(history_entries(&fresh, "rework_history").len(), 6);
    assert_eq!(fresh["rework_history"]["trace_truncated"], true);
    assert_eq!(
        fresh["rework_history"]["entries"],
        json!(expected_reworks.iter().rev().take(6).collect::<Vec<_>>())
    );
    assert_newest_first(history_entries(&fresh, "decision_history"));
    assert_newest_first(history_entries(&fresh, "rework_history"));
    assert_eq!(fresh["current"]["attempt"]["attempt_id"], fresh_attempt_id);
    assert_eq!(fresh["current"]["node"]["graph_node_id"], "review");
    assert_eq!(fresh["missing_required_item_ids"], json!(["note"]));
    assert_eq!(fresh["references"][0]["state"], "resolved");
    assert_eq!(
        fresh["references"][0]["source_attempt_id"], final_work_attempt_id,
        "fresh review must resolve evidence from the latest valid work attempt"
    );

    let standard = status(&production, &selector, &session_id, &mut number);
    for history in HISTORY_KEYS {
        assert!(
            !standard.contains_key(history),
            "standard status leaked verbose-only {history}"
        );
    }
    let next = query(
        &production,
        &selector,
        &session_id,
        &mut number,
        "session.next",
        Map::new(),
    );
    assert_eq!(next["readiness"]["can_advance"], false);
    assert!(
        !next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.decide"))
    );
    assert!(
        next["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["command"] != "session.decide")
    );

    let before_reads = read_fingerprint(&standard);
    let mut collected_decisions = Vec::new();
    let mut cursor = None;
    loop {
        let page = verbose_status(&production, &selector, &session_id, &mut number, cursor);
        let entries = history_entries(&page, "decision_history");
        assert!(entries.len() <= 1);
        if entries.is_empty() {
            break;
        }
        assert_newest_first(entries);
        collected_decisions.extend_from_slice(entries);
        cursor = Some(entries.last().unwrap()["trace_sequence"].as_u64().unwrap());
    }
    let mut expected_decisions_newest = expected_decisions.clone();
    expected_decisions_newest.reverse();
    assert_eq!(collected_decisions, expected_decisions_newest);

    let mut collected_reworks = Vec::new();
    let mut cursor = None;
    loop {
        let page = verbose_status(&production, &selector, &session_id, &mut number, cursor);
        let entries = history_entries(&page, "rework_history");
        assert!(entries.len() <= 6);
        if entries.is_empty() {
            break;
        }
        assert_newest_first(entries);
        collected_reworks.extend_from_slice(entries);
        cursor = Some(entries.last().unwrap()["trace_sequence"].as_u64().unwrap());
    }
    let mut expected_reworks_newest = expected_reworks.clone();
    expected_reworks_newest.reverse();
    assert_eq!(collected_reworks, expected_reworks_newest);

    let mut stale_attempts = Vec::new();
    let mut cursor = None;
    loop {
        let page = verbose_status(&production, &selector, &session_id, &mut number, cursor);
        let entries = history_entries(&page, "stale_attempt_history");
        if entries.is_empty() {
            break;
        }
        stale_attempts.push(entries[0].clone());
        cursor = Some(entries[0]["trace_sequence"].as_u64().unwrap());
    }
    let stale_attempt_ids = stale_attempts
        .iter()
        .map(|entry| entry["attempt_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(stale_attempt_ids.contains(&accepted_attempt_id));
    assert!(!stale_attempt_ids.contains(&fresh_attempt_id));
    let stale_prior_review = stale_attempts
        .iter()
        .find(|entry| {
            entry["graph_node_id"] == "review"
                && entry["references"].as_array().is_some_and(|references| {
                    references.first().is_some_and(|reference| {
                        reference["source_attempt_id"] != final_work_attempt_id
                    })
                })
        })
        .expect("declared rework must retain an older review with stale source evidence");
    assert_eq!(stale_prior_review["references"][0]["state"], "stale");
    assert_ne!(
        stale_prior_review["references"][0]["source_attempt_id"],
        fresh["references"][0]["source_attempt_id"],
        "fresh re-entry must re-resolve evidence instead of inheriting the stale snapshot"
    );

    let after_reads = status(&production, &selector, &session_id, &mut number);
    assert_eq!(read_fingerprint(&after_reads), before_reads);

    set_item(
        &production,
        &selector,
        &session_id,
        &mut number,
        "note",
        "fresh decision note",
    );
    let ready_next = query(
        &production,
        &selector,
        &session_id,
        &mut number,
        "session.next",
        Map::new(),
    );
    assert_eq!(ready_next["readiness"]["can_advance"], true);
    assert!(
        ready_next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.decide"))
    );
    assert_eq!(
        ready_next["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|suggestion| suggestion["command"] == "session.decide")
            .count(),
        2
    );

    let durable_verbose = verbose_status(&production, &selector, &session_id, &mut number, None);
    let durable_next = query(
        &production,
        &selector,
        &session_id,
        &mut number,
        "session.next",
        Map::new(),
    );
    drop(production);
    drop(manager);

    let reopened_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(Arc::clone(&reopened_manager), "v2drw005-reopened");
    let reopened_verbose = query_after_cold_reopen(
        &reopened,
        &selector,
        &session_id,
        &mut number,
        "session.status",
        json!({"verbose": true}).as_object().unwrap().clone(),
    );
    assert_eq!(reopened_verbose, durable_verbose);
    let reopened_next = query_after_cold_reopen(
        &reopened,
        &selector,
        &session_id,
        &mut number,
        "session.next",
        Map::new(),
    );
    assert_eq!(reopened_next, durable_next);
}
