//! Production vertical coverage for V2RUN-004 clean action and decision retries.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::SessionId;
use podway_daemon::server::RequestDispatcherV1;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1,
};
use serde_json::{Map, Value, json};

const RETRY_PROCEDURE: &str = include_str!("fixtures/retry-procedure.yaml");

fn status_with(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
    verbose: bool,
) -> Map<String, Value> {
    let request = runtime::request(
        request_number,
        "session.status",
        selector,
        if verbose {
            json!({"verbose": true}).as_object().unwrap().clone()
        } else {
            Map::new()
        },
        "unused-retry-status-key",
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

fn next(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
) -> Map<String, Value> {
    let request = runtime::request(
        request_number,
        "session.next",
        selector,
        Map::new(),
        "unused-retry-next-key",
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
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.next")
}

fn counter<'a>(status: &'a Map<String, Value>, graph_node_id: &str) -> &'a Value {
    status["counters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|counter| counter["graph_node_id"] == graph_node_id)
        .unwrap()
}

fn raw_retry_request(
    request_number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    reason: String,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
) -> RequestEnvelopeV1 {
    let mut payload = json!({"reason": reason}).as_object().unwrap().clone();
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .unwrap(),
        client: ClientInfoV1::new("v2run004-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.retry").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new(idempotency_key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap()
}

fn unchanged_graph_cursor(before: &Map<String, Value>, after: &Map<String, Value>) {
    assert_eq!(after["session"]["revision"], before["session"]["revision"]);
    assert_eq!(after["trace_length"], before["trace_length"]);
    assert_eq!(after["current"], before["current"]);
    assert_eq!(
        after["queue"]["latest_workspace_sequence"],
        before["queue"]["latest_workspace_sequence"]
    );
    assert_eq!(after["queue"]["queued_count"], 0);
    assert_eq!(after["queue"]["pending_mutations"], false);
    assert!(after["queue"]["running_job_id"].is_null());
}

#[test]
fn v2run004_retry_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(RETRY_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2RUN-004 retry fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "retry-procedure.yaml",
        RETRY_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2RUN-004 retry fixture must pass structural vetting: {diagnostics:?}"
    );
}

#[test]
fn v2run004_production_retry_is_clean_durable_replayable_and_re_resolves_evidence() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("retry.yaml"), RETRY_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(RETRY_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let workspace_selector = runtime::selector(fixture.main());
    let runtime_manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(Arc::clone(&runtime_manager), "v2run004-production");

    let initialize = runtime::request(
        40_001,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2run004-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = runtime::request(
        40_002,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "retry.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "V2RUN-004 production retry"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run004-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&production, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    runtime::begin(
        &production,
        &workspace_selector,
        40_009,
        &session_id,
        Map::new(),
        "v2run004-begin",
    );

    runtime::mutate_item(
        &production,
        &workspace_selector,
        40_010,
        &session_id,
        "item.set",
        "result",
        json!({"value": "discarded first-attempt value"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run004-seed-old-action",
    );
    let action_before = status_with(&production, &workspace_selector, 40_020, &session_id, true);
    let old_action_attempt = action_before["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let action_retry = runtime::request(
        40_021,
        "session.retry",
        &workspace_selector,
        json!({"reason": "redo the action from clean state"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run004-retry-action",
        runtime::session_preconditions(&action_before),
    );
    let action_retry_once = runtime::dispatch(&production, &action_retry);
    let action_result = runtime::v2_result(action_retry_once.clone(), "session.retry");
    assert_eq!(action_result["schema"], "podway.stage-transition-result/v2");
    assert_eq!(action_result["transition"], "retry");
    assert_eq!(action_result["from_graph_node_id"], "work");
    assert_eq!(action_result["to_graph_node_id"], "work");
    assert_eq!(action_result["from_attempt_id"], old_action_attempt);
    assert_ne!(action_result["to_attempt_id"], old_action_attempt);
    assert_eq!(action_result["reason"], "redo the action from clean state");
    assert_eq!(action_result["session_state"], "running");
    assert_eq!(
        action_result["revision"],
        action_before["session"]["revision"].as_u64().unwrap() + 1
    );

    let action_replay_request = runtime::request(
        40_022,
        "session.retry",
        &workspace_selector,
        json!({"reason": "redo the action from clean state"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run004-retry-action",
        runtime::session_preconditions(&action_before),
    );
    let action_retry_replay = runtime::dispatch(&production, &action_replay_request);
    assert_eq!(
        runtime::response_request_id(&action_retry_replay),
        action_replay_request.0.request_id()
    );
    assert_eq!(
        runtime::without_request_id(&action_retry_replay),
        runtime::without_request_id(&action_retry_once)
    );

    let action_after = status_with(&production, &workspace_selector, 40_023, &session_id, true);
    let new_action_attempt = action_after["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(new_action_attempt, action_result["to_attempt_id"]);
    assert_eq!(action_after["current"]["attempt"]["attempt_number"], 2);
    assert_eq!(
        action_after["current_trace_history"]["entries"][0]["trace_sequence"],
        2
    );
    assert_eq!(action_after["current"]["missing_required_item_count"], 1);
    assert_eq!(action_after["items_total"], 0);
    assert!(action_after["item_values"].as_array().unwrap().is_empty());
    assert_eq!(counter(&action_after, "work")["attempt_count"], 2);
    assert_eq!(counter(&action_after, "work")["rework_traversal_count"], 0);
    let stale_action = &action_after["stale_attempt_history"]["entries"][0];
    assert_eq!(stale_action["attempt_id"], old_action_attempt);
    assert_eq!(stale_action["lifecycle"], "abandoned");
    assert_eq!(stale_action["validity"], "stale");
    assert_eq!(
        stale_action["terminal_reason"],
        "redo the action from clean state"
    );
    assert_eq!(stale_action["items"][0]["item_id"], "result");
    assert_eq!(
        stale_action["items"][0]["value"],
        "discarded first-attempt value"
    );

    runtime::mutate_item(
        &production,
        &workspace_selector,
        40_030,
        &session_id,
        "item.set",
        "result",
        json!({"value": "durable second-attempt value"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run004-set-fresh-action",
    );
    let action_ready = status_with(&production, &workspace_selector, 40_040, &session_id, false);
    let complete_action = runtime::request(
        40_041,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "v2run004-complete-action",
        runtime::session_preconditions(&action_ready),
    );
    let completed = runtime::v2_result(
        runtime::dispatch(&production, &complete_action),
        "session.complete",
    );
    assert_eq!(completed["to_graph_node_id"], "review");

    let decision_before_next = next(&production, &workspace_selector, 40_050, &session_id);
    assert_eq!(decision_before_next["node"]["graph_node_id"], "review");
    assert_eq!(decision_before_next["readback"][0]["state"], "resolved");
    assert_eq!(
        decision_before_next["readback"][0]["source_attempt_id"],
        new_action_attempt
    );
    assert_eq!(
        decision_before_next["readback"][0]["items"][0]["value"],
        "durable second-attempt value"
    );
    runtime::mutate_item(
        &production,
        &workspace_selector,
        40_060,
        &session_id,
        "item.set",
        "note",
        json!({"value": "discarded review note"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run004-seed-old-decision",
    );
    let decision_before = status_with(&production, &workspace_selector, 40_070, &session_id, true);
    let old_decision_attempt = decision_before["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let decision_retry = runtime::request(
        40_071,
        "session.retry",
        &workspace_selector,
        json!({"reason": "reconsider the decision cleanly"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run004-retry-decision",
        runtime::session_preconditions(&decision_before),
    );
    let decision_retry_once = runtime::dispatch(&production, &decision_retry);
    let decision_result = runtime::v2_result(decision_retry_once.clone(), "session.retry");
    assert_eq!(decision_result["from_graph_node_id"], "review");
    assert_eq!(decision_result["to_graph_node_id"], "review");
    assert_eq!(decision_result["from_attempt_id"], old_decision_attempt);
    assert_ne!(decision_result["to_attempt_id"], old_decision_attempt);
    assert_eq!(decision_result["reason"], "reconsider the decision cleanly");

    let decision_after = status_with(&production, &workspace_selector, 40_072, &session_id, true);
    assert_eq!(decision_after["current"]["attempt"]["attempt_number"], 2);
    assert_eq!(decision_after["items_total"], 0);
    assert!(decision_after["item_values"].as_array().unwrap().is_empty());
    assert_eq!(counter(&decision_after, "review")["attempt_count"], 2);
    assert_eq!(
        counter(&decision_after, "review")["rework_traversal_count"],
        0
    );
    let fresh_decision_next = next(&production, &workspace_selector, 40_073, &session_id);
    assert_eq!(
        fresh_decision_next["readback"], decision_before_next["readback"],
        "retry must independently resolve the fresh decision attempt against immutable history"
    );
    let stale_decision = &decision_after["stale_attempt_history"]["entries"][0];
    assert_eq!(stale_decision["attempt_id"], old_decision_attempt);
    assert_eq!(
        stale_decision["terminal_reason"],
        "reconsider the decision cleanly"
    );
    assert_eq!(stale_decision["items"][0]["value"], "discarded review note");

    fs::remove_file(fixture.main().join("retry.yaml")).unwrap();
    drop(production);
    drop(runtime_manager);
    let restarted_manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let restarted = runtime::dispatcher(
        Arc::clone(&restarted_manager),
        "v2run004-production-restarted",
    );
    let decision_replay_request = runtime::request(
        40_074,
        "session.retry",
        &workspace_selector,
        json!({"reason": "reconsider the decision cleanly"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run004-retry-decision",
        runtime::session_preconditions(&decision_before),
    );
    let decision_retry_replay = runtime::dispatch(&restarted, &decision_replay_request);
    assert_eq!(
        runtime::response_request_id(&decision_retry_replay),
        decision_replay_request.0.request_id()
    );
    assert_eq!(
        runtime::without_request_id(&decision_retry_replay),
        runtime::without_request_id(&decision_retry_once)
    );
    assert_eq!(
        next(&restarted, &workspace_selector, 40_075, &session_id)["readback"],
        decision_before_next["readback"]
    );
}

#[test]
fn v2run004_retry_reason_uses_the_v2_unicode_scalar_boundary_before_admission() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("retry.yaml"), RETRY_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(RETRY_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let workspace_selector = runtime::selector(fixture.main());
    let runtime_manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(runtime_manager, "v2run004-reason-boundary");
    let initialize = runtime::request(
        40_201,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2run004-boundary-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = runtime::request(
        40_202,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "retry.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "V2RUN-004 retry reason boundary"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run004-boundary-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&production, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap();
    runtime::begin(
        &production,
        &workspace_selector,
        40_299,
        session_id,
        Map::new(),
        "v2run004-boundary-begin",
    );
    let before = status_with(&production, &workspace_selector, 40_203, session_id, false);

    let too_long = raw_retry_request(
        40_204,
        &workspace_selector,
        "한".repeat(2_001),
        "v2run004-boundary-too-long",
        runtime::session_preconditions(&before),
    );
    let too_long_daemon = podway_daemon::server::DaemonRequestV1::from_envelope(&too_long)
        .expect("the general wire request must admit a 2,001-scalar reason");
    let ResponseEnvelopeV2::Error(too_long_error) =
        production.dispatch_daemon(&too_long, &too_long_daemon)
    else {
        panic!("a 2,001-scalar Procedure v2 retry reason must be rejected")
    };
    assert_eq!(too_long_error.code().as_str(), "REQUEST_INVALID");
    assert_eq!(too_long_error.details()["admission"]["admitted"], false);
    let after_too_long = status_with(&production, &workspace_selector, 40_205, session_id, false);
    unchanged_graph_cursor(&before, &after_too_long);

    let blank = raw_retry_request(
        40_206,
        &workspace_selector,
        " \t\n".to_owned(),
        "v2run004-boundary-blank",
        runtime::session_preconditions(&before),
    );
    assert!(
        podway_daemon::server::DaemonRequestV1::from_envelope(&blank).is_err(),
        "the daemon transport maps a blank retry reason parse failure to REQUEST_INVALID"
    );
    let after_blank = status_with(&production, &workspace_selector, 40_207, session_id, false);
    unchanged_graph_cursor(&before, &after_blank);

    let maximum = "한".repeat(2_000);
    assert_eq!(maximum.chars().count(), 2_000);
    let accepted = raw_retry_request(
        40_208,
        &workspace_selector,
        maximum.clone(),
        "v2run004-boundary-maximum",
        runtime::session_preconditions(&before),
    );
    let accepted_daemon = podway_daemon::server::DaemonRequestV1::from_envelope(&accepted)
        .expect("the maximum v2 reason must pass transport parsing");
    let accepted_result = runtime::v2_result(
        production.dispatch_daemon(&accepted, &accepted_daemon),
        "session.retry",
    );
    assert_eq!(accepted_result["transition"], "retry");
    assert_eq!(accepted_result["reason"], maximum);
    assert_eq!(
        accepted_result["revision"],
        before["session"]["revision"].as_u64().unwrap() + 1
    );
}
