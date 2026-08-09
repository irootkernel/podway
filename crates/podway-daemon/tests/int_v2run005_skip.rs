//! Production vertical coverage for V2RUN-005 eligible action skips.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::SessionId;
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    StatusResultV1, WorkspaceContextV1,
};
use serde_json::{Map, Value, json};

const SKIP_PROCEDURE: &str = include_str!("fixtures/skip-procedure.yaml");
const DECISION_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: skip-decision-runtime
version: "2"
name: Skip decision runtime
purpose: Prove that decision placements reject the action-only skip transition.
node_definitions:
  choose:
    type: decision
    title: Choose
    objective: Select the only route.
    prompt: Continue?
    options:
      - id: yes
        label: Yes
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the decision fixture.
graph:
  entry: choose
  nodes:
    - id: choose
      use: choose
      routes:
        yes:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
"#;
const V1_SKIP_PROCEDURE: &str = r#"schema: podway.procedure/v1
id: retained-v1-skip
version: "1"
name: Retained v1 skip
stages:
  - id: only
    title: Only
    instructions: []
    skip:
      allowed: true
      reason_required: true
    items: []
rework:
  allow_return_to: any_previous
"#;

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
        "unused-skip-status-key",
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
        "unused-skip-next-key",
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

fn raw_skip_request(
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
        client: ClientInfoV1::new("v2run005-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.skip").unwrap(),
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
    assert_eq!(after["queue"]["queued_count"], 0);
    assert_eq!(after["queue"]["pending_mutations"], false);
    assert!(after["queue"]["running_job_id"].is_null());
}

#[test]
fn v2run005_skip_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(SKIP_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("the V2RUN-005 skip fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "skip-procedure.yaml",
        SKIP_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2RUN-005 skip fixture must pass structural vetting: {diagnostics:?}"
    );
}

#[test]
fn v2run005_production_skip_discards_values_advances_terminates_restarts_and_replays() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("skip.yaml"), SKIP_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(SKIP_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2run005-production");

    let initialize = runtime::request(
        50_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2run005-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let start = runtime::request(
        50_002,
        "session.start",
        &selector,
        json!({
            "procedure": "skip.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "V2RUN-005 production skip"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run005-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&production, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();

    let prepare = status_with(&production, &selector, 50_003, &session_id, false);
    let rejected_prepare = runtime::request(
        50_004,
        "session.skip",
        &selector,
        json!({"reason": "not eligible"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run005-reject-prepare",
        runtime::session_preconditions(&prepare),
    );
    let ResponseEnvelopeV2::Error(error) = runtime::dispatch(&production, &rejected_prepare) else {
        panic!("a placement without a skip policy must reject session.skip")
    };
    assert_eq!(error.code().as_str(), "STAGE_NOT_SKIPPABLE");
    let after_rejected_prepare = status_with(&production, &selector, 50_005, &session_id, false);
    unchanged_graph_cursor(&prepare, &after_rejected_prepare);
    assert_eq!(
        after_rejected_prepare["queue"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        prepare["queue"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap()
            + 1
    );

    let complete_prepare = runtime::request(
        50_006,
        "session.complete",
        &selector,
        Map::new(),
        "v2run005-complete-prepare",
        runtime::session_preconditions(&prepare),
    );
    let completed_prepare = runtime::v2_result(
        runtime::dispatch(&production, &complete_prepare),
        "session.complete",
    );
    assert_eq!(completed_prepare["to_graph_node_id"], "source");

    runtime::mutate_item(
        &production,
        &selector,
        50_010,
        &session_id,
        "item.set",
        "optional-note",
        json!({"value": "must be discarded"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run005-set-optional",
    );
    let source = status_with(&production, &selector, 50_020, &session_id, true);
    assert_eq!(source["current"]["missing_required_item_count"], 1);
    assert_eq!(source["items_total"], 1);
    assert_eq!(source["item_values"][0]["value"], "must be discarded");
    let source_attempt_id = source["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let missing_reason = runtime::request(
        50_021,
        "session.skip",
        &selector,
        Map::new(),
        "v2run005-missing-reason",
        runtime::session_preconditions(&source),
    );
    let ResponseEnvelopeV2::Error(error) = runtime::dispatch(&production, &missing_reason) else {
        panic!("a reason-required skip policy must reject an omitted reason")
    };
    assert_eq!(error.code().as_str(), "STAGE_NOT_SKIPPABLE");
    let after_missing_reason = status_with(&production, &selector, 50_022, &session_id, true);
    unchanged_graph_cursor(&source, &after_missing_reason);
    assert_eq!(
        after_missing_reason["queue"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        source["queue"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap()
            + 1
    );

    let skip_source = runtime::request(
        50_023,
        "session.skip",
        &selector,
        json!({"reason": "source is not applicable"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run005-skip-source",
        runtime::session_preconditions(&source),
    );
    let skip_once = runtime::dispatch(&production, &skip_source);
    let skipped = runtime::v2_result(skip_once.clone(), "session.skip");
    assert_eq!(skipped["schema"], "podway.stage-transition-result/v2");
    assert_eq!(skipped["transition"], "skip");
    assert_eq!(skipped["from_graph_node_id"], "source");
    assert_eq!(skipped["to_graph_node_id"], "consumer");
    assert_eq!(skipped["from_attempt_id"], source_attempt_id);
    assert_eq!(skipped["reason"], "source is not applicable");
    assert_eq!(skipped["session_state"], "running");

    let replay_request = runtime::request(
        50_024,
        "session.skip",
        &selector,
        json!({"reason": "source is not applicable"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run005-skip-source",
        runtime::session_preconditions(&source),
    );
    let replay = runtime::dispatch(&production, &replay_request);
    assert_eq!(
        runtime::response_request_id(&replay),
        replay_request.0.request_id()
    );
    assert_eq!(
        runtime::without_request_id(&replay),
        runtime::without_request_id(&skip_once)
    );

    let consumer = status_with(&production, &selector, 50_025, &session_id, true);
    let skipped_history = consumer["current_trace_history"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["graph_node_id"] == "source")
        .unwrap();
    assert_eq!(skipped_history["attempt_id"], source_attempt_id);
    assert_eq!(skipped_history["lifecycle"], "skipped");
    assert_eq!(skipped_history["validity"], "valid");
    let before_restart_next = next(&production, &selector, 50_026, &session_id);
    assert_eq!(before_restart_next["readback"][0]["state"], "skipped");
    assert_eq!(
        before_restart_next["readback"][0]["source_attempt_id"],
        source_attempt_id
    );
    assert!(
        before_restart_next["readback"][0]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    fs::remove_file(fixture.main().join("skip.yaml")).unwrap();
    drop(production);
    drop(manager);
    let restarted_manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let restarted = runtime::dispatcher(restarted_manager, "v2run005-restarted");
    let restarted_replay_request = runtime::request(
        50_027,
        "session.skip",
        &selector,
        json!({"reason": "source is not applicable"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run005-skip-source",
        runtime::session_preconditions(&source),
    );
    let restarted_replay = runtime::dispatch(&restarted, &restarted_replay_request);
    assert_eq!(
        runtime::without_request_id(&restarted_replay),
        runtime::without_request_id(&skip_once)
    );
    assert_eq!(
        next(&restarted, &selector, 50_028, &session_id)["readback"],
        before_restart_next["readback"]
    );

    let consumer = status_with(&restarted, &selector, 50_029, &session_id, false);
    let skip_consumer = runtime::request(
        50_030,
        "session.skip",
        &selector,
        Map::new(),
        "v2run005-skip-consumer",
        runtime::session_preconditions(&consumer),
    );
    let skipped_consumer = runtime::v2_result(
        runtime::dispatch(&restarted, &skip_consumer),
        "session.skip",
    );
    assert_eq!(skipped_consumer["to_graph_node_id"], "finish");
    assert!(skipped_consumer.get("reason").is_none());

    let finish = status_with(&restarted, &selector, 50_031, &session_id, false);
    let skip_finish = runtime::request(
        50_032,
        "session.skip",
        &selector,
        Map::new(),
        "v2run005-skip-finish",
        runtime::session_preconditions(&finish),
    );
    let terminal = runtime::v2_result(runtime::dispatch(&restarted, &skip_finish), "session.skip");
    assert_eq!(terminal["transition"], "skip");
    assert_eq!(terminal["from_graph_node_id"], "finish");
    assert_eq!(terminal["session_state"], "completed");
    assert!(terminal.get("to_graph_node_id").is_none());
    assert!(terminal.get("to_attempt_id").is_none());
}

#[test]
fn v2run005_skip_reason_uses_the_v2_unicode_scalar_boundary_before_admission() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("skip.yaml"), SKIP_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(SKIP_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(manager, "v2run005-reason-boundary");
    let initialize = runtime::request(
        50_201,
        "workspace.init",
        &selector,
        Map::new(),
        "v2run005-boundary-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let start = runtime::request(
        50_202,
        "session.start",
        &selector,
        json!({
            "procedure": "skip.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "V2RUN-005 skip reason boundary"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run005-boundary-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&production, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap();
    let prepare = status_with(&production, &selector, 50_203, session_id, false);
    let complete_prepare = runtime::request(
        50_204,
        "session.complete",
        &selector,
        Map::new(),
        "v2run005-boundary-complete-prepare",
        runtime::session_preconditions(&prepare),
    );
    runtime::v2_result(
        runtime::dispatch(&production, &complete_prepare),
        "session.complete",
    );
    let source = status_with(&production, &selector, 50_205, session_id, false);

    let too_long = raw_skip_request(
        50_206,
        &selector,
        "한".repeat(2_001),
        "v2run005-boundary-too-long",
        runtime::session_preconditions(&source),
    );
    let too_long_daemon = DaemonRequestV1::from_envelope(&too_long)
        .expect("the retained v1 wire contract must admit a 2,001-scalar reason");
    let ResponseEnvelopeV2::Error(error) = production.dispatch_daemon(&too_long, &too_long_daemon)
    else {
        panic!("a 2,001-scalar Procedure v2 skip reason must be rejected")
    };
    assert_eq!(error.code().as_str(), "REQUEST_INVALID");
    assert_eq!(error.details()["admission"]["admitted"], false);
    unchanged_graph_cursor(
        &source,
        &status_with(&production, &selector, 50_207, session_id, false),
    );

    let blank = raw_skip_request(
        50_208,
        &selector,
        " \t\n".to_owned(),
        "v2run005-boundary-blank",
        runtime::session_preconditions(&source),
    );
    assert!(
        DaemonRequestV1::from_envelope(&blank).is_err(),
        "the daemon transport must reject a blank skip reason"
    );
    unchanged_graph_cursor(
        &source,
        &status_with(&production, &selector, 50_209, session_id, false),
    );

    let maximum = "한".repeat(2_000);
    let accepted = raw_skip_request(
        50_210,
        &selector,
        maximum.clone(),
        "v2run005-boundary-maximum",
        runtime::session_preconditions(&source),
    );
    let accepted_daemon = DaemonRequestV1::from_envelope(&accepted)
        .expect("the maximum v2 skip reason must pass transport parsing");
    let result = runtime::v2_result(
        production.dispatch_daemon(&accepted, &accepted_daemon),
        "session.skip",
    );
    assert_eq!(result["transition"], "skip");
    assert_eq!(result["reason"], maximum);
}

#[test]
fn v2run005_production_rejects_decision_skip_and_retains_v1_output() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    runtime::make_runtime_private(fixture.linked());
    fs::write(fixture.main().join("decision.yaml"), DECISION_PROCEDURE).unwrap();
    fs::write(fixture.linked().join("v1-skip.yaml"), V1_SKIP_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(DECISION_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let decision_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let main_selector = runtime::selector(fixture.main());
    let linked_selector = runtime::selector(fixture.linked());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(manager, "v2run005-compatibility");

    for (number, selector, key) in [
        (50_301, &main_selector, "v2run005-decision-initialize"),
        (50_302, &linked_selector, "v2run005-v1-initialize"),
    ] {
        let initialize = runtime::request(
            number,
            "workspace.init",
            selector,
            Map::new(),
            key,
            PreconditionsV1::default(),
        );
        assert!(matches!(
            runtime::dispatch(&production, &initialize),
            ResponseEnvelopeV2::OutputV1(_)
        ));
    }

    let start_decision = runtime::request(
        50_303,
        "session.start",
        &main_selector,
        json!({
            "procedure": "decision.yaml",
            "expected_procedure_digest": decision_digest,
            "task_title": "V2RUN-005 decision rejection"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run005-decision-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(
        runtime::dispatch(&production, &start_decision),
        "session.start",
    );
    let decision_session_id = started["session_id"].as_str().unwrap();
    let decision_status = status_with(
        &production,
        &main_selector,
        50_304,
        decision_session_id,
        false,
    );
    let skip_decision = runtime::request(
        50_305,
        "session.skip",
        &main_selector,
        json!({"reason": "not an action"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run005-skip-decision",
        runtime::session_preconditions(&decision_status),
    );
    let ResponseEnvelopeV2::Error(error) = runtime::dispatch(&production, &skip_decision) else {
        panic!("session.skip must reject an active decision placement")
    };
    assert_eq!(error.code().as_str(), "GRAPH_NODE_TYPE_MISMATCH");
    unchanged_graph_cursor(
        &decision_status,
        &status_with(
            &production,
            &main_selector,
            50_306,
            decision_session_id,
            false,
        ),
    );

    let start_v1 = runtime::request(
        50_307,
        "session.start",
        &linked_selector,
        json!({
            "procedure": "v1-skip.yaml",
            "task_title": "Retained v1 skip"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run005-v1-start",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(&production, &start_v1),
        ResponseEnvelopeV2::OutputV1(_)
    ));
    let status_v1_request = runtime::request(
        50_308,
        "session.status",
        &linked_selector,
        Map::new(),
        "unused-v1-status-key",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV1(status_output) =
        runtime::dispatch(&production, &status_v1_request)
    else {
        panic!("a retained Procedure v1 session must return podway.output/v1")
    };
    let status = StatusResultV1::from_result_map(status_output.result()).unwrap();
    let current = status.current.as_ref().unwrap();
    let preconditions = PreconditionsV1::new(
        Some(status.session.id.clone()),
        Some(status.session.revision),
        Some(current.attempt_id.clone()),
        None,
        None,
        None,
    )
    .unwrap();
    let skip_v1 = raw_skip_request(
        50_309,
        &linked_selector,
        "v".repeat(3_000),
        "v2run005-retained-v1-skip",
        preconditions,
    );
    let daemon_v1 = DaemonRequestV1::from_envelope(&skip_v1).unwrap();
    assert!(matches!(
        production.dispatch_daemon(&skip_v1, &daemon_v1),
        ResponseEnvelopeV2::OutputV1(_)
    ));
}
