//! V2REF isolated runtime coverage for every analysis-v2 route and manual target.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::sync::Arc;

use podway_core::GoalRevisionNumberV2;
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
        self.next_request += 10;
        number
    }

    fn key(&self, operation: &str, number: u64) -> String {
        format!("v2ref_analysis-{operation}-{number}")
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
        &format!("v2ref_analysis-init-{base}"),
        PreconditionsV1::default(),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &initialize), "workspace.init");
    let start = runtime::request(
        base + 1,
        "session.start",
        &selector,
        json!({
            "preset": "analysis-v2",
            "task_title": "Dogfood every analysis-v2 reference path"
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2ref_analysis-start-{base}"),
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
            "goal": "Produce a reviewed decision-ready analysis from bounded evidence.",
            "criteria": [{
                "criterion_id": "decision-ready",
                "statement": "Fresh synthesis and review evidence supports the analysis conclusion."
            }]
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2ref_analysis-begin-{base}"),
    );
    Session {
        selector,
        id: session_id,
        next_request: base + 20,
    }
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
        client: ClientInfoV1::new("v2ref_analysis-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(
            WorkspaceContextV1::new(selector.display(), selector.expected_uuid().cloned()).unwrap(),
        ),
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, runtime::TEST_WAIT_TIMEOUT_MILLIS).unwrap(),
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
    let request = mutation_request(
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
    let request = mutation_request(
        number,
        "session.decide",
        &session.selector,
        json!({
            "option_id": option_id,
            "reason": "The selected fresh evidence supports this analysis route.",
            "actor": "V2REF runtime dogfood"
        })
        .as_object()
        .unwrap()
        .clone(),
        &session.key("decide", number),
        preconditions,
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.decide")
}

fn complete_define(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    assert_node(dispatcher, session, "define");
    record_many(
        dispatcher,
        session,
        json!([
            {"item_id":"analysis-question","expected_item_revision":0,"record":{"type":"text","value":"Which bounded option is best supported by the repository evidence?"}},
            {"item_id":"decision-context","expected_item_revision":0,"record":{"type":"text","value":"Support one maintainership decision."}},
            {"item_id":"scope-and-constraints","expected_item_revision":0,"record":{"type":"list","value":["Use repository-owned evidence only."]}}
        ]),
        "define",
    );
    complete(dispatcher, session);
}

fn complete_collect(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    assert_node(dispatcher, session, "collect");
    record_many(
        dispatcher,
        session,
        json!([
            {"item_id":"sources","expected_item_revision":0,"record":{"type":"list","value":["Canonical specification", "Executable runtime test"]}},
            {"item_id":"source-limitations","expected_item_revision":0,"record":{"type":"list","value":["The evidence is bounded to this disposable scenario."]}}
        ]),
        "collect",
    );
    complete(dispatcher, session);
}

fn complete_analyze(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    assert_node(dispatcher, session, "analyze");
    record_many(
        dispatcher,
        session,
        json!([
            {"item_id":"analysis-summary","expected_item_revision":0,"record":{"type":"text","value":"The specification and runtime evidence support the bounded option."}},
            {"item_id":"key-findings","expected_item_revision":0,"record":{"type":"list","value":["The canonical and runtime evidence agree."]}},
            {"item_id":"assumptions","expected_item_revision":0,"record":{"type":"list","value":[]}}
        ]),
        "analyze",
    );
    complete(dispatcher, session);
}

fn complete_challenge(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    blocking_gaps: Vec<&str>,
) {
    assert_node(dispatcher, session, "challenge");
    record_many(
        dispatcher,
        session,
        json!([
            {"item_id":"counterarguments","expected_item_revision":0,"record":{"type":"list","value":["A broader environment could produce a different result."]}},
            {"item_id":"uncertainty","expected_item_revision":0,"record":{"type":"text","value":"The conclusion is limited to the recorded evidence boundary."}},
            {"item_id":"blocking-gaps","expected_item_revision":0,"record":{"type":"list","value":blocking_gaps}}
        ]),
        "challenge",
    );
    complete(dispatcher, session);
}

fn complete_synthesis(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    assert_node(dispatcher, session, "synthesize");
    record_many(
        dispatcher,
        session,
        json!([
            {"item_id":"conclusion","expected_item_revision":0,"record":{"type":"text","value":"The bounded option is supported by the fresh evidence."}},
            {"item_id":"recommendation","expected_item_revision":0,"record":{"type":"text","value":"Adopt the bounded option and retain the stated limitation."}},
            {"item_id":"confidence","expected_item_revision":0,"record":{"type":"choice","value":"high"}},
            {"item_id":"residual-risks","expected_item_revision":0,"record":{"type":"list","value":["Evidence outside the declared scope may change the conclusion."]}}
        ]),
        "synthesis",
    );
    complete(dispatcher, session);
}

fn complete_review(
    dispatcher: &impl RequestDispatcherV1,
    session: &mut Session,
    source_findings: i64,
    analysis_findings: i64,
) {
    assert_node(dispatcher, session, "review");
    let mut operations = vec![
        json!({"item_id":"review-summary","expected_item_revision":0,"record":{"type":"text","value":"The source basis and analysis claims were reviewed separately."}}),
        json!({"item_id":"unresolved-source-findings","expected_item_revision":0,"record":{"type":"integer","value":source_findings}}),
        json!({"item_id":"unresolved-analysis-findings","expected_item_revision":0,"record":{"type":"integer","value":analysis_findings}}),
    ];
    if source_findings > 0 {
        operations.push(json!({"item_id":"source-findings","expected_item_revision":0,"record":{"type":"list","value":["One source finding requires collection rework."]}}));
    }
    if analysis_findings > 0 {
        operations.push(json!({"item_id":"analysis-findings","expected_item_revision":0,"record":{"type":"list","value":["One reasoning finding requires analysis rework."]}}));
    }
    record_many(dispatcher, session, Value::Array(operations), "review");
    complete(dispatcher, session);
}

fn reach_evidence_decision(dispatcher: &impl RequestDispatcherV1, session: &mut Session) {
    complete_define(dispatcher, session);
    complete_collect(dispatcher, session);
    complete_analyze(dispatcher, session);
    complete_challenge(dispatcher, session, Vec::new());
    assert_node(dispatcher, session, "evaluate-evidence");
}

fn rebuild_from(dispatcher: &impl RequestDispatcherV1, session: &mut Session, target: &str) {
    if target == "define" {
        complete_define(dispatcher, session);
    }
    if matches!(target, "define" | "collect") {
        complete_collect(dispatcher, session);
    }
    if matches!(target, "define" | "collect" | "analyze") {
        complete_analyze(dispatcher, session);
    }
    if matches!(target, "define" | "collect" | "analyze" | "challenge") {
        complete_challenge(dispatcher, session, Vec::new());
        decide(dispatcher, session, "evaluate-evidence", "sufficient");
    }
    if matches!(
        target,
        "define" | "collect" | "analyze" | "challenge" | "synthesize"
    ) {
        complete_synthesis(dispatcher, session);
    }
    complete_review(dispatcher, session, 0, 0);
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
        vec![json!("synthesize")]
    };
    let preconditions = runtime::session_preconditions(&before)
        .with_goal_revision(GoalRevisionNumberV2::FIRST)
        .unwrap();
    let request = mutation_request(
        number,
        "goal.assess_criterion",
        &session.selector,
        json!({
            "criterion_id":"decision-ready",
            "status":criterion_status,
            "reason":"The selected synthesis and review evidence support this criterion outcome.",
            "evidence":evidence,
            "items":[],
            "actor":"V2REF runtime dogfood"
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
    record_many(
        dispatcher,
        session,
        json!([{"item_id":"closeout-note","expected_item_revision":0,"record":{"type":"text","value":"The analysis reached its declared terminal outcome."}}]),
        "closeout",
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
    let request = mutation_request(
        number,
        "session.rework",
        &session.selector,
        json!({
            "target_graph_node_id":target,
            "reason":"Exercise the exact declared analysis manual-rework target.",
            "actor":"V2REF runtime dogfood"
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
fn v2ref_analysis_guarded_and_phase_owner_rework_reach_achieved_closeout() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2ref_analysis-analysis-routes");
    let mut session = start(&production, workspace.main(), 188_000);

    complete_define(&production, &mut session);
    complete_collect(&production, &mut session);
    complete_analyze(&production, &mut session);
    complete_challenge(
        &production,
        &mut session,
        vec!["One authoritative source is still required."],
    );
    let research = decide(
        &production,
        &mut session,
        "evaluate-evidence",
        "more-research",
    );
    assert_eq!(research["effect"], "rework");
    assert_eq!(research["target_graph_node_id"], "collect");
    complete_collect(&production, &mut session);
    complete_analyze(&production, &mut session);
    complete_challenge(&production, &mut session, Vec::new());
    decide(&production, &mut session, "evaluate-evidence", "sufficient");
    complete_synthesis(&production, &mut session);

    complete_review(&production, &mut session, 1, 0);
    let source = decide(
        &production,
        &mut session,
        "evaluate-review",
        "source-changes",
    );
    assert_eq!(source["target_graph_node_id"], "collect");
    complete_collect(&production, &mut session);
    complete_analyze(&production, &mut session);
    complete_challenge(&production, &mut session, Vec::new());
    decide(&production, &mut session, "evaluate-evidence", "sufficient");
    complete_synthesis(&production, &mut session);
    complete_review(&production, &mut session, 0, 1);
    let analysis = decide(
        &production,
        &mut session,
        "evaluate-review",
        "analysis-changes",
    );
    assert_eq!(analysis["target_graph_node_id"], "analyze");
    complete_analyze(&production, &mut session);
    complete_challenge(&production, &mut session, Vec::new());
    decide(&production, &mut session, "evaluate-evidence", "sufficient");
    complete_synthesis(&production, &mut session);
    complete_review(&production, &mut session, 0, 0);
    decide(&production, &mut session, "evaluate-review", "approved");
    assess(&production, &mut session, "satisfied", "achieved");
}

#[test]
fn v2ref_analysis_conditional_review_findings_block_completion_when_missing() {
    for (index, source_count, analysis_count, missing_item) in
        [(0, 1, 0, "source-findings"), (1, 0, 1, "analysis-findings")]
    {
        let workspace = support_phase4_workspace::git_worktrees();
        let manager = Arc::new(runtime::manager(workspace.temporary_path()));
        let worker = format!("v2ref_analysis-analysis-conditional-{index}");
        let production = runtime::dispatcher(manager, &worker);
        let mut session = start(&production, workspace.main(), 190_000 + index * 1_000);
        reach_evidence_decision(&production, &mut session);
        decide(&production, &mut session, "evaluate-evidence", "sufficient");
        complete_synthesis(&production, &mut session);
        assert_node(&production, &mut session, "review");
        record_many(
            &production,
            &mut session,
            json!([
                {"item_id":"review-summary","expected_item_revision":0,"record":{"type":"text","value":"The review records one unresolved owned finding."}},
                {"item_id":"unresolved-source-findings","expected_item_revision":0,"record":{"type":"integer","value":source_count}},
                {"item_id":"unresolved-analysis-findings","expected_item_revision":0,"record":{"type":"integer","value":analysis_count}}
            ]),
            "conditional-review",
        );
        let blocked = status(&production, &mut session);
        assert_eq!(blocked["current"]["missing_required_item_count"], 1);
        assert!(
            blocked["missing_required_item_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == missing_item)
        );
        let number = session.number();
        let request = mutation_request(
            number,
            "session.complete",
            &session.selector,
            Map::new(),
            &session.key("blocked-complete", number),
            runtime::session_preconditions(&blocked),
        );
        let ResponseEnvelopeV2::Error(error) = runtime::dispatch(&production, &request) else {
            panic!("conditionally required review findings must block completion")
        };
        assert_eq!(error.code().as_str(), "REQUIRED_ITEMS_MISSING");
    }
}

#[test]
fn v2ref_analysis_goal_not_achieved_and_superseded_paths_are_reachable() {
    for (index, criterion_status, option) in [
        (0, "unsatisfied", "not-achieved"),
        (1, "not_applicable", "superseded"),
    ] {
        let workspace = support_phase4_workspace::git_worktrees();
        let manager = Arc::new(runtime::manager(workspace.temporary_path()));
        let worker = format!("v2ref_analysis-analysis-goal-{index}");
        let production = runtime::dispatcher(manager, &worker);
        let mut session = start(&production, workspace.main(), 193_000 + index * 1_000);
        reach_evidence_decision(&production, &mut session);
        decide(&production, &mut session, "evaluate-evidence", "sufficient");
        complete_synthesis(&production, &mut session);
        complete_review(&production, &mut session, 0, 0);
        decide(&production, &mut session, "evaluate-review", "approved");
        assess(&production, &mut session, criterion_status, option);
    }
}

#[test]
fn v2ref_analysis_accepts_every_declared_manual_rework_target() {
    let workspace = support_phase4_workspace::git_worktrees();
    let manager = Arc::new(runtime::manager(workspace.temporary_path()));
    let production = runtime::dispatcher(manager, "v2ref_analysis-analysis-manual");
    let mut session = start(&production, workspace.main(), 196_000);
    reach_evidence_decision(&production, &mut session);
    decide(&production, &mut session, "evaluate-evidence", "sufficient");
    complete_synthesis(&production, &mut session);
    complete_review(&production, &mut session, 0, 0);
    decide(&production, &mut session, "evaluate-review", "approved");

    for target in [
        "review",
        "synthesize",
        "challenge",
        "analyze",
        "collect",
        "define",
    ] {
        let result = rework(&production, &mut session, target);
        assert_eq!(result["to_graph_node_id"], target);
        rebuild_from(&production, &mut session, target);
    }
    assess(&production, &mut session, "satisfied", "achieved");
}
