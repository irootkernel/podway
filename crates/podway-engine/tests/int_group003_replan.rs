//! Guarded replanning against actual candidate Engine and SQLite transactions.

use super::int_group002_transactions::{
    Fixture, TIME, action, activation, add_counter_chain, base_document, canonical, group_document,
    inflate_current_guidance, mutation,
};
use podway_config::ProcedureDocumentFormat;
use podway_config::execution_groups::{parse_successor_document, validate_successor_definition};
use podway_core::execution_groups::*;
use podway_core::{
    AttemptLifecycle, AttemptValidityV2, CheckResultInputBasisV2, DomainError, ItemId, OptionId,
    ReasonV2, RecordedItemValueV2, Revision, recorded_value_digest_v2,
};
use podway_engine::direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1};
use podway_engine::successor_execution::{
    SuccessorExecutionError, activate_group, replan_group, return_from_group,
};
use podway_store::IdempotencyKeyV1;
use podway_store::direct_store::{DirectOutcomeScopeV1, DirectStoreErrorV1};
use podway_store::successor_store::{CandidateSources, CandidateStoreError};
use rusqlite::Connection;
use serde_json::{Value, json};

fn assessment_items(score_required: bool) -> Value {
    json!([
        {"id":"verdict", "type":"choice", "prompt":"Record verdict", "required":true,
         "choices":["success","failure"]},
        {"id":"stage", "type":"choice", "prompt":"Record stage", "required":true,
         "choices":["not-scored","lightweight","scored"]},
        {"id":"score", "type":"integer", "prompt":"Record score", "required":score_required,
         "minimum":0,"maximum":100}
    ])
}

fn decision_definition(source: &Value) -> Value {
    let item = |id| {
        let mut item = source.clone();
        item["item"] = json!(id);
        item
    };
    json!({
        "type":"decision","title":"Choose","objective":"Choose a declared path.",
        "prompt":"Choose a path.","reason":{"required":true},"options":[
            {"id":"finish","label":"Finish"},
            {"id":"redesign","label":"Redesign","guards":[
                {"evidence":item("verdict"),"equals":"failure"},
                {"evidence":item("stage"),"equals":"scored"},
                {"evidence":item("score"),"at_least":80}
            ]}
        ]
    })
}

fn decision_placement(id: &str, target: &str, mut source: Value) -> Value {
    source["required"] = json!(true);
    source["items"] = json!(["verdict", "stage", "score"]);
    json!({
        "id":id,"use":id,"evidence_from":[source],"routes":{
            "finish":{"effect":"advance","to":target},
            "redesign":{"effect":"replan"}
        }
    })
}

pub(super) fn local_decision_group(score_required: bool) -> Value {
    let mut group = group_document();
    let source = json!({"scope":"local","node":"work"});
    group["node_definitions"]["work"]["items"] = assessment_items(score_required);
    group["node_definitions"]["choose"] = decision_definition(&source);
    group["graph"]["nodes"][0]["next"] = json!("choose");
    group["graph"]["nodes"]
        .as_array_mut()
        .unwrap()
        .insert(1, decision_placement("choose", "finish", source));
    group
}

pub(super) fn completed_group_documents() -> (Value, Value) {
    let mut base = base_document();
    let source = json!({"scope":"execution_result"});
    base["node_definitions"]["review"] = decision_definition(&source);
    base["graph"]["nodes"][1] = decision_placement("review", "close", source);
    base["execution_group"]["result_items"] = assessment_items(true);
    base["execution_group"]["replan_decisions"] = json!(["review"]);
    let mut group = group_document();
    group["node_definitions"]["finish"] = action("Record terminal assessment");
    group["node_definitions"]["finish"]["items"] = assessment_items(true);
    (base, group)
}

fn open_group(base: &Value, group: &Value) -> Fixture {
    let mut fixture = Fixture::new(base, true);
    let (design, _) = fixture.state();
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &canonical(group), "activate"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    fixture
}

fn record(state: &mut ExecutionSessionV3, id: &str, value: RecordedItemValueV2) {
    let target = state.active_attempt().unwrap().target().clone();
    state
        .record_item(
            &target,
            &ItemId::new(id).unwrap(),
            Revision::ZERO,
            Some(value),
            TIME,
        )
        .unwrap();
}

pub(super) fn assessment(
    state: &mut ExecutionSessionV3,
    verdict: &str,
    stage: &str,
    score: Option<i64>,
) {
    record(
        state,
        "verdict",
        RecordedItemValueV2::choice(verdict).unwrap(),
    );
    record(state, "stage", RecordedItemValueV2::choice(stage).unwrap());
    if let Some(score) = score {
        record(state, "score", RecordedItemValueV2::integer(score));
    }
}

pub(super) fn complete(state: &mut ExecutionSessionV3) {
    state
        .complete(
            &state.fence(),
            Some(NativeDirectIdSourceV1.next_attempt_id()),
            TIME,
        )
        .unwrap();
}

pub(super) fn feedback(state: &ExecutionSessionV3, option: &str) -> ReplanFeedbackV1 {
    ReplanFeedbackV1::new(
        state.active_attempt().unwrap().target().clone(),
        OptionId::new(option).unwrap(),
        "The recorded assessment requires a fresh design.".into(),
        CheckResultInputBasisV2::new("Observed fixture source", state.base_digest().clone())
            .unwrap(),
        "Keep applicable prior work and revise the design.".into(),
    )
    .unwrap()
}

#[test]
fn running_group_replan_commits_exact_decision_lineage_and_replays_after_reopen() {
    let mut fixture = open_group(&base_document(), &local_decision_group(true));
    let (mut state, sources) = fixture.state();
    record(
        &mut state,
        "verdict",
        RecordedItemValueV2::choice("success").unwrap(),
    );
    state
        .retry(
            &state.fence(),
            ReasonV2::new("Keep the incomplete first assessment.").unwrap(),
            NativeDirectIdSourceV1.next_attempt_id(),
            TIME,
        )
        .unwrap();
    assessment(&mut state, "failure", "scored", Some(79));
    state
        .record_item(
            &state.active_attempt().unwrap().target().clone(),
            &ItemId::new("score").unwrap(),
            Revision::ZERO.checked_next().unwrap(),
            Some(RecordedItemValueV2::integer(80)),
            TIME,
        )
        .unwrap();
    complete(&mut state);
    fixture.edit_fixture(&state, &sources);
    let decision = feedback(&state, "redesign");
    let mut ordinary = state.clone();
    assert_eq!(
        ordinary.decide(
            &ordinary.fence(),
            decision.option().clone(),
            ReasonV2::new(decision.reason()).unwrap(),
            None,
            NativeDirectIdSourceV1.next_attempt_id(),
            TIME,
        ),
        Err(DomainError::InvalidState {
            reason: "the selected option requires explicit group replan"
        })
    );
    assert_eq!(ordinary, state);

    let request = mutation(&fixture.workspace, &state, "replan");
    let original_id = request.request_id.clone();
    let committed = replan_group(
        &mut fixture.store,
        request,
        decision.clone(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(!committed.replayed());
    assert_eq!(committed.outcome().sequence(), 2);
    assert_eq!(committed.outcome().command(), "group.replan");
    assert_eq!(
        committed.outcome().scope(),
        &DirectOutcomeScopeV1::Session(fixture.session.clone())
    );
    assert_eq!(fixture.ledger(), (2, 2, 2));
    let (after, after_sources) = fixture.state();
    assert_eq!(after_sources, sources);
    assert_eq!(
        &after.attempt_history()[1..3],
        &state.attempt_history()[1..3]
    );
    assert!(
        after.attempt_history()[1]
            .slots()
            .iter()
            .any(|slot| slot.value().is_none())
    );
    assert_eq!(
        after.attempt_history()[0].validity(),
        AttemptValidityV2::Stale
    );
    assert_eq!(
        after.attempt_history()[0].slots(),
        state.attempt_history()[0].slots()
    );
    let anchor = assert_replan(
        &after,
        &state,
        &decision,
        GroupExecutionOutcomeV1::Abandoned,
    );
    let envelope: Value = serde_json::from_str(committed.outcome().response().as_str()).unwrap();
    assert!(podway_protocol::OutputEnvelopeV4::from_value(envelope.clone()).is_err());
    assert_eq!(envelope["request_id"], original_id.as_str());
    let result = &envelope["result"];
    assert_eq!(result["active_attempt"], json!(anchor.target()));
    assert_eq!(result["closed_attempt"], json!(decision.decision()));
    assert_eq!(result["group_outcome"], "abandoned");
    assert_eq!(result["closure"]["source"], json!(decision.decision()));
    assert_eq!(result["closure"]["cause"], "replan");
    assert_eq!(result["closure"]["feedback"], decision.feedback());
    assert_eq!(result["decision"]["option"], "redesign");
    let frozen = after.attempt_history()[3].decision().unwrap();
    assert_eq!(
        result["decision"]["guard_values"],
        json!(
            frozen
                .guard_values
                .iter()
                .map(|guard| json!({
                    "source":guard.source,"item":guard.item.item(),"revision":guard.item.revision(),
                    "value_digest":guard.item.value_digest()
                }))
                .collect::<Vec<_>>()
        )
    );
    assert!(
        result["counters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|counter| counter["scope"] == "base")
    );
    assert_eq!(result["guidance"]["placement"]["id"], "design");

    fixture.reopen();
    let replay_request = mutation(&fixture.workspace, &state, "replan");
    assert_ne!(replay_request.request_id, original_id);
    let replay = replan_group(
        &mut fixture.store,
        replay_request,
        decision.clone(),
        || panic!("exact replay cannot enter transition"),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(replay.replayed());
    assert_eq!(replay.outcome(), committed.outcome());
    assert_eq!(
        fixture
            .store
            .outcome(&IdempotencyKeyV1::new("replan").unwrap())
            .unwrap()
            .as_ref(),
        Some(committed.outcome())
    );
    let changed = ReplanFeedbackV1::new(
        decision.decision().clone(),
        decision.option().clone(),
        decision.reason().to_owned(),
        decision.working_copy_basis().clone(),
        "Different feedback for the same key.".into(),
    )
    .unwrap();
    assert!(matches!(
        replan_group(
            &mut fixture.store,
            mutation(&fixture.workspace, &state, "replan"),
            changed,
            || panic!("key conflict cannot enter transition"),
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Direct(
            DirectStoreErrorV1::IdempotencyKeyReused
        )))
    ));
    assert_eq!(fixture.state(), (after, sources));
    assert_eq!(fixture.ledger(), (2, 2, 2));
}

fn assert_replan<'a>(
    after: &'a ExecutionSessionV3,
    before: &ExecutionSessionV3,
    feedback: &ReplanFeedbackV1,
    outcome: GroupExecutionOutcomeV1,
) -> &'a GroupAttemptStateV1 {
    assert!(after.selected().is_none());
    assert_eq!(
        after.fence().revision,
        before.fence().revision.checked_next().unwrap()
    );
    assert_eq!(
        after.attempt_history().len(),
        before.attempt_history().len() + 1
    );
    assert_eq!(after.group_history().len(), before.group_history().len());
    let previous = before.selected().unwrap();
    let closed = after.group_history().last().unwrap();
    assert_eq!(closed.identity(), previous.identity());
    assert_eq!(closed.outcome(), outcome);
    assert_eq!(closed.terminal_attempt(), previous.terminal_attempt());
    let closure = closed.closure().unwrap();
    assert_eq!(closure.cause, SelectionClosureCauseV1::Replan);
    assert_eq!(&closure.source, feedback.decision());
    assert_eq!(closure.at, TIME);
    let retained = closure.feedback.as_ref().unwrap();
    assert_eq!(retained.reason(), feedback.reason());
    assert_eq!(retained.working_copy_basis(), feedback.working_copy_basis());
    assert_eq!(retained.feedback(), feedback.feedback());

    let current_index = before.attempt_history().len() - 1;
    let decision = &after.attempt_history()[current_index];
    let record = decision.decision().unwrap();
    assert_eq!(decision.target(), feedback.decision());
    assert_eq!(decision.lifecycle(), AttemptLifecycle::Completed);
    assert_eq!(decision.closed_at(), Some(TIME));
    assert_eq!(&record.option, feedback.option());
    assert_eq!(record.reason, feedback.reason());
    assert_eq!(record.goal_assessment, None);
    let source = &before.attempt_history()[current_index - 1];
    assert_eq!(record.guard_values.len(), 3);
    for (guard, id) in record
        .guard_values
        .iter()
        .zip(["verdict", "stage", "score"])
    {
        let slot = source
            .slots()
            .iter()
            .find(|slot| slot.item_id().as_str() == id)
            .unwrap();
        assert_eq!(&guard.source, source.target());
        assert_eq!(guard.item.item(), slot.item_id());
        assert_eq!(guard.item.revision(), slot.revision());
        assert_eq!(
            guard.item.value_digest(),
            &recorded_value_digest_v2(slot.value().unwrap()).unwrap()
        );
    }
    let anchor = after.active_attempt().unwrap();
    assert_eq!(anchor.target().scope(), &ExecutionScopeV1::Base);
    assert_eq!(
        anchor.target().node(),
        before.base_definition().boundary().anchor()
    );
    assert!(
        before
            .attempt_history()
            .iter()
            .all(|attempt| attempt.target().attempt() != anchor.target().attempt())
    );
    assert!(anchor.slots().iter().all(|slot| slot.value().is_none()));
    anchor
}

#[test]
fn candidate_replan_rejects_action_undeclared_and_unevaluable_or_false_current_guards() {
    let cases = [
        ("missing", "failure", "scored", None),
        ("unscored", "failure", "not-scored", Some(100)),
        ("lightweight", "failure", "lightweight", Some(100)),
        ("success-verdict", "success", "scored", Some(100)),
        ("subthreshold", "failure", "scored", Some(79)),
    ];
    for (key, verdict, stage, score) in cases {
        // The optional score is a Config-admitted custom source with a required evidence reference.
        let mut fixture = open_group(&base_document(), &local_decision_group(score.is_some()));
        let mut before = fixture.state();
        assert_rejected(
            &mut fixture,
            &before,
            "action",
            "redesign",
            "the current placement is not a Decision",
        );
        assessment(&mut before.0, verdict, stage, score);
        complete(&mut before.0);
        fixture.edit_fixture(&before.0, &before.1);
        assert_rejected(
            &mut fixture,
            &before,
            "unknown",
            "unknown",
            "the selected option has no declared route",
        );
        assert_rejected(
            &mut fixture,
            &before,
            "advance",
            "finish",
            "replan requires a declared ordinary Decision at the current selected placement",
        );
        assert_rejected(
            &mut fixture,
            &before,
            key,
            "redesign",
            "guard is unmet or unevaluable in current evidence",
        );
    }
}

fn assert_rejected(
    fixture: &mut Fixture,
    before: &(ExecutionSessionV3, CandidateSources),
    key: &str,
    option: &str,
    rejection: &str,
) {
    let counts = fixture.ledger();
    let error = replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &before.0, key),
        feedback(&before.0, option),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap_err();
    assert!(
        matches!(&error, SuccessorExecutionError::Domain(DomainError::InvalidState { reason }) if *reason == rejection),
        "{key}: {error:?}"
    );
    fixture.compare_rejected(before, key, counts);
}

#[test]
fn admitted_base_review_replan_preserves_completed_group_and_terminal_records() {
    let (base, group) = completed_group_documents();
    let mut disallowed = base.clone();
    disallowed["execution_group"]["replan_decisions"] = json!([]);
    let admitted = parse_successor_document(
        disallowed.to_string().as_bytes(),
        ProcedureDocumentFormat::Json,
    )
    .unwrap();
    assert!(
        validate_successor_definition(admitted)
            .unwrap_err()
            .to_string()
            .contains("base replan requires explicit anchor-return policy")
    );

    let mut fixture = open_group(&base, &group);
    let (mut state, sources) = fixture.state();
    complete(&mut state);
    assessment(&mut state, "failure", "scored", Some(80));
    fixture.edit_fixture(&state, &sources);
    return_from_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &state, "return"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (review, review_sources) = fixture.state();
    assert_eq!(
        review.selected().unwrap().outcome(),
        GroupExecutionOutcomeV1::Completed
    );
    assert_eq!(review.active_attempt().unwrap().decision_entry(), Some(1));
    let decision = feedback(&review, "redesign");
    assert_eq!(decision.decision().scope(), &ExecutionScopeV1::Base);
    let committed = replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &review, "review-replan"),
        decision.clone(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let envelope: Value = serde_json::from_str(committed.outcome().response().as_str()).unwrap();
    assert_eq!(
        envelope["result"]["counters"],
        json!([
            {"scope":"base","node":"design","attempt_count":2,"decision_entry":null},
            {"scope":"base","node":"review","attempt_count":1,"decision_entry":null},
            {"scope":"base","node":"close","attempt_count":0,"decision_entry":null}
        ])
    );
    fixture.reopen();
    let (after, after_sources) = fixture.state();
    assert_eq!(after_sources, review_sources);
    assert_replan(
        &after,
        &review,
        &decision,
        GroupExecutionOutcomeV1::Completed,
    );
    assert_eq!(
        &after.attempt_history()[1..3],
        &review.attempt_history()[1..3]
    );
    assert_eq!(fixture.ledger(), (3, 3, 3));
}

#[test]
fn outcome_insert_failure_rolls_back_replan_and_same_request_can_be_retried() {
    let mut fixture = open_group(&base_document(), &local_decision_group(true));
    let (mut state, sources) = fixture.state();
    assessment(&mut state, "failure", "scored", Some(80));
    complete(&mut state);
    fixture.edit_fixture(&state, &sources);
    let before = (state, sources);
    let decision = feedback(&before.0, "redesign");
    // Occupy the next sequence using the existing outcome row in this owned fixture.
    // The uniqueness failure occurs after the Engine writes state and Store advances sequence.
    let connection = Connection::open(&fixture.file).unwrap();
    assert_eq!(connection.execute("UPDATE mutation_outcomes SET workspace_sequence=2 WHERE idempotency_key='activate' AND workspace_sequence=1", []).unwrap(), 1);
    let failure = replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &before.0, "retry-replan"),
        decision.clone(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap_err();
    assert!(
        matches!(failure, SuccessorExecutionError::Store(CandidateStoreError::Direct(DirectStoreErrorV1::Sqlite(rusqlite::Error::SqliteFailure(code, Some(message)))))
        if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE && message.contains("mutation_outcomes.workspace_sequence"))
    );
    fixture.reopen();
    fixture.compare_rejected(&before, "retry-replan", (1, 1, 1));
    assert_eq!(connection.execute("UPDATE mutation_outcomes SET workspace_sequence=1 WHERE idempotency_key='activate' AND workspace_sequence=2", []).unwrap(), 1);
    let committed = replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &before.0, "retry-replan"),
        decision.clone(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(!committed.replayed());
    assert_replan(
        &fixture.state().0,
        &before.0,
        &decision,
        GroupExecutionOutcomeV1::Abandoned,
    );
    assert_eq!(fixture.ledger(), (2, 2, 2));
}

#[test]
fn oversized_replan_success_leaves_the_current_decision_and_selection_untouched() {
    let mut base = base_document();
    let plan = base["node_definitions"]["design"]["items"][0].clone();
    inflate_current_guidance(&mut base, "design");
    base["node_definitions"]["design"]["items"][0] = plan;
    add_counter_chain(&mut base, "review", "close", 120);
    let mut fixture = open_group(&base, &local_decision_group(true));
    let (mut state, sources) = fixture.state();
    assessment(&mut state, "failure", "scored", Some(80));
    complete(&mut state);
    fixture.edit_fixture(&state, &sources);
    let before = (state, sources);
    let decision = feedback(&before.0, "redesign");
    let expanded = ReplanFeedbackV1::new(
        decision.decision().clone(),
        decision.option().clone(),
        format!("R{}", "\u{0001}".repeat(1_999)),
        decision.working_copy_basis().clone(),
        format!("F{}", "\u{0001}".repeat(3_999)),
    )
    .unwrap();
    assert!(matches!(
        replan_group(
            &mut fixture.store,
            mutation(&fixture.workspace, &before.0, "replan-budget"),
            expanded,
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::ResponseBudget)
    ));
    fixture.compare_rejected(&before, "replan-budget", (1, 1, 1));
    replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &before.0, "replan-budget"),
        decision,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert_eq!(fixture.ledger(), (2, 2, 2));
}
