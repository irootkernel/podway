//! V2REF-005 isolated runtime coverage for every bug-fix-v2 route and manual target.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::sync::Arc;

use podway_core::GoalRevisionNumberV2;
use podway_daemon::server::{DaemonRequestV1, RequestDispatcherV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, WorkspaceContextV1,
    WorktreeSelectorWireV1,
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
        self.next_request += 10;
        number
    }

    fn key(&self, operation: &str, number: u64) -> String {
        format!("v2ref005-{operation}-{number}")
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
        &format!("v2ref005-init-{base}"),
        PreconditionsV1::default(),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &initialize), "workspace.init");
    let start = runtime::request(
        base + 1,
        "session.start",
        &selector,
        json!({
            "preset": "bug-fix-v2",
            "task_title": "Dogfood every bug-fix-v2 reference path"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2ref005-start-{base}"),
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    runtime::begin(
        dispatcher,
        &selector,
        base + 2,
        &session_id,
        json!({
            "goal": "Correct the disposable defect with fresh verification and review evidence.",
            "criteria": [{
                "criterion_id": "verified",
                "statement": "Fresh evidence supports the corrected behavior."
            }]
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2ref005-begin-{base}"),
    );
    Session {
        selector,
        id: session_id,
        next_request: base + 20,
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
        client: ClientInfoV1::new("v2ref005-test", "1", 1).unwrap(),
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

fn record_many(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    operations: Value,
    label: &str,
) {
    let before = status(dispatcher, session);
    let number = session.number();
    let request = runtime::request(
        number,
        "item.record_many",
        &session.selector,
        json!({"operations": operations})
            .as_object()
            .unwrap()
            .clone(),
        &session.key(label, number),
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "item.record_many");
}

fn complete(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
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
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.complete");
}

fn decide(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    node: &str,
    option_id: &str,
) -> Map<String, Value> {
    let before = assert_node(dispatcher, session, node);
    let number = session.number();
    let preconditions = runtime::session_preconditions(&before)
        .with_goal_revision(GoalRevisionNumberV2::FIRST)
        .unwrap();
    let request = v2_mutation_request(
        number,
        "session.decide",
        &session.selector,
        json!({
            "option_id": option_id,
            "reason": "The selected fresh evidence supports this recorded route.",
            "actor": "V2REF-005 runtime dogfood"
        })
        .as_object()
        .unwrap()
        .clone(),
        &session.key("decide", number),
        preconditions,
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.decide")
}

fn complete_reproduce(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    assert_node(dispatcher, session, "reproduce");
    set_item(
        dispatcher,
        session,
        "reproduction-status",
        json!("reproduced"),
    );
    set_item(
        dispatcher,
        session,
        "observed-behavior",
        json!("The disposable request produces the reproduced defect."),
    );
    set_item(
        dispatcher,
        session,
        "expected-behavior",
        json!("The disposable request completes without the defect."),
    );
    set_item(
        dispatcher,
        session,
        "regression-check",
        json!("Run the disposable regression check."),
    );
    complete(dispatcher, session);
}

fn complete_diagnose(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    assert_node(dispatcher, session, "diagnose");
    set_item(
        dispatcher,
        session,
        "cause",
        json!("The disposable state transition accepts a stale boundary."),
    );
    set_item(
        dispatcher,
        session,
        "affected-boundary",
        json!("Disposable session transition"),
    );
    complete(dispatcher, session);
}

fn complete_implement(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    assert_node(dispatcher, session, "implement");
    record_many(
        dispatcher,
        session,
        json!([
            {"item_id":"fix-summary","expected_item_revision":0,"record":{"type":"text","value":"Reject the stale disposable transition."}},
            {"item_id":"changed-boundaries","expected_item_revision":0,"record":{"type":"list","value":["Disposable session transition"]}}
        ]),
        "implementation",
    );
    complete(dispatcher, session);
}

fn complete_verify(dispatcher: &impl RequestDispatcherV1, session: &mut Session, outcome: &str) {
    assert_node(dispatcher, session, "verify");
    record_many(
        dispatcher,
        session,
        json!([{
            "item_id":"verification-result",
            "expected_item_revision":0,
            "record":{
                "type":"check_result",
                "operation_id":"bug-fix-verification",
                "operation_digest":"sha256:01122b6057efcfa3f22c453aa48fb647ccba9f7db437dd44473a9f32a854a979",
                "input_basis":{
                    "descriptor":"Disposable bug-fix candidate",
                    "digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                "executor":{"name":"V2REF-005 runtime dogfood","version":"1"},
                "outcome":outcome,
                "summary":format!("The caller records the {outcome} result in English."),
                "output_digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }
        }]),
        "verification",
    );
    complete(dispatcher, session);
}

fn complete_review(dispatcher: &impl RequestDispatcherV1, session: &mut Session, findings: i64) {
    assert_node(dispatcher, session, "review");
    let mut operations = vec![
        json!({"item_id":"review-summary","expected_item_revision":0,"record":{"type":"text","value":"The disposable bug fix was reviewed against its fresh evidence."}}),
        json!({"item_id":"unresolved-valid-findings","expected_item_revision":0,"record":{"type":"integer","value":findings}}),
    ];
    if findings > 0 {
        operations.push(json!({"item_id":"review-findings","expected_item_revision":0,"record":{"type":"list","value":["One disposable finding requires implementation rework."]}}));
    }
    record_many(dispatcher, session, Value::Array(operations), "review");
    complete(dispatcher, session);
}

fn reach_verification(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    complete_reproduce(dispatcher, session);
    complete_diagnose(dispatcher, session);
    complete_implement(dispatcher, session);
}

fn reach_assessment(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    reach_verification(dispatcher, session);
    complete_verify(dispatcher, session, "pass");
    decide(dispatcher, session, "evaluate-verification", "passed");
    complete_review(dispatcher, session, 0);
    decide(dispatcher, session, "evaluate-review", "approved");
    assert_node(dispatcher, session, "assess-goal");
}

fn assess(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    criterion_status: &str,
    option: &str,
) {
    let before = assert_node(dispatcher, session, "assess-goal");
    let number = session.number();
    let evidence = if criterion_status == "not_applicable" {
        Vec::<Value>::new()
    } else {
        vec![json!("verify")]
    };
    let preconditions = runtime::session_preconditions(&before)
        .with_goal_revision(GoalRevisionNumberV2::FIRST)
        .unwrap();
    let request = v2_mutation_request(
        number,
        "goal.assess_criterion",
        &session.selector,
        json!({
            "criterion_id":"verified",
            "status":criterion_status,
            "reason":"The caller records the criterion outcome from the selected fresh evidence.",
            "evidence":evidence,
            "items":[],
            "actor":"V2REF-005 runtime dogfood"
        })
        .as_object()
        .unwrap()
        .clone(),
        &session.key("assess", number),
        preconditions,
    );
    runtime::v2_result(
        runtime::dispatch(dispatcher, &request),
        "goal.assess_criterion",
    );
    decide(dispatcher, session, "assess-goal", option);
    assert_node(dispatcher, session, "closeout");
    set_item(
        dispatcher,
        session,
        "closeout-note",
        json!("The disposable bug-fix path reached its declared terminal outcome."),
    );
    complete(dispatcher, session);
    assert_eq!(
        status(dispatcher, session)["session"]["lifecycle"],
        "completed"
    );
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
            "target_graph_node_id":target,
            "reason":"Exercise the exact declared bug-fix manual-rework target.",
            "actor":"V2REF-005 runtime dogfood"
        })
        .as_object()
        .unwrap()
        .clone(),
        &session.key("rework", number),
        runtime::session_preconditions(&before),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.rework")
}

fn rebuild_to_assessment(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    target: &str,
) {
    match target {
        "reproduce" => complete_reproduce(dispatcher, session),
        "diagnose" => {}
        "implement" | "verify" | "review" => {}
        _ => unreachable!(),
    }
    if matches!(target, "reproduce" | "diagnose") {
        complete_diagnose(dispatcher, session);
    }
    if matches!(target, "reproduce" | "diagnose" | "implement") {
        complete_implement(dispatcher, session);
    }
    if matches!(target, "reproduce" | "diagnose" | "implement" | "verify") {
        complete_verify(dispatcher, session, "pass");
        decide(dispatcher, session, "evaluate-verification", "passed");
    }
    complete_review(dispatcher, session, 0);
    decide(dispatcher, session, "evaluate-review", "approved");
    assert_node(dispatcher, session, "assess-goal");
}

#[test]
fn v2ref005_bug_fix_guarded_failure_and_review_rework_reach_achieved_closeout() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2ref005-bug-guarded");
    let mut session = start(&production, workspace.main(), 180_000);

    reach_verification(&production, &mut session);
    complete_verify(&production, &mut session, "inconclusive");
    let retry = decide(&production, &mut session, "evaluate-verification", "retry");
    assert_eq!(retry["effect"], "rework");
    assert_eq!(retry["target_graph_node_id"], "implement");
    complete_implement(&production, &mut session);
    complete_verify(&production, &mut session, "pass");
    decide(&production, &mut session, "evaluate-verification", "passed");
    complete_review(&production, &mut session, 1);
    let changes = decide(
        &production,
        &mut session,
        "evaluate-review",
        "changes-requested",
    );
    assert_eq!(changes["effect"], "rework");
    assert_eq!(changes["target_graph_node_id"], "implement");
    complete_implement(&production, &mut session);
    complete_verify(&production, &mut session, "pass");
    decide(&production, &mut session, "evaluate-verification", "passed");
    complete_review(&production, &mut session, 0);
    decide(&production, &mut session, "evaluate-review", "approved");
    assess(&production, &mut session, "satisfied", "achieved");
}

#[test]
fn v2ref005_bug_fix_goal_not_achieved_and_superseded_paths_are_reachable() {
    for (index, criterion_status, option) in [
        (0, "unsatisfied", "not-achieved"),
        (1, "not_applicable", "superseded"),
    ] {
        let workspace = support_phase4_workspace::git_worktrees();
        let manager = Arc::new(runtime::manager(workspace.temporary_path()));
        let worker_id = format!("v2ref005-bug-goal-{index}");
        let production = runtime::dispatcher(manager, &worker_id);
        let mut session = start(&production, workspace.main(), 181_000 + index * 1_000);
        reach_assessment(&production, &mut session);
        assess(&production, &mut session, criterion_status, option);
    }
}

#[test]
fn v2ref005_bug_fix_accepts_every_declared_manual_rework_target() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2ref005-bug-manual");
    let mut session = start(&production, workspace.main(), 184_000);
    reach_assessment(&production, &mut session);

    for target in ["review", "verify", "implement", "diagnose", "reproduce"] {
        let result = rework(&production, &mut session, target);
        assert_eq!(result["to_graph_node_id"], target);
        rebuild_to_assessment(&production, &mut session, target);
    }
    assess(&production, &mut session, "satisfied", "achieved");
}
