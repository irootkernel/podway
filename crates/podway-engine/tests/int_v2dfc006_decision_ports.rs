//! Retained decision and manual rework assertions through the typed direct Engine.

use std::{
    sync::{Arc, Barrier},
    thread,
};

use podway_core::{AttemptId, GraphNodeId, OptionId, Revision, SessionLifecycle, TraceSequenceV2};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
    v2_read_service::GraphStatusTierV2,
};
use podway_protocol::{
    ProcedureV2MutationCommandV1 as P, RequestIdV1, ReworkPreconditionsWireV2, SessionCancelV1,
    SessionCompleteV1, SessionDecideV2, SessionReworkV2,
};
use podway_store::IdempotencyKeyV1;
use serde_json::{Value, json};

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline},
    int_v2dfc006_fixture_ports::{DECISION_PROCEDURE, FAILURE_PROCEDURE},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};

fn decide(engine: &Engine, option: &str) -> DirectCommandV1 {
    DirectCommandV1::Procedure(P::SessionDecide(SessionDecideV2 {
        option_id: OptionId::new(option).unwrap(),
        reason: "Evidence and review support acceptance.".into(),
        actor: Some("V2DRW reviewer".into()),
        preconditions: engine.fences(),
        expected_goal_revision: None,
    }))
}

fn rework(engine: &Engine, target: &str) -> DirectCommandV1 {
    let graph = engine.graph();
    DirectCommandV1::Procedure(P::SessionRework(SessionReworkV2 {
        target_graph_node_id: GraphNodeId::new(target).unwrap(),
        reason: "Return to the branch decision after a new finding.".into(),
        actor: Some("manual reviewer".into()),
        preconditions: ReworkPreconditionsWireV2 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph
                .trace()
                .active_attempt()
                .map(|attempt| attempt.attempt_id().clone()),
        },
    }))
}

fn read(engine: &Engine, tier: GraphStatusTierV2, before: Option<u64>) -> Value {
    let command = DirectCommandV1::Read {
        command: DirectReadCommandV1::Status {
            tier,
            history_before: before.map(TraceSequenceV2::new),
        },
        expected_session_id: Some(engine.graph().trace().session_id().clone()),
    };
    let value = engine.success(command, "unused-status");
    assert!(serde_json::to_vec(&value).unwrap().len() <= 1_048_576);
    value["result"].clone()
}
fn status(engine: &Engine) -> Value {
    read(engine, GraphStatusTierV2::Verbose, None)
}

fn at_review(source: &str, note: bool) -> Engine {
    let engine = Engine::new(source);
    engine.set("result", "work evidence", "result");
    engine.complete("complete-work");
    if note {
        engine.set("note", "reviewed locally", "note");
    }
    engine
}
fn left_branch() -> Engine {
    let engine = Engine::new(REWORK_PROCEDURE);
    assert_eq!(
        engine.success(decide(&engine, "left"), "left")["result"]["target_graph_node_id"],
        "left-work"
    );
    engine
}

fn reject(engine: &mut Engine, command: DirectCommandV1, key: &str, code: &str) -> Value {
    let graph = engine.graph();
    let sequence = engine.sequence();
    let before = status(engine);
    let request = engine.request(command, key);
    let value = match engine.execute(request.clone(), &Clock) {
        DirectResponseV1::Failure(error) => error.into_value(),
        DirectResponseV1::Success(output) => {
            panic!("expected {code}, received {:?}", output.as_value())
        }
    };
    assert_eq!(value["code"], code, "{key}");
    assert_eq!(engine.graph(), graph);
    assert_eq!(engine.sequence(), sequence);
    assert_eq!(status(engine), before);
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
        panic!("cold rejection accepted")
    };
    assert_eq!(rechecked.into_value()["code"], code);
    assert_eq!(engine.graph(), graph);
    assert_eq!(engine.sequence(), sequence);
    assert_eq!(status(engine), before);
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

fn counter<'a>(status: &'a Value, node: &str) -> &'a Value {
    status["counters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|counter| counter["graph_node_id"] == node)
        .unwrap()
}
fn history_window(value: &Value, total: u64, maximum: usize) {
    let entries = value["entries"].as_array().unwrap();
    assert_eq!(entries.len(), (total as usize).min(maximum));
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
            *sequences.iter().min().unwrap()
        );
        assert_eq!(
            value["trace_window"]["last_sequence"],
            *sequences.iter().max().unwrap()
        );
    }
}
fn cycle(engine: &Engine, n: u64) {
    assert_eq!(
        engine.success(decide(engine, "reject"), &format!("reject-{n}"))["result"]["target_graph_node_id"],
        "work"
    );
    engine.set("result", &format!("correction {n}"), &format!("result-{n}"));
    assert_eq!(
        engine.complete(&format!("complete-{n}"))["result"]["to_graph_node_id"],
        "review"
    );
    engine.set("note", &format!("review {n}"), &format!("note-{n}"));
}

#[test]
fn v2drw001_decide_validates_fences_and_rules_then_persists_exact_replay() {
    let mut engine = at_review(DECISION_PROCEDURE, false);
    let before = status(&engine);
    assert_eq!(before["current"]["node"]["graph_node_id"], "review");
    assert_eq!(before["current"]["readiness"]["items_satisfied"], false);
    assert_eq!(before["references"][0]["state"], "resolved");
    let command = DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
        preconditions: engine.fences(),
    }));
    let value = reject(
        &mut engine,
        command,
        "wrong-complete",
        "GRAPH_NODE_TYPE_MISMATCH",
    );
    assert_eq!(value["details"]["graph_node_id"], "review");
    assert_eq!(value["details"]["expected_node_type"], "action");
    assert_eq!(value["details"]["actual_node_type"], "decision");
    let command = decide(&engine, "accept");
    reject(
        &mut engine,
        command,
        "missing-note",
        "REQUIRED_ITEMS_MISSING",
    );
    let mut command = decide(&engine, "accept");
    let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut command else {
        unreachable!()
    };
    input.preconditions.expected_session_revision =
        Revision::new(engine.graph().trace().revision().get() + 1);
    reject(
        &mut engine,
        command,
        "stale-revision",
        "SESSION_REVISION_CONFLICT",
    );
    let mut command = decide(&engine, "accept");
    let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut command else {
        unreachable!()
    };
    input.preconditions.expected_attempt_id =
        AttemptId::new("00000000-0000-4000-8000-000000009199").unwrap();
    reject(&mut engine, command, "stale-attempt", "ATTEMPT_NOT_CURRENT");
    engine.set("note", "reviewed locally", "note");
    let ready = status(&engine);
    assert_eq!(ready["current"]["readiness"]["can_advance"], true);
    let command = decide(&engine, "unknown");
    assert_eq!(
        reject(&mut engine, command, "unknown", "OPTION_NOT_ALLOWED")["details"]["allowed_option_ids"],
        json!(["accept", "reject"])
    );
    let request = engine.request(decide(&engine, "accept"), "decide");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    let result = &original["result"];
    assert_eq!(
        result
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "attempt_id",
            "attempt_number",
            "effect",
            "graph_node_id",
            "option_id",
            "record",
            "revision",
            "schema",
            "session_state",
            "target_attempt_id",
            "target_graph_node_id"
        ]
    );
    assert_eq!(result["schema"], "podway.decision-result/v2");
    assert_eq!(result["graph_node_id"], "review");
    assert_eq!(
        result["attempt_id"],
        ready["current"]["attempt"]["attempt_id"]
    );
    assert_eq!(result["attempt_number"], 1);
    assert_eq!(result["option_id"], "accept");
    assert_eq!(result["effect"], "advance");
    assert_eq!(result["target_graph_node_id"], "finish");
    assert_eq!(result["session_state"], "running");
    assert_eq!(
        original["session"]["revision_after"],
        ready["session"]["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(result["revision"], original["session"]["revision_after"]);
    assert_eq!(result["record"]["session_id"], ready["session"]["id"]);
    assert_eq!(
        result["record"]["session_revision"],
        original["session"]["revision_after"]
    );
    assert_eq!(result["record"]["target_graph_node_id"], "finish");
    assert_eq!(
        result["record"]["reason"],
        "Evidence and review support acceptance."
    );
    assert_eq!(result["record"]["actor"], "V2DRW reviewer");
    assert_eq!(result["record"]["references"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["record"]["references"][0]["source_graph_node_id"],
        "work"
    );
    assert_eq!(result["record"]["references"][0]["state"], "resolved");
    engine.cold();
    let mut replay = request;
    replay.request_id = RequestIdV1::new("00000000-0000-4000-8000-000000009161").unwrap();
    assert_eq!(engine.output(engine.execute(replay, &NoClock)), original);
    let after = status(&engine);
    assert_eq!(after["current"]["node"]["graph_node_id"], "finish");
    assert_eq!(
        after["current"]["attempt"]["attempt_id"],
        result["target_attempt_id"]
    );
    assert_eq!(
        after["session"]["revision"],
        original["session"]["revision_after"]
    );
    assert_eq!(
        after["decision_history"]["entries"],
        json!([result["record"]])
    );
}

#[test]
fn v2drw002_declared_rework_preserves_the_exact_decision_and_branch_after_restart() {
    let mut engine = at_review(DECISION_PROCEDURE, true);
    let ready = status(&engine);
    let mut command = decide(&engine, "reject");
    let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut command else {
        unreachable!()
    };
    input.reason = "The reviewed evidence requires another work attempt.".into();
    input.actor = Some("V2DRW transition reviewer".into());
    let request = engine.request(command, "reject");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    let result = &original["result"];
    let record = &result["record"];
    assert_eq!(result["option_id"], "reject");
    assert_eq!(result["effect"], "rework");
    assert_eq!(result["target_graph_node_id"], "work");
    assert_eq!(record["session_id"], ready["session"]["id"]);
    assert_eq!(
        record["session_revision"],
        original["session"]["revision_after"]
    );
    assert_eq!(record["graph_node_id"], "review");
    assert_eq!(
        record["attempt_id"],
        ready["current"]["attempt"]["attempt_id"]
    );
    assert_eq!(record["option_id"], "reject");
    assert_eq!(record["effect"], "rework");
    assert_eq!(record["target_graph_node_id"], "work");
    assert_eq!(
        record["reason"],
        "The reviewed evidence requires another work attempt."
    );
    assert_eq!(record["actor"], "V2DRW transition reviewer");
    assert_eq!(record["references"][0]["state"], "resolved");
    assert_eq!(record["references"][0]["source_graph_node_id"], "work");
    assert!(
        record["references"][0]["items_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    let after = status(&engine);
    assert_eq!(after["current"]["node"]["graph_node_id"], "work");
    assert_eq!(after["current"]["attempt"]["attempt_number"], 2);
    assert_eq!(
        after["current"]["attempt"]["attempt_id"],
        result["target_attempt_id"]
    );
    assert_eq!(after["decision_history"]["entries"], json!([record]));
    let reworked = &after["rework_history"]["entries"];
    assert_eq!(reworked.as_array().unwrap().len(), 1);
    assert_eq!(reworked[0]["kind"], "declared");
    assert_eq!(reworked[0]["from_graph_node_id"], "review");
    assert_eq!(reworked[0]["to_graph_node_id"], "work");
    assert_eq!(
        reworked[0]["target_attempt_id"],
        result["target_attempt_id"]
    );
    assert!(
        after["stale_attempt_history"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|attempt| attempt["attempt_id"] == ready["current"]["attempt"]["attempt_id"])
    );
}

#[test]
fn v2drw003_running_rework_enforces_targets_and_fences_then_records_manual_transition() {
    let mut engine = left_branch();
    let before = status(&engine);
    let revision = engine.graph().trace().revision().get();
    let sequence = engine.sequence();
    let mut missing = rework(&engine, "choose");
    let DirectCommandV1::Procedure(P::SessionRework(input)) = &mut missing else {
        unreachable!()
    };
    input.preconditions.expected_attempt_id = None;
    reject(&mut engine, missing, "missing-attempt", "REQUEST_INVALID");
    let mut stale = rework(&engine, "choose");
    let DirectCommandV1::Procedure(P::SessionRework(input)) = &mut stale else {
        unreachable!()
    };
    input.preconditions.expected_attempt_id =
        Some(AttemptId::new("00000000-0000-4000-8000-000000093012").unwrap());
    reject(&mut engine, stale, "stale-attempt", "ATTEMPT_NOT_CURRENT");
    let mut stale = rework(&engine, "choose");
    let DirectCommandV1::Procedure(P::SessionRework(input)) = &mut stale else {
        unreachable!()
    };
    input.preconditions.expected_session_revision = Revision::new(revision + 1);
    reject(
        &mut engine,
        stale,
        "stale-revision",
        "SESSION_REVISION_CONFLICT",
    );
    let command = rework(&engine, "finish");
    assert_eq!(
        reject(
            &mut engine,
            command,
            "not-allowed",
            "MANUAL_REWORK_TARGET_NOT_ALLOWED"
        )["details"]["target_graph_node_id"],
        "finish"
    );
    let command = rework(&engine, "right-work");
    assert_eq!(
        reject(
            &mut engine,
            command,
            "not-on-trace",
            "MANUAL_REWORK_TARGET_NOT_ON_TRACE"
        )["details"]["target_graph_node_id"],
        "right-work"
    );
    assert_eq!(status(&engine), before);
    assert_eq!(engine.sequence(), sequence);
    let request = engine.request(rework(&engine, "choose"), "rework");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    let result = &original["result"];
    assert_eq!(
        result
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "from_graph_node_id",
            "reactivated",
            "reason",
            "revision",
            "schema",
            "target_attempt_id",
            "to_graph_node_id"
        ]
    );
    assert_eq!(result["schema"], "podway.rework-result/v2");
    assert_eq!(result["from_graph_node_id"], "left-work");
    assert_eq!(result["to_graph_node_id"], "choose");
    assert_eq!(
        result["reason"],
        "Return to the branch decision after a new finding."
    );
    assert_eq!(result["reactivated"], false);
    assert_eq!(engine.graph().trace().revision().get(), revision + 1);
    assert_eq!(engine.sequence(), sequence + 1);
    assert_eq!(result["revision"], revision + 1);
    let after = status(&engine);
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
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    assert_eq!(status(&engine), after);
}

#[test]
fn v2drw003_completed_reactivation_cold_replays_and_rejects_attempt_or_payload_drift() {
    let mut engine = left_branch();
    assert_eq!(
        engine.complete("left-complete")["result"]["to_graph_node_id"],
        "finish"
    );
    let finish = engine.fences().expected_attempt_id;
    engine.complete("finish");
    let revision = engine.graph().trace().revision().get();
    let mut supplied = rework(&engine, "choose");
    let DirectCommandV1::Procedure(P::SessionRework(input)) = &mut supplied else {
        unreachable!()
    };
    input.preconditions.expected_attempt_id = Some(finish);
    reject(&mut engine, supplied, "supplied-attempt", "REQUEST_INVALID");
    let mut command = rework(&engine, "choose");
    let DirectCommandV1::Procedure(P::SessionRework(input)) = &mut command else {
        unreachable!()
    };
    input.actor = Some("completion reviewer".into());
    let request = engine.request(command, "reactivate");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    let result = &original["result"];
    assert_eq!(result["schema"], "podway.rework-result/v2");
    assert_eq!(result["from_graph_node_id"], "finish");
    assert_eq!(result["to_graph_node_id"], "choose");
    assert_eq!(result["reactivated"], true);
    assert_eq!(engine.graph().trace().revision().get(), revision + 1);
    engine.cold();
    assert_eq!(
        engine.output(engine.execute(request.clone(), &NoClock)),
        original
    );
    let mut drift = request;
    let DirectCommandV1::Procedure(P::SessionRework(input)) = &mut drift.command else {
        unreachable!()
    };
    input.target_graph_node_id = GraphNodeId::new("left-work").unwrap();
    let before = engine.graph();
    let sequence = engine.sequence();
    let DirectResponseV1::Failure(error) = engine.execute(drift, &NoClock) else {
        panic!("successful key accepted changed payload")
    };
    assert_eq!(error.into_value()["code"], "IDEMPOTENCY_KEY_REUSED");
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    let after = status(&engine);
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
    let mut engine = left_branch();
    engine.success(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
            reason: "Cancel before rework.".into(),
            preconditions: engine.fences(),
        })),
        "cancel",
    );
    assert_eq!(
        engine.graph().trace().lifecycle(),
        SessionLifecycle::Cancelled
    );
    let command = rework(&engine, "choose");
    reject(
        &mut engine,
        command,
        "cancelled-rework",
        "SESSION_CANCELLED",
    );
}

#[test]
fn v2drw006_failure_receipts_are_atomic_and_cold_replayable() {
    use podway_protocol::{
        ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, ProcedureV2MutationRequestV1,
        RequestEnvelopeInputV1, RequestEnvelopeV1, RequestOptionsV1, WorkspaceContextV1,
        WorktreeSelectorWireV1,
    };
    use std::os::unix::ffi::OsStrExt;
    let mut engine = at_review(FAILURE_PROCEDURE, true);
    let before = status(&engine);
    assert_eq!(before["current"]["readiness"]["can_advance"], true);
    let selector = WorktreeSelectorWireV1::new(
        engine.temporary.path().as_os_str().as_bytes(),
        engine.temporary.path().display().to_string(),
        None,
    )
    .unwrap();
    for (n, command, payload) in [
        (1, "session.decide", json!({"option_id":"accept"})),
        (
            2,
            "session.decide",
            json!({"option_id":"accept","reason":" \t\n"}),
        ),
        (
            3,
            "session.rework",
            json!({"target_graph_node_id":"review"}),
        ),
        (
            4,
            "session.rework",
            json!({"target_graph_node_id":"review","reason":" \t\n"}),
        ),
    ] {
        let mut payload = payload.as_object().unwrap().clone();
        payload.insert("selector".into(), serde_json::to_value(&selector).unwrap());
        let fences = engine.fences();
        let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
            request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
            client: ClientInfoV1::new("direct-decision-port", "1", 1).unwrap(),
            operation: OperationV1::Mutate,
            command: CommandNameV1::new(command).unwrap(),
            workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
            idempotency_key: Some(
                podway_protocol::IdempotencyKeyV1::new(format!("malformed-{n}")).unwrap(),
            ),
            preconditions: PreconditionsV1::new(
                Some(fences.expected_session_id),
                Some(fences.expected_session_revision),
                Some(fences.expected_attempt_id),
                None,
                None,
                None,
            )
            .unwrap(),
            options: RequestOptionsV1::new(false, 5000).unwrap(),
            payload,
        })
        .unwrap();
        assert!(ProcedureV2MutationRequestV1::from_envelope(&envelope).is_err());
    }
    assert_eq!(status(&engine), before);
    for reason in ["", " \t\n"] {
        let mut command = decide(&engine, "accept");
        let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut command else {
            unreachable!()
        };
        input.reason = reason.into();
        reject(
            &mut engine,
            command,
            &format!("invalid-decide-reason-{}", reason.len()),
            "REQUEST_INVALID",
        );
        let mut command = rework(&engine, "review");
        let DirectCommandV1::Procedure(P::SessionRework(input)) = &mut command else {
            unreachable!()
        };
        input.reason = reason.into();
        reject(
            &mut engine,
            command,
            &format!("invalid-rework-reason-{}", reason.len()),
            "REQUEST_INVALID",
        );
    }
    let command = decide(&engine, "unknown");
    reject(&mut engine, command, "invalid-option", "OPTION_NOT_ALLOWED");
    // A rejected key retains no outcome and may be used for a corrected request.
    let accepted = engine.success(decide(&engine, "accept"), "invalid-option");
    assert_eq!(accepted["result"]["option_id"], "accept");
    assert!(
        engine
            .store
            .read(deadline(), |snapshot| snapshot
                .outcome(&IdempotencyKeyV1::new("invalid-option").unwrap()))
            .unwrap()
            .is_some()
    );
    let mut cancelled = at_review(FAILURE_PROCEDURE, true);
    cancelled.success(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
            reason: "Cancel before manual rework failure.".into(),
            preconditions: cancelled.fences(),
        })),
        "cancel",
    );
    let command = rework(&cancelled, "review");
    reject(&mut cancelled, command, "cancelled", "SESSION_CANCELLED");
    let after = status(&cancelled);
    assert_eq!(after["session"]["lifecycle"], "cancelled");
    assert!(after["current"].is_null());
    assert_eq!(after["decision_history"]["entries"], json!([]));
    assert_eq!(after["rework_history"]["entries"], json!([]));
}

#[test]
fn v2drw006_concurrent_decisions_keep_one_cursor_and_stale_history_non_satisfying() {
    let mut engine = at_review(FAILURE_PROCEDURE, true);
    let before = status(&engine);
    let attempt = engine.fences().expected_attempt_id;
    let left = engine.request(decide(&engine, "accept"), "left");
    let right = engine.request(decide(&engine, "accept"), "right");
    let barrier = Arc::new(Barrier::new(3));
    let responses = thread::scope(|scope| {
        let borrowed = &engine;
        let left_barrier = Arc::clone(&barrier);
        let right_barrier = Arc::clone(&barrier);
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
    for response in &responses {
        let value = match response {
            DirectResponseV1::Success(output) => output.as_value().clone(),
            DirectResponseV1::Failure(error) => error.clone().into_value(),
        };
        assert!(serde_json::to_vec(&value).unwrap().len() <= 1_048_576);
    }
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
    let after = status(&engine);
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
        json!(attempt)
    );
    let decision = after["decision_history"]["entries"][0].clone();
    assert_eq!(after["rework_history"]["entries"], json!([]));
    assert_eq!(
        engine.success(rework(&engine, "review"), "rework")["result"]["to_graph_node_id"],
        "review"
    );
    let fresh = status(&engine);
    assert_eq!(fresh["current"]["node"]["graph_node_id"], "review");
    assert_ne!(engine.fences().expected_attempt_id, attempt);
    assert_eq!(fresh["current"]["readiness"]["items_satisfied"], false);
    assert_eq!(fresh["current"]["readiness"]["can_advance"], false);
    assert_eq!(fresh["decision_history"]["entries"][0], decision);
    let newest = &fresh["stale_attempt_history"]["entries"][0];
    let older = read(
        &engine,
        GraphStatusTierV2::Verbose,
        Some(newest["trace_sequence"].as_u64().unwrap()),
    );
    assert_eq!(
        older["stale_attempt_history"]["entries"][0]["attempt_id"],
        json!(attempt)
    );
    assert_eq!(
        fresh["rework_history"]["entries"].as_array().unwrap().len(),
        1
    );
    let mut delayed = decide(&engine, "accept");
    let DirectCommandV1::Procedure(P::SessionDecide(input)) = &mut delayed else {
        unreachable!()
    };
    input.preconditions.expected_attempt_id = attempt;
    input.preconditions.expected_session_revision =
        Revision::new(before["session"]["revision"].as_u64().unwrap());
    reject(&mut engine, delayed, "delayed", "SESSION_REVISION_CONFLICT");
    engine.cold();
    assert_eq!(status(&engine), fresh);
}

#[test]
fn v2drw006_repeated_declared_and_manual_cycles_survive_midrun_cold_reopen() {
    let mut engine = at_review(FAILURE_PROCEDURE, true);
    let initial = status(&engine);
    assert_eq!(initial["trace_length"], 2);
    assert_eq!(counter(&initial, "work")["attempt_count"], 1);
    assert_eq!(counter(&initial, "review")["attempt_count"], 1);
    let mut previous = engine.graph().trace().revision().get();
    let mut trace = 2;
    for n in 0..6 {
        cycle(&engine, n);
        let after = status(&engine);
        assert!(engine.graph().trace().revision().get() > previous);
        assert_eq!(after["trace_length"], trace + 2);
        assert_eq!(counter(&after, "work")["attempt_count"], n + 2);
        assert_eq!(counter(&after, "work")["rework_traversal_count"], n + 1);
        assert_eq!(counter(&after, "review")["attempt_count"], n + 2);
        assert_eq!(counter(&after, "review")["rework_traversal_count"], 0);
        assert_eq!(after["current"]["node"]["graph_node_id"], "review");
        previous = engine.graph().trace().revision().get();
        trace += 2;
        if n == 2 {
            engine.cold();
            assert_eq!(status(&engine), after);
        }
    }
    let accepted = engine.success(decide(&engine, "accept"), "accept");
    assert_eq!(accepted["result"]["target_graph_node_id"], "finish");
    let at_finish = status(&engine);
    assert!(engine.graph().trace().revision().get() > previous);
    assert_eq!(at_finish["trace_length"], trace + 1);
    let reworked = engine.success(rework(&engine, "review"), "manual");
    assert_eq!(reworked["result"]["to_graph_node_id"], "review");
    assert_eq!(reworked["result"]["reactivated"], false);
    let final_status = status(&engine);
    assert!(
        engine.graph().trace().revision().get()
            > at_finish["session"]["revision"].as_u64().unwrap()
    );
    assert_eq!(final_status["trace_length"], 16);
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
    let newest = &final_status["stale_attempt_history"]["entries"][0];
    let older = read(
        &engine,
        GraphStatusTierV2::Verbose,
        Some(newest["trace_sequence"].as_u64().unwrap()),
    );
    assert_eq!(
        older["stale_attempt_history"]["entries"][0]["attempt_id"],
        accepted["result"]["attempt_id"]
    );
}

#[test]
fn v2drw006_generated_cycle_counts_have_no_runtime_traversal_limit_and_stay_bounded() {
    let engine = at_review(FAILURE_PROCEDURE, true);
    for cycles in 0..=12 {
        let compact = read(&engine, GraphStatusTierV2::Compact, None);
        let standard = read(&engine, GraphStatusTierV2::Standard, None);
        let verbose = status(&engine);
        let next = engine.success(
            DirectCommandV1::Read {
                command: DirectReadCommandV1::Next,
                expected_session_id: Some(engine.graph().trace().session_id().clone()),
            },
            "next",
        );
        assert!(serde_json::to_vec(&next).unwrap().len() <= 1_048_576);
        for projection in [&compact, &standard, &verbose, &next["result"]] {
            assert_eq!(projection["trace_length"], 2 + cycles * 2);
            assert_eq!(counter(projection, "work")["attempt_count"], cycles + 1);
            assert_eq!(
                counter(projection, "work")["rework_traversal_count"],
                cycles
            );
            assert_eq!(counter(projection, "review")["attempt_count"], cycles + 1);
            assert_eq!(counter(projection, "review")["rework_traversal_count"], 0);
            assert_eq!(counter(projection, "finish")["attempt_count"], 0);
        }
        assert_eq!(compact["schema"], "podway.compact-status-result/v5");
        assert_eq!(standard["tier"], "standard");
        assert_eq!(verbose["tier"], "verbose");
        assert_eq!(next["result"]["schema"], "podway.next-result/v5");
        history_window(&verbose["decision_history"], cycles, 1);
        history_window(&verbose["rework_history"], cycles, 6);
        if cycles < 12 {
            cycle(&engine, cycles);
        }
    }
}

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
