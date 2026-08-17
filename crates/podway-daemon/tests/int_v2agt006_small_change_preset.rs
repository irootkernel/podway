//! V2AGT-006 production dogfood coverage for the lightweight built-in preset.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::sync::Arc;

use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    WorkspaceContextV1, WorktreeSelectorWireV1,
};
use serde_json::{Map, Value, json};

struct Session {
    selector: WorktreeSelectorWireV1,
    id: String,
    next_request: u64,
}

impl Session {
    fn number(&mut self) -> u64 {
        let number = self.next_request;
        self.next_request += 2;
        number
    }

    fn key(&self, operation: &str, number: u64) -> String {
        format!("v2agt006-{operation}-{number}")
    }
}

fn start(dispatcher: &impl RequestDispatcherV1, root: &std::path::Path, base: u64) -> Session {
    runtime::make_runtime_private(root);
    let selector = runtime::selector(root);
    let initialize = runtime::request(
        base,
        "workspace.init",
        &selector,
        Map::new(),
        &format!("v2agt006-init-{base}"),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = runtime::request(
        base + 1,
        "session.start",
        &selector,
        json!({
            "preset": "small-change-v2",
            "task_title": "Dogfood the lightweight verified-change procedure"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2agt006-start-{base}"),
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    runtime::begin(
        dispatcher,
        &selector,
        base + 2,
        &session_id,
        Map::new(),
        &format!("v2agt006-begin-{base}"),
    );
    Session {
        selector,
        id: session_id,
        next_request: base + 10,
    }
}

fn v2_mutation_request(
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
        client: ClientInfoV1::new("v2agt006-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(
            WorkspaceContextV1::new(selector.display(), selector.expected_uuid().cloned()).unwrap(),
        ),
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    (envelope, daemon)
}

fn status(dispatcher: &impl RequestDispatcherV1, session: &mut Session) -> Map<String, Value> {
    let number = session.number();
    runtime::status(dispatcher, &session.selector, number, &session.id)
}

fn assert_node(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    expected: &str,
) -> Map<String, Value> {
    let current = status(dispatcher, session);
    assert_eq!(current["current"]["node"]["graph_node_id"], expected);
    current
}

fn set_item(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    item_id: &str,
    value: Value,
) {
    let number = session.number();
    runtime::mutate_item(
        dispatcher,
        &session.selector,
        number,
        &session.id,
        "item.set",
        item_id,
        json!({"value": value}).as_object().unwrap().clone(),
        &session.key(item_id, number),
    );
}

fn complete(dispatcher: &impl RequestDispatcherV1, session: &mut Session) -> Map<String, Value> {
    let before = status(dispatcher, session);
    let number = session.number();
    let request = v2_mutation_request(
        number,
        "session.complete",
        &session.selector,
        Map::new(),
        &session.key("complete", number),
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.complete")
}

fn complete_action(dispatcher: &impl RequestDispatcherV1, session: &mut Session, node: &str) {
    assert_node(dispatcher, session, node);
    match node {
        "inspect" => set_item(
            dispatcher,
            session,
            "scope-summary",
            json!("The change is bounded to the requested implementation and tests."),
        ),
        "implement" => set_item(
            dispatcher,
            session,
            "implementation-summary",
            json!("Implemented the bounded change without unrelated edits."),
        ),
        "verify" => {
            set_item(
                dispatcher,
                session,
                "verification-command",
                json!("make test"),
            );
            set_item(dispatcher, session, "verification-exit-status", json!("0"));
        }
        "closeout" => set_item(
            dispatcher,
            session,
            "closeout-note",
            json!("The reviewed and verified small change is complete."),
        ),
        _ => panic!("unsupported action node {node}"),
    }
    complete(dispatcher, session);
}

fn reach_review(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    for node in ["inspect", "implement", "verify"] {
        complete_action(dispatcher, session, node);
    }
    assert_node(dispatcher, session, "review");
}

fn decide(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    option_id: &str,
) -> Map<String, Value> {
    let before = assert_node(dispatcher, session, "review");
    let number = session.number();
    let request = v2_mutation_request(
        number,
        "session.decide",
        &session.selector,
        json!({
            "option_id": option_id,
            "reason": "The recorded scope, implementation, and verification support this decision."
        })
        .as_object()
        .unwrap()
        .clone(),
        &session.key("decide", number),
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.decide")
}

fn finish(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    let decision = decide(dispatcher, session, "ready");
    assert_eq!(decision["target_graph_node_id"], "closeout");
    complete_action(dispatcher, session, "closeout");
    let terminal = status(dispatcher, session);
    assert_eq!(terminal["session"]["lifecycle"], "completed");
    assert!(!terminal.contains_key("goal"));
}

fn retry(dispatcher: &impl RequestDispatcherV1, session: &mut Session) -> Map<String, Value> {
    let before = status(dispatcher, session);
    let number = session.number();
    let request = runtime::request(
        number,
        "session.retry",
        &session.selector,
        json!({"reason": "Repeat this node with fresh evidence."})
            .as_object()
            .unwrap()
            .clone(),
        &session.key("retry", number),
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.retry")
}

fn rework(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    target: &str,
) -> Map<String, Value> {
    let before = status(dispatcher, session);
    let number = session.number();
    let request = v2_mutation_request(
        number,
        "session.rework",
        &session.selector,
        json!({
            "target_graph_node_id": target,
            "reason": "Revisit the selected declared manual-rework boundary."
        })
        .as_object()
        .unwrap()
        .clone(),
        &session.key("rework", number),
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.rework")
}

#[test]
fn small_change_happy_path_closes_without_a_goal() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2agt006-happy");
    let mut session = start(&production, workspace.main(), 160_000);

    reach_review(&production, &mut session);
    finish(&production, &mut session);
}

#[test]
fn small_change_review_rework_returns_to_implementation_and_then_closes() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2agt006-review-rework");
    let mut session = start(&production, workspace.main(), 161_000);

    reach_review(&production, &mut session);
    let decision = decide(&production, &mut session, "changes-requested");
    assert_eq!(decision["target_graph_node_id"], "implement");
    assert_eq!(decision["effect"], "rework");
    complete_action(&production, &mut session, "implement");
    complete_action(&production, &mut session, "verify");
    finish(&production, &mut session);
}

#[test]
fn small_change_retry_discards_the_verify_attempt_and_requires_fresh_items() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2agt006-retry");
    let mut session = start(&production, workspace.main(), 162_000);

    complete_action(&production, &mut session, "inspect");
    complete_action(&production, &mut session, "implement");
    assert_node(&production, &mut session, "verify");
    set_item(
        &production,
        &mut session,
        "verification-command",
        json!("cargo test --locked"),
    );
    set_item(
        &production,
        &mut session,
        "verification-exit-status",
        json!("0"),
    );
    let before = status(&production, &mut session);
    let old_attempt = before["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let retried = retry(&production, &mut session);
    assert_ne!(retried["to_attempt_id"], old_attempt);
    let after = assert_node(&production, &mut session, "verify");
    assert_eq!(after["current"]["missing_required_item_count"], 2);
    assert!(after["item_values"].as_array().unwrap().is_empty());
    complete_action(&production, &mut session, "verify");
    finish(&production, &mut session);
}

#[test]
fn small_change_manual_rework_accepts_each_declared_target_and_then_closes() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2agt006-manual-rework");
    let mut session = start(&production, workspace.main(), 163_000);

    reach_review(&production, &mut session);
    for target in ["verify", "implement", "inspect"] {
        let result = rework(&production, &mut session, target);
        assert_eq!(result["to_graph_node_id"], target);
        match target {
            "verify" => complete_action(&production, &mut session, "verify"),
            "implement" => {
                complete_action(&production, &mut session, "implement");
                complete_action(&production, &mut session, "verify");
            }
            "inspect" => {
                complete_action(&production, &mut session, "inspect");
                complete_action(&production, &mut session, "implement");
                complete_action(&production, &mut session, "verify");
            }
            _ => unreachable!(),
        }
        assert_node(&production, &mut session, "review");
    }
    finish(&production, &mut session);
}
