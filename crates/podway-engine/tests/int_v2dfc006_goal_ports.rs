//! Retained Goal semantics through typed direct execution, native Git and the direct Store.

use std::{
    sync::{Arc, Barrier},
    thread,
};

use podway_core::{
    AttemptId, CriterionId, GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2,
    GoalRevisionReasonV2, GoalStatementV2, GraphNodeId, ItemId, OptionId, Revision,
    SessionLifecycle, UnixMillis,
};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
    direct_start::DirectStartCommandV1,
    execution::prepare_workspace_procedure_source_v2,
    v2_read_service::GraphStatusTierV2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::NativeGitResolverV1;
use podway_protocol::{
    CriterionAssessmentManyEntryWireV1, GoalAssessCriteriaV2, GoalAssessCriterionV2,
    GoalCriterionWireV2, GoalDefineV2, GoalReviseV2, GoalRevisionPreconditionsWireV2,
    InitialGoalWireV2, ProcedureV2MutationCommandV1 as P, SessionBeginV1, SessionCancelV1,
    SessionDecideV2, SessionDeletionModeV1, SessionIdentityPreconditionsWireV1,
    SessionStartCommandV1, SessionStartReplaceV1, SessionStartSourceV1, SessionStartV1,
};
use podway_store::IdempotencyKeyV1;
use serde_json::{Value, json};

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};

fn criteria() -> Vec<GoalCriterionWireV2> {
    vec![
        GoalCriterionWireV2 {
            criterion_id: CriterionId::new("correct").unwrap(),
            statement: "The result is correct.".into(),
        },
        GoalCriterionWireV2 {
            criterion_id: CriterionId::new("tested").unwrap(),
            statement: "The result is tested.".into(),
        },
    ]
}

fn initial_goal() -> InitialGoalWireV2 {
    InitialGoalWireV2 {
        goal: "Verify the retained Goal semantics.".into(),
        criteria: criteria(),
        actor: None,
    }
}

fn identity(engine: &Engine) -> SessionIdentityPreconditionsWireV1 {
    let graph = engine.graph();
    SessionIdentityPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
    }
}

fn start_input(source: &str) -> SessionStartV1 {
    SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "source.yaml".into(),
        },
        expected_procedure_digest: Some(
            prepare_workspace_procedure_source_v2("source.yaml", source.as_bytes())
                .unwrap()
                .digest()
                .clone(),
        ),
        task_title: "Retained Goal port".into(),
        dry_run: false,
    }
}

fn prepared(source: &str) -> (Engine, podway_engine::direct_api::DirectRequestV1, Value) {
    let (temporary, selector, store, _) = fixture();
    std::fs::write(temporary.path().join("source.yaml"), source).unwrap();
    let engine = Engine {
        temporary,
        selector,
        store,
        resolver: DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest()),
    };
    let request = engine.request(
        DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: start_input(source),
            preconditions: None,
        })),
        "start",
    );
    let response = engine.output(engine.execute(request.clone(), &Clock));
    (engine, request, response)
}

fn begin(
    engine: &Engine,
    goal: bool,
    key: &str,
) -> (podway_engine::direct_api::DirectRequestV1, Value) {
    let request = engine.request(
        DirectCommandV1::Procedure(P::SessionBegin(SessionBeginV1 {
            initial_goal: goal.then(initial_goal),
            preconditions: identity(engine),
        })),
        key,
    );
    let response = engine.output(engine.execute(request.clone(), &Clock));
    (request, response)
}

fn goal_engine(source: &str) -> Engine {
    let (engine, _, _) = prepared(source);
    begin(&engine, true, "begin");
    engine
}

fn define(engine: &Engine) -> DirectCommandV1 {
    DirectCommandV1::Procedure(P::GoalDefine(GoalDefineV2 {
        goal: initial_goal().goal,
        criteria: criteria(),
        actor: None,
        preconditions: identity(engine),
    }))
}

fn revise(engine: &Engine, target: &str, reactivate: bool) -> DirectCommandV1 {
    let graph = engine.graph();
    DirectCommandV1::Procedure(P::GoalRevise(GoalReviseV2 {
        goal: "Revise the retained goal.".into(),
        criteria: criteria(),
        target_graph_node_id: GraphNodeId::new(target).unwrap(),
        reason: "The required outcome changed.".into(),
        actor: Some("owner".into()),
        reactivate,
        preconditions: GoalRevisionPreconditionsWireV2 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph
                .trace()
                .active_attempt()
                .map(|attempt| attempt.attempt_id().clone()),
            expected_goal_revision: graph
                .goal_state()
                .current_revision()
                .map_or(1, |revision| revision.get()),
        },
    }))
}

fn assess(engine: &Engine, criterion: &str, status: &str) -> DirectCommandV1 {
    DirectCommandV1::Procedure(P::GoalAssessCriterion(GoalAssessCriterionV2 {
        criterion_id: CriterionId::new(criterion).unwrap(),
        status: status.into(),
        reason: "The source and local note support correctness.".into(),
        evidence: vec![],
        items: vec![],
        actor: None,
        preconditions: engine.fences(),
        expected_goal_revision: engine
            .graph()
            .goal_state()
            .current_revision()
            .map_or(1, |revision| revision.get()),
    }))
}

fn decide(engine: &Engine, option: &str) -> DirectCommandV1 {
    DirectCommandV1::Procedure(P::SessionDecide(SessionDecideV2 {
        option_id: OptionId::new(option).unwrap(),
        reason: "Select the recorded outcome.".into(),
        actor: Some("owner".into()),
        preconditions: engine.fences(),
        expected_goal_revision: Some(
            engine
                .graph()
                .goal_state()
                .current_revision()
                .unwrap()
                .get(),
        ),
    }))
}

fn status(engine: &Engine) -> Value {
    engine.read(DirectReadCommandV1::Status {
        tier: GraphStatusTierV2::Verbose,
        history_before: None,
    })
}

fn running_projection(readback: &Value) -> Value {
    json!({"session_lifecycle":readback["session"]["lifecycle"], "session_revision":readback["session"]["revision"],
        "current_node":readback["current"]["node"], "current_attempt_number":readback["current"]["attempt"]["attempt_number"],
        "current_readiness":readback["current"]["readiness"], "goal_revision":readback["goal_revision"], "goal":readback["goal"],
        "latest_goal_outcome":readback.get("latest_goal_outcome").cloned().unwrap_or(Value::Null),
        "trace_length":readback["trace_length"], "counters":readback["counters"],
        "allowed_manual_rework_targets":readback["allowed_manual_rework_targets"]})
}

fn at_decision(source: &str) -> Engine {
    let engine = goal_engine(source);
    if engine
        .graph()
        .trace()
        .active_attempt()
        .unwrap()
        .graph_node_id()
        .as_str()
        == "prelude"
    {
        engine.complete("prelude");
    }
    engine.set("result", "The retained evidence.", "result");
    engine.complete("perform");
    engine
}

fn error(engine: &mut Engine, command: DirectCommandV1, key: &str, code: &str) -> Value {
    let before = engine.graph();
    let sequence = engine.sequence();
    let request = engine.request(command, key);
    let value = match engine.execute(request.clone(), &Clock) {
        DirectResponseV1::Failure(error) => error.into_value(),
        DirectResponseV1::Success(output) => {
            panic!("expected {code}, received {:?}", output.as_value())
        }
    };
    assert_eq!(value["code"], code, "{key}");
    assert_eq!(engine.graph(), before, "{key} changed graph state");
    assert_eq!(
        engine.sequence(),
        sequence,
        "{key} changed workspace sequence"
    );
    assert!(
        engine
            .store
            .read(deadline(), |snapshot| snapshot
                .outcome(&IdempotencyKeyV1::new(key).unwrap()))
            .unwrap()
            .is_none()
    );
    engine.cold();
    let DirectResponseV1::Failure(rechecked) = engine.execute(request, &Clock) else {
        panic!("{key} accepted after cold reopen")
    };
    assert_eq!(rechecked.into_value()["code"], code);
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    assert_eq!(
        engine.success(
            DirectCommandV1::OutcomeLookup {
                key: IdempotencyKeyV1::new(key).unwrap()
            },
            "lookup"
        )["result"]["found"],
        false
    );
    value
}

fn finish_assessment(engine: &Engine, key: &str) -> Value {
    engine.success(
        assess(engine, "correct", "satisfied"),
        &format!("{key}-correct"),
    );
    engine.success(
        assess(engine, "tested", "satisfied"),
        &format!("{key}-tested"),
    );
    engine.success(decide(engine, "achieved"), &format!("{key}-decide"))
}

#[test]
fn v2gol001_initial_goal_is_durable_for_begin_and_replace() {
    let (mut engine, start, started) = prepared(GOAL_PROCEDURE);
    assert_eq!(started["result"]["goal_defined"], false);
    assert_eq!(engine.graph().trace().revision(), Revision::ZERO);
    assert_eq!(engine.output(engine.execute(start, &NoClock)), started);
    let original_session = engine.graph().trace().session_id().clone();
    let (request, begun) = begin(&engine, true, "begin");
    assert_eq!(begun["result"]["goal_tracking"], true);
    assert_eq!(begun["result"]["goal_defined"], true);
    assert_eq!(engine.graph().trace().revision().get(), 1);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), begun);
    let replacement = engine.success(
        DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
            start: start_input(GOAL_PROCEDURE),
            mode: SessionDeletionModeV1::Delete {
                confirmed: true,
                progress_summary: Some("Replace the isolated running Goal fixture.".into()),
            },
            preconditions: identity(&engine),
        })),
        "replace",
    );
    assert_eq!(replacement["result"]["goal_defined"], false);
    assert_eq!(engine.graph().trace().revision(), Revision::ZERO);
    assert_ne!(engine.graph().trace().session_id(), &original_session);
    let (request, begun) = begin(&engine, true, "replacement-begin");
    assert_eq!(begun["result"]["goal_defined"], true);
    assert_eq!(engine.graph().trace().revision().get(), 1);
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), begun);
}

#[test]
fn v2gol001_define_and_running_revise_are_fenced_replayable_and_restart_safe() {
    let mut engine = Engine::new(GOAL_PROCEDURE);
    assert!(engine.graph().goal_state().current_revision().is_none());
    let before = engine.graph().trace().revision().get();
    let request = engine.request(define(&engine), "define");
    let defined = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(
        defined["result"]["schema"],
        "podway.goal-definition-result/v2"
    );
    assert_eq!(defined["result"]["goal_revision"], 1);
    assert_eq!(engine.graph().trace().revision().get(), before + 1);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), defined);
    let duplicate = define(&engine);
    error(
        &mut engine,
        duplicate,
        "duplicate",
        "SESSION_GOAL_ALREADY_DEFINED",
    );
    let mut stale = revise(&engine, "perform", false);
    let DirectCommandV1::Procedure(P::GoalRevise(input)) = &mut stale else {
        unreachable!()
    };
    input.preconditions.expected_goal_revision = 2;
    let rejected = error(&mut engine, stale, "stale-goal", "GOAL_REVISION_STALE");
    assert_eq!(rejected["details"]["expected_goal_revision"], 2);
    assert_eq!(rejected["details"]["actual_goal_revision"], 1);
    let mut missing = revise(&engine, "perform", false);
    let DirectCommandV1::Procedure(P::GoalRevise(input)) = &mut missing else {
        unreachable!()
    };
    input.preconditions.expected_attempt_id = None;
    error(&mut engine, missing, "missing-attempt", "REQUEST_INVALID");
    let old_attempt = engine.fences().expected_attempt_id;
    let request = engine.request(revise(&engine, "perform", false), "revise");
    let revised = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(
        revised["result"]["schema"],
        "podway.goal-revision-result/v2"
    );
    assert_eq!(revised["result"]["goal_revision"], 2);
    assert_eq!(revised["result"]["rework_to"], "perform");
    assert_eq!(revised["result"]["reactivated"], false);
    assert_ne!(engine.fences().expected_attempt_id, old_attempt);
    let graph = engine.graph();
    engine.cold();
    assert_eq!(engine.graph(), graph);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), revised);
}

#[test]
fn v2gol001_completed_revision_requires_explicit_reactivation_and_creates_fresh_cursor() {
    let mut engine = at_decision(GOAL_PROCEDURE);
    finish_assessment(&engine, "finish");
    engine.complete("terminal");
    let completed = engine.graph();
    assert_eq!(completed.trace().lifecycle(), SessionLifecycle::Completed);
    let fresh = AttemptId::new("00000000-0000-4000-8000-000000103007").unwrap();
    let pure_revision = |reactivate| {
        completed.revise_goal_v2(
            completed.trace().revision(),
            None,
            GoalRevisionNumberV2::FIRST,
            GoalStatementV2::new("Prove completed recovery and reactivation.").unwrap(),
            GoalDefinitionV2::new(vec![
                GoalCriterionV2::new(
                    CriterionId::new("verified").unwrap(),
                    "The reactivated session owns a fresh cursor.",
                )
                .unwrap(),
            ])
            .unwrap(),
            GraphNodeId::new("perform").unwrap(),
            fresh.clone(),
            GoalRevisionReasonV2::new("The completed goal now includes recovery.").unwrap(),
            None,
            reactivate,
            UnixMillis::new(1_700_000_000_004),
        )
    };
    assert_eq!(
        pure_revision(false).unwrap_err(),
        podway_store::GraphMutationErrorV2::ReactivationFlagRequired
    );
    let pure = pure_revision(true).unwrap();
    assert_eq!(pure.revision().revision().get(), 2);
    assert!(pure.revision().reactivated());
    assert_eq!(pure.state().trace().lifecycle(), SessionLifecycle::Running);
    assert_eq!(
        pure.state().trace().active_attempt().unwrap().attempt_id(),
        &fresh
    );
    assert_eq!(
        pure.state().trace().attempts().len(),
        completed.trace().attempts().len() + 1
    );
    assert_eq!(pure.state().completed_at(), None);
    let command = revise(&engine, "perform", false);
    error(
        &mut engine,
        command,
        "no-reactivation",
        "REACTIVATION_FLAG_REQUIRED",
    );
    let length = completed.trace().attempts().len();
    let request = engine.request(revise(&engine, "perform", true), "reactivate");
    let response = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(response["result"]["goal_revision"], 2);
    assert_eq!(response["result"]["reactivated"], true);
    let revised = engine.graph();
    assert_eq!(revised.trace().lifecycle(), SessionLifecycle::Running);
    assert_eq!(revised.trace().attempts().len(), length + 1);
    assert!(revised.trace().active_attempt().is_some());
    assert_eq!(revised.completed_at(), None);
    engine.cold();
    assert_eq!(engine.graph(), revised);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), response);
}

#[test]
fn v2gol002_assessment_mode_records_citations_attribution_replay_and_restart() {
    let mut engine = at_decision(ASSESSMENT_PROCEDURE);
    engine.set("assessment-note", "decision-local note", "local-note");
    let attempt = engine.fences().expected_attempt_id;
    let mut command = assess(&engine, "correct", "satisfied");
    let DirectCommandV1::Procedure(P::GoalAssessCriterion(input)) = &mut command else {
        unreachable!()
    };
    input.evidence = vec![GraphNodeId::new("perform").unwrap()];
    input.items = vec![ItemId::new("assessment-note").unwrap()];
    input.actor = Some("reviewer".into());
    let request = engine.request(command, "correct");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    let result = &original["result"];
    assert_eq!(result["schema"], "podway.criterion-assessment-result/v2");
    assert_eq!(result["graph_node_id"], "decide");
    assert_eq!(result["attempt_id"], json!(attempt));
    assert_eq!(result["goal_revision"], 1);
    assert_eq!(result["mode"], "assessment");
    assert_eq!(result["result"]["criterion_id"], "correct");
    assert_eq!(result["result"]["status"], "satisfied");
    assert_eq!(
        result["result"]["reason"],
        "The source and local note support correctness."
    );
    assert_eq!(
        result["result"]["citations"],
        json!([{ "reference_graph_node_id": "perform" }, { "local_item_id": "assessment-note" }])
    );
    assert_eq!(result["complete"], false);
    assert!(result.get("determined_outcome").is_none());
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    assert_eq!(engine.fences().expected_attempt_id, attempt);
    let request = engine.request(assess(&engine, "tested", "unsatisfied"), "tested");
    let second = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(second["result"]["mode"], "assessment");
    assert_eq!(second["result"]["complete"], true);
    assert_eq!(second["result"]["determined_outcome"], "not_achieved");
    engine.cold();
    assert_eq!(engine.fences().expected_attempt_id, attempt);
    let readback = status(&engine);
    assert_eq!(readback["goal"]["determined_outcome"], "not_achieved");
    assert_eq!(
        readback["goal"]["criteria"],
        json!([
        {"criterion_id":"correct","statement":"The result is correct.","status":"satisfied"},
        {"criterion_id":"tested","statement":"The result is tested.","status":"unsatisfied"}])
    );
    assert_eq!(engine.output(engine.execute(request, &NoClock)), second);
    let graph = engine.graph();
    let results = graph.goal_state().attempt_assessments()[0].results();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].actor().unwrap().as_str(), "reviewer");
    assert_eq!(results[1].actor(), None);
}

#[test]
fn v2gol002_applicability_mode_accepts_only_uncited_not_applicable_results() {
    let mut engine = at_decision(ASSESSMENT_PROCEDURE);
    let mut cited = assess(&engine, "correct", "not_applicable");
    let DirectCommandV1::Procedure(P::GoalAssessCriterion(input)) = &mut cited else {
        unreachable!()
    };
    input.evidence.push(GraphNodeId::new("perform").unwrap());
    error(&mut engine, cited, "cited-na", "REQUEST_INVALID");
    let first = engine.success(assess(&engine, "correct", "not_applicable"), "correct");
    assert_eq!(first["result"]["mode"], "applicability");
    assert_eq!(first["result"]["result"]["citations"], json!([]));
    assert_eq!(first["result"]["complete"], false);
    let second = engine.success(assess(&engine, "tested", "not_applicable"), "tested");
    assert_eq!(second["result"]["mode"], "applicability");
    assert_eq!(second["result"]["complete"], true);
    assert_eq!(second["result"]["determined_outcome"], "superseded");
}

#[test]
fn v2gol002_invalid_runtime_assessments_are_atomic() {
    let mut engine = at_decision(ASSESSMENT_PROCEDURE);
    engine.success(assess(&engine, "correct", "satisfied"), "correct");
    let cases = [
        ("mixed", "tested", "not_applicable", "CRITERION_MODE_MIXED"),
        ("duplicate", "correct", "unsatisfied", "REQUEST_INVALID"),
        ("missing", "missing", "satisfied", "CRITERION_NOT_FOUND"),
        (
            "evidence",
            "tested",
            "satisfied",
            "CRITERION_CITATION_INVALID",
        ),
        ("item", "tested", "satisfied", "CRITERION_CITATION_INVALID"),
        ("goal", "tested", "satisfied", "GOAL_REVISION_STALE"),
        (
            "session",
            "tested",
            "satisfied",
            "SESSION_REVISION_CONFLICT",
        ),
        ("attempt", "tested", "satisfied", "ATTEMPT_NOT_CURRENT"),
    ];
    for (label, criterion, state, code) in cases {
        let mut command = assess(&engine, criterion, state);
        let DirectCommandV1::Procedure(P::GoalAssessCriterion(input)) = &mut command else {
            unreachable!()
        };
        match label {
            "evidence" => input.evidence.push(GraphNodeId::new("finish").unwrap()),
            "item" => input.items.push(ItemId::new("result").unwrap()),
            "goal" => input.expected_goal_revision = 2,
            "session" => input.preconditions.expected_session_revision = Revision::ZERO,
            "attempt" => {
                input.preconditions.expected_attempt_id =
                    AttemptId::new("00000000-0000-4000-8000-000000102099").unwrap()
            }
            _ => (),
        }
        error(&mut engine, command, label, code);
    }
}

#[test]
fn v2gol002_duplicate_citations_fail_during_closed_request_decode_without_mutation() {
    let mut engine = at_decision(ASSESSMENT_PROCEDURE);
    for local in [false, true] {
        let mut command = assess(&engine, "correct", "satisfied");
        let DirectCommandV1::Procedure(P::GoalAssessCriterion(input)) = &mut command else {
            unreachable!()
        };
        if local {
            input.items = vec![ItemId::new("assessment-note").unwrap(); 2];
        } else {
            input.evidence = vec![GraphNodeId::new("perform").unwrap(); 2];
        }
        error(
            &mut engine,
            command,
            if local {
                "duplicate-item"
            } else {
                "duplicate-evidence"
            },
            "REQUEST_INVALID",
        );
    }
}

#[test]
fn v2gol003_outcome_state_table_is_actor_claimed_and_replayable() {
    for (left, right, option, outcome) in [
        ("satisfied", "satisfied", "achieved", "achieved"),
        ("satisfied", "unsatisfied", "not-achieved", "not_achieved"),
        (
            "not_applicable",
            "not_applicable",
            "superseded",
            "superseded",
        ),
    ] {
        let mut engine = at_decision(GOAL_PROCEDURE);
        engine.success(assess(&engine, "correct", left), "correct");
        engine.success(assess(&engine, "tested", right), "tested");
        let request = engine.request(decide(&engine, option), "decide");
        let original = engine.output(engine.execute(request.clone(), &Clock));
        assert_eq!(original["result"]["option_id"], option);
        assert_eq!(original["result"]["target_graph_node_id"], "finish");
        let record = &original["result"]["record"];
        assert_eq!(record["assessment"], "session_goal");
        assert_eq!(record["goal_outcome"], outcome);
        assert_eq!(
            record["assessment_mode"],
            if outcome == "superseded" {
                "applicability"
            } else {
                "assessment"
            }
        );
        assert_eq!(record["criterion_results"].as_array().unwrap().len(), 2);
        assert_eq!(record["criterion_results"][0]["criterion_id"], "correct");
        assert_eq!(record["criterion_results"][0]["status"], left);
        assert_eq!(record["criterion_results"][1]["criterion_id"], "tested");
        assert_eq!(record["criterion_results"][1]["status"], right);
        let graph = engine.graph();
        let assessment = graph.goal_state().assessments().last().unwrap();
        assert_eq!(assessment.outcome().as_str(), outcome);
        assert_eq!(assessment.actor().unwrap().as_str(), "owner");
        assert_eq!(
            graph.goal_state().attempt_assessments()[0].results().len(),
            2
        );
        engine.cold();
        assert_eq!(engine.graph(), graph);
        assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    }
}

#[test]
fn v2gol003_incomplete_and_mismatched_outcomes_fail_atomically() {
    let mut engine = at_decision(GOAL_PROCEDURE);
    engine.success(assess(&engine, "correct", "satisfied"), "correct");
    let mut missing = decide(&engine, "achieved");
    let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut missing else {
        unreachable!()
    };
    input.expected_goal_revision = None;
    error(
        &mut engine,
        missing,
        "missing-goal-fence",
        "REQUEST_INVALID",
    );
    let mut stale = decide(&engine, "achieved");
    let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut stale else {
        unreachable!()
    };
    input.expected_goal_revision = Some(2);
    let value = error(
        &mut engine,
        stale,
        "stale-goal-fence",
        "GOAL_REVISION_STALE",
    );
    assert_eq!(value["details"]["expected_goal_revision"], 2);
    assert_eq!(value["details"]["actual_goal_revision"], 1);
    let command = decide(&engine, "achieved");
    let value = error(
        &mut engine,
        command,
        "incomplete",
        "CRITERION_RESULT_MISSING",
    );
    assert_eq!(value["details"]["missing_criterion_ids"], json!(["tested"]));
    engine.success(assess(&engine, "tested", "satisfied"), "tested");
    let command = decide(&engine, "not-achieved");
    let value = error(
        &mut engine,
        command,
        "mismatch",
        "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED",
    );
    assert_eq!(value["details"]["option_id"], "not-achieved");
    assert_eq!(value["details"]["determined_outcome"], "achieved");
    assert_eq!(value["details"]["allowed_option_ids"], json!(["achieved"]));
}

#[test]
fn v2gol003_general_decision_never_creates_goal_assessment_state() {
    let mut engine = goal_engine(GENERAL_DECISION_PROCEDURE);
    let mut stale = decide(&engine, "continue");
    let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut stale else {
        unreachable!()
    };
    input.expected_goal_revision = Some(2);
    error(&mut engine, stale, "stale", "GOAL_REVISION_STALE");
    let request = engine.request(decide(&engine, "continue"), "decide");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(original["result"]["record"]["goal_revision"], 1);
    for field in [
        "assessment",
        "goal_outcome",
        "criterion_results",
        "assessment_mode",
        "goal_assessment",
    ] {
        assert!(
            original["result"]["record"].get(field).is_none(),
            "unexpected {field}"
        );
    }
    assert!(engine.graph().goal_state().attempt_assessments().is_empty());
    assert!(engine.graph().goal_state().assessments().is_empty());
    let graph = engine.graph();
    engine.cold();
    assert_eq!(engine.graph(), graph);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
}

#[test]
fn v2gol003_goal_assessment_wrong_verb_on_general_decision_is_not_admitted_and_changes_no_state() {
    let mut engine = goal_engine(GENERAL_DECISION_PROCEDURE);
    let command = assess(&engine, "correct", "satisfied");
    error(&mut engine, command, "wrong-verb", "REQUEST_INVALID");
}

#[test]
fn v2gol003_queued_v9_goal_assessment_fails_and_replays_after_cold_reopen() {
    let mut engine = at_decision(GOAL_PROCEDURE);
    let command = decide(&engine, "achieved");
    error(
        &mut engine,
        command,
        "requires-assessment",
        "CRITERION_RESULT_MISSING",
    );
    assert!(engine.graph().goal_state().assessments().is_empty());
}

#[test]
fn v2gol003_fresh_assessment_allows_terminal_complete_and_skip_after_restart() {
    for skip in [false, true] {
        let mut engine = at_decision(GOAL_PROCEDURE);
        engine.success(assess(&engine, "correct", "satisfied"), "correct");
        engine.success(assess(&engine, "tested", "satisfied"), "tested");
        let request = engine.request(decide(&engine, "achieved"), "decide");
        let original = engine.output(engine.execute(request.clone(), &Clock));
        engine.cold();
        assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
        let response = if skip {
            engine.success(
                engine.skip(Some("Terminal work is inapplicable.".into())),
                "skip",
            )
        } else {
            engine.complete("complete")
        };
        assert_eq!(response["result"]["session_state"], "completed");
        assert_eq!(
            engine.graph().trace().lifecycle(),
            SessionLifecycle::Completed
        );
        assert!(engine.graph().trace().active_attempt().is_none());
        assert!(status(&engine)["current"].is_null());
    }
}

#[test]
fn v2gol005_failure_matrix_is_atomic_causal_and_cold_replayable() {
    let mut engine = at_decision(FAILURE_PROCEDURE);
    let first_attempt = engine.fences().expected_attempt_id;
    engine.success(assess(&engine, "correct", "satisfied"), "correct");
    let command = assess(&engine, "tested", "not_applicable");
    error(&mut engine, command, "mixed", "CRITERION_MODE_MIXED");
    engine.success(engine.retry("Retry applicability.".into()), "retry-na");
    engine.success(assess(&engine, "correct", "not_applicable"), "na");
    let command = assess(&engine, "tested", "satisfied");
    error(
        &mut engine,
        command,
        "reverse-mixed",
        "CRITERION_MODE_MIXED",
    );
    engine.success(engine.retry("Retry assessment.".into()), "retry-assessment");
    let before = engine.graph().trace().revision();
    let delayed = revise(&engine, "perform", false);
    let revised = engine.success(revise(&engine, "perform", false), "revision-b");
    assert_eq!(revised["result"]["goal_revision"], 2);
    let current = engine.graph().trace().revision();
    let value = error(&mut engine, delayed, "delayed", "SESSION_REVISION_CONFLICT");
    assert_eq!(value["details"]["expected_revision"], before.get());
    assert_eq!(value["details"]["current_revision"], current.get());
    let mut old_goal = assess(&engine, "correct", "satisfied");
    let DirectCommandV1::Procedure(P::GoalAssessCriterion(input)) = &mut old_goal else {
        unreachable!()
    };
    input.expected_goal_revision = 1;
    let value = error(&mut engine, old_goal, "old-goal", "GOAL_REVISION_STALE");
    assert_eq!(value["details"]["expected_goal_revision"], 1);
    assert_eq!(value["details"]["actual_goal_revision"], 2);
    let mut stale_attempt = assess(&engine, "correct", "satisfied");
    let DirectCommandV1::Procedure(P::GoalAssessCriterion(input)) = &mut stale_attempt else {
        unreachable!()
    };
    input.preconditions.expected_attempt_id = first_attempt;
    error(
        &mut engine,
        stale_attempt,
        "stale-attempt",
        "ATTEMPT_NOT_CURRENT",
    );
    let command = revise(&engine, "prelude", false);
    assert_eq!(
        error(
            &mut engine,
            command,
            "forbidden",
            "GOAL_REVISION_TARGET_NOT_ALLOWED"
        )["details"]["target_graph_node_id"],
        "prelude"
    );
    let command = revise(&engine, "decide", false);
    error(
        &mut engine,
        command,
        "off-trace",
        "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
    );
    engine.set("result", "Updated evidence.", "updated");
    engine.complete("reworked-perform");
    engine.success(assess(&engine, "correct", "satisfied"), "final-correct");
    engine.success(assess(&engine, "tested", "satisfied"), "final-tested");
    let ready = status(&engine);
    assert_eq!(ready["goal"]["determined_outcome"], "achieved");
    assert_eq!(
        ready["goal"]["criteria"],
        json!([
        {"criterion_id":"correct","statement":"The result is correct.","status":"satisfied"},
        {"criterion_id":"tested","statement":"The result is tested.","status":"satisfied"}])
    );
    assert_eq!(ready["current"]["readiness"]["can_advance"], true);
    engine.success(decide(&engine, "achieved"), "final-decide");
    engine.complete("terminal");
    assert_eq!(
        engine.graph().trace().lifecycle(),
        SessionLifecycle::Completed
    );
    let command = revise(&engine, "perform", false);
    error(
        &mut engine,
        command,
        "missing-reactivate",
        "REACTIVATION_FLAG_REQUIRED",
    );
    let command = revise(&engine, "finish", true);
    assert_eq!(
        error(
            &mut engine,
            command,
            "unsafe",
            "GOAL_REVISION_TARGET_NOT_REVISION_SAFE"
        )["details"]["target_graph_node_id"],
        "finish"
    );
    engine.success(revise(&engine, "perform", true), "reactivate");
    engine.success(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
            reason: "Cancelled sessions cannot revise goals.".into(),
            preconditions: engine.fences(),
        })),
        "cancel",
    );
    let command = revise(&engine, "perform", true);
    error(&mut engine, command, "cancelled", "SESSION_CANCELLED");
}

#[test]
fn v2gol005_queued_assessment_executes_after_cold_reopen_and_replays_exactly() {
    let mut engine = at_decision(FAILURE_PROCEDURE);
    let attempt = engine.fences().expected_attempt_id;
    let mut command = assess(&engine, "correct", "satisfied");
    let DirectCommandV1::Procedure(P::GoalAssessCriterion(input)) = &mut command else {
        unreachable!()
    };
    input.evidence.push(GraphNodeId::new("perform").unwrap());
    let request = engine.request(command, "assessment");
    engine.cold();
    let original = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(original["result"]["result"]["criterion_id"], "correct");
    assert_eq!(original["result"]["result"]["status"], "satisfied");
    let lookup = engine.success(
        DirectCommandV1::OutcomeLookup {
            key: IdempotencyKeyV1::new("assessment").unwrap(),
        },
        "lookup",
    );
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(engine.fences().expected_attempt_id, attempt);
    assert_eq!(
        status(&engine)["goal"]["criteria"][0]["status"],
        "satisfied"
    );
    let drift = assess(&engine, "correct", "unsatisfied");
    error(&mut engine, drift, "drift", "REQUEST_INVALID");
    let drift = engine.request(assess(&engine, "tested", "satisfied"), "assessment");
    let DirectResponseV1::Failure(error) = engine.execute(drift, &NoClock) else {
        panic!("payload drift accepted")
    };
    assert_eq!(error.into_value()["code"], "IDEMPOTENCY_KEY_REUSED");
    let graph = engine.graph();
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    assert_eq!(engine.graph(), graph);
}

#[test]
fn v2gol005_concurrent_distinct_criteria_commit_exactly_one_result() {
    let mut engine = at_decision(FAILURE_PROCEDURE);
    let graph = engine.graph();
    let attempt = engine.fences().expected_attempt_id;
    let left = engine.request(assess(&engine, "correct", "satisfied"), "left");
    let right = engine.request(assess(&engine, "tested", "satisfied"), "right");
    let barrier = Arc::new(Barrier::new(3));
    let responses = thread::scope(|scope| {
        let left_barrier = Arc::clone(&barrier);
        let right_barrier = Arc::clone(&barrier);
        let borrowed = &engine;
        let left = scope.spawn(move || {
            left_barrier.wait();
            borrowed.execute(left, &Clock)
        });
        let right = scope.spawn(move || {
            right_barrier.wait();
            borrowed.execute(right, &Clock)
        });
        barrier.wait();
        [left.join().unwrap(), right.join().unwrap()]
    });
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(response, DirectResponseV1::Success(_)))
            .count(),
        1
    );
    let loser = responses
        .iter()
        .find_map(|response| match response {
            DirectResponseV1::Failure(error) => Some(error.clone().into_value()),
            _ => None,
        })
        .unwrap();
    assert_eq!(loser["code"], "SESSION_REVISION_CONFLICT");
    let after = status(&engine);
    assert_eq!(engine.fences().expected_attempt_id, attempt);
    assert_eq!(
        after["goal"]["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|criterion| criterion["status"] == "satisfied")
            .count(),
        1
    );
    assert_eq!(after["goal"]["determined_outcome"], Value::Null);
    assert_eq!(
        engine.graph().trace().attempts().len(),
        graph.trace().attempts().len()
    );
    let outcomes = engine
        .store
        .read(deadline(), |snapshot| {
            Ok([
                snapshot
                    .outcome(&IdempotencyKeyV1::new("left").unwrap())?
                    .is_some(),
                snapshot
                    .outcome(&IdempotencyKeyV1::new("right").unwrap())?
                    .is_some(),
            ])
        })
        .unwrap();
    assert_eq!(outcomes.iter().filter(|found| **found).count(), 1);
    let graph = engine.graph();
    engine.cold();
    assert_eq!(engine.graph(), graph);
}

#[test]
fn v2gol_epic_goal_mutations_without_opt_in_fail_atomically_and_replay_after_restart() {
    let source = GOAL_PROCEDURE.replace("goal_tracking: true\n", "").replace("    assessment:\n      target: session_goal\n      outcomes:\n        achieved: achieved\n        not-achieved: not_achieved\n        superseded: superseded\n", "");
    let mut engine = Engine::new(&source);
    let command = define(&engine);
    error(
        &mut engine,
        command,
        "define-disabled",
        "GOAL_TRACKING_NOT_ENABLED",
    );
    let command = revise(&engine, "perform", false);
    error(
        &mut engine,
        command,
        "revise-disabled",
        "GOAL_TRACKING_NOT_ENABLED",
    );
    let command = assess(&engine, "correct", "satisfied");
    error(
        &mut engine,
        command,
        "assess-disabled",
        "GOAL_TRACKING_NOT_ENABLED",
    );
    let command = assess_many(
        &engine,
        vec![entry("correct", "satisfied", "ok", &[], &[])],
        None,
    );
    error(
        &mut engine,
        command,
        "assess-many-disabled",
        "GOAL_TRACKING_NOT_ENABLED",
    );
}

#[test]
fn v2gol_epic_running_reactivate_true_is_inert_and_cold_replayable() {
    let mut enabled = goal_engine(GOAL_PROCEDURE);
    let disabled = goal_engine(GOAL_PROCEDURE);
    let request = enabled.request(revise(&enabled, "perform", true), "revise");
    let original = enabled.output(enabled.execute(request.clone(), &Clock));
    let other = disabled.success(revise(&disabled, "perform", false), "revise");
    assert_eq!(original["result"]["reactivated"], false);
    assert_eq!(other["result"]["reactivated"], false);
    assert_eq!(original["result"]["goal_revision"], 2);
    assert_eq!(original["result"]["rework_to"], "perform");
    assert_eq!(other["result"]["goal_revision"], 2);
    assert_eq!(other["result"]["rework_to"], "perform");
    let warm_projection = running_projection(&status(&enabled));
    assert_eq!(warm_projection, running_projection(&status(&disabled)));
    let left = enabled.graph();
    let right = disabled.graph();
    assert_eq!(left.trace().lifecycle(), right.trace().lifecycle());
    assert_eq!(left.trace().revision(), right.trace().revision());
    assert_eq!(
        left.goal_state().current_revision(),
        right.goal_state().current_revision()
    );
    assert_eq!(
        left.trace().attempts().len(),
        right.trace().attempts().len()
    );
    assert_eq!(
        left.trace().active_attempt().unwrap().graph_node_id(),
        right.trace().active_attempt().unwrap().graph_node_id()
    );
    enabled.cold();
    assert_eq!(enabled.graph(), left);
    assert_eq!(enabled.output(enabled.execute(request, &NoClock)), original);
    assert_eq!(running_projection(&status(&enabled)), warm_projection);
}

fn assess_many(
    engine: &Engine,
    assessments: Vec<CriterionAssessmentManyEntryWireV1>,
    actor: Option<&str>,
) -> DirectCommandV1 {
    DirectCommandV1::Procedure(P::GoalAssessCriteria(GoalAssessCriteriaV2 {
        assessments,
        actor: actor.map(str::to_owned),
        preconditions: engine.fences(),
        expected_goal_revision: engine
            .graph()
            .goal_state()
            .current_revision()
            .map_or(1, |revision| revision.get()),
    }))
}

fn entry(
    criterion: &str,
    status: &str,
    reason: &str,
    evidence: &[&str],
    items: &[&str],
) -> CriterionAssessmentManyEntryWireV1 {
    CriterionAssessmentManyEntryWireV1 {
        criterion_id: CriterionId::new(criterion).unwrap(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        evidence: evidence
            .iter()
            .map(|node| GraphNodeId::new(*node).unwrap())
            .collect(),
        items: items.iter().map(|id| ItemId::new(*id).unwrap()).collect(),
    }
}

#[test]
fn agent003_goal_assess_criteria_enforces_pre_fence_bounds_fences_and_ordered_entry_errors() {
    use podway_protocol::ContractGenerationV1;

    let mut engine = at_decision(ASSESSMENT_PROCEDURE);
    engine.set("assessment-note", "Recorded note on decision.", "set-note");

    // 1. Pre-fence shape/resource bounds fail before clock or state mutation.
    let empty_cmd = assess_many(&engine, Vec::new(), None);
    let empty_req = engine.request(empty_cmd, "agent003-empty");
    let DirectResponseV1::Failure(err) = engine.execute(empty_req, &NoClock) else {
        panic!("expected empty assessments failure")
    };
    assert_eq!(err.into_value()["code"], "REQUEST_INVALID");

    let seventeen_entries: Vec<_> = (1..=17)
        .map(|i| entry(&format!("c-{i}"), "satisfied", "ok", &[], &[]))
        .collect();
    let seventeen_req = engine.request(
        assess_many(&engine, seventeen_entries, None),
        "agent003-seventeen",
    );
    let DirectResponseV1::Failure(err) = engine.execute(seventeen_req, &NoClock) else {
        panic!("expected 17 assessments failure")
    };
    assert_eq!(err.into_value()["code"], "REQUEST_INVALID");

    let mut dup_cmd = assess_many(
        &engine,
        vec![
            entry("correct", "satisfied", "first", &[], &[]),
            entry("correct", "satisfied", "second", &[], &[]),
        ],
        None,
    );
    if let DirectCommandV1::Procedure(P::GoalAssessCriteria(input)) = &mut dup_cmd {
        input.preconditions.expected_session_revision = Revision::ZERO;
    }
    let dup_req = engine.request(dup_cmd, "agent003-dup");
    let DirectResponseV1::Failure(err) = engine.execute(dup_req, &NoClock) else {
        panic!("expected duplicate criterion failure")
    };
    assert_eq!(err.into_value()["code"], "REQUEST_INVALID");

    let mut zero_goal_cmd = assess_many(
        &engine,
        vec![entry("correct", "satisfied", "ok", &[], &[])],
        None,
    );
    if let DirectCommandV1::Procedure(P::GoalAssessCriteria(input)) = &mut zero_goal_cmd {
        input.expected_goal_revision = 0;
    }
    error(
        &mut engine,
        zero_goal_cmd,
        "agent003-zero-goal",
        "REQUEST_INVALID",
    );

    let huge_reason = "x".repeat(1_048_576);
    let huge_cmd = assess_many(
        &engine,
        vec![entry("correct", "satisfied", &huge_reason, &[], &[])],
        None,
    );
    let huge_req = engine.request(huge_cmd, "agent003-too-large");
    let DirectResponseV1::Failure(err) = engine.execute(huge_req, &NoClock) else {
        panic!("expected REQUEST_TOO_LARGE")
    };
    assert_eq!(err.into_value()["code"], "REQUEST_TOO_LARGE");

    for (key, mut pre_fence_cmd) in [
        (
            "agent003-blank-actor",
            assess_many(
                &engine,
                vec![entry("correct", "satisfied", "ok", &[], &[])],
                Some("   "),
            ),
        ),
        (
            "agent003-overlong-actor",
            assess_many(
                &engine,
                vec![entry("correct", "satisfied", "ok", &[], &[])],
                Some(&"a".repeat(257)),
            ),
        ),
        (
            "agent003-overlong-reason",
            assess_many(
                &engine,
                vec![entry("correct", "satisfied", &"r".repeat(2001), &[], &[])],
                None,
            ),
        ),
        (
            "agent003-five-evidence-list",
            assess_many(
                &engine,
                vec![entry(
                    "correct",
                    "satisfied",
                    "ok",
                    &["perform", "decide", "finish", "perform", "decide"],
                    &[],
                )],
                None,
            ),
        ),
        (
            "agent003-five-items-list",
            assess_many(
                &engine,
                vec![entry(
                    "correct",
                    "satisfied",
                    "ok",
                    &[],
                    &[
                        "assessment-note",
                        "result",
                        "assessment-note",
                        "result",
                        "assessment-note",
                    ],
                )],
                None,
            ),
        ),
    ] {
        if let DirectCommandV1::Procedure(P::GoalAssessCriteria(input)) = &mut pre_fence_cmd {
            input.preconditions.expected_session_revision = Revision::ZERO;
        }
        let req = engine.request(pre_fence_cmd, key);
        let DirectResponseV1::Failure(err) = engine.execute(req, &NoClock) else {
            panic!("expected pre-fence REQUEST_INVALID for {key}")
        };
        assert_eq!(err.into_value()["code"], "REQUEST_INVALID", "{key}");
    }

    // 2. Fence and placement checks precede semantic entry evaluation.
    let before_ws_mismatch = engine.graph();
    let seq_before_ws_mismatch = engine.sequence();
    let mut bad_ws_req = engine.request(
        assess_many(
            &engine,
            vec![entry("correct", "invalid-status", "ok", &[], &[])],
            None,
        ),
        "agent003-ws-uuid-before-entry",
    );
    bad_ws_req.expected_workspace_id =
        Some(podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000999999").unwrap());
    let DirectResponseV1::Failure(ws_err) = engine.execute(bad_ws_req, &NoClock) else {
        panic!("expected WORKSPACE_UUID_MISMATCH")
    };
    assert_eq!(ws_err.into_value()["code"], "WORKSPACE_UUID_MISMATCH");
    assert_eq!(engine.graph(), before_ws_mismatch);
    assert_eq!(engine.sequence(), seq_before_ws_mismatch);
    assert!(
        engine
            .store
            .read(deadline(), |snapshot| {
                snapshot.outcome(&IdempotencyKeyV1::new("agent003-ws-uuid-before-entry").unwrap())
            })
            .unwrap()
            .is_none()
    );

    let mut bad_session_id_and_status = assess_many(
        &engine,
        vec![entry("correct", "invalid-status", "ok", &[], &[])],
        None,
    );
    if let DirectCommandV1::Procedure(P::GoalAssessCriteria(input)) = &mut bad_session_id_and_status
    {
        input.preconditions.expected_session_id =
            podway_core::SessionId::new("00000000-0000-4000-8000-000000999999").unwrap();
    }
    error(
        &mut engine,
        bad_session_id_and_status,
        "agent003-session-id-before-entry",
        "SESSION_ID_MISMATCH",
    );

    let mut bad_attempt_id_and_status = assess_many(
        &engine,
        vec![entry("correct", "invalid-status", "ok", &[], &[])],
        None,
    );
    if let DirectCommandV1::Procedure(P::GoalAssessCriteria(input)) = &mut bad_attempt_id_and_status
    {
        input.preconditions.expected_attempt_id =
            AttemptId::new("00000000-0000-4000-8000-000000999999").unwrap();
    }
    error(
        &mut engine,
        bad_attempt_id_and_status,
        "agent003-attempt-id-before-entry",
        "ATTEMPT_NOT_CURRENT",
    );

    let mut bad_rev_and_status = assess_many(
        &engine,
        vec![entry("correct", "invalid-status", "ok", &[], &[])],
        None,
    );
    if let DirectCommandV1::Procedure(P::GoalAssessCriteria(input)) = &mut bad_rev_and_status {
        input.preconditions.expected_session_revision = Revision::new(999);
    }
    error(
        &mut engine,
        bad_rev_and_status,
        "agent003-rev-before-entry",
        "SESSION_REVISION_CONFLICT",
    );

    let mut bad_goal_rev_and_status = assess_many(
        &engine,
        vec![entry("correct", "invalid-status", "ok", &[], &[])],
        None,
    );
    if let DirectCommandV1::Procedure(P::GoalAssessCriteria(input)) = &mut bad_goal_rev_and_status {
        input.expected_goal_revision = 99;
    }
    error(
        &mut engine,
        bad_goal_rev_and_status,
        "agent003-goal-rev-before-entry",
        "GOAL_REVISION_STALE",
    );

    let mut action_engine = goal_engine(ASSESSMENT_PROCEDURE);
    let on_action = assess_many(
        &action_engine,
        vec![entry("correct", "satisfied", "ok", &[], &[])],
        None,
    );
    error(
        &mut action_engine,
        on_action,
        "agent003-on-action",
        "GRAPH_NODE_TYPE_MISMATCH",
    );

    let mut general_engine = goal_engine(GENERAL_DECISION_PROCEDURE);
    let on_general_decision = assess_many(
        &general_engine,
        vec![entry("correct", "satisfied", "ok", &[], &[])],
        None,
    );
    error(
        &mut general_engine,
        on_general_decision,
        "agent003-on-general-decision",
        "REQUEST_INVALID",
    );

    // 3. Ordered entry validation and generation 1 vs generation 2 citation error details.
    let blank_reason = assess_many(
        &engine,
        vec![entry("correct", "satisfied", "   ", &[], &[])],
        None,
    );
    error(
        &mut engine,
        blank_reason,
        "agent003-blank-reason",
        "REQUEST_INVALID",
    );

    let missing_criterion_cmd = assess_many(
        &engine,
        vec![
            entry("missing", "satisfied", "ok", &[], &[]),
            entry("tested", "not_applicable", "later mode mismatch", &[], &[]),
        ],
        None,
    );
    let missing_err = error(
        &mut engine,
        missing_criterion_cmd,
        "agent003-criterion-not-found",
        "CRITERION_NOT_FOUND",
    );
    assert_eq!(missing_err["details"]["criterion_id"], "missing");

    let five_citations = assess_many(
        &engine,
        vec![entry(
            "correct",
            "satisfied",
            "ok",
            &["perform", "decide", "finish"],
            &["assessment-note", "result"],
        )],
        None,
    );
    error(
        &mut engine,
        five_citations,
        "agent003-five-citations",
        "REQUEST_INVALID",
    );

    let dup_citation = assess_many(
        &engine,
        vec![entry(
            "correct",
            "satisfied",
            "ok",
            &["perform", "perform"],
            &[],
        )],
        None,
    );
    error(
        &mut engine,
        dup_citation,
        "agent003-dup-citation",
        "REQUEST_INVALID",
    );

    let mixed_batch = assess_many(
        &engine,
        vec![
            entry("correct", "satisfied", "ok", &[], &[]),
            entry("tested", "not_applicable", "superseded", &[], &[]),
        ],
        None,
    );
    error(
        &mut engine,
        mixed_batch,
        "agent003-mixed-batch",
        "CRITERION_MODE_MIXED",
    );

    let invalid_citation_cmd = assess_many(
        &engine,
        vec![
            entry(
                "correct",
                "satisfied",
                "ok",
                &["perform"],
                &["assessment-note"],
            ),
            entry("tested", "satisfied", "bad item", &[], &["unrecorded-item"]),
        ],
        None,
    );
    let gen1_err = error(
        &mut engine,
        invalid_citation_cmd.clone(),
        "agent003-cite-gen1",
        "CRITERION_CITATION_INVALID",
    );
    assert_eq!(
        gen1_err["details"]["schema"],
        "podway.v2-runtime-error-details/v2"
    );
    assert_eq!(gen1_err["details"]["criterion_id"], "tested");
    assert_eq!(
        gen1_err["details"]["citation"],
        json!({"local_item_id": "unrecorded-item"})
    );

    let mut gen2_req = engine.request(invalid_citation_cmd, "agent003-cite-gen2");
    gen2_req.generation = ContractGenerationV1::Generation2;
    let DirectResponseV1::Failure(gen2_failure) = engine.execute(gen2_req, &Clock) else {
        panic!("expected gen2 citation failure")
    };
    let gen2_val = gen2_failure.into_value();
    assert_eq!(gen2_val["code"], "CRITERION_CITATION_INVALID");
    assert_eq!(
        gen2_val["details"]["schema"],
        "podway.criterion-citation-invalid-details/v1"
    );
    assert_eq!(
        gen2_val["details"]["citable_evidence_source_nodes"],
        json!(["perform"])
    );
    assert_eq!(
        gen2_val["details"]["citable_item_ids"],
        json!(["assessment-note"])
    );

    let invalid_evidence_cmd = assess_many(
        &engine,
        vec![
            entry("correct", "satisfied", "bad evidence", &["finish"], &[]),
            entry("tested", "invalid-status", "later bad status", &[], &[]),
        ],
        None,
    );
    let gen1_ev_err = error(
        &mut engine,
        invalid_evidence_cmd.clone(),
        "agent003-cite-evidence-gen1",
        "CRITERION_CITATION_INVALID",
    );
    assert_eq!(gen1_ev_err["details"]["criterion_id"], "correct");
    assert_eq!(
        gen1_ev_err["details"]["citation"],
        json!({"reference_graph_node_id": "finish"})
    );
    let mut gen2_ev_req = engine.request(invalid_evidence_cmd, "agent003-cite-evidence-gen2");
    gen2_ev_req.generation = ContractGenerationV1::Generation2;
    let DirectResponseV1::Failure(gen2_ev_failure) = engine.execute(gen2_ev_req, &Clock) else {
        panic!("expected gen2 evidence citation failure")
    };
    let gen2_ev_val = gen2_ev_failure.into_value();
    assert_eq!(gen2_ev_val["code"], "CRITERION_CITATION_INVALID");
    assert_eq!(
        gen2_ev_val["details"]["citation"],
        json!({"reference_graph_node_id": "finish"})
    );
    assert_eq!(
        gen2_ev_val["details"]["citable_evidence_source_nodes"],
        json!(["perform"])
    );

    // Skipped and unresolved evidence sources are also rejected in goal.assess_criteria.
    let exclusion_source = r#"schema: podway.procedure/v2
id: agent003-exclusions
version: "2"
name: Agent003 exclusions
purpose: Exercise skipped and unresolved evidence citations in batch assessment.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Produce evidence
    intent: Record the resolved source.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        max_length: 1000
  side:
    type: action
    title: Optional side work
    intent: Skipped before the consumer.
  lone:
    type: action
    title: Unvisited branch
    intent: Never attempted.
  pick:
    type: decision
    title: Pick the branch
    objective: Choose the main branch.
    prompt: Which branch?
    options:
      - id: main
        label: Main
      - id: alt
        label: Alt
    reason:
      required: true
  assess:
    type: decision
    title: Assess the goal
    objective: Select the outcome determined by the recorded criteria.
    prompt: Which goal outcome applies?
    items:
      - id: keep
        type: text
        prompt: Kept.
        required: false
        max_length: 100
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Finish.
graph:
  entry: pick
  nodes:
    - id: pick
      use: pick
      routes:
        main:
          to: source-r
          effect: advance
        alt:
          to: lone
          effect: advance
    - id: source-r
      use: work
      next: source-s
    - id: source-s
      use: side
      next: decide
      skip:
        allowed: true
        reason_required: false
    - id: lone
      use: lone
      next: source-r
    - id: decide
      use: assess
      evidence_from:
        - node: source-r
          required: true
          items:
            - result
        - node: source-s
          required: false
        - node: lone
          required: false
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - source-r
"#;
    let mut exclusion_engine = goal_engine(exclusion_source);
    exclusion_engine.success(
        DirectCommandV1::Procedure(P::SessionDecide(SessionDecideV2 {
            option_id: OptionId::new("main").unwrap(),
            reason: "Take main branch.".into(),
            actor: None,
            preconditions: exclusion_engine.fences(),
            expected_goal_revision: None,
        })),
        "agent003-excl-pick",
    );
    exclusion_engine.set("result", "Resolved evidence.", "agent003-excl-result");
    exclusion_engine.complete("agent003-excl-complete");
    exclusion_engine.success(
        exclusion_engine.skip(Some("Skip optional side work.".into())),
        "agent003-excl-skip",
    );
    exclusion_engine.set("keep", "Kept local note.", "agent003-excl-keep");
    for (bad_source, key_prefix) in [
        ("source-s", "agent003-cite-skipped"),
        ("lone", "agent003-cite-unresolved"),
    ] {
        let cmd = assess_many(
            &exclusion_engine,
            vec![
                entry(
                    "correct",
                    "satisfied",
                    "cites non-resolved",
                    &[bad_source],
                    &[],
                ),
                entry("tested", "invalid-status", "later bad status", &[], &[]),
            ],
            None,
        );
        let err_gen1 = error(
            &mut exclusion_engine,
            cmd.clone(),
            &format!("{key_prefix}-gen1"),
            "CRITERION_CITATION_INVALID",
        );
        assert_eq!(err_gen1["details"]["criterion_id"], "correct");
        assert_eq!(
            err_gen1["details"]["citation"],
            json!({"reference_graph_node_id": bad_source})
        );
        let mut req_gen2 = exclusion_engine.request(cmd, &format!("{key_prefix}-gen2"));
        req_gen2.generation = ContractGenerationV1::Generation2;
        let DirectResponseV1::Failure(failure_gen2) = exclusion_engine.execute(req_gen2, &Clock)
        else {
            panic!("expected gen2 failure for {bad_source}")
        };
        let val_gen2 = failure_gen2.into_value();
        assert_eq!(val_gen2["code"], "CRITERION_CITATION_INVALID");
        assert_eq!(
            val_gen2["details"]["citation"],
            json!({"reference_graph_node_id": bad_source})
        );
        assert_eq!(
            val_gen2["details"]["citable_evidence_source_nodes"],
            json!(["source-r"])
        );
        assert_eq!(val_gen2["details"]["citable_item_ids"], json!(["keep"]));
    }
}

#[test]
fn agent003_goal_assess_criteria_commits_partial_and_full_batches_replays_and_rejects_reordered_key_reuse()
 {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingClock {
        reads: AtomicUsize,
    }
    impl podway_engine::execution::ExecutionClockV1 for CountingClock {
        fn now(&self) -> UnixMillis {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Clock.now()
        }
    }

    let mut engine = at_decision(ASSESSMENT_PROCEDURE);
    engine.set("assessment-note", "Local decision note.", "set-note");

    let reverse_cmd = assess_many(
        &engine,
        vec![
            entry(
                "tested",
                "satisfied",
                "Tested thoroughly.",
                &[],
                &["assessment-note"],
            ),
            entry(
                "correct",
                "satisfied",
                "Verified against perform.",
                &["perform"],
                &[],
            ),
        ],
        Some("agent-to"),
    );
    let batch_req = engine.request(reverse_cmd.clone(), "agent003-batch-both");
    let seq_before_batch = engine.sequence();
    let rev_before_batch = engine.graph().trace().revision().get();
    let clock = CountingClock {
        reads: AtomicUsize::new(0),
    };
    let batch_out = engine.output(engine.execute(batch_req.clone(), &clock));
    assert_eq!(clock.reads.load(Ordering::SeqCst), 1);
    assert_eq!(engine.sequence(), seq_before_batch + 1);
    assert_eq!(
        engine.graph().trace().revision().get(),
        rev_before_batch + 1
    );
    assert_eq!(batch_out["command"], "goal.assess_criteria");
    assert_eq!(
        batch_out["result"]["schema"],
        "podway.criterion-assessment-many-result/v1"
    );
    assert_eq!(batch_out["result"]["mode"], "assessment");
    assert_eq!(batch_out["result"]["complete"], true);
    assert_eq!(batch_out["result"]["determined_outcome"], "achieved");
    let results = batch_out["result"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["criterion_id"], "correct");
    assert_eq!(
        results[0]["citations"],
        json!([{"reference_graph_node_id": "perform"}])
    );
    assert_eq!(results[1]["criterion_id"], "tested");
    assert_eq!(
        results[1]["citations"],
        json!([{"local_item_id": "assessment-note"}])
    );

    let duplicate_after_commit = assess_many(
        &engine,
        vec![entry("correct", "satisfied", "Again.", &[], &[])],
        None,
    );
    error(
        &mut engine,
        duplicate_after_commit,
        "agent003-already-recorded",
        "REQUEST_INVALID",
    );

    let DirectCommandV1::Procedure(P::GoalAssessCriteria(mut reordered_input)) = reverse_cmd else {
        unreachable!()
    };
    reordered_input.assessments.reverse();
    let reordered_req = engine.request(
        DirectCommandV1::Procedure(P::GoalAssessCriteria(reordered_input)),
        "agent003-batch-both",
    );
    let seq_before_reordered = engine.sequence();
    let DirectResponseV1::Failure(reordered_err) = engine.execute(reordered_req, &NoClock) else {
        panic!("expected IDEMPOTENCY_KEY_REUSED for changed assessment order")
    };
    assert_eq!(reordered_err.into_value()["code"], "IDEMPOTENCY_KEY_REUSED");
    assert_eq!(engine.sequence(), seq_before_reordered);

    engine.success(decide(&engine, "achieved"), "agent003-decide");
    engine.cold();
    let seq_before_replay = engine.sequence();
    let graph_before_replay = engine.graph();
    let replayed = engine.output(engine.execute(batch_req, &NoClock));
    assert_eq!(replayed, batch_out);
    assert_eq!(engine.sequence(), seq_before_replay);
    assert_eq!(engine.graph(), graph_before_replay);
    let looked_up = engine.success(
        DirectCommandV1::OutcomeLookup {
            key: IdempotencyKeyV1::new("agent003-batch-both").unwrap(),
        },
        "agent003-lookup",
    );
    assert_eq!(looked_up["result"]["found"], true);
    assert_eq!(looked_up["result"]["outcome"]["response"], batch_out);

    let mut partial_engine = at_decision(ASSESSMENT_PROCEDURE);
    let first_partial = partial_engine.success(
        assess_many(
            &partial_engine,
            vec![entry(
                "correct",
                "unsatisfied",
                "Edge case fails.",
                &["perform"],
                &[],
            )],
            None,
        ),
        "agent003-partial-1",
    );
    assert_eq!(first_partial["result"]["complete"], false);
    assert!(first_partial["result"].get("determined_outcome").is_none());
    assert_eq!(
        first_partial["result"]["results"].as_array().unwrap().len(),
        1
    );

    let mixed_second = assess_many(
        &partial_engine,
        vec![entry("tested", "not_applicable", "Wrong mode.", &[], &[])],
        None,
    );
    error(
        &mut partial_engine,
        mixed_second,
        "agent003-prior-mixed",
        "CRITERION_MODE_MIXED",
    );

    let second_partial = partial_engine.success(
        assess_many(
            &partial_engine,
            vec![entry("tested", "satisfied", "Tests ran.", &[], &[])],
            None,
        ),
        "agent003-partial-2",
    );
    assert_eq!(second_partial["result"]["complete"], true);
    assert_eq!(
        second_partial["result"]["determined_outcome"],
        "not_achieved"
    );
    assert_eq!(
        second_partial["result"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        second_partial["result"]["results"][0]["criterion_id"],
        "tested"
    );
}

const ASSESSMENT_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol002-assessment
version: "2"
name: Criterion assessment
purpose: Exercise durable criterion assessment.
description: A focused V2GOL-002 runtime fixture.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Produce evidence
    intent: Record evidence for assessment.
    instructions:
      - Record the evidence value.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        max_length: 1000
  assess:
    type: decision
    title: Assess the goal
    objective: Record every criterion result.
    prompt: Which goal outcome applies?
    items:
      - id: assessment-note
        type: text
        prompt: Record a local assessment note.
        required: false
        max_length: 1000
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
      prompt: Explain the assessment.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Finish the workflow.
    instructions:
      - Finish outside Podway.
graph:
  entry: perform
  nodes:
    - id: perform
      use: work
      next: decide
    - id: decide
      use: assess
      evidence_from:
        - node: perform
          required: true
          items:
            - result
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - perform
"#;

const GOAL_PROCEDURE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/goal-outcomes.yaml");

const GENERAL_DECISION_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol003-general-decision
version: "2"
name: General decision regression
purpose: Prove that an ordinary decision never creates a goal assessment.
goal_tracking: true
node_definitions:
  choose:
    type: decision
    title: Choose
    objective: Select the ordinary route.
    prompt: Continue?
    options:
      - id: continue
        label: Continue
    reason:
      required: true
  assess:
    type: decision
    title: Assess the goal
    objective: Select the recorded goal outcome.
    prompt: Which outcome applies?
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Finish the ordinary decision fixture.
graph:
  entry: choose
  nodes:
    - id: choose
      use: choose
      routes:
        continue:
          to: assess
          effect: advance
    - id: assess
      use: assess
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
"#;

const FAILURE_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol005-failure-closure
version: "2"
name: Goal failure closure
purpose: Exercise deterministic goal failure and recovery behavior.
goal_tracking: true
node_definitions:
  prelude:
    type: action
    title: Prepare
    intent: Establish an earlier valid trace placement.
  work:
    type: action
    title: Produce evidence
    intent: Record the evidence considered by the goal assessment.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        max_length: 1000
  assess:
    type: decision
    title: Assess the goal
    objective: Select the outcome determined by the criterion results.
    prompt: Which goal outcome applies?
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Finish after a fresh goal assessment.
graph:
  entry: prelude
  nodes:
    - id: prelude
      use: prelude
      next: perform
    - id: perform
      use: work
      next: decide
    - id: decide
      use: assess
      evidence_from:
        - node: perform
          required: true
          items:
            - result
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - perform
    - decide
    - finish
"#;
