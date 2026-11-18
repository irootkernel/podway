//! Process qualification of the admitted candidate Engine and Store, with owned Git fixtures.

#[path = "int_group005_driver.rs"]
mod driver;

use super::int_group002_transactions::{
    add_counter_chain, base_document, canonical, group_document, inflate_current_guidance,
};
use super::int_group003_replan::{completed_group_documents, local_decision_group};
use super::int_group004_fences::tracked_base;
use driver::{
    Action, Call, Edit, Fixture, ItemValue, Mode, Outcome, PriorWork, Probe, Refusal, Reply,
    response_digest,
};
use podway_core::execution_groups::{GroupExecutionIdV1, HistoricalItemV1, HistoricalReferenceV1};
use podway_core::{ItemId, Revision};
use serde_json::{Value, json};

fn text(item: &str, expected: u64, value: &str) -> Edit {
    Edit {
        item: item.into(),
        expected,
        value: ItemValue::Text(value.into()),
    }
}

fn optional_text(item: &str) -> Value {
    json!({"id":item,"type":"text","prompt":"Record a value.","required":false})
}

fn report_group() -> Value {
    let mut group = group_document();
    group["node_definitions"]["work"]["items"] =
        json!([optional_text("alpha"), optional_text("beta")]);
    group
}

fn execution(group: &Value) -> GroupExecutionIdV1 {
    serde_json::from_value(group["id"].clone()).unwrap()
}

fn activate(probe: &Probe, key: &str, group: &Value, work: Vec<PriorWork>) -> Call {
    Call::new(
        probe,
        key,
        Action::Activate {
            document: group.clone(),
            digest: canonical(group).digest().clone(),
            plan: probe.plan.clone().unwrap(),
            predecessor: probe.groups.last().map(execution),
            prior_work: work,
        },
    )
}

fn success(reply: Reply, replay: bool) -> Outcome {
    let Reply::Success { outcome, replayed } = reply else {
        panic!("expected success, got {reply:?}")
    };
    assert_eq!(replayed, replay);
    outcome
}

fn rejection(reply: Reply, expected: Refusal) {
    let Reply::Rejected(error) = reply else {
        panic!("expected {expected:?}, got {reply:?}")
    };
    assert_eq!(error, expected);
}

fn write(fixture: &mut Fixture, key: &str, action: Action) -> Outcome {
    let current = fixture.read(&[]);
    let call = Call::new(&current, key, action);
    let outcome = success(fixture.run(&call), false);
    assert_eq!(outcome.session, fixture.session);
    assert_eq!(outcome.document()["request_id"], call.response_id);
    outcome
}

fn race(fixture: &mut Fixture, winning: &Call, losing: &[Call]) -> (Outcome, Vec<Reply>) {
    let mut writer = fixture.launch(winning, Mode::ParkBeforeCommit);
    let mut contenders: Vec<_> = losing
        .iter()
        .map(|call| fixture.launch(call, Mode::Contender))
        .collect();
    writer.start();
    let mark = writer.boundary("AfterLedgerBeforeCommit");
    for contender in &contenders {
        contender.start();
    }
    for contender in &mut contenders {
        contender.busy();
    }
    writer.release();
    let outcome = success(writer.finish(), false);
    assert_eq!(outcome.sequence, mark.sequence);
    assert_eq!(outcome.document()["request_id"], winning.response_id);
    assert_eq!(response_digest(&outcome.response), mark.response_digest);
    (
        outcome,
        contenders.into_iter().map(|child| child.finish()).collect(),
    )
}

fn outcome_at<'a>(probe: &'a Probe, key: &str) -> Option<&'a Outcome> {
    probe
        .outcomes
        .get(key)
        .expect("key requested in probe")
        .as_ref()
}

fn attempt_node(probe: &Probe) -> &str {
    probe.cursor.attempt.as_ref().unwrap().node().as_str()
}

fn selection_mismatch(before: &Probe, after: &Probe) -> Refusal {
    Refusal::Selection {
        expected: before.cursor.selected.clone(),
        actual: after.cursor.selected.clone(),
    }
}

fn assessment(verdict: &str, score: i64) -> Action {
    Action::Batch(vec![
        Edit {
            item: "verdict".into(),
            expected: 0,
            value: ItemValue::Choice(verdict.into()),
        },
        Edit {
            item: "stage".into(),
            expected: 0,
            value: ItemValue::Choice("scored".into()),
        },
        Edit {
            item: "score".into(),
            expected: 0,
            value: ItemValue::Number(score),
        },
    ])
}

#[test]
fn activation_contenders_use_one_commit_for_different_and_same_keys() {
    for same_key in [false, true] {
        let mut fixture = Fixture::create(&base_document(), true);
        let initial = fixture.read(&[]);
        let group = group_document();
        let win = activate(&initial, "activation", &group, vec![]);
        let contenders = if same_key {
            let replay = win.retry();
            assert_ne!(replay.response_id, win.response_id);
            vec![replay]
        } else {
            vec![
                activate(&initial, "other-activation", &group, vec![]),
                Call::new(
                    &initial,
                    "anchor-report",
                    Action::Record(text("plan", 1, "Late design write")),
                ),
            ]
        };
        let (committed, replies) = race(&mut fixture, &win, &contenders);
        let current = fixture.read(&["activation", "other-activation", "anchor-report"]);
        let mut replies = replies.into_iter();
        if same_key {
            assert_eq!(success(replies.next().unwrap(), true), committed);
        } else {
            rejection(
                replies.next().unwrap(),
                Refusal::Attempt {
                    expected: initial.cursor.attempt.as_ref().unwrap().attempt().clone(),
                    actual: Some(current.cursor.attempt.as_ref().unwrap().attempt().clone()),
                },
            );
            rejection(
                replies.next().unwrap(),
                selection_mismatch(&initial, &current),
            );
        }
        assert!(replies.next().is_none());
        assert_eq!(current.ledger, (1, 1, 1));
        assert_eq!(current.groups.len(), 1);
        assert_eq!(current.attempts.len(), 2);
        assert_eq!(attempt_node(&current), "work");
        assert_eq!(
            current.cursor.selected.as_ref(),
            Some(&execution(&current.groups[0]))
        );
        assert_eq!(outcome_at(&current, "activation"), Some(&committed));
        assert!(outcome_at(&current, "other-activation").is_none());
        assert!(outcome_at(&current, "anchor-report").is_none());
        fixture.finish();
    }
}

#[test]
fn replan_and_current_report_contend_in_both_orders() {
    let mut fixture = Fixture::create(&base_document(), true);
    let mut group = local_decision_group(true);
    group["node_definitions"]["choose"]["items"] = json!([optional_text("memo")]);
    let design = fixture.read(&[]);
    success(
        fixture.run(&activate(&design, "activate", &group, vec![])),
        false,
    );
    write(&mut fixture, "assessment", assessment("failure", 80));
    write(&mut fixture, "complete-work", Action::Complete);
    let decision = fixture.read(&[]);
    assert_eq!(attempt_node(&decision), "choose");
    assert_eq!(decision.observation.as_ref().unwrap()["decision_entry"], 1);
    let report = Call::new(
        &decision,
        "report-first",
        Action::Record(text("memo", 0, "Keep this exact assessment note")),
    );
    let stale_replan = Call::new(&decision, "stale-replan", Action::Replan);
    let (recorded, replies) = race(&mut fixture, &report, &[stale_replan]);
    let assessed = fixture.read(&["report-first", "stale-replan"]);
    rejection(
        replies.into_iter().next().unwrap(),
        Refusal::Revision {
            expected: decision.cursor.revision,
            actual: assessed.cursor.revision,
        },
    );
    assert_eq!(assessed.cursor.attempt, decision.cursor.attempt);
    assert_eq!(assessed.observation.as_ref().unwrap()["decision_entry"], 1);
    assert_eq!(outcome_at(&assessed, "report-first"), Some(&recorded));
    assert!(outcome_at(&assessed, "stale-replan").is_none());

    let replan = Call::new(&assessed, "replan-first", Action::Replan);
    let late_report = Call::new(
        &assessed,
        "late-report",
        Action::Record(text("memo", 1, "Do not retain a stale contribution")),
    );
    let (closed, replies) = race(&mut fixture, &replan, &[late_report]);
    let current = fixture.read(&["replan-first", "late-report"]);
    rejection(
        replies.into_iter().next().unwrap(),
        selection_mismatch(&assessed, &current),
    );
    assert!(current.cursor.selected.is_none());
    assert_eq!(attempt_node(&current), "design");
    assert_eq!(current.ledger, (5, 5, 5));
    assert_eq!(current.attempts.len(), 4);
    assert_eq!(current.attempts[2]["decision"]["option"], "redesign");
    assert_eq!(
        current.attempts[2]["decision"]["guards"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        current.attempts[2]["items"][0]["value"]["value"],
        "Keep this exact assessment note"
    );
    assert_eq!(current.groups[0]["outcome"], "abandoned");
    assert_eq!(
        current.groups[0]["closure"]["source"],
        json!(assessed.cursor.attempt)
    );
    assert_eq!(outcome_at(&current, "replan-first"), Some(&closed));
    assert!(outcome_at(&current, "late-report").is_none());
    assert!(
        current.observation.as_ref().unwrap()["counters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["scope"] == "base")
    );
    write(
        &mut fixture,
        "new-design",
        Action::Record(text("plan", 0, "Preserve the prior guard and note")),
    );
    let redesign = fixture.read(&[]);
    let memo = HistoricalReferenceV1::Item(HistoricalItemV1::new(
        assessed.cursor.attempt.as_ref().unwrap().attempt().clone(),
        ItemId::new("memo").unwrap(),
        podway_core::recorded_value_digest_v2(
            &podway_core::RecordedItemValueV2::text("Keep this exact assessment note").unwrap(),
        )
        .unwrap(),
    ));
    let replacement = activate(
        &redesign,
        "same-definition-replacement",
        &group,
        vec![retain(&current.groups[0], Some(memo.clone()))],
    );
    success(fixture.run(&replacement), false);
    let fresh = fixture.read(&["replan-first"]);
    assert_eq!(fresh.groups[0], current.groups[0]);
    assert_ne!(fresh.groups[0]["id"], fresh.groups[1]["id"]);
    assert_eq!(fresh.groups[0]["definition"], fresh.groups[1]["definition"]);
    assert_eq!(fresh.groups[1]["prior_work"][0]["item"], json!(memo));
    assert_eq!(
        fresh.observation.as_ref().unwrap()["next_decision_entry"],
        1
    );
    assert_eq!(outcome_at(&fresh, "replan-first"), Some(&closed));
    fixture.finish();
}

#[test]
fn contending_disjoint_item_writes_commit_and_a_same_slot_loser_is_stale() {
    let mut fixture = Fixture::create(&base_document(), true);
    let design = fixture.read(&[]);
    success(
        fixture.run(&activate(&design, "activate", &report_group(), vec![])),
        false,
    );
    let work = fixture.read(&[]);
    let alpha = Call::new(
        &work,
        "alpha",
        Action::Record(text("alpha", 0, "First slot")),
    );
    let beta = Call::new(
        &work,
        "beta",
        Action::Record(text("beta", 0, "Second slot")),
    );
    let (first, replies) = race(&mut fixture, &alpha, &[beta]);
    let second = success(replies.into_iter().next().unwrap(), false);
    assert_eq!((first.sequence, second.sequence), (2, 3));
    assert_eq!(
        second.document()["session"]["revision_before"],
        first.document()["session"]["revision_after"]
    );
    let current = fixture.read(&["alpha", "beta"]);
    assert_eq!(current.ledger, (3, 3, 3));
    assert_eq!(current.cursor.attempt, work.cursor.attempt);
    assert_eq!(outcome_at(&current, "alpha"), Some(&first));
    assert_eq!(outcome_at(&current, "beta"), Some(&second));
    let accepted = Call::new(
        &current,
        "same-winner",
        Action::Record(text("alpha", 1, "Exact winner")),
    );
    let refused = Call::new(
        &current,
        "same-loser",
        Action::Record(text("alpha", 1, "Do not persist")),
    );
    let (accepted, replies) = race(&mut fixture, &accepted, &[refused]);
    rejection(
        replies.into_iter().next().unwrap(),
        Refusal::ItemRevision {
            expected: Revision::new(1),
            actual: Revision::new(2),
        },
    );
    let current = fixture.read(&["same-winner", "same-loser"]);
    assert_eq!(current.ledger, (4, 4, 4));
    assert_eq!(current.groups.len(), 1);
    assert_eq!(current.attempts.len(), 2);
    assert_eq!(
        current.attempts[1]["items"][0]["value"]["value"],
        "Exact winner"
    );
    assert_eq!(
        current.attempts[1]["items"][1]["value"]["value"],
        "Second slot"
    );
    assert_eq!(outcome_at(&current, "same-winner"), Some(&accepted));
    assert!(outcome_at(&current, "same-loser").is_none());
    fixture.finish();
}

#[test]
fn precommit_kill_and_process_exit_roll_back_candidate_state_and_ledger() {
    for mode in [Mode::ParkBeforeCommit, Mode::ExitBeforeCommit] {
        let mut fixture = Fixture::create(&base_document(), true);
        let before = fixture.read(&["interrupted"]);
        let call = activate(&before, "interrupted", &group_document(), vec![]);
        let mut writer = fixture.launch(&call, mode);
        writer.start();
        let mark = writer.boundary("AfterLedgerBeforeCommit");
        assert_eq!(mark.sequence, 1);
        assert_eq!(fixture.read(&["interrupted"]), before);
        if mode == Mode::ExitBeforeCommit {
            writer.controlled_exit();
        } else {
            writer.kill();
        }
        let recovered = fixture.read(&["interrupted"]);
        assert_eq!(recovered, before);
        assert!(outcome_at(&recovered, "interrupted").is_none());
        let retry = call.retry();
        let committed = success(fixture.run(&retry), false);
        assert_eq!(committed.document()["request_id"], retry.response_id);
        assert_ne!(response_digest(&committed.response), mark.response_digest);
        let settled = fixture.read(&["interrupted"]);
        assert_eq!(settled.ledger, (1, 1, 1));
        assert_eq!(settled.groups.len(), 1);
        assert_eq!(settled.attempts.len(), 2);
        assert_eq!(outcome_at(&settled, "interrupted"), Some(&committed));
        fixture.finish();
    }
}

#[test]
fn postcommit_kill_preserves_original_success_after_fresh_process_advances() {
    let mut fixture = Fixture::create(&base_document(), true);
    let design = fixture.read(&[]);
    let call = activate(&design, "response-lost", &report_group(), vec![]);
    let mut writer = fixture.launch(&call, Mode::ParkAfterCommit);
    writer.start();
    let mark = writer.boundary("AfterCommitBeforeReturn");
    writer.kill();
    let recovered = fixture.read(&["response-lost"]);
    let original = outcome_at(&recovered, "response-lost").unwrap();
    assert_eq!(recovered.ledger, (1, 1, 1));
    assert_eq!(recovered.groups.len(), 1);
    assert_eq!(recovered.attempts.len(), 2);
    assert_eq!(original.sequence, mark.sequence);
    assert_eq!(original.document()["request_id"], call.response_id);
    assert_eq!(response_digest(&original.response), mark.response_digest);
    write(
        &mut fixture,
        "new-work",
        Action::Record(text("alpha", 0, "A later acknowledgment")),
    );
    write(&mut fixture, "advance", Action::Complete);
    let later = fixture.read(&["response-lost", "old-design"]);
    assert_eq!(later.ledger, (3, 3, 3));
    assert_eq!(attempt_node(&later), "finish");
    assert_eq!(outcome_at(&later, "response-lost"), Some(original));
    assert_eq!(success(fixture.run(&call.retry()), true), *original);
    let fresh_key_with_stale_target = Call::new(
        &design,
        "old-design",
        Action::Record(text("plan", 1, "An old observation is not authority")),
    );
    rejection(
        fixture.run(&fresh_key_with_stale_target),
        selection_mismatch(&design, &later),
    );
    assert_eq!(fixture.read(&["response-lost", "old-design"]), later);
    fixture.finish();
}

#[test]
fn base_review_replan_retains_the_selected_completed_group_across_processes() {
    let (base, group) = completed_group_documents();
    let mut fixture = Fixture::create(&base, true);
    let initial = fixture.read(&[]);
    success(
        fixture.run(&activate(&initial, "activate", &group, vec![])),
        false,
    );
    write(&mut fixture, "work", Action::Complete);
    write(
        &mut fixture,
        "terminal-assessment",
        assessment("failure", 80),
    );
    write(&mut fixture, "return", Action::Return);
    let review = fixture.read(&[]);
    assert_eq!(attempt_node(&review), "review");
    assert_eq!(review.groups[0]["outcome"], "completed");
    assert_eq!(review.observation.as_ref().unwrap()["decision_entry"], 1);
    let closed = write(&mut fixture, "base-replan", Action::Replan);
    let latest = fixture.read(&["base-replan"]);
    assert_eq!(latest.ledger, (5, 5, 5));
    assert!(latest.cursor.selected.is_none());
    assert_eq!(attempt_node(&latest), "design");
    assert_eq!(latest.groups[0]["terminal"], review.groups[0]["terminal"]);
    assert_eq!(latest.groups[0]["outcome"], "completed");
    assert_eq!(
        latest.groups[0]["closure"]["source"],
        json!(review.cursor.attempt)
    );
    assert_eq!(outcome_at(&latest, "base-replan"), Some(&closed));
    write(
        &mut fixture,
        "review-redesign",
        Action::Record(text("plan", 0, "Use a revised group definition")),
    );
    let design = fixture.read(&[]);
    let mut revised = group.clone();
    revised["name"] = json!("Revised group after completed review");
    let activation = activate(
        &design,
        "different-definition-replacement",
        &revised,
        vec![retain(&latest.groups[0], None)],
    );
    success(fixture.run(&activation), false);
    let new = fixture.read(&[]);
    assert_eq!(new.groups[0], latest.groups[0]);
    assert_ne!(new.groups[0]["definition"], new.groups[1]["definition"]);
    assert_eq!(new.groups[1]["predecessor"], new.groups[0]["id"]);
    assert_eq!(new.groups[1]["prior_work"][0]["group"], new.groups[0]["id"]);
    fixture.finish();
}

fn retain(group: &Value, item: Option<HistoricalReferenceV1>) -> PriorWork {
    PriorWork {
        execution: execution(group),
        item,
    }
}

fn revise(title: &str, reactivate: bool) -> Action {
    Action::ReviseGoal {
        title: title.into(),
        reactivate,
    }
}

#[test]
fn processes_preserve_goal_revisions_prior_work_and_completed_cancellation() {
    let mut fixture = Fixture::create(&tracked_base(), false);
    let group = report_group();
    write(&mut fixture, "prefix", Action::Complete);
    write(
        &mut fixture,
        "design-one",
        Action::Record(text("plan", 0, "First design")),
    );
    let design = fixture.read(&["without-goal"]);
    rejection(
        fixture.run(&activate(&design, "without-goal", &group, vec![])),
        Refusal::Domain(
            "activation requires an unselected ready anchor with its current goal".into(),
        ),
    );
    assert_eq!(fixture.read(&["without-goal"]), design);
    write(
        &mut fixture,
        "define-goal",
        Action::DefineGoal("The first goal".into()),
    );
    let design = fixture.read(&[]);
    success(
        fixture.run(&activate(&design, "run-one", &group, vec![])),
        false,
    );
    write(
        &mut fixture,
        "preserved-work",
        Action::Record(text("alpha", 0, "Partially finished work")),
    );
    let first = fixture.read(&[]);
    assert_eq!(
        first.groups[0]["working_copy_basis"],
        json!(design.working_copy_digest)
    );
    let original_value = &first.attempts[2]["items"][0]["value"];
    let value =
        podway_core::RecordedItemValueV2::text(original_value["value"].as_str().unwrap()).unwrap();
    let exact = HistoricalReferenceV1::Item(HistoricalItemV1::new(
        first.cursor.attempt.as_ref().unwrap().attempt().clone(),
        ItemId::new("alpha").unwrap(),
        podway_core::recorded_value_digest_v2(&value).unwrap(),
    ));

    fixture.change_work(b"First run kept this work.\n");
    write(
        &mut fixture,
        "revise-one",
        revise("Goal with retained work", false),
    );
    let after_one = fixture.read(&[]);
    assert_eq!(after_one.cursor.goal, Some(2));
    assert_eq!(after_one.groups[0]["outcome"], "abandoned");
    assert_eq!(after_one.groups[0]["closure"]["cause"], "goal_revision");
    assert_eq!(
        after_one.groups[0]["closure"]["feedback"]["basis"],
        json!(after_one.working_copy_digest)
    );
    assert_eq!(after_one.attempts[2]["items"][0]["value"], *original_value);
    write(
        &mut fixture,
        "design-two",
        Action::Record(text("plan", 0, "Second design")),
    );
    let design_two = fixture.read(&[]);
    success(
        fixture.run(&activate(
            &design_two,
            "run-two",
            &group,
            vec![retain(&after_one.groups[0], Some(exact.clone()))],
        )),
        false,
    );
    let empty = fixture.read(&[]);
    assert_ne!(empty.groups[0]["id"], empty.groups[1]["id"]);
    assert_eq!(empty.groups[0]["definition"], empty.groups[1]["definition"]);
    assert_eq!(
        empty.groups[1]["working_copy_basis"],
        json!(after_one.working_copy_digest)
    );
    assert_ne!(
        empty.groups[0]["working_copy_basis"],
        empty.groups[1]["working_copy_basis"]
    );
    assert!(
        empty.attempts.last().unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|slot| slot["value"].is_null())
    );
    write(
        &mut fixture,
        "revise-two",
        revise("Goal around remaining work", false),
    );
    write(
        &mut fixture,
        "design-three",
        Action::Record(text("plan", 0, "Third design")),
    );
    let design_three = fixture.read(&[]);
    let mut different = group.clone();
    different["name"] = json!("Replacement execution definition");
    success(
        fixture.run(&activate(
            &design_three,
            "run-three",
            &different,
            vec![retain(&after_one.groups[0], Some(exact.clone()))],
        )),
        false,
    );
    let third = fixture.read(&[]);
    assert_eq!(third.cursor.goal, Some(3));
    assert_ne!(third.groups[2]["definition"], third.groups[1]["definition"]);
    assert_eq!(third.groups[2]["predecessor"], third.groups[1]["id"]);
    assert_eq!(
        third.groups[2]["prior_work"],
        json!([{"group":third.groups[0]["id"], "item":exact}])
    );
    write(&mut fixture, "work-three", Action::Complete);
    write(
        &mut fixture,
        "report-three",
        Action::Record(text("report", 0, "First terminal output")),
    );
    write(&mut fixture, "return-three", Action::Return);
    write(&mut fixture, "achieve", Action::DecideAchieved);
    write(&mut fixture, "complete", Action::Complete);
    let finished = fixture.read(&["reactivation-required"]);
    assert!(finished.cursor.attempt.is_none());
    assert_eq!(finished.goal_outcome.as_deref(), Some("achieved"));
    assert_eq!(
        finished.observation.as_ref().unwrap()["lifecycle"],
        "completed"
    );
    assert_eq!(finished.groups[2]["closure"]["cause"], "completion");
    rejection(
        fixture.run(&Call::new(
            &finished,
            "reactivation-required",
            revise("New completed-session goal", false),
        )),
        Refusal::Domain(
            "goal revision requires a running session or explicit completed-session reactivation"
                .into(),
        ),
    );
    assert_eq!(fixture.read(&["reactivation-required"]), finished);
    write(
        &mut fixture,
        "reactivate",
        revise("Explicitly reactivated goal", true),
    );
    let reactivated = fixture.read(&[]);
    assert_eq!(reactivated.cursor.goal, Some(4));
    assert!(reactivated.goal_outcome.is_none());
    assert_eq!(attempt_node(&reactivated), "design");
    assert_eq!(reactivated.goal_history[2]["cause"], "reactivation");
    assert_eq!(
        reactivated.goal_history[2]["predecessor"],
        finished.groups[2]["id"]
    );
    assert_eq!(reactivated.groups[2], finished.groups[2]);
    write(
        &mut fixture,
        "design-four",
        Action::Record(text("plan", 0, "Reactivated design")),
    );
    let design_four = fixture.read(&[]);
    success(
        fixture.run(&activate(
            &design_four,
            "run-four",
            &different,
            vec![retain(&finished.groups[2], None)],
        )),
        false,
    );
    write(&mut fixture, "work-four", Action::Complete);
    write(
        &mut fixture,
        "report-four",
        Action::Record(text("report", 0, "Cancellation retains this")),
    );
    write(&mut fixture, "return-four", Action::Return);
    let completed = fixture.read(&[]);
    write(&mut fixture, "cancel", Action::Cancel);
    let cancelled = fixture.read(&["cancel"]);
    assert!(cancelled.cursor.attempt.is_none());
    assert!(cancelled.cursor.selected.is_none());
    assert_eq!(
        cancelled.observation.as_ref().unwrap()["lifecycle"],
        "cancelled"
    );
    assert_eq!(cancelled.groups[3]["outcome"], "completed");
    assert_eq!(
        cancelled.groups[3]["terminal"],
        completed.groups[3]["terminal"]
    );
    assert_eq!(cancelled.groups[3]["closure"]["cause"], "cancel");
    assert_eq!(
        cancelled.groups[3]["closure"]["source"],
        json!(completed.cursor.attempt)
    );
    let ledger = cancelled.ledger;
    assert_eq!(ledger.0, ledger.1);
    assert_eq!(ledger.0, ledger.2);
    assert!(outcome_at(&cancelled, "cancel").is_some());
    fixture.finish();
}

#[test]
fn combined_counter_and_guidance_budget_rejects_before_effects_in_a_process() {
    let mut base = base_document();
    add_counter_chain(&mut base, "review", "close", 900);
    let mut group = group_document();
    inflate_current_guidance(&mut group, "work");
    let mut fixture = Fixture::create(&base, true);
    let before = fixture.read(&["over-budget"]);
    assert!(before.observation.is_ok());
    let rejected = activate(&before, "over-budget", &group, vec![]);
    rejection(fixture.run(&rejected), Refusal::ResponseBudget);
    assert_eq!(fixture.read(&["over-budget"]), before);
    let admitted = success(
        fixture.run(&activate(
            &before,
            "within-budget",
            &group_document(),
            vec![],
        )),
        false,
    );
    let after = fixture.read(&["over-budget", "within-budget"]);
    assert_eq!(after.ledger, (1, 1, 1));
    assert_eq!(after.groups.len(), 1);
    assert!(outcome_at(&after, "over-budget").is_none());
    assert_eq!(outcome_at(&after, "within-budget"), Some(&admitted));
    let observed = after.observation.as_ref().unwrap();
    assert_eq!(observed["counters"].as_array().unwrap().len(), 905);
    fixture.finish();
}
