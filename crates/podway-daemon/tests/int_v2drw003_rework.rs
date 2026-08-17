//! Production vertical coverage for V2DRW-003 typed manual rework execution.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, path::Path, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AttemptId, Revision, SessionId};
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1,
};
use serde_json::{Map, Value, json};

const REWORK_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: manual-rework-runtime
version: "2"
name: Manual rework runtime
purpose: Exercise running and completed manual rework against a branched valid trace.
node_definitions:
  choose:
    type: decision
    title: Choose a branch
    objective: Select the branch used by the test.
    prompt: Which branch?
    evidence_guidance:
      - Select the branch required by the scenario.
    options:
      - id: left
        label: Left
      - id: right
        label: Right
    reason:
      required: true
  work:
    type: action
    title: Work
    intent: Complete the selected branch.
  finish:
    type: action
    title: Finish
    intent: Complete the session.
graph:
  entry: choose
  nodes:
    - id: choose
      use: choose
      routes:
        left:
          to: left-work
          effect: advance
        right:
          to: right-work
          effect: advance
    - id: left-work
      use: work
      next: finish
    - id: right-work
      use: work
      next: finish
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - choose
    - left-work
    - right-work
"#;

fn typed_request(
    number: u64,
    command: &str,
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
        client: ClientInfoV1::new("v2drw003-test", "1", 1).unwrap(),
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
    assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    (envelope, daemon)
}

fn decide_request(
    number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    typed_request(
        number,
        "session.decide",
        selector,
        json!({
            "option_id": "left",
            "reason": "Use the left branch for manual rework coverage."
        })
        .as_object()
        .unwrap()
        .clone(),
        key,
        preconditions,
    )
}

#[allow(clippy::too_many_arguments)]
fn rework_request(
    number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    target: &str,
    reason: &str,
    actor: Option<&str>,
    key: &str,
    session_id: &str,
    revision: u64,
    attempt_id: Option<&str>,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let mut payload = json!({
        "target_graph_node_id": target,
        "reason": reason,
    })
    .as_object()
    .unwrap()
    .clone();
    if let Some(actor) = actor {
        payload.insert("actor".to_owned(), json!(actor));
    }
    typed_request(
        number,
        "session.rework",
        selector,
        payload,
        key,
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            Some(Revision::new(revision)),
            attempt_id.map(|value| AttemptId::new(value).unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    )
}

fn assert_error(response: ResponseEnvelopeV2, code: &str, admitted: bool) -> Value {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{code} must be returned as a public error")
    };
    assert_eq!(error.code().as_str(), code);
    assert_eq!(error.details()["admission"]["admitted"], admitted);
    serde_json::to_value(error).unwrap()
}

struct RunningFixture {
    selector: podway_protocol::WorktreeSelectorWireV1,
    session_id: String,
    last_sequence: u64,
}

fn start_left_branch(
    dispatcher: &impl RequestDispatcherV1,
    root: &Path,
    number: u64,
) -> RunningFixture {
    runtime::make_runtime_private(root);
    fs::write(root.join("manual-rework.yaml"), REWORK_PROCEDURE).unwrap();
    let selector = runtime::selector(root);
    let initialize = runtime::request(
        number,
        "workspace.init",
        &selector,
        Map::new(),
        &format!("v2drw003-init-{number}"),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(REWORK_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
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
            "procedure": "manual-rework.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2DRW-003 production manual rework"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2drw003-start-{number}"),
        PreconditionsV1::default(),
    );
    let session_id =
        runtime::v2_result(runtime::dispatch(dispatcher, &start), "session.start")["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
    runtime::begin(
        dispatcher,
        &selector,
        number + 2,
        &session_id,
        Map::new(),
        &format!("v2drw003-begin-{number}"),
    );
    let decision = runtime::status(dispatcher, &selector, number + 3, &session_id);
    let decide = decide_request(
        number + 4,
        &selector,
        &format!("v2drw003-decide-{number}"),
        runtime::session_preconditions(&decision),
    );
    let decided = runtime::v2_result(runtime::dispatch(dispatcher, &decide), "session.decide");
    assert_eq!(decided["target_graph_node_id"], "left-work");
    RunningFixture {
        selector,
        session_id,
        last_sequence: decided["admission"]["workspace_sequence"].as_u64().unwrap(),
    }
}

fn complete_current(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &RunningFixture,
    number: u64,
    key: &str,
) -> Map<String, Value> {
    let before = runtime::status(dispatcher, &fixture.selector, number, &fixture.session_id);
    let complete = runtime::request(
        number + 1,
        "session.complete",
        &fixture.selector,
        Map::new(),
        key,
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &complete), "session.complete")
}

fn verbose_status(
    dispatcher: &impl RequestDispatcherV1,
    fixture: &RunningFixture,
    number: u64,
) -> Map<String, Value> {
    let request = runtime::request(
        number,
        "session.status",
        &fixture.selector,
        json!({"verbose": true}).as_object().unwrap().clone(),
        "unused-v2drw003-status-key",
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
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.status")
}

#[test]
fn v2drw003_running_rework_enforces_targets_and_fences_then_records_manual_transition() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2drw003-running");
    let fixture = start_left_branch(&production, workspace.main(), 93_000);
    let before = runtime::status(&production, &fixture.selector, 93_010, &fixture.session_id);
    let revision = before["session"]["revision"].as_u64().unwrap();
    let attempt_id = before["current"]["attempt"]["attempt_id"].as_str().unwrap();

    let missing_attempt = rework_request(
        93_011,
        &fixture.selector,
        "choose",
        "Running rework requires the active attempt fence.",
        None,
        "v2drw003-missing-attempt",
        &fixture.session_id,
        revision,
        None,
    );
    assert_error(
        runtime::dispatch(&production, &missing_attempt),
        "REQUEST_INVALID",
        false,
    );

    let stale_attempt = rework_request(
        93_012,
        &fixture.selector,
        "choose",
        "Reject a stale active attempt fence.",
        None,
        "v2drw003-stale-attempt",
        &fixture.session_id,
        revision,
        Some("00000000-0000-4000-8000-000000093012"),
    );
    let stale_attempt_error = assert_error(
        runtime::dispatch(&production, &stale_attempt),
        "ATTEMPT_NOT_CURRENT",
        false,
    );
    assert_eq!(
        stale_attempt_error["details"]["admission"],
        json!({"admitted": false})
    );

    let stale_revision = rework_request(
        93_013,
        &fixture.selector,
        "choose",
        "Reject a stale session revision fence.",
        None,
        "v2drw003-stale-revision",
        &fixture.session_id,
        revision + 1,
        Some(attempt_id),
    );
    let stale_revision_error = assert_error(
        runtime::dispatch(&production, &stale_revision),
        "SESSION_REVISION_CONFLICT",
        false,
    );
    assert_eq!(
        stale_revision_error["details"]["admission"],
        json!({"admitted": false})
    );

    let outside_allowlist = rework_request(
        93_014,
        &fixture.selector,
        "finish",
        "The terminal node is deliberately outside the allowlist.",
        None,
        "v2drw003-not-allowed",
        &fixture.session_id,
        revision,
        Some(attempt_id),
    );
    let not_allowed = assert_error(
        runtime::dispatch(&production, &outside_allowlist),
        "MANUAL_REWORK_TARGET_NOT_ALLOWED",
        true,
    );
    assert_eq!(not_allowed["details"]["target_graph_node_id"], "finish");

    let unvisited = rework_request(
        93_015,
        &fixture.selector,
        "right-work",
        "The allowed target has not been traversed.",
        None,
        "v2drw003-not-on-trace",
        &fixture.session_id,
        revision,
        Some(attempt_id),
    );
    let not_on_trace = assert_error(
        runtime::dispatch(&production, &unvisited),
        "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
        true,
    );
    assert_eq!(
        not_on_trace["details"]["target_graph_node_id"],
        "right-work"
    );

    let unchanged = runtime::status(&production, &fixture.selector, 93_016, &fixture.session_id);
    assert_eq!(
        unchanged["session"]["revision"],
        before["session"]["revision"]
    );
    assert_eq!(unchanged["current"], before["current"]);
    assert_eq!(unchanged["queue"]["queued_count"], 0);
    assert_eq!(unchanged["queue"]["pending_mutations"], false);

    let rework = rework_request(
        93_020,
        &fixture.selector,
        "choose",
        "Return to the branch decision after a new finding.",
        Some("manual reviewer"),
        "v2drw003-running-success",
        &fixture.session_id,
        revision,
        Some(attempt_id),
    );
    let response = runtime::dispatch(&production, &rework);
    let result = runtime::v2_result(response, "session.rework");
    assert_eq!(
        result.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "admission",
            "from_graph_node_id",
            "reactivated",
            "reason",
            "revision",
            "schema",
            "target_attempt_id",
            "to_graph_node_id",
        ]
    );
    assert_eq!(result["schema"], "podway.rework-result/v1");
    assert_eq!(result["admission"]["admitted"], true);
    assert_eq!(
        result["admission"]["workspace_sequence"],
        fixture.last_sequence + 3,
        "only the two admitted policy failures may precede the successful rework job"
    );
    assert_eq!(result["from_graph_node_id"], "left-work");
    assert_eq!(result["to_graph_node_id"], "choose");
    assert_eq!(
        result["reason"],
        "Return to the branch decision after a new finding."
    );
    assert_eq!(result["reactivated"], false);
    assert_eq!(result["revision"], revision + 1);

    let after = verbose_status(&production, &fixture, 93_021);
    assert_eq!(
        after["current"]["attempt"]["attempt_id"],
        result["target_attempt_id"]
    );
    assert_eq!(after["current"]["node"]["graph_node_id"], "choose");
    let record = &after["rework_history"]["entries"][0];
    assert_eq!(record["kind"], "manual");
    assert_eq!(record["from_graph_node_id"], "left-work");
    assert_eq!(record["to_graph_node_id"], "choose");
    assert_eq!(record["target_attempt_id"], result["target_attempt_id"]);
    assert_eq!(record["reactivated"], false);
    assert_eq!(record["actor"], "manual reviewer");
}

#[test]
fn v2drw003_completed_reactivation_cold_replays_and_rejects_attempt_or_payload_drift() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager_root = workspace.temporary_path().to_path_buf();
    let first_manager = Arc::new(runtime::manager(&manager_root));
    let production = runtime::dispatcher(Arc::clone(&first_manager), "v2drw003-completed");
    let fixture = start_left_branch(&production, workspace.main(), 94_000);
    let left = complete_current(&production, &fixture, 94_010, "v2drw003-complete-left");
    assert_eq!(left["to_graph_node_id"], "finish");
    let before_finish =
        runtime::status(&production, &fixture.selector, 94_020, &fixture.session_id);
    let finish_attempt = before_finish["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let completed = complete_current(&production, &fixture, 94_021, "v2drw003-complete-finish");
    assert_eq!(completed["session_state"], "completed");
    let completed_revision = completed["revision"].as_u64().unwrap();

    let supplied_attempt = rework_request(
        94_030,
        &fixture.selector,
        "choose",
        "A completed session must not carry an active attempt fence.",
        None,
        "v2drw003-completed-attempt",
        &fixture.session_id,
        completed_revision,
        Some(&finish_attempt),
    );
    assert_error(
        runtime::dispatch(&production, &supplied_attempt),
        "REQUEST_INVALID",
        false,
    );

    let rework = rework_request(
        94_040,
        &fixture.selector,
        "choose",
        "Reactivate the completed session for corrected work.",
        Some("completion reviewer"),
        "v2drw003-completed-success",
        &fixture.session_id,
        completed_revision,
        None,
    );
    let first_response = runtime::dispatch(&production, &rework);
    let result = runtime::v2_result(first_response.clone(), "session.rework");
    assert_eq!(result["schema"], "podway.rework-result/v1");
    assert_eq!(result["from_graph_node_id"], "finish");
    assert_eq!(result["to_graph_node_id"], "choose");
    assert_eq!(result["reactivated"], true);
    assert_eq!(result["revision"], completed_revision + 1);

    drop(production);
    drop(first_manager);
    let reopened_manager = Arc::new(runtime::manager(&manager_root));
    let reopened = runtime::dispatcher(Arc::clone(&reopened_manager), "v2drw003-reopened");
    let replay = rework_request(
        94_041,
        &fixture.selector,
        "choose",
        "Reactivate the completed session for corrected work.",
        Some("completion reviewer"),
        "v2drw003-completed-success",
        &fixture.session_id,
        completed_revision,
        None,
    );
    let replayed = runtime::dispatch_after_cold_reopen(&reopened, &replay);
    assert_eq!(
        runtime::without_request_id(&replayed),
        runtime::without_request_id(&first_response),
        "a real cold manager reopen must preserve the frozen manual rework response"
    );

    let payload_drift = rework_request(
        94_042,
        &fixture.selector,
        "left-work",
        "Reuse the key with a different manual target.",
        Some("completion reviewer"),
        "v2drw003-completed-success",
        &fixture.session_id,
        completed_revision,
        None,
    );
    assert_error(
        runtime::dispatch(&reopened, &payload_drift),
        "IDEMPOTENCY_KEY_REUSED",
        false,
    );

    let after = verbose_status(&reopened, &fixture, 94_050);
    assert_eq!(after["session"]["lifecycle"], "running");
    assert_eq!(after["current"]["node"]["graph_node_id"], "choose");
    assert_eq!(
        after["current"]["attempt"]["attempt_id"],
        result["target_attempt_id"]
    );
    let record = &after["rework_history"]["entries"][0];
    assert_eq!(record["kind"], "manual");
    assert_eq!(record["reactivated"], true);
    assert_eq!(record["actor"], "completion reviewer");
}

#[test]
fn v2drw003_cancelled_session_rejects_manual_rework() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2drw003-cancelled");
    let fixture = start_left_branch(&production, workspace.main(), 95_000);
    let before = runtime::status(&production, &fixture.selector, 95_010, &fixture.session_id);
    let cancel = runtime::request(
        95_011,
        "session.cancel",
        &fixture.selector,
        json!({"reason": "Cancel before testing rework rejection."})
            .as_object()
            .unwrap()
            .clone(),
        "v2drw003-cancel",
        runtime::session_preconditions(&before),
    );
    let cancelled = runtime::v2_result(runtime::dispatch(&production, &cancel), "session.cancel");
    assert_eq!(cancelled["session_state"], "cancelled");
    let revision = cancelled["revision"].as_u64().unwrap();
    let rework = rework_request(
        95_020,
        &fixture.selector,
        "choose",
        "Cancelled sessions cannot be manually reactivated.",
        None,
        "v2drw003-cancelled-rework",
        &fixture.session_id,
        revision,
        None,
    );
    assert_error(
        runtime::dispatch(&production, &rework),
        "SESSION_CANCELLED",
        true,
    );
}
