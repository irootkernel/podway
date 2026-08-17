//! Production vertical coverage for V2RUN-006 derived terminal and blocked states.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{AttemptId, GoalRevisionNumberV2, Revision, SessionId};
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1, validate_command_result_v2,
};
use serde_json::{Map, Value, json};

const STATE_PROCEDURE: &str = include_str!("fixtures/state-derivation-procedure.yaml");
fn status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
) -> Map<String, Value> {
    let request = runtime::request(
        request_number,
        "session.status",
        selector,
        Map::new(),
        "unused-v2run006-status",
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

fn verbose_status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
) -> Map<String, Value> {
    let request = runtime::request(
        request_number,
        "session.status",
        selector,
        json!({"verbose": true}).as_object().unwrap().clone(),
        "unused-v2run006-verbose-status",
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

fn dry_run_reset_request(
    request_number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    let mut payload = json!({"dry_run": true}).as_object().unwrap().clone();
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .unwrap(),
        client: ClientInfoV1::new("v2run006-test", "1", 1).unwrap(),
        operation: OperationV1::Query,
        command: CommandNameV1::new("session.reset").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: None,
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    (envelope, daemon)
}

fn typed_mutation_request(
    request_number: u64,
    command: &str,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    mut payload: Map<String, Value>,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .unwrap(),
        client: ClientInfoV1::new("v2run006-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).unwrap(),
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

fn assert_absent_session_mismatch(response: ResponseEnvelopeV2, session_id: &str) {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("an expected session with no current session must be rejected")
    };
    assert_eq!(error.code().as_str(), "SESSION_ID_MISMATCH");
    assert_eq!(error.exit_code().get(), 4);
    assert!(!error.retryable());
    assert_eq!(
        error.details()["schema"],
        "podway.session-id-mismatch-details/v2"
    );
    assert_eq!(error.details()["expected_session_id"], session_id);
    assert!(error.details()["actual_session_id"].is_null());
    assert_eq!(error.details()["admission"], json!({"admitted": false}));
}

fn assert_session_absent(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
) {
    let request = runtime::request(
        request_number,
        "session.status",
        selector,
        Map::new(),
        "unused-v2run006-absent-status",
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
    assert_absent_session_mismatch(runtime::dispatch(dispatcher, &request), session_id);
}

fn next_response(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
) -> ResponseEnvelopeV2 {
    let request = runtime::request(
        request_number,
        "session.next",
        selector,
        Map::new(),
        "unused-v2run006-next",
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
    runtime::dispatch(dispatcher, &request)
}

fn transition(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
    command: &str,
    payload: Map<String, Value>,
    key: &str,
) -> (Map<String, Value>, ResponseEnvelopeV2, Map<String, Value>) {
    let before = status(dispatcher, selector, request_number, session_id);
    let request = runtime::request(
        request_number + 1,
        command,
        selector,
        payload,
        key,
        runtime::session_preconditions(&before),
    );
    let response = runtime::dispatch(dispatcher, &request);
    let result = runtime::v2_result(response.clone(), command);
    assert_eq!(result["schema"], "podway.stage-transition-result/v2");
    (result, response, before)
}

fn start(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    procedure_digest: &podway_core::Sha256Digest,
    request_number: u64,
    title: &str,
) -> String {
    let initialize = runtime::request(
        request_number,
        "workspace.init",
        selector,
        Map::new(),
        &format!("v2run006-init-{request_number}"),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = runtime::request(
        request_number + 1,
        "session.start",
        selector,
        json!({
            "procedure": "states.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": title,
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2run006-start-{request_number}"),
        PreconditionsV1::default(),
    );
    let session_id =
        runtime::v2_result(runtime::dispatch(dispatcher, &start), "session.start")["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
    runtime::begin(
        dispatcher,
        selector,
        request_number + 2,
        &session_id,
        Map::new(),
        &format!("v2run006-begin-{request_number}"),
    );
    session_id
}

fn procedure_digest() -> podway_core::Sha256Digest {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(STATE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    validate_procedure_v2(parsed).unwrap().digest().clone()
}

fn assert_replay(
    original: &ResponseEnvelopeV2,
    replay: &ResponseEnvelopeV2,
    replay_request_id: &podway_protocol::RequestIdV1,
) {
    assert_eq!(runtime::response_request_id(replay), replay_request_id);
    assert_eq!(
        runtime::without_request_id(replay),
        runtime::without_request_id(original)
    );
}

#[test]
fn v2run006_state_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(STATE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2RUN-006 state fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "state-derivation-procedure.yaml",
        STATE_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    assert!(vet_procedure_v2(&validated, &context).is_empty());
}

#[test]
fn v2run006_reset_dry_run_result_accepts_a_not_admitted_projection() {
    let result = json!({
        "schema": "podway.session-reset-result/v1",
        "dry_run": true,
        "mode": "eligible",
        "session_id": "00000000-0000-4000-8000-000000006000",
        "session_revision": 1,
        "lifecycle": "running",
        "current_terminal_disposition": false,
        "eligible": false,
        "required_action": "force",
        "reset": false,
    });
    validate_command_result_v2("session.reset", result.as_object().unwrap()).unwrap();
}

#[test]
fn v2run006_blocked_state_unblocks_restarts_completes_and_resets() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("states.yaml"), STATE_PROCEDURE).unwrap();
    let selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2run006-blocked");
    let session_id = start(
        &production,
        &selector,
        &procedure_digest(),
        60_000,
        "V2RUN-006 blocked state",
    );
    runtime::mutate_item(
        &production,
        &selector,
        60_010,
        &session_id,
        "item.check",
        "ready",
        Map::new(),
        "v2run006-ready",
    );

    let (blocked, block_once, before_block) = transition(
        &production,
        &selector,
        60_020,
        &session_id,
        "session.block",
        json!({"reason": "Waiting for the dependency."})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-block",
    );
    assert_eq!(blocked["transition"], "block");
    assert_eq!(blocked["from_graph_node_id"], "work");
    assert_eq!(blocked["reason"], "Waiting for the dependency.");
    assert_eq!(blocked["session_state"], "running");
    let blocker_id = blocked["blocker_id"].as_str().unwrap().to_owned();
    let replay_request = runtime::request(
        60_022,
        "session.block",
        &selector,
        json!({"reason": "Waiting for the dependency."})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-block",
        runtime::session_preconditions(&before_block),
    );
    let replay = runtime::dispatch(&production, &replay_request);
    assert_replay(&block_once, &replay, replay_request.0.request_id());

    let blocked_status = status(&production, &selector, 60_023, &session_id);
    assert_eq!(
        blocked_status["current"]["readiness"]["items_satisfied"],
        true
    );
    assert_eq!(blocked_status["current"]["readiness"]["unblocked"], false);
    assert_eq!(blocked_status["current"]["readiness"]["can_advance"], false);
    assert_eq!(blocked_status["current"]["blockers_total"], 1);
    let blocked_next = runtime::v2_result(
        next_response(&production, &selector, 60_024, &session_id),
        "session.next",
    );
    assert!(
        !blocked_next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.complete"))
    );
    assert!(
        blocked_next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.skip"))
    );

    fs::remove_file(fixture.main().join("states.yaml")).unwrap();
    drop(production);
    drop(manager);
    let restarted_manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let restarted = runtime::dispatcher(restarted_manager, "v2run006-blocked-restarted");
    let restarted_replay_request = runtime::request(
        60_025,
        "session.block",
        &selector,
        json!({"reason": "Waiting for the dependency."})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-block",
        runtime::session_preconditions(&before_block),
    );
    let restarted_replay = runtime::dispatch(&restarted, &restarted_replay_request);
    assert_replay(
        &block_once,
        &restarted_replay,
        restarted_replay_request.0.request_id(),
    );
    assert_eq!(
        status(&restarted, &selector, 60_026, &session_id)["current"]["blockers_total"],
        1
    );

    let (unblocked, _, _) = transition(
        &restarted,
        &selector,
        60_027,
        &session_id,
        "session.unblock",
        json!({"blocker_id": blocker_id, "all": false})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-unblock-one",
    );
    assert_eq!(unblocked["transition"], "unblock");
    assert_eq!(unblocked["blocker_id"], blocker_id);
    assert_eq!(unblocked["session_state"], "running");
    let unblocked_status = status(&restarted, &selector, 60_030, &session_id);
    assert_eq!(unblocked_status["current"]["readiness"]["unblocked"], true);
    assert_eq!(
        unblocked_status["current"]["readiness"]["can_advance"],
        true
    );

    let (advanced, _, _) = transition(
        &restarted,
        &selector,
        60_031,
        &session_id,
        "session.complete",
        Map::new(),
        "v2run006-complete-work",
    );
    assert_eq!(advanced["to_graph_node_id"], "finish");
    let (completed, _, _) = transition(
        &restarted,
        &selector,
        60_033,
        &session_id,
        "session.complete",
        Map::new(),
        "v2run006-complete-finish",
    );
    assert_eq!(completed["session_state"], "completed");
    let terminal_status = status(&restarted, &selector, 60_035, &session_id);
    assert_eq!(terminal_status["session"]["lifecycle"], "completed");
    assert!(terminal_status["current"].is_null());
    assert!(terminal_status.get("latest_goal_outcome").is_none());
    let ResponseEnvelopeV2::Error(error) =
        next_response(&restarted, &selector, 60_036, &session_id)
    else {
        panic!("a completed Procedure v2 session must have no actionable next cursor")
    };
    assert_eq!(error.code().as_str(), "SESSION_NOT_RUNNING");

    let reset_before = terminal_status;
    let dry_run_request = dry_run_reset_request(
        60_037,
        &selector,
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(Revision::new(
                reset_before["session"]["revision"].as_u64().unwrap(),
            )),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let dry_run_response = runtime::dispatch(&restarted, &dry_run_request);
    let dry_run_value = serde_json::to_value(&dry_run_response).unwrap();
    assert_eq!(
        dry_run_value["schema"], "podway.output/v3",
        "v2 reset dry-run must project a non-admitted result: {dry_run_value}"
    );
    assert!(dry_run_value.get("job").is_none());
    assert_eq!(
        dry_run_value["result"]["schema"],
        "podway.session-reset-result/v1"
    );
    assert_eq!(dry_run_value["result"]["mode"], "eligible");
    assert_eq!(dry_run_value["result"]["lifecycle"], "completed");
    assert_eq!(
        dry_run_value["result"]["current_terminal_disposition"],
        false
    );
    assert_eq!(dry_run_value["result"]["eligible"], false);
    assert_eq!(
        dry_run_value["result"]["required_action"],
        "record_disposition"
    );
    assert_eq!(dry_run_value["result"]["reset"], false);
    validate_command_result_v2(
        "session.reset",
        dry_run_value["result"].as_object().unwrap(),
    )
    .unwrap();
    let after_dry_run = status(&restarted, &selector, 60_038, &session_id);
    assert_eq!(after_dry_run["session"], reset_before["session"]);
    assert_eq!(after_dry_run["current"], reset_before["current"]);
    assert_eq!(
        after_dry_run["queue"]["latest_workspace_sequence"],
        reset_before["queue"]["latest_workspace_sequence"]
    );

    let disposition_request = runtime::request(
        60_039,
        "session.terminal_disposition",
        &selector,
        json!({
            "kind": "not_required",
            "reason": "The isolated test session needs no external handoff."
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run006-dispose-completed",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(Revision::new(
                reset_before["session"]["revision"].as_u64().unwrap(),
            )),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let disposition = runtime::v2_result(
        runtime::dispatch(&restarted, &disposition_request),
        "session.terminal_disposition",
    );
    assert_eq!(disposition["disposition"]["kind"], "not_required");
    let disposed = status(&restarted, &selector, 60_390, &session_id);

    let reset_request = runtime::request(
        60_391,
        "session.reset",
        &selector,
        Map::new(),
        "v2run006-reset-completed",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(Revision::new(
                disposed["session"]["revision"].as_u64().unwrap(),
            )),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let reset_once = runtime::dispatch(&restarted, &reset_request);
    let reset = runtime::v2_result(reset_once.clone(), "session.reset");
    assert_eq!(reset["schema"], "podway.session-reset-result/v1");
    assert_eq!(reset["mode"], "eligible");
    assert_eq!(reset["eligible"], true);
    assert_eq!(reset["reset"], true);
    let reset_replay_request = runtime::request(
        60_392,
        "session.reset",
        &selector,
        Map::new(),
        "v2run006-reset-completed",
        reset_request.0.preconditions().clone(),
    );
    let reset_replay = runtime::dispatch(&restarted, &reset_replay_request);
    assert_replay(
        &reset_once,
        &reset_replay,
        reset_replay_request.0.request_id(),
    );
    assert_session_absent(&restarted, &selector, 60_041, &session_id);

    assert_absent_session_mismatch(
        next_response(&restarted, &selector, 60_042, &session_id),
        &session_id,
    );
    let status_without_identity = runtime::request(
        60_043,
        "session.status",
        &selector,
        Map::new(),
        "unused-v2run006-status-without-identity",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::Error(error) = runtime::dispatch(&restarted, &status_without_identity)
    else {
        panic!("a read without an expected identity must report the absent session")
    };
    assert_eq!(error.code().as_str(), "SESSION_NOT_FOUND");

    let absent_revision = Revision::new(reset_before["session"]["revision"].as_u64().unwrap());
    let session_identity = PreconditionsV1::new(
        Some(SessionId::new(&session_id).unwrap()),
        Some(absent_revision),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let absent_dry_run = dry_run_reset_request(60_044, &selector, session_identity.clone());
    assert_absent_session_mismatch(runtime::dispatch(&restarted, &absent_dry_run), &session_id);

    let absent_reset = runtime::request(
        60_045,
        "session.reset",
        &selector,
        Map::new(),
        "v2run006-reset-absent",
        session_identity.clone(),
    );
    assert_absent_session_mismatch(runtime::dispatch(&restarted, &absent_reset), &session_id);

    let absent_attempt = AttemptId::new("00000000-0000-4000-8000-000000006006").unwrap();
    let attempt_preconditions = PreconditionsV1::new(
        Some(SessionId::new(&session_id).unwrap()),
        Some(absent_revision),
        Some(absent_attempt),
        None,
        None,
        None,
    )
    .unwrap();
    let typed_mutations = [
        typed_mutation_request(
            60_046,
            "session.decide",
            &selector,
            json!({"option_id": "accept", "reason": "Reject an absent session."})
                .as_object()
                .unwrap()
                .clone(),
            "v2run006-decide-absent",
            attempt_preconditions.clone(),
        ),
        typed_mutation_request(
            60_047,
            "session.rework",
            &selector,
            json!({
                "target_graph_node_id": "work",
                "reason": "Reject an absent session."
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2run006-rework-absent",
            session_identity.clone(),
        ),
        typed_mutation_request(
            60_048,
            "goal.define",
            &selector,
            json!({
                "goal": "Reject an absent session.",
                "criteria": [{
                    "criterion_id": "verified",
                    "statement": "The mutation is rejected before admission."
                }]
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2run006-goal-define-absent",
            session_identity.clone(),
        ),
        typed_mutation_request(
            60_049,
            "goal.revise",
            &selector,
            json!({
                "goal": "Reject an absent session.",
                "criteria": [{
                    "criterion_id": "verified",
                    "statement": "The mutation is rejected before admission."
                }],
                "target_graph_node_id": "work",
                "reason": "Reject an absent session."
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2run006-goal-revise-absent",
            session_identity
                .clone()
                .with_goal_revision(GoalRevisionNumberV2::FIRST)
                .unwrap(),
        ),
        typed_mutation_request(
            60_050,
            "goal.assess_criterion",
            &selector,
            json!({
                "criterion_id": "verified",
                "status": "satisfied",
                "reason": "Reject an absent session."
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2run006-goal-assess-absent",
            attempt_preconditions
                .with_goal_revision(GoalRevisionNumberV2::FIRST)
                .unwrap(),
        ),
    ];
    for request in typed_mutations {
        assert_absent_session_mismatch(runtime::dispatch(&restarted, &request), &session_id);
    }

    drop(restarted);
    let cold_manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let cold = runtime::dispatcher(cold_manager, "v2run006-reset-cold-read");
    assert_session_absent(&cold, &selector, 60_051, &session_id);
    assert_absent_session_mismatch(
        next_response(&cold, &selector, 60_052, &session_id),
        &session_id,
    );
}

#[test]
fn v2run006_block_limit_unblock_all_and_blocked_skip_are_exact() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("states.yaml"), STATE_PROCEDURE).unwrap();
    let selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(manager, "v2run006-block-limit");
    let session_id = start(
        &production,
        &selector,
        &procedure_digest(),
        60_100,
        "V2RUN-006 blocker limit",
    );
    for index in 0_u64..64 {
        let (result, _, _) = transition(
            &production,
            &selector,
            60_110 + index * 3,
            &session_id,
            "session.block",
            json!({"reason": format!("Blocker {index:02}")})
                .as_object()
                .unwrap()
                .clone(),
            &format!("v2run006-block-{index:02}"),
        );
        assert_eq!(result["transition"], "block");
    }
    let at_limit = status(&production, &selector, 60_302, &session_id);
    assert_eq!(at_limit["current"]["blockers_total"], 64);
    let rejected = runtime::request(
        60_303,
        "session.block",
        &selector,
        json!({"reason": "The sixty-fifth blocker"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-block-65",
        runtime::session_preconditions(&at_limit),
    );
    let ResponseEnvelopeV2::Error(error) = runtime::dispatch(&production, &rejected) else {
        panic!("the sixty-fifth active blocker must be rejected")
    };
    assert_eq!(error.code().as_str(), "BLOCKER_LIMIT_REACHED");
    assert_eq!(
        status(&production, &selector, 60_304, &session_id)["current"]["blockers_total"],
        64
    );

    let (all, _, _) = transition(
        &production,
        &selector,
        60_305,
        &session_id,
        "session.unblock",
        json!({"all": true}).as_object().unwrap().clone(),
        "v2run006-unblock-all",
    );
    assert_eq!(all["transition"], "unblock");
    assert_eq!(all["all"], true);
    assert!(all.get("blocker_id").is_none());
    assert_eq!(
        status(&production, &selector, 60_308, &session_id)["current"]["blockers_total"],
        0
    );

    transition(
        &production,
        &selector,
        60_309,
        &session_id,
        "session.block",
        json!({"reason": "Skip remains legal"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-block-before-skip",
    );
    let (skipped, _, _) = transition(
        &production,
        &selector,
        60_312,
        &session_id,
        "session.skip",
        Map::new(),
        "v2run006-skip-blocked",
    );
    assert_eq!(skipped["transition"], "skip");
    assert_eq!(skipped["to_graph_node_id"], "finish");
    assert_eq!(
        status(&production, &selector, 60_315, &session_id)["current"]["blockers_total"],
        0
    );
    let (terminal, _, _) = transition(
        &production,
        &selector,
        60_316,
        &session_id,
        "session.skip",
        Map::new(),
        "v2run006-skip-terminal",
    );
    assert_eq!(terminal["session_state"], "completed");
    let terminal_status = status(&production, &selector, 60_319, &session_id);
    assert_eq!(terminal_status["session"]["lifecycle"], "completed");
    assert!(terminal_status["current"].is_null());
    assert!(terminal_status.get("latest_goal_outcome").is_none());
    let ResponseEnvelopeV2::Error(error) =
        next_response(&production, &selector, 60_320, &session_id)
    else {
        panic!("a terminal skip must leave no actionable cursor")
    };
    assert_eq!(error.code().as_str(), "SESSION_NOT_RUNNING");
}

#[test]
fn v2run006_cancel_restarts_without_a_cursor_and_reset_clears_running_and_cancelled() {
    let fixture = support_phase4_workspace::git_worktrees();
    for root in [fixture.main(), fixture.linked()] {
        runtime::make_runtime_private(root);
        fs::write(root.join("states.yaml"), STATE_PROCEDURE).unwrap();
    }
    let main_selector = runtime::selector(fixture.main());
    let linked_selector = runtime::selector(fixture.linked());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = runtime::dispatcher(Arc::clone(&manager), "v2run006-cancel-reset");
    let cancelled_session = start(
        &production,
        &main_selector,
        &procedure_digest(),
        60_400,
        "V2RUN-006 cancelled state",
    );
    let running_session = start(
        &production,
        &linked_selector,
        &procedure_digest(),
        60_410,
        "V2RUN-006 running reset",
    );

    transition(
        &production,
        &main_selector,
        60_420,
        &cancelled_session,
        "session.block",
        json!({"reason": "Still cancellable"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-block-before-cancel",
    );
    let blocked_before_cancel = status(&production, &main_selector, 60_422, &cancelled_session);
    assert_eq!(blocked_before_cancel["current"]["blockers_total"], 1);
    let (cancelled, cancel_once, before_cancel) = transition(
        &production,
        &main_selector,
        60_423,
        &cancelled_session,
        "session.cancel",
        json!({"reason": "No longer needed"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-cancel",
    );
    assert_eq!(cancelled["transition"], "cancel");
    assert_eq!(cancelled["session_state"], "cancelled");
    assert!(cancelled.get("reason").is_none());
    let cancel_replay_request = runtime::request(
        60_425,
        "session.cancel",
        &main_selector,
        json!({"reason": "No longer needed"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run006-cancel",
        runtime::session_preconditions(&before_cancel),
    );
    let cancel_replay = runtime::dispatch(&production, &cancel_replay_request);
    assert_replay(
        &cancel_once,
        &cancel_replay,
        cancel_replay_request.0.request_id(),
    );

    fs::remove_file(fixture.main().join("states.yaml")).unwrap();
    drop(production);
    drop(manager);
    let restarted_manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let restarted = runtime::dispatcher(restarted_manager, "v2run006-cancel-restarted");
    let cancelled_status = status(&restarted, &main_selector, 60_426, &cancelled_session);
    assert_eq!(cancelled_status["session"]["lifecycle"], "cancelled");
    assert!(cancelled_status["current"].is_null());
    let ResponseEnvelopeV2::Error(error) =
        next_response(&restarted, &main_selector, 60_427, &cancelled_session)
    else {
        panic!("a cancelled Procedure v2 session must have no actionable next cursor")
    };
    assert_eq!(error.code().as_str(), "SESSION_NOT_RUNNING");
    let cancelled_verbose = verbose_status(&restarted, &main_selector, 60_428, &cancelled_session);
    let abandoned = &cancelled_verbose["stale_attempt_history"]["entries"][0];
    assert_eq!(abandoned["lifecycle"], "abandoned");
    assert_eq!(abandoned["terminal_reason"], "No longer needed");

    for (number, selector, session_id, key) in [
        (
            60_430,
            &main_selector,
            cancelled_session.as_str(),
            "v2run006-reset-cancelled",
        ),
        (
            60_440,
            &linked_selector,
            running_session.as_str(),
            "v2run006-reset-running",
        ),
    ] {
        let before = status(&restarted, selector, number, session_id);
        let reset_request = runtime::request(
            number + 1,
            "session.reset",
            selector,
            json!({
                "confirmed": true,
                "progress_summary": "The isolated state test intentionally discards this session."
            })
            .as_object()
            .unwrap()
            .clone(),
            key,
            PreconditionsV1::new(
                Some(SessionId::new(session_id).unwrap()),
                Some(Revision::new(
                    before["session"]["revision"].as_u64().unwrap(),
                )),
                None,
                None,
                None,
                None,
            )
            .unwrap(),
        );
        let reset = runtime::v2_result(
            runtime::dispatch(&restarted, &reset_request),
            "session.reset",
        );
        assert_eq!(reset["schema"], "podway.session-reset-result/v1");
        assert_eq!(reset["mode"], "force");
        assert_eq!(reset["reset"], true);
        assert_session_absent(&restarted, selector, number + 2, session_id);
    }
}
