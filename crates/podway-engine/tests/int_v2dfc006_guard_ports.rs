//! Conditional requirements and decision guards through the typed direct Engine.

use super::{
    int_v2dfc006_direct_execution::Clock,
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};
use podway_core::OptionId;
use podway_engine::{
    direct_api::{DirectCommandV1, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
};
use podway_protocol::{ProcedureV2MutationCommandV1, SessionCompleteV1, SessionDecideV2};
use serde_json::{Value, json};

const CONDITIONAL_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: conditional-items
version: "1"
name: Conditional items
purpose: Exercise same-attempt conditional item requirements.
node_definitions:
  work:
    type: action
    title: Work
    intent: Record the selected mode and any required notes.
    items:
      - id: mode
        type: choice
        prompt: Select a mode.
        required: true
        choices: [strict, relaxed]
      - id: notes
        type: text
        prompt: Record strict-mode notes.
        required: false
        required_when:
          - item: mode
            equals: strict
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
"#;

const GUARDED_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: guarded-options
version: "1"
name: Guarded options
purpose: Exercise selected-evidence option guards.
node_definitions:
  review:
    type: action
    title: Review
    intent: Record the review result.
    items:
      - id: findings
        type: integer
        prompt: How many findings remain?
        required: true
      - id: notes
        type: text
        prompt: Optional review notes.
        required: false
  decide:
    type: decision
    title: Decide
    objective: Select the supported disposition.
    prompt: Which disposition is supported?
    items:
      - id: acknowledgement
        type: text
        prompt: Record that the evidence was reviewed.
        required: true
    options:
      - id: approved
        label: Approved
        criteria: No findings remain.
        guards:
          - evidence:
              node: review
              item: findings
            equals: 0
      - id: blocked
        label: Blocked
        criteria: At least one finding remains.
        guards:
          - evidence:
              node: review
              item: findings
            at_least: 1
      - id: noted
        label: Notes recorded
        criteria: Optional notes were recorded.
        guards:
          - evidence:
              node: review
              item: notes
            non_empty: true
      - id: fallback
        label: Fallback
        criteria: No guarded disposition is needed.
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the guarded workflow.
graph:
  entry: review
  nodes:
    - id: review
      use: review
      next: decide
    - id: decide
      use: decide
      evidence_from:
        - node: review
          required: true
          items: [notes]
        - node: review
          required: true
          items: [findings]
      routes:
        approved:
          to: finish
          effect: advance
        blocked:
          to: finish
          effect: advance
        noted:
          to: finish
          effect: advance
        fallback:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
"#;

fn item<'a>(observation: &'a Value, id: &str) -> &'a Value {
    observation["active_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == id)
        .unwrap()
}
fn decide(engine: &Engine, option: &str) -> DirectCommandV1 {
    DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionDecide(
        SessionDecideV2 {
            option_id: OptionId::new(option).unwrap(),
            reason: "Choose the supported disposition.".into(),
            actor: None,
            expected_goal_revision: None,
            preconditions: engine.fences(),
        },
    ))
}

#[test]
fn v2grd003_required_now_and_completion_share_the_authoritative_item_snapshot() {
    let mut engine = Engine::new(CONDITIONAL_PROCEDURE);
    let initial = engine.read(DirectReadCommandV1::Observe);
    assert_eq!(
        initial["status"]["current"]["readiness"]["can_advance"],
        false
    );
    assert_eq!(item(&initial, "mode")["required_now"], true);
    assert_eq!(item(&initial, "notes")["required_now"], false);
    assert_eq!(
        item(&initial, "notes")["condition_status"]["state"],
        "unevaluable"
    );
    engine.set("mode", "relaxed", "relaxed");
    let relaxed = engine.read(DirectReadCommandV1::Observe);
    assert_eq!(
        relaxed["status"]["current"]["readiness"]["can_advance"],
        true
    );
    assert_eq!(item(&relaxed, "notes")["required_now"], false);
    assert_eq!(
        item(&relaxed, "notes")["condition_status"]["state"],
        "unmet"
    );
    engine.set("mode", "strict", "strict");
    engine.cold();
    let strict = engine.read(DirectReadCommandV1::Observe);
    assert_eq!(
        strict["status"]["current"]["missing_required_item_count"],
        1
    );
    assert_eq!(item(&strict, "notes")["required_now"], true);
    assert_eq!(item(&strict, "notes")["condition_status"]["state"], "met");
    assert_eq!(
        item(&strict, "notes")["condition_status"]["predicates"][0]["actual"],
        "strict"
    );
    engine.reject(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: engine.fences(),
        })),
        "missing",
        "REQUIRED_ITEMS_MISSING",
        &Clock,
    );
    engine.set("notes", "required evidence", "notes");
    assert_eq!(
        engine.complete("complete")["result"]["session_state"],
        "completed"
    );
}

#[test]
fn v2grd004_next_keeps_all_options_and_projects_authoritative_guard_statuses() {
    let mut engine = Engine::new(GUARDED_PROCEDURE);
    engine.set("findings", "0", "findings");
    engine.complete("review");
    engine.cold();
    let observation = engine.read(DirectReadCommandV1::Observe);
    let guidance = &observation["guidance"];
    assert_eq!(guidance["options"].as_array().unwrap().len(), 4);
    assert_eq!(guidance["allowed_option_ids"], json!([]));
    let statuses = guidance["option_guard_statuses"].as_array().unwrap();
    assert_eq!(statuses.len(), 4);
    assert_eq!(statuses[0]["state"], "met");
    assert_eq!(statuses[0]["predicates"][0]["actual"], 0);
    assert_eq!(statuses[1]["state"], "unmet");
    assert_eq!(statuses[2]["state"], "unevaluable");
    assert!(statuses[2]["predicates"][0].get("actual").is_none());
    assert_eq!(
        statuses[3],
        json!({"option_id":"fallback","state":"met","predicates":[]})
    );
    assert_eq!(
        guidance["options"][0]["guards"][0]["evidence"]["node"],
        "review"
    );
    let stale = decide(&engine, "blocked");
    engine.reject(stale.clone(), "missing", "REQUIRED_ITEMS_MISSING", &Clock);
    engine.set("acknowledgement", "Evidence reviewed.", "ack");
    engine.reject(stale, "stale", "SESSION_REVISION_CONFLICT", &NoClock);
    assert_eq!(
        engine.read(DirectReadCommandV1::Observe)["guidance"]["allowed_option_ids"],
        json!(["approved", "fallback"])
    );
    let before = engine.graph();
    let sequence = engine.sequence();
    let request = engine.request(decide(&engine, "blocked"), "blocked");
    let DirectResponseV1::Failure(error) = engine.execute(request, &Clock) else {
        panic!("guard must reject")
    };
    let value = error.into_value();
    assert_eq!(value["code"], "OPTION_GUARD_UNSATISFIED");
    assert_eq!(value["exit_code"], 1);
    assert_eq!(value["retryable"], false);
    assert_eq!(value["details"]["graph_node_id"], "decide");
    assert_eq!(value["details"]["option_id"], "blocked");
    assert_eq!(value["details"]["state"], "unmet");
    assert_eq!(value["details"]["predicates"][0]["actual"], 0);
    assert_eq!(value["details"]["predicates"][0]["expected"], 1);
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    assert!(
        engine
            .store
            .read(
                super::int_v2dfc006_direct_execution::deadline(),
                |snapshot| snapshot
                    .outcome(&podway_store::IdempotencyKeyV1::new("blocked").unwrap())
            )
            .unwrap()
            .is_none()
    );
    let request = engine.request(decide(&engine, "approved"), "approved");
    let result = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(result["result"]["option_id"], "approved");
    assert_eq!(result["result"]["graph_node_id"], "decide");
    assert_eq!(result["result"]["target_graph_node_id"], "finish");
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), result);
}

#[test]
fn v2grd005_no_available_option_keeps_session_running_without_decide_action() {
    let procedure=GUARDED_PROCEDURE.replace(
        "      - id: fallback\n        label: Fallback\n        criteria: No guarded disposition is needed.\n",
        "      - id: fallback\n        label: Fallback\n        criteria: No guarded disposition is needed.\n        guards:\n          - evidence:\n              node: review\n              item: notes\n            non_empty: true\n");
    let engine = Engine::new(&procedure);
    engine.set("findings", "-1", "findings");
    engine.complete("review");
    let observation = engine.read(DirectReadCommandV1::Observe);
    assert_eq!(observation["status"]["session"]["lifecycle"], "running");
    let guidance = &observation["guidance"];
    assert_eq!(guidance["allowed_option_ids"], json!([]));
    assert!(
        !guidance["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.decide"))
    );
    assert!(
        guidance["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["command"] != "session.decide")
    );
}
