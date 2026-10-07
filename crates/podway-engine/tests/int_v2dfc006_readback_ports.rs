//! Retained immutable and paged readback through the typed direct API.

use podway_core::{AttemptId, GraphNodeId, ItemId, OptionId, Revision, SessionId, TraceSequenceV2};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
    direct_items::DirectItemCommandV1,
    v2_read_service::GraphStatusTierV2,
};
use podway_protocol::{
    ErrorEnvelopeV2, EvidencePageTokenV1, GoalAssessCriterionV2, GoalDefineV2, GoalReviseV2,
    GoalRevisionPreconditionsWireV2, ItemMutationPreconditionsWireV1, ItemSetV1, OutputEnvelopeV4,
    ProcedureV2MutationCommandV1 as P, ReworkPreconditionsWireV2, SessionCompleteV1,
    SessionDecideV2, SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1,
    SessionReworkV2,
};
use podway_store::IdempotencyKeyV1;
use serde_json::{Map, Value, json};

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline},
    int_v2dfc006_fixture_ports::READBACK_PROCEDURE,
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};

const HISTORY_KEYS: [&str; 6] = [
    "current_trace_history",
    "stale_attempt_history",
    "decision_history",
    "rework_history",
    "stale_goal_revision_history",
    "stale_goal_assessment_history",
];

struct Fences {
    session: SessionId,
    revision: Revision,
    attempt: Option<AttemptId>,
    goal: Option<u64>,
}

fn session_identity_preconditions(status: &Map<String, Value>) -> Fences {
    Fences {
        session: SessionId::new(status["session"]["id"].as_str().unwrap()).unwrap(),
        revision: Revision::new(status["session"]["revision"].as_u64().unwrap()),
        attempt: None,
        goal: None,
    }
}

fn session_preconditions(status: &Map<String, Value>) -> Fences {
    let mut fences = session_identity_preconditions(status);
    fences.attempt = status["current"]["attempt"]["attempt_id"]
        .as_str()
        .map(|id| AttemptId::new(id).unwrap());
    fences
}

fn goal_preconditions(status: &Map<String, Value>, revision: u64) -> Fences {
    let mut fences = session_preconditions(status);
    fences.goal = Some(revision);
    fences
}

fn mutation_request(
    number: u64,
    command: &str,
    engine: &Engine,
    payload: Map<String, Value>,
    key: &str,
    fences: Fences,
) -> DirectRequestV1 {
    let actor = payload
        .get("actor")
        .map(|value| value.as_str().unwrap().to_owned());
    let identity = SessionIdentityPreconditionsWireV1 {
        expected_session_id: fences.session.clone(),
        expected_session_revision: fences.revision,
    };
    let active = || SessionMutationPreconditionsWireV1 {
        expected_session_id: fences.session.clone(),
        expected_session_revision: fences.revision,
        expected_attempt_id: fences.attempt.clone().unwrap(),
    };
    let command = match command {
        "session.complete" => {
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: active(),
            }))
        }
        "session.decide" => DirectCommandV1::Procedure(P::SessionDecide(SessionDecideV2 {
            option_id: OptionId::new(payload["option_id"].as_str().unwrap()).unwrap(),
            reason: payload["reason"].as_str().unwrap().into(),
            actor,
            preconditions: active(),
            expected_goal_revision: fences.goal,
        })),
        "session.rework" => DirectCommandV1::Procedure(P::SessionRework(SessionReworkV2 {
            target_graph_node_id: GraphNodeId::new(
                payload["target_graph_node_id"].as_str().unwrap(),
            )
            .unwrap(),
            reason: payload["reason"].as_str().unwrap().into(),
            actor,
            preconditions: ReworkPreconditionsWireV2 {
                expected_session_id: fences.session,
                expected_session_revision: fences.revision,
                expected_attempt_id: fences.attempt,
            },
        })),
        "goal.define" => DirectCommandV1::Procedure(P::GoalDefine(GoalDefineV2 {
            goal: payload["goal"].as_str().unwrap().into(),
            criteria: serde_json::from_value(payload["criteria"].clone()).unwrap(),
            actor,
            preconditions: identity,
        })),
        "goal.revise" => DirectCommandV1::Procedure(P::GoalRevise(GoalReviseV2 {
            goal: payload["goal"].as_str().unwrap().into(),
            criteria: serde_json::from_value(payload["criteria"].clone()).unwrap(),
            target_graph_node_id: GraphNodeId::new(
                payload["target_graph_node_id"].as_str().unwrap(),
            )
            .unwrap(),
            reason: payload["reason"].as_str().unwrap().into(),
            actor,
            reactivate: false,
            preconditions: GoalRevisionPreconditionsWireV2 {
                expected_session_id: fences.session,
                expected_session_revision: fences.revision,
                expected_attempt_id: fences.attempt,
                expected_goal_revision: fences.goal.unwrap(),
            },
        })),
        "goal.assess_criterion" => {
            DirectCommandV1::Procedure(P::GoalAssessCriterion(GoalAssessCriterionV2 {
                criterion_id: podway_core::CriterionId::new(
                    payload["criterion_id"].as_str().unwrap(),
                )
                .unwrap(),
                status: payload["status"].as_str().unwrap().into(),
                reason: payload["reason"].as_str().unwrap().into(),
                evidence: payload["evidence"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| GraphNodeId::new(value.as_str().unwrap()).unwrap())
                    .collect(),
                items: payload["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| ItemId::new(value.as_str().unwrap()).unwrap())
                    .collect(),
                actor,
                preconditions: active(),
                expected_goal_revision: fences.goal.unwrap(),
            }))
        }
        other => panic!("unexpected retained mutation {other}"),
    };
    let mut request = engine.request(command, key);
    request.request_id =
        podway_protocol::RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}"))
            .unwrap();
    request
}

fn query_request(
    number: u64,
    command: &str,
    engine: &Engine,
    session: &str,
    payload: Map<String, Value>,
) -> DirectRequestV1 {
    let command = match command {
        "session.status" => DirectReadCommandV1::Status {
            tier: if payload.get("verbose") == Some(&json!(true)) {
                GraphStatusTierV2::Verbose
            } else if payload.get("compact") == Some(&json!(true)) {
                GraphStatusTierV2::Compact
            } else {
                GraphStatusTierV2::Standard
            },
            history_before: payload
                .get("history_before")
                .map(|value| TraceSequenceV2::new(value.as_u64().unwrap())),
        },
        "session.next" => DirectReadCommandV1::Next,
        "evidence.read" => DirectReadCommandV1::Evidence {
            source: GraphNodeId::new(payload["source"].as_str().unwrap()).unwrap(),
            item_id: ItemId::new(payload["item_id"].as_str().unwrap()).unwrap(),
            token: payload
                .get("page_token")
                .map(|value| EvidencePageTokenV1::decode(value.as_str().unwrap()).unwrap()),
        },
        other => panic!("unexpected retained query {other}"),
    };
    let mut request = engine.request(
        DirectCommandV1::Read {
            command,
            expected_session_id: Some(SessionId::new(session).unwrap()),
        },
        "readback-query",
    );
    request.request_id =
        podway_protocol::RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}"))
            .unwrap();
    request
}

fn dispatch(engine: &Engine, request: &DirectRequestV1) -> DirectResponseV1 {
    let before = engine.graph();
    let sequence = engine.sequence();
    let response = engine.execute(request.clone(), &Clock);
    match &response {
        DirectResponseV1::Success(output) => {
            let bytes = serde_json::to_vec(output).unwrap();
            assert!(bytes.len() <= 1_048_576);
            assert_eq!(
                serde_json::from_slice::<OutputEnvelopeV4>(&bytes).unwrap(),
                *output
            );
            if matches!(request.command, DirectCommandV1::Read { .. }) {
                assert_eq!(engine.graph(), before);
                assert_eq!(engine.sequence(), sequence);
                assert!(
                    engine
                        .store
                        .read(deadline(), |snapshot| snapshot
                            .outcome(&IdempotencyKeyV1::new("readback-query").unwrap()))
                        .unwrap()
                        .is_none()
                );
            } else {
                assert_eq!(
                    engine.output(engine.execute(request.clone(), &NoClock)),
                    output.as_value().clone()
                );
            }
        }
        DirectResponseV1::Failure(error) => {
            let bytes = serde_json::to_vec(error).unwrap();
            assert!(bytes.len() <= 1_048_576);
            assert_eq!(
                serde_json::from_slice::<ErrorEnvelopeV2>(&bytes).unwrap(),
                *error
            );
            assert_eq!(engine.graph(), before);
            assert_eq!(engine.sequence(), sequence);
            assert!(
                engine
                    .store
                    .read(deadline(), |snapshot| snapshot
                        .outcome(request.idempotency_key.as_ref().unwrap()))
                    .unwrap()
                    .is_none()
            );
        }
    }
    response
}

fn result(response: DirectResponseV1, command: &str) -> Map<String, Value> {
    match response {
        DirectResponseV1::Success(output) => {
            assert_eq!(output.as_value()["command"], command);
            output.as_value()["result"].as_object().unwrap().clone()
        }
        DirectResponseV1::Failure(error) => {
            panic!("unexpected {command} rejection: {:?}", error.into_value())
        }
    }
}

fn next_number(number: &mut u64) -> u64 {
    let current = *number;
    *number += 1;
    current
}

fn query(
    engine: &Engine,
    session: &str,
    number: &mut u64,
    command: &str,
    payload: Map<String, Value>,
) -> Map<String, Value> {
    result(
        dispatch(
            engine,
            &query_request(next_number(number), command, engine, session, payload),
        ),
        command,
    )
}

fn status(engine: &Engine, session: &str, number: &mut u64) -> Map<String, Value> {
    query(engine, session, number, "session.status", Map::new())
}

fn set_item(engine: &Engine, session: &str, number: &mut u64, item: &str, value: &str) {
    let graph = engine.graph();
    let attempt = graph.trace().active_attempt().unwrap().attempt_id();
    let item_id = ItemId::new(item).unwrap();
    let revision = graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt)
        .unwrap()
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id)
        .unwrap()
        .revision();
    let number = next_number(number);
    let request = engine.request(
        DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
            item_id,
            value: value.into(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: SessionId::new(session).unwrap(),
                expected_attempt_id: attempt.clone(),
                expected_item_revision: revision,
            },
        })),
        &format!("readback-set-{item}-{number}"),
    );
    result(dispatch(engine, &request), "item.set");
}

fn complete(engine: &Engine, session: &str, number: &mut u64) -> Map<String, Value> {
    let before = status(engine, session, number);
    let number = next_number(number);
    let request = mutation_request(
        number,
        "session.complete",
        engine,
        Map::new(),
        &format!("readback-complete-{number}"),
        session_preconditions(&before),
    );
    result(dispatch(engine, &request), "session.complete")
}

fn assert_malformed_token_rejected_at_protocol_boundary() {
    use podway_protocol::{
        ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
        RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, WorkspaceContextV1,
        WorktreeSelectorWireV1,
    };
    let selector =
        WorktreeSelectorWireV1::new(b"/pure-token-fixture", "/pure-token-fixture", None).unwrap();
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000091").unwrap(),
        client: ClientInfoV1::new("v2scl004-test", "1", 1).unwrap(),
        operation: OperationV1::Query,
        command: CommandNameV1::new("evidence.read").unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: None,
        preconditions: PreconditionsV1::new(Some(SessionId::new("00000000-0000-4000-8000-000000000092").unwrap()),
            None, None, None, None, None).unwrap(),
        options: RequestOptionsV1::new(false, 5000).unwrap(),
        payload: json!({"selector":selector,"source":"work","item_id":"result","page_token":"not-a-token"})
            .as_object().unwrap().clone(),
    }).unwrap();
    assert!(podway_protocol::SliceRequestV1::from_envelope(&envelope).is_err());
}

mod decisions {
    use super::*;
    fn verbose_status(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        history_before: Option<u64>,
    ) -> Map<String, Value> {
        let mut payload = json!({"verbose": true}).as_object().unwrap().clone();
        if let Some(cursor) = history_before {
            payload.insert("history_before".to_owned(), json!(cursor));
        }
        let value = query(engine, session_id, number, "session.status", payload);
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
    fn decide(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        option_id: &str,
        reason: &str,
    ) -> Map<String, Value> {
        let before = status(engine, session_id, number);
        let request_number = next_number(number);
        let request = mutation_request(
            request_number,
            "session.decide",
            engine,
            json!({
                "option_id": option_id,
                "reason": reason,
                "actor": "V2DRW-005 reviewer"
            })
            .as_object()
            .unwrap()
            .clone(),
            &format!("v2drw005-decide-{request_number}"),
            session_preconditions(&before),
        );
        result(dispatch(engine, &request), "session.decide")
    }
    fn rework(engine: &Engine, session_id: &str, number: &mut u64) -> Map<String, Value> {
        let before = status(engine, session_id, number);
        let request_number = next_number(number);
        let request = mutation_request(
            request_number,
            "session.rework",
            engine,
            json!({
                "target_graph_node_id": "review",
                "reason": "Re-open the accepted decision after a new finding.",
                "actor": "V2DRW-005 reviewer"
            })
            .as_object()
            .unwrap()
            .clone(),
            &format!("v2drw005-rework-{request_number}"),
            session_preconditions(&before),
        );
        result(dispatch(engine, &request), "session.rework")
    }
    fn history_entries<'a>(status: &'a Map<String, Value>, history: &str) -> &'a [Value] {
        status[history]["entries"].as_array().unwrap()
    }
    fn assert_newest_first(entries: &[Value]) {
        assert!(entries.windows(2).all(|pair| {
            pair[0]["trace_sequence"].as_u64().unwrap()
                > pair[1]["trace_sequence"].as_u64().unwrap()
        }));
    }
    fn read_fingerprint(status: &Map<String, Value>) -> Value {
        json!({
            "revision": status["session"]["revision"],
            "lifecycle": status["session"]["lifecycle"],
            "current": status["current"],
            "trace_length": status["trace_length"],
            "counters": status["counters"],

        })
    }

    #[test]
    fn v2drw005_production_readback_is_bounded_immutable_pageable_and_cold_stable() {
        let mut engine = Engine::new(READBACK_PROCEDURE);
        let session_id = engine.graph().trace().session_id().as_str().to_owned();
        let mut number = 95_500;

        let mut expected_decisions = Vec::new();
        let mut expected_reworks = Vec::new();
        for cycle in 1..=7 {
            set_item(
                &engine,
                &session_id,
                &mut number,
                "result",
                &format!("work result {cycle}"),
            );
            let completed = complete(&engine, &session_id, &mut number);
            assert_eq!(completed["to_graph_node_id"], "review");
            set_item(
                &engine,
                &session_id,
                &mut number,
                "note",
                &format!("reject note {cycle}"),
            );
            let rejected = decide(
                &engine,
                &session_id,
                &mut number,
                "reject",
                &format!("Reject cycle {cycle} for another recorded attempt."),
            );
            expected_decisions.push(rejected["record"].clone());
            let captured = verbose_status(&engine, &session_id, &mut number, None);
            expected_reworks.push(captured["rework_history"]["entries"][0].clone());
        }

        set_item(
            &engine,
            &session_id,
            &mut number,
            "result",
            "final accepted work result",
        );
        let final_work = status(&engine, &session_id, &mut number);
        let final_work_attempt_id = final_work["current"]["attempt"]["attempt_id"]
            .as_str()
            .unwrap()
            .to_owned();
        complete(&engine, &session_id, &mut number);
        set_item(&engine, &session_id, &mut number, "note", "accept note");
        let accepted = decide(
            &engine,
            &session_id,
            &mut number,
            "accept",
            "Accept the final recorded attempt.",
        );
        expected_decisions.push(accepted["record"].clone());
        let accepted_attempt_id = accepted["attempt_id"].as_str().unwrap().to_owned();
        assert_eq!(accepted["target_graph_node_id"], "finish");

        let manual = rework(&engine, &session_id, &mut number);
        assert_eq!(manual["from_graph_node_id"], "finish");
        assert_eq!(manual["to_graph_node_id"], "review");
        let fresh_attempt_id = manual["target_attempt_id"].as_str().unwrap().to_owned();
        let fresh = verbose_status(&engine, &session_id, &mut number, None);
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

        let standard = status(&engine, &session_id, &mut number);
        for history in HISTORY_KEYS {
            assert!(
                !standard.contains_key(history),
                "standard status leaked verbose-only {history}"
            );
        }
        let next = query(
            &engine,
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
            let page = verbose_status(&engine, &session_id, &mut number, cursor);
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
            let page = verbose_status(&engine, &session_id, &mut number, cursor);
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
            let page = verbose_status(&engine, &session_id, &mut number, cursor);
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

        let after_reads = status(&engine, &session_id, &mut number);
        assert_eq!(read_fingerprint(&after_reads), before_reads);

        set_item(
            &engine,
            &session_id,
            &mut number,
            "note",
            "fresh decision note",
        );
        let ready_next = query(
            &engine,
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

        let durable_verbose = verbose_status(&engine, &session_id, &mut number, None);
        let durable_next = query(
            &engine,
            &session_id,
            &mut number,
            "session.next",
            Map::new(),
        );
        engine.cold();
        let reopened_verbose = query(
            &engine,
            &session_id,
            &mut number,
            "session.status",
            json!({"verbose": true}).as_object().unwrap().clone(),
        );
        assert_eq!(reopened_verbose, durable_verbose);
        let reopened_next = query(
            &engine,
            &session_id,
            &mut number,
            "session.next",
            Map::new(),
        );
        assert_eq!(reopened_next, durable_next);
    }

    #[test]
    fn v2scl004_production_dispatch_serves_and_fences_evidence_read() {
        let mut engine = Engine::new(READBACK_PROCEDURE);
        let session_id = engine.graph().trace().session_id().as_str().to_owned();
        let mut number = 95_500;
        set_item(
            &engine,
            &session_id,
            &mut number,
            "result",
            "recorded evidence for the paged read",
        );
        let completed = complete(&engine, &session_id, &mut number);
        assert_eq!(completed["to_graph_node_id"], "review");

        // The route answers through typed direct execution, not only through the projection helper.
        let page = query(
            &engine,
            &session_id,
            &mut number,
            "evidence.read",
            json!({"source": "work", "item_id": "result"})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(page["schema"], "podway.evidence-read-result/v1");
        assert_eq!(page["procedure_schema"], "podway.procedure/v2");
        assert_eq!(page["item"]["item_id"], "result");
        assert_eq!(page["item"]["type"], "text");
        assert_eq!(page["source"]["graph_node_id"], "work");
        assert_eq!(page["size_unit"], "scalars");
        assert_eq!(page["total_size"], 36);
        assert_eq!(page["truncated"], false);
        assert_eq!(page["next_page_token"], Value::Null);
        assert_eq!(page["page"]["data"], "recorded evidence for the paged read");

        // A query creates no revision and no success outcome, so the session is exactly where it was.
        let after = status(&engine, &session_id, &mut number);
        assert_eq!(after["current"]["node"]["graph_node_id"], "review");
        let repeated = query(
            &engine,
            &session_id,
            &mut number,
            "evidence.read",
            json!({"source": "work", "item_id": "result"})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(repeated, page);

        // An item outside the reference selector is not readable through the route either.
        let payload = json!({"source":"work","item_id":"note"})
            .as_object()
            .unwrap()
            .clone();
        let request = query_request(
            next_number(&mut number),
            "evidence.read",
            &engine,
            &session_id,
            payload,
        );
        let DirectResponseV1::Failure(error) = dispatch(&engine, &request) else {
            panic!("expected evidence refusal");
        };
        let error = error.into_value();
        assert_eq!(error["code"], "EVIDENCE_NOT_AVAILABLE");
        assert_eq!(error["exit_code"], 1);
        assert_eq!(error["retryable"], false);

        // A value wider than one page must page over the public direct API, not only through the projection
        // helper: the continuation the caller receives has to survive encoding, decoding, and the
        // request boundary that re-validates it.
        let wide = "한".repeat(50_000);
        set_item(&engine, &session_id, &mut number, "note", "decision note");
        let reworked = decide(
            &engine,
            &session_id,
            &mut number,
            "reject",
            "Reject so the work node can record a value wider than one page.",
        );
        assert_eq!(reworked["record"]["option_id"], "reject");
        let back_at_work = status(&engine, &session_id, &mut number);
        assert_eq!(back_at_work["current"]["node"]["graph_node_id"], "work");
        set_item(&engine, &session_id, &mut number, "result", &wide);
        let completed = complete(&engine, &session_id, &mut number);
        assert_eq!(completed["to_graph_node_id"], "review");

        let first_wide = query(
            &engine,
            &session_id,
            &mut number,
            "evidence.read",
            json!({"source": "work", "item_id": "result"})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(first_wide["total_size"], 50_000);
        assert_eq!(first_wide["truncated"], true);
        let first_chunk = first_wide["page"]["data"].as_str().unwrap().to_owned();
        assert_eq!(first_chunk.chars().count(), 43_690);
        let continuation = first_wide["next_page_token"].as_str().unwrap().to_owned();

        engine.cold();
        let second_wide = query(
            &engine,
            &session_id,
            &mut number,
            "evidence.read",
            json!({"source": "work", "item_id": "result", "page_token": continuation})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(second_wide["page"]["offset"], 43_690);
        assert_eq!(second_wide["truncated"], false);
        assert_eq!(second_wide["next_page_token"], Value::Null);
        let second_chunk = second_wide["page"]["data"].as_str().unwrap().to_owned();
        assert_eq!(format!("{first_chunk}{second_chunk}"), wide);

        // A token from an earlier consumer attempt is the recoverable case: the caller re-reads state
        // and restarts the item, so the public envelope must say retryable with the state-refresh exit
        // rather than reporting a malformed request.
        let source_attempt = page["source"]["attempt_id"].as_str().unwrap().to_owned();
        let stale = podway_protocol::EvidencePageTokenV1::new(
            &session_id,
            &source_attempt,
            &source_attempt,
            "result",
            page["value_digest"].as_str().unwrap(),
            0,
        )
        .unwrap()
        .encode();
        let payload = json!({"source":"work","item_id":"result","page_token":stale})
            .as_object()
            .unwrap()
            .clone();
        let request = query_request(
            next_number(&mut number),
            "evidence.read",
            &engine,
            &session_id,
            payload,
        );
        let DirectResponseV1::Failure(error) = dispatch(&engine, &request) else {
            panic!("expected evidence refusal");
        };
        let error = error.into_value();
        assert_eq!(error["code"], "EVIDENCE_PAGE_TOKEN_STALE");
        assert_eq!(error["retryable"], true);
        assert_eq!(error["exit_code"], 4);
        assert_eq!(
            error["details"]["schema"],
            "podway.recoverable-v2-runtime-error-details/v2"
        );
        assert_eq!(error["details"]["kind"], "EVIDENCE_PAGE_TOKEN_STALE");
        assert_eq!(error["details"]["item_id"], "result");
        assert_eq!(error["details"]["source_graph_node_id"], "work");
        // The recovery recipe must send the caller back to a read, never to another mutation.
        assert_eq!(error["details"]["recovery"]["command"], "session.observe");

        // A token naming another session is refused before any state comparison, so an invented token
        // cannot reveal whether a session exists. That ordering is a property of the dispatch path,
        // not only of the projection helper, so it is asserted on the public envelope.
        let foreign = podway_protocol::EvidencePageTokenV1::new(
            "00000000-0000-4000-8000-0000000000ff",
            &source_attempt,
            &source_attempt,
            "result",
            page["value_digest"].as_str().unwrap(),
            0,
        )
        .unwrap()
        .encode();
        let payload = json!({"source":"work","item_id":"result","page_token":foreign})
            .as_object()
            .unwrap()
            .clone();
        let request = query_request(
            next_number(&mut number),
            "evidence.read",
            &engine,
            &session_id,
            payload,
        );
        let DirectResponseV1::Failure(error) = dispatch(&engine, &request) else {
            panic!("expected evidence refusal");
        };
        let error = error.into_value();
        assert_eq!(error["code"], "REQUEST_INVALID");
        assert_eq!(error["retryable"], false);
        // Nothing about the named session, attempt, or item may leak into the refusal.
        assert_eq!(error["details"], json!({}));

        assert_malformed_token_rejected_at_protocol_boundary();
    }
}

mod goals {
    use super::*;
    use podway_config::{
        AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
        validate_procedure_v2, vet_procedure_v2,
    };
    const PROCEDURE: &str = r#"schema: podway.procedure/v2
id: v2gol004-readback
version: "2"
name: Goal history read-back
purpose: Exercise current and stale goal history through production dispatch.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Produce evidence
    intent: Record evidence for the current goal revision.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        max_length: 1000
  assess:
    type: decision
    title: Assess the goal
    objective: Select the outcome determined by the current criteria.
    prompt: Which goal outcome applies?
    items:
      - id: note-one
        type: text
        prompt: Record the first local citation target.
        required: false
        max_length: 100
      - id: note-two
        type: text
        prompt: Record the second local citation target.
        required: false
        max_length: 100
      - id: note-three
        type: text
        prompt: Record the third local citation target.
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
    intent: Read the assessment back before terminal completion.
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
      evidence_from:
        - node: decide
          required: true
      terminal: true
manual_rework:
  allowed_targets:
    - perform
"#;
    fn compact_status(engine: &Engine, session_id: &str, number: &mut u64) -> Map<String, Value> {
        query(
            engine,
            session_id,
            number,
            "session.status",
            json!({"compact": true}).as_object().unwrap().clone(),
        )
    }
    fn verbose_status(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        history_before: Option<u64>,
    ) -> Map<String, Value> {
        let mut payload = json!({"verbose": true}).as_object().unwrap().clone();
        if let Some(cursor) = history_before {
            payload.insert("history_before".to_owned(), json!(cursor));
        }
        let status = query(engine, session_id, number, "session.status", payload);
        for history in HISTORY_KEYS {
            let entries = status[history]["entries"].as_array().unwrap();
            assert!(serde_json::to_vec(&status[history]).unwrap().len() <= 65_536);
            assert!(entries.windows(2).all(|pair| {
                pair[0]["trace_sequence"].as_u64().unwrap()
                    > pair[1]["trace_sequence"].as_u64().unwrap()
            }));
            if let Some(cursor) = history_before {
                assert!(
                    entries
                        .iter()
                        .all(|entry| entry["trace_sequence"].as_u64().unwrap() < cursor)
                );
            }
        }
        status
    }
    fn complete_action(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        key: &str,
    ) -> Map<String, Value> {
        let before = status(engine, session_id, number);
        let request = mutation_request(
            next_number(number),
            "session.complete",
            engine,
            Map::new(),
            key,
            session_preconditions(&before),
        );
        result(dispatch(engine, &request), "session.complete")
    }
    fn assess(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        revision: u64,
        criterion_id: &str,
        criterion_status: &str,
    ) {
        let before = status(engine, session_id, number);
        let citations = if criterion_status == "not_applicable" {
            Vec::new()
        } else {
            vec![json!("perform")]
        };
        let request_number = next_number(number);
        let request = mutation_request(
            request_number,
            "goal.assess_criterion",
            engine,
            json!({
                "criterion_id": criterion_id,
                "status": criterion_status,
                "reason": format!("revision {revision} {criterion_id} is {criterion_status}"),
                "evidence": citations,
                "items": [],
                "actor": format!("revision {revision} assessor")
            })
            .as_object()
            .unwrap()
            .clone(),
            &format!("v2gol004-assess-{revision}-{criterion_id}"),
            goal_preconditions(&before, revision),
        );
        result(dispatch(engine, &request), "goal.assess_criterion");
    }
    fn decide(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        revision: u64,
        option_id: &str,
    ) -> Value {
        let before = status(engine, session_id, number);
        let request_number = next_number(number);
        let request = mutation_request(
            request_number,
            "session.decide",
            engine,
            json!({
                "option_id": option_id,
                "reason": format!("select the recorded outcome for revision {revision}"),
                "actor": format!("revision {revision} decider")
            })
            .as_object()
            .unwrap()
            .clone(),
            &format!("v2gol004-decide-{revision}"),
            goal_preconditions(&before, revision),
        );
        result(dispatch(engine, &request), "session.decide")["record"].clone()
    }
    fn revise(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        prior_revision: u64,
        revision: u64,
    ) {
        let before = status(engine, session_id, number);
        let request_number = next_number(number);
        let request = mutation_request(
        request_number,
        "goal.revise",
        engine,
        json!({
            "goal": format!("Deliver goal revision {revision}."),
            "criteria": [
                {"criterion_id": "correct", "statement": format!("Revision {revision} is correct.")},
                {"criterion_id": "tested", "statement": format!("Revision {revision} is tested.")}
            ],
            "target_graph_node_id": "perform",
            "reason": format!("replace revision {prior_revision} with revision {revision}"),
            "actor": format!("revision {revision} author")
        })
        .as_object()
        .unwrap()
        .clone(),
        &format!("v2gol004-revise-{revision}"),
        goal_preconditions(&before, prior_revision),
    );
        let result = result(dispatch(engine, &request), "goal.revise");
        assert_eq!(result["goal_revision"], revision);
        assert_eq!(result["rework_to"], "perform");
    }
    fn run_revision(
        engine: &Engine,
        session_id: &str,
        number: &mut u64,
        revision: u64,
        statuses: [&str; 2],
        option_id: &str,
    ) -> Value {
        set_result(engine, session_id, number, revision);
        complete_action(
            engine,
            session_id,
            number,
            &format!("v2gol004-complete-work-{revision}"),
        );
        assess(engine, session_id, number, revision, "correct", statuses[0]);
        assess(engine, session_id, number, revision, "tested", statuses[1]);
        decide(engine, session_id, number, revision, option_id)
    }
    fn read_fingerprint(status: &Map<String, Value>) -> Value {
        json!({
            "session": status["session"],
            "current": status["current"],
            "goal_revision": status["goal_revision"],
            "latest_goal_outcome": status["latest_goal_outcome"],
            "trace_length": status["trace_length"],
            "counters": status["counters"],

        })
    }
    fn assert_no_histories(status: &Map<String, Value>) {
        for history in HISTORY_KEYS {
            assert!(!status.contains_key(history), "status leaked {history}");
        }
    }
    fn ordinary_decision_record(enriched: &Value) -> Value {
        let mut record = enriched.as_object().unwrap().clone();
        for field in [
            "assessment",
            "assessment_mode",
            "goal_outcome",
            "criterion_results",
        ] {
            record.remove(field);
        }
        Value::Object(record)
    }
    fn assert_assessment_summary(summary: &Value, decision: &Value, outcome: &str) {
        assert_eq!(summary["trace_sequence"], decision["trace_sequence"]);
        assert_eq!(summary["session_id"], decision["session_id"]);
        assert_eq!(summary["session_revision"], decision["session_revision"]);
        assert_eq!(
            summary["procedure_snapshot_id"],
            decision["procedure_snapshot_id"]
        );
        assert_eq!(summary["procedure_digest"], decision["procedure_digest"]);
        assert_eq!(summary["graph_node_id"], decision["graph_node_id"]);
        assert_eq!(
            summary["node_definition_id"],
            decision["node_definition_id"]
        );
        assert_eq!(summary["attempt_id"], decision["attempt_id"]);
        assert_eq!(summary["attempt_number"], decision["attempt_number"]);
        assert_eq!(summary["goal_revision"], decision["goal_revision"]);
        assert_eq!(summary["option_id"], decision["option_id"]);
        assert_eq!(summary["effect"], decision["effect"]);
        assert_eq!(
            summary["target_graph_node_id"],
            decision["target_graph_node_id"]
        );
        assert_eq!(summary["mode"], decision["assessment_mode"]);
        assert_eq!(summary["outcome"], outcome);
        assert_eq!(
            summary["criterion_statuses"],
            Value::Array(
                decision["criterion_results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|result| {
                        json!({
                            "criterion_id": result["criterion_id"],
                            "status": result["status"],
                            "citations": result["citations"],
                        })
                    })
                    .collect(),
            )
        );
        assert_eq!(summary["references"], decision["references"]);
        assert_eq!(summary["actor"], decision["actor"]);
        assert_eq!(summary["recorded_at"], decision["recorded_at"]);
        let digest = summary["record_digest"].as_str().unwrap();
        assert_eq!(digest.len(), 71);
        assert!(digest.starts_with("sha256:"));
    }
    fn set_result(engine: &Engine, session_id: &str, number: &mut u64, revision: u64) {
        set_item(
            engine,
            session_id,
            number,
            "result",
            &format!("evidence for goal revision {revision}"),
        );
    }

    #[test]
    fn v2gol004_command_generated_goal_history_is_complete_pageable_and_cold_stable() {
        let ParsedProcedure::V2(parsed) =
            parse_procedure_document(PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
        else {
            panic!("V2GOL-004 fixture must be Procedure v2")
        };
        let validated = validate_procedure_v2(parsed).unwrap();
        let diagnostics = vet_procedure_v2(
            &validated,
            &AuthoringContext::new(
                "goal-readback.yaml",
                PROCEDURE,
                ProcedureDocumentFormat::Yaml,
            ),
        );
        assert!(diagnostics.is_empty(), "invalid fixture: {diagnostics:?}");

        let mut engine = Engine::new(PROCEDURE);
        let session_id = engine.graph().trace().session_id().as_str().to_owned();
        let mut number = 104_000;

        let before_define = status(&engine, &session_id, &mut number);
        let define = mutation_request(
            next_number(&mut number),
            "goal.define",
            &engine,
            json!({
                "goal": "Deliver goal revision 1.",
                "criteria": [
                    {"criterion_id": "correct", "statement": "Revision 1 is correct."},
                    {"criterion_id": "tested", "statement": "Revision 1 is tested."}
                ],
                "actor": "revision 1 author"
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2gol004-define",
            session_identity_preconditions(&before_define),
        );
        let defined = result(dispatch(&engine, &define), "goal.define");
        assert_eq!(defined["goal_revision"], 1);

        let revision_one = run_revision(
            &engine,
            &session_id,
            &mut number,
            1,
            ["satisfied", "satisfied"],
            "achieved",
        );
        revise(&engine, &session_id, &mut number, 1, 2);
        let revision_two = run_revision(
            &engine,
            &session_id,
            &mut number,
            2,
            ["satisfied", "unsatisfied"],
            "not-achieved",
        );
        revise(&engine, &session_id, &mut number, 2, 3);
        let revision_three = run_revision(
            &engine,
            &session_id,
            &mut number,
            3,
            ["not_applicable", "not_applicable"],
            "superseded",
        );

        let standard = status(&engine, &session_id, &mut number);
        let compact = compact_status(&engine, &session_id, &mut number);
        assert_no_histories(&standard);
        assert_no_histories(&compact);
        assert_eq!(standard["goal_revision"], 3);
        assert_eq!(standard["latest_goal_outcome"], "superseded");
        assert_eq!(standard["session"]["lifecycle"], "running");
        assert_eq!(
            standard["goal"]["criteria"],
            json!([
                {"criterion_id":"correct", "statement":"Revision 3 is correct.", "status":"not_applicable"},
                {"criterion_id":"tested", "statement":"Revision 3 is tested.", "status":"not_applicable"}
            ])
        );

        let next = query(
            &engine,
            &session_id,
            &mut number,
            "session.next",
            Map::new(),
        );
        assert_eq!(next["readback"].as_array().unwrap().len(), 1);
        assert_eq!(
            next["readback"][0]["decision_record"], revision_three,
            "next must replay the complete assessment decision record"
        );
        assert_eq!(
            next["readback"][0]["decision_record"]["criterion_results"][0],
            json!({
                "criterion_id":"correct",
                "status":"not_applicable",
                "reason":"revision 3 correct is not_applicable",
                "citations": []
            })
        );

        let verbose = verbose_status(&engine, &session_id, &mut number, None);
        assert_eq!(
            verbose["decision_history"]["entries"][0],
            ordinary_decision_record(&revision_three)
        );
        for assessment_field in [
            "assessment",
            "assessment_mode",
            "goal_outcome",
            "criterion_results",
        ] {
            assert!(
                verbose["decision_history"]["entries"][0]
                    .get(assessment_field)
                    .is_none(),
                "decision history leaked assessment-only field {assessment_field}"
            );
        }
        assert_eq!(verbose["decision_history"]["trace_truncated"], true);
        let revision_three_trace = revision_three["trace_sequence"].as_u64().unwrap();
        let prior_decision = verbose_status(
            &engine,
            &session_id,
            &mut number,
            Some(revision_three_trace),
        );
        assert_eq!(
            prior_decision["decision_history"]["entries"][0],
            ordinary_decision_record(&revision_two)
        );
        assert_eq!(prior_decision["decision_history"]["trace_truncated"], true);
        let revision_two_trace = revision_two["trace_sequence"].as_u64().unwrap();
        let first_decision =
            verbose_status(&engine, &session_id, &mut number, Some(revision_two_trace));
        assert_eq!(
            first_decision["decision_history"]["entries"][0],
            ordinary_decision_record(&revision_one)
        );
        assert_eq!(first_decision["decision_history"]["trace_truncated"], false);
        let stale_revisions = verbose["stale_goal_revision_history"]["entries"]
            .as_array()
            .unwrap();
        let stale_assessments = verbose["stale_goal_assessment_history"]["entries"]
            .as_array()
            .unwrap();
        assert_eq!(stale_revisions.len(), 1);
        assert_eq!(stale_assessments.len(), 1);
        assert_eq!(
            verbose["stale_goal_revision_history"]["trace_truncated"],
            true
        );
        assert_eq!(
            verbose["stale_goal_assessment_history"]["trace_truncated"],
            true
        );
        assert_eq!(stale_revisions[0]["revision"], 2);
        assert_eq!(
            stale_revisions[0]["criteria"],
            json!([
                {"criterion_id":"correct", "statement":"Revision 2 is correct.", "status":"satisfied"},
                {"criterion_id":"tested", "statement":"Revision 2 is tested.", "status":"unsatisfied"}
            ])
        );
        assert_assessment_summary(&stale_assessments[0], &revision_two, "not_achieved");

        let revision_cursor = stale_revisions[0]["trace_sequence"].as_u64().unwrap();
        let earlier_revision =
            verbose_status(&engine, &session_id, &mut number, Some(revision_cursor));
        assert_eq!(
            earlier_revision["stale_goal_revision_history"]["entries"][0]["revision"],
            1
        );
        assert_eq!(
            earlier_revision["stale_goal_revision_history"]["entries"][0]["criteria"],
            json!([
                {"criterion_id":"correct", "statement":"Revision 1 is correct.", "status":"satisfied"},
                {"criterion_id":"tested", "statement":"Revision 1 is tested.", "status":"satisfied"}
            ])
        );
        assert_eq!(
            earlier_revision["stale_goal_revision_history"]["trace_truncated"],
            false
        );

        let assessment_cursor = stale_assessments[0]["trace_sequence"].as_u64().unwrap();
        let earlier_assessment =
            verbose_status(&engine, &session_id, &mut number, Some(assessment_cursor));
        let earlier_assessment = &earlier_assessment["stale_goal_assessment_history"]["entries"][0];
        assert_assessment_summary(earlier_assessment, &revision_one, "achieved");
        assert_ne!(
            earlier_assessment["record_digest"],
            stale_assessments[0]["record_digest"]
        );
        assert!(
            !verbose["stale_goal_assessment_history"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["goal_revision"] == 3)
        );

        let fingerprint = read_fingerprint(&standard);
        let after_reads = status(&engine, &session_id, &mut number);
        assert_eq!(read_fingerprint(&after_reads), fingerprint);

        engine.cold();
        assert_eq!(
            query(
                &engine,
                &session_id,
                &mut number,
                "session.status",
                json!({"verbose": true}).as_object().unwrap().clone(),
            ),
            verbose
        );
        assert_eq!(
            query(
                &engine,
                &session_id,
                &mut number,
                "session.next",
                Map::new(),
            ),
            next
        );

        let terminal = complete_action(
            &engine,
            &session_id,
            &mut number,
            "v2gol004-complete-terminal",
        );
        assert_eq!(terminal["session_state"], "completed");
        let completed_standard = status(&engine, &session_id, &mut number);
        let completed_verbose = verbose_status(&engine, &session_id, &mut number, None);
        assert_eq!(completed_standard["session"]["lifecycle"], "completed");
        assert_eq!(completed_standard["latest_goal_outcome"], "superseded");
        assert_eq!(
            completed_verbose["decision_history"]["entries"][0],
            ordinary_decision_record(&revision_three)
        );

        engine.cold();
        assert_eq!(
            query(
                &engine,
                &session_id,
                &mut number,
                "session.status",
                Map::new(),
            ),
            completed_standard
        );
        assert_eq!(
            query(
                &engine,
                &session_id,
                &mut number,
                "session.status",
                json!({"verbose": true}).as_object().unwrap().clone(),
            ),
            completed_verbose
        );
    }

    #[test]
    fn v2gol004_max_escaped_assessment_stays_out_of_history_but_survives_next_readback() {
        let ParsedProcedure::V2(parsed) =
            parse_procedure_document(PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
        else {
            panic!("V2GOL-004 fixture must be Procedure v2")
        };
        let validated = validate_procedure_v2(parsed).unwrap();
        let diagnostics = vet_procedure_v2(
            &validated,
            &AuthoringContext::new(
                "goal-readback.yaml",
                PROCEDURE,
                ProcedureDocumentFormat::Yaml,
            ),
        );
        assert!(diagnostics.is_empty(), "invalid fixture: {diagnostics:?}");

        let engine = Engine::new(PROCEDURE);
        let session_id = engine.graph().trace().session_id().as_str().to_owned();
        let mut number = 104_000;

        let criteria = (0..16)
            .map(|index| {
                json!({
                    "criterion_id": format!("criterion-{index:02}"),
                    "statement": format!("Maximum criterion {index:02} is satisfied."),
                })
            })
            .collect::<Vec<_>>();
        let before_define = status(&engine, &session_id, &mut number);
        let define = mutation_request(
            next_number(&mut number),
            "goal.define",
            &engine,
            json!({
                "goal": "Preserve the maximum escaped assessment readback.",
                "criteria": criteria,
                "actor": "maximum goal author"
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2gol004-max-define",
            session_identity_preconditions(&before_define),
        );
        result(dispatch(&engine, &define), "goal.define");

        set_result(&engine, &session_id, &mut number, 1);
        complete_action(
            &engine,
            &session_id,
            &mut number,
            "v2gol004-max-complete-work",
        );
        for item_id in ["note-one", "note-two", "note-three"] {
            set_item(
                &engine,
                &session_id,
                &mut number,
                item_id,
                &format!("persisted {item_id}"),
            );
        }

        let maximum_reason = "\u{1}".repeat(2_000);
        let maximum_actor = "a".repeat(256);
        for index in 0..16 {
            let before = status(&engine, &session_id, &mut number);
            let criterion_id = format!("criterion-{index:02}");
            let request_number = next_number(&mut number);
            let request = mutation_request(
                request_number,
                "goal.assess_criterion",
                &engine,
                json!({
                    "criterion_id": criterion_id,
                    "status": "satisfied",
                    "reason": maximum_reason,
                    "evidence": ["perform"],
                    "items": ["note-one", "note-two", "note-three"],
                    "actor": maximum_actor,
                })
                .as_object()
                .unwrap()
                .clone(),
                &format!("v2gol004-max-assess-{index:02}"),
                goal_preconditions(&before, 1),
            );
            result(dispatch(&engine, &request), "goal.assess_criterion");
        }

        let before_decide = status(&engine, &session_id, &mut number);
        let decide = mutation_request(
            next_number(&mut number),
            "session.decide",
            &engine,
            json!({
                "option_id": "achieved",
                "reason": maximum_reason,
                "actor": maximum_actor,
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2gol004-max-decide",
            goal_preconditions(&before_decide, 1),
        );
        let decision = result(dispatch(&engine, &decide), "session.decide")["record"].clone();
        assert_eq!(decision["reason"], maximum_reason);
        assert_eq!(decision["actor"], maximum_actor);
        assert_eq!(decision["criterion_results"].as_array().unwrap().len(), 16);

        let next_request = query_request(
            next_number(&mut number),
            "session.next",
            &engine,
            &session_id,
            Map::new(),
        );
        let DirectResponseV1::Success(next_response) = dispatch(&engine, &next_request) else {
            panic!("expected bounded read success")
        };
        let next_encoded = serde_json::to_vec(&next_response).unwrap();
        assert!(next_encoded.len() <= 1_048_576);
        let next_decoded: OutputEnvelopeV4 = serde_json::from_slice(&next_encoded).unwrap();
        assert_eq!(next_decoded, next_response);
        let next = result(DirectResponseV1::Success(next_response), "session.next");
        let readback = &next["readback"][0]["decision_record"];
        assert_eq!(readback, &decision);
        for result in readback["criterion_results"].as_array().unwrap() {
            assert_eq!(result["reason"], maximum_reason);
            assert_eq!(result["citations"].as_array().unwrap().len(), 4);
            assert_eq!(
                result["citations"],
                json!([
                    {"reference_graph_node_id": "perform"},
                    {"local_item_id": "note-one"},
                    {"local_item_id": "note-two"},
                    {"local_item_id": "note-three"}
                ])
            );
        }

        revise(&engine, &session_id, &mut number, 1, 2);
        let cursor = decision["trace_sequence"].as_u64().unwrap() + 1;
        let mut payload = json!({"verbose": true, "history_before": cursor})
            .as_object()
            .unwrap()
            .clone();
        let verbose_request = query_request(
            next_number(&mut number),
            "session.status",
            &engine,
            &session_id,
            std::mem::take(&mut payload),
        );
        let DirectResponseV1::Success(verbose_response) = dispatch(&engine, &verbose_request)
        else {
            panic!("expected bounded read success")
        };
        let verbose_encoded = serde_json::to_vec(&verbose_response).unwrap();
        assert!(verbose_encoded.len() <= 1_048_576);
        let verbose_decoded: OutputEnvelopeV4 = serde_json::from_slice(&verbose_encoded).unwrap();
        assert_eq!(verbose_decoded, verbose_response);
        let verbose = result(
            DirectResponseV1::Success(verbose_response),
            "session.status",
        );
        let ordinary = &verbose["decision_history"]["entries"][0];
        assert_eq!(ordinary, &ordinary_decision_record(&decision));
        for assessment_field in [
            "assessment",
            "assessment_mode",
            "goal_outcome",
            "criterion_results",
        ] {
            assert!(ordinary.get(assessment_field).is_none());
        }
        assert!(
            serde_json::to_vec(&verbose["decision_history"])
                .unwrap()
                .len()
                <= 65_536
        );
        let assessment = &verbose["stale_goal_assessment_history"]["entries"][0];
        assert_assessment_summary(assessment, &decision, "achieved");
        assert_eq!(
            assessment["criterion_statuses"].as_array().unwrap().len(),
            16
        );
        assert!(
            serde_json::to_vec(&verbose["stale_goal_assessment_history"])
                .unwrap()
                .len()
                <= 65_536
        );
    }
}
