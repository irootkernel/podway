//! Bounded retrospective reads against an actual candidate Store.

use super::int_group002_transactions::{
    Fixture, TIME, activation, base_document, canonical, definition, mutation,
};
use super::int_group003_replan::{assessment, complete, feedback, local_decision_group};
use super::int_group004_fences::tracked_base;
use podway_core::execution_groups::{
    ExecutionGoalChangeV3, ExecutionGoalV3, GroupClosureFeedbackV1, GroupExecutionIdV1,
    GroupItemWriteV1, PriorWorkChoiceV1, PriorWorkDispositionV1, ScopedAttemptV1,
};
use podway_core::{
    CheckResultInputBasisV2, CriterionId, DomainError, GoalCriterionV2, GoalDefinitionV2,
    GoalOutcome, GoalStatementV2, GraphNodeId, ItemId, OptionId, ReasonV2, RecordedItemValueV2,
    Revision, canonicalize_json_v1, recorded_value_digest_v2,
};
use podway_engine::direct_execution::NativeDirectIdSourceV1;
use podway_engine::successor_execution::*;
use podway_store::successor_store::{CandidateStoreError, persist_candidate_execution};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn page(limit: usize, continuation: Option<&str>) -> SuccessorHistoryPage<'_> {
    SuccessorHistoryPage {
        limit,
        continuation,
    }
}

fn evidence_item(item: &str, revision: u64, value: RecordedItemValueV2, scalar: Value) -> Value {
    json!({
        "binding": {
            "item": item, "revision": revision,
            "value_digest": recorded_value_digest_v2(&value).unwrap(),
        },
        "scalar": scalar,
    })
}

fn assessment_items(verdict: &str, stage: &str, score: i64, score_revision: u64) -> [Value; 3] {
    [
        evidence_item(
            "verdict",
            1,
            RecordedItemValueV2::choice(verdict).unwrap(),
            json!(verdict),
        ),
        evidence_item(
            "stage",
            1,
            RecordedItemValueV2::choice(stage).unwrap(),
            json!(stage),
        ),
        evidence_item(
            "score",
            score_revision,
            RecordedItemValueV2::integer(score),
            json!(score),
        ),
    ]
}

fn list(
    fixture: &mut Fixture,
    location: CandidateSessionLocation,
    page: SuccessorHistoryPage<'_>,
) -> Result<Value, SuccessorExecutionError> {
    let result = list_successor_groups(
        &mut fixture.store,
        SuccessorHistorySession {
            workspace: &fixture.workspace,
            session: &fixture.session,
            location,
        },
        page,
    )?;
    assert!(result.as_str().len() <= podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1);
    Ok(serde_json::from_str(result.as_str()).unwrap())
}

fn show(
    fixture: &mut Fixture,
    location: CandidateSessionLocation,
    execution: &GroupExecutionIdV1,
    page: SuccessorHistoryPage<'_>,
) -> Result<Value, SuccessorExecutionError> {
    let result = show_successor_group(
        &mut fixture.store,
        SuccessorHistorySession {
            workspace: &fixture.workspace,
            session: &fixture.session,
            location,
        },
        execution,
        page,
    )?;
    assert!(result.as_str().len() <= podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1);
    Ok(serde_json::from_str(result.as_str()).unwrap())
}

fn goal_revisions(
    fixture: &mut Fixture,
    location: CandidateSessionLocation,
    page: SuccessorHistoryPage<'_>,
) -> Value {
    let result = list_successor_goal_revisions(
        &mut fixture.store,
        SuccessorHistorySession {
            workspace: &fixture.workspace,
            session: &fixture.session,
            location,
        },
        page,
    )
    .unwrap();
    serde_json::from_str(result.as_str()).unwrap()
}

fn logical_store(fixture: &Fixture) -> Vec<Vec<rusqlite::types::Value>> {
    let connection = Connection::open(&fixture.file).unwrap();
    let mut output = Vec::new();
    for table in [
        "workspace_state",
        "writer_contract",
        "successor_definitions",
        "successor_sessions",
        "successor_activations",
        "successor_attempts",
        "successor_goal_revisions",
        "mutation_outcomes",
        "operational_journal",
    ] {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        output.extend(
            statement
                .query_map([], |row| {
                    (0..columns).map(|column| row.get(column)).collect()
                })
                .unwrap()
                .map(Result::unwrap),
        );
    }
    output
}

fn issue(fixture: &mut Fixture, key: &str, operation: SuccessorCursorMutation) {
    let (state, _) = fixture.state();
    issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &state, key),
        operation,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
}

fn write_item(fixture: &mut Fixture, key: &str, item: &str, value: RecordedItemValueV2) {
    let (state, _) = fixture.state();
    let request = item_request(fixture, &state, key);
    record_successor_item(
        &mut fixture.store,
        request,
        GroupItemWriteV1 {
            item: ItemId::new(item).unwrap(),
            expected_revision: Revision::ZERO,
            value: Some(value),
        },
        || TIME,
    )
    .unwrap();
}

fn goal(statement: &str) -> ExecutionGoalV3 {
    ExecutionGoalV3 {
        statement: GoalStatementV2::new(statement).unwrap(),
        criteria: GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(CriterionId::new("result").unwrap(), "Record the result.")
                .unwrap(),
        ])
        .unwrap(),
    }
}

fn change_goal(fixture: &mut Fixture, key: &str, reactivate: bool) -> ScopedAttemptV1 {
    let (state, _) = fixture.state();
    let basis = CheckResultInputBasisV2::new(key, state.base_digest().clone()).unwrap();
    issue(
        fixture,
        key,
        SuccessorCursorMutation::ReviseGoal(ExecutionGoalChangeV3 {
            goal: goal("Evaluate the updated plan."),
            target: GraphNodeId::new("design").unwrap(),
            feedback: GroupClosureFeedbackV1::new(
                "Reassess the retained execution.".into(),
                basis,
                "Use the admitted working-copy basis.".into(),
            )
            .unwrap(),
            reactivate,
        }),
    );
    fixture.state().0.active_attempt().unwrap().target().clone()
}

#[test]
fn goal_lineage_keeps_closures_and_boundary_decisions_across_reactivation() {
    use CandidateSessionLocation::{Archived, Current};

    let confirmation = json!({"id":"confirmed","type":"confirm","prompt":"Confirm the planning basis.","required":true});
    let mut base = tracked_base();
    base["node_definitions"]["design"]["items"]
        .as_array_mut()
        .unwrap()
        .push(confirmation.clone());
    base["execution_group"]["plan_inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!("confirmed"));
    let mut group = local_decision_group(true);
    group["plan_inputs"]
        .as_array_mut()
        .unwrap()
        .push(confirmation);
    group["graph"]["nodes"][1]["evidence_from"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "scope":"plan","required":true,"items":["confirmed"]
        }));
    let group = canonical(&group);
    let mut fixture = Fixture::new(&base, false);
    issue(
        &mut fixture,
        "enter-design",
        SuccessorCursorMutation::Complete,
    );
    issue(
        &mut fixture,
        "define-goal",
        SuccessorCursorMutation::DefineGoal(goal("Evaluate the first plan.")),
    );
    let first_source = change_goal(&mut fixture, "before-first-activation", false);
    write_item(
        &mut fixture,
        "first-plan",
        "plan",
        RecordedItemValueV2::text("Review a proposal.").unwrap(),
    );
    write_item(
        &mut fixture,
        "first-confirm",
        "confirmed",
        RecordedItemValueV2::confirm(),
    );
    let (design, _) = fixture.state();
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &group, "first-activation"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (mut work, sources) = fixture.state();
    let first_group = work.selected().unwrap().identity().execution().clone();
    assessment(&mut work, "failure", "scored", Some(80));
    complete(&mut work);
    fixture.edit_fixture(&work, &sources);
    let replan = feedback(&work, "redesign");
    replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &work, "redesign"),
        replan.clone(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let closed =
        show(&mut fixture, Current, &first_group, page(64, None)).unwrap()["group"]["closure"]
            .clone();
    let revision_source = fixture.state().0.active_attempt().unwrap().target().clone();
    let revised_design = change_goal(&mut fixture, "after-replan", false);

    write_item(
        &mut fixture,
        "second-plan",
        "plan",
        RecordedItemValueV2::text("Review the revised proposal.").unwrap(),
    );
    write_item(
        &mut fixture,
        "second-confirm",
        "confirmed",
        RecordedItemValueV2::confirm(),
    );
    let (design, _) = fixture.state();
    let mut request = activation(&fixture.workspace, &design, &group, "second-activation");
    request.prior_work.push(
        PriorWorkDispositionV1::new(
            first_group.clone(),
            None,
            PriorWorkChoiceV1::Revalidate,
            "Recheck the first proposal.".into(),
        )
        .unwrap(),
    );
    let activation_basis = request.working_copy_basis.clone();
    activate_group(
        &mut fixture.store,
        request,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (mut work, sources) = fixture.state();
    let second_group = work.selected().unwrap().identity().execution().clone();
    let work_attempt = work.active_attempt().unwrap().target().clone();
    assessment(&mut work, "success", "not-scored", Some(0));
    complete(&mut work);
    fixture.edit_fixture(&work, &sources);
    issue(
        &mut fixture,
        "finish-group",
        SuccessorCursorMutation::Decide {
            option: OptionId::new("finish").unwrap(),
            reason: ReasonV2::new("Accept the group result.").unwrap(),
            goal_outcome: None,
        },
    );
    write_item(
        &mut fixture,
        "report",
        "report",
        RecordedItemValueV2::text("Retained terminal result.").unwrap(),
    );
    let (terminal, _) = fixture.state();
    let terminal_attempt = terminal.active_attempt().unwrap().target().clone();
    return_from_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &terminal, "return"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let review = fixture.state().0.active_attempt().unwrap().target().clone();
    issue(
        &mut fixture,
        "accept-goal",
        SuccessorCursorMutation::Decide {
            option: OptionId::new("accept").unwrap(),
            reason: ReasonV2::new("Accept the recorded result.").unwrap(),
            goal_outcome: Some(GoalOutcome::Achieved),
        },
    );
    let close = fixture.state().0.active_attempt().unwrap().target().clone();
    issue(
        &mut fixture,
        "complete-session",
        SuccessorCursorMutation::Complete,
    );
    let before_reactivation = show(&mut fixture, Current, &second_group, page(64, None)).unwrap();
    assert_eq!(
        before_reactivation["group"]["closure"]["cause"],
        "completion"
    );
    let reactivated_design = change_goal(&mut fixture, "after-completion", true);

    let before_reads = logical_store(&fixture);
    let first_page = goal_revisions(&mut fixture, Current, page(1, None));
    assert_eq!(first_page["total"], 3);
    let first = &first_page["rows"][0];
    assert_eq!(first["goal_revision"], 2);
    assert_eq!(first["predecessor"], Value::Null);
    assert_eq!(first["successor"], json!(first_source));
    let rest = goal_revisions(
        &mut fixture,
        Current,
        page(64, first_page["next_page_token"].as_str()),
    );
    assert_eq!(rest["total"], 3);
    assert_eq!(rest["truncated"], false);
    for (index, (cause, predecessor, source, successor, descriptor)) in [
        (
            "goal_revision",
            &first_group,
            &revision_source,
            &revised_design,
            "after-replan",
        ),
        (
            "reactivation",
            &second_group,
            &close,
            &reactivated_design,
            "after-completion",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let row = &rest["rows"][index];
        assert_eq!(row["goal_revision"], index + 3);
        assert_eq!(row["predecessor"], json!(predecessor));
        assert_eq!(row["successor"], json!(successor));
        assert_eq!(row["closure"]["cause"], cause);
        assert_eq!(row["closure"]["source"], json!(source));
        assert_eq!(
            row["closure"]["feedback"]["reason"],
            "Reassess the retained execution."
        );
        assert_eq!(
            row["closure"]["feedback"]["working_copy_basis"]["descriptor"],
            descriptor
        );
        assert_eq!(
            row["closure"]["feedback"]["text"],
            "Use the admitted working-copy basis."
        );
    }
    assert_eq!(
        show(&mut fixture, Current, &first_group, page(64, None)).unwrap()["group"]["closure"],
        closed
    );
    let retained = show(&mut fixture, Current, &second_group, page(64, None)).unwrap();
    assert_eq!(retained["group"], before_reactivation["group"]);
    assert_eq!(retained["group"]["summary"]["outcome"], "completed");
    assert_eq!(
        retained["group"]["summary"]["definition_digest"],
        json!(group.digest())
    );
    assert_eq!(
        retained["group"]["terminal_attempt"],
        json!(terminal_attempt.attempt())
    );
    assert_eq!(
        retained["group"]["working_copy_basis"],
        json!(activation_basis)
    );
    assert_eq!(
        retained["group"]["plan"]["design_attempt"],
        json!(revised_design.attempt())
    );
    assert_eq!(
        retained["rows"][0]["assessment"],
        json!([
            {"source": work_attempt, "items": assessment_items("success", "not-scored", 0, 1)},
            {"source": revised_design, "items": [
                evidence_item("confirmed", 1, RecordedItemValueV2::confirm(), json!(true)),
            ]},
        ])
    );
    let base_choice = &retained["rows"][1];
    assert_eq!(base_choice["attempt"], json!(review));
    assert_eq!(base_choice["choice"], "accept");
    assert_eq!(
        base_choice["goal_assessment"],
        json!({"revision": 3, "outcome": "achieved"})
    );
    assert_eq!(
        base_choice["assessment"],
        json!([{
            "source": terminal_attempt,
            "items": [evidence_item("report", 1,
                RecordedItemValueV2::text("Retained terminal result.").unwrap(), Value::Null)],
        }])
    );
    assert_eq!(logical_store(&fixture), before_reads);

    issue(
        &mut fixture,
        "cancel-after-reactivation",
        SuccessorCursorMutation::Cancel(ReasonV2::new("End the candidate fixture.").unwrap()),
    );
    let revision = fixture.state().0.fence().revision;
    fixture
        .store
        .archive_fixture_session(&fixture.workspace, &fixture.session, revision, definition)
        .unwrap();
    fixture.reopen();
    let archived = goal_revisions(&mut fixture, Archived, page(64, None));
    assert_eq!(archived["rows"][0], *first);
    assert_eq!(
        archived["rows"].as_array().unwrap()[1..],
        rest["rows"].as_array().unwrap()[..]
    );
}

fn item_request(
    fixture: &Fixture,
    state: &podway_core::execution_groups::ExecutionSessionV3,
    key: &str,
) -> SuccessorItemRequest {
    let request = mutation(&fixture.workspace, state, key);
    SuccessorItemRequest {
        workspace: request.workspace,
        key: request.key,
        request_id: request.request_id,
        target: request.cursor.execution.attempt.unwrap(),
        selected_group: request.cursor.selected_group,
    }
}

#[test]
fn exact_group_history_survives_replacement_cancellation_and_archive() {
    use CandidateSessionLocation::{Archived, Current};

    let mut fixture = Fixture::new(&base_document(), true);
    let empty = list(&mut fixture, Current, page(64, None)).unwrap();
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["rows"], json!([]));
    assert_eq!(empty["next_page_token"], Value::Null);
    for limit in [0, 65] {
        assert!(matches!(
            list(&mut fixture, Current, page(limit, None)),
            Err(SuccessorExecutionError::InvalidHistoryPageLimit)
        ));
    }
    assert!(matches!(
        list(&mut fixture, Archived, page(1, None)),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Domain(
            DomainError::SessionIdentityMismatch { actual: None, .. }
        )))
    ));
    assert!(matches!(
        list(&mut fixture, Current, page(1, Some("not-a-token"))),
        Err(SuccessorExecutionError::InvalidHistoryPageToken)
    ));

    let group = canonical(&local_decision_group(true));
    let (design, _) = fixture.state();
    let first_design = design.plan_binding().unwrap();
    assert!(matches!(
        fixture.store.archive_fixture_session(
            &fixture.workspace,
            &fixture.session,
            design.fence().revision,
            definition
        ),
        Err(CandidateStoreError::ArchiveRequiresTerminalSession)
    ));
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &group, "initial"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();

    let (mut running, sources) = fixture.state();
    let first = running.selected().unwrap().identity().execution().clone();
    let assessment_attempt = running.active_attempt().unwrap().target().clone();
    assessment(&mut running, "failure", "scored", Some(79));
    running
        .record_item(
            &assessment_attempt,
            &ItemId::new("score").unwrap(),
            Revision::new(1),
            Some(RecordedItemValueV2::integer(80)),
            TIME,
        )
        .unwrap();
    complete(&mut running);
    fixture.edit_fixture(&running, &sources);
    issue(
        &mut fixture,
        "retry-choice",
        SuccessorCursorMutation::Retry(
            ReasonV2::new("Re-enter the recorded Decision after review.").unwrap(),
        ),
    );
    let (running, _) = fixture.state();
    let recorded_choice = feedback(&running, "redesign");
    replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &running, "redesign"),
        recorded_choice.clone(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();

    let (mut redesigned, sources) = fixture.state();
    let anchor = redesigned.active_attempt().unwrap().target().clone();
    redesigned
        .record_item(
            &anchor,
            &ItemId::new("plan").unwrap(),
            Revision::ZERO,
            Some(RecordedItemValueV2::text("Use a fresh implementation arrangement.").unwrap()),
            TIME,
        )
        .unwrap();
    fixture.edit_fixture(&redesigned, &sources);
    let mut replacement = activation(&fixture.workspace, &redesigned, &group, "replacement");
    replacement.prior_work.push(
        PriorWorkDispositionV1::new(
            first.clone(),
            None,
            PriorWorkChoiceV1::Revalidate,
            "Reverify the previous assessment basis.".into(),
        )
        .unwrap(),
    );
    activate_group(
        &mut fixture.store,
        replacement,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();

    let (selected, _) = fixture.state();
    let second = selected.selected().unwrap().identity().execution().clone();
    assert_ne!(first, second);
    let before_write = list(&mut fixture, Current, page(1, None)).unwrap();
    write_item(
        &mut fixture,
        "second-verdict",
        "verdict",
        RecordedItemValueV2::choice("success").unwrap(),
    );
    assert!(matches!(
        list(
            &mut fixture,
            Current,
            page(1, before_write["next_page_token"].as_str())
        ),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    write_item(
        &mut fixture,
        "second-stage",
        "stage",
        RecordedItemValueV2::choice("not-scored").unwrap(),
    );
    write_item(
        &mut fixture,
        "second-score",
        "score",
        RecordedItemValueV2::integer(0),
    );
    issue(
        &mut fixture,
        "second-assessed",
        SuccessorCursorMutation::Complete,
    );
    let (selected, _) = fixture.state();
    let before_reads = logical_store(&fixture);
    let first_page = list(&mut fixture, Current, page(1, None)).unwrap();
    assert_eq!(first_page["total"], 2);
    assert_eq!(first_page["truncated"], true);
    assert_eq!(first_page["rows"][0]["activation_sequence"], 1);
    assert_eq!(first_page["rows"][0]["execution"], json!(first));
    assert_eq!(
        first_page["rows"][0]["design_attempt"],
        json!(first_design.design_attempt())
    );
    assert_eq!(first_page["rows"][0]["decision_entries_total"], 2);
    assert_eq!(first_page["rows"][0]["predecessor"], Value::Null);
    assert_eq!(first_page["rows"][0]["closure_cause"], "replan");
    let token = first_page["next_page_token"].as_str().unwrap();
    let (binding, _) = token.rsplit_once(':').unwrap();
    for offset in [0, 2] {
        let invalid = format!("{binding}:{offset:016x}");
        assert!(matches!(
            list(&mut fixture, Current, page(1, Some(&invalid))),
            Err(SuccessorExecutionError::InvalidHistoryPageToken)
        ));
    }
    let second_page = list(&mut fixture, Current, page(1, Some(token))).unwrap();
    assert_eq!(second_page["total"], 2);
    assert_eq!(second_page["truncated"], false);
    assert_eq!(second_page["next_page_token"], Value::Null);
    assert_eq!(second_page["rows"][0]["activation_sequence"], 2);
    assert_eq!(second_page["rows"][0]["execution"], json!(second));
    assert_eq!(second_page["rows"][0]["predecessor"], json!(first));
    assert_eq!(
        first_page["rows"][0]["definition_digest"],
        second_page["rows"][0]["definition_digest"]
    );
    assert_eq!(second_page["rows"][0]["decision_entries_total"], 1);
    assert_eq!(
        second_page["rows"][0]["definition_digest"],
        json!(group.digest())
    );
    let second_design = redesigned.plan_binding().unwrap();
    assert_eq!(
        second_page["rows"][0]["design_attempt"],
        json!(second_design.design_attempt())
    );
    let pending = show(&mut fixture, Current, &second, page(1, None)).unwrap();
    assert_eq!(
        pending["group"]["plan"]["item_set_digest"],
        json!(second_design.item_set_digest())
    );
    assert_eq!(pending["rows"][0]["choice"], Value::Null);
    assert!(matches!(
        show(
            &mut fixture,
            Current,
            &GroupExecutionIdV1::new("0b3c063d-d778-4641-9646-54a7e81ad195").unwrap(),
            page(1, None)
        ),
        Err(SuccessorExecutionError::HistoryGroupNotFound)
    ));

    assert!(matches!(
        show(&mut fixture, Current, &first, page(1, Some(token))),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let mut other = Fixture::new(&base_document(), true);
    assert!(matches!(
        list(&mut other, Current, page(1, Some(token))),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let detail_first = show(&mut fixture, Current, &first, page(1, None)).unwrap();
    assert_eq!(detail_first["total"], 2);
    assert_eq!(detail_first["truncated"], true);
    assert_eq!(detail_first["rows"][0]["entry_ordinal"], 1);
    assert_eq!(detail_first["rows"][0]["choice"], Value::Null);
    let detail_token = detail_first["next_page_token"].as_str().unwrap();
    assert!(matches!(
        show(&mut fixture, Current, &second, page(1, Some(detail_token))),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let detail = show(&mut fixture, Current, &first, page(1, Some(detail_token))).unwrap();
    assert_eq!(detail["truncated"], false);
    assert_eq!(detail["next_page_token"], Value::Null);
    assert_eq!(
        detail["group"]["plan"]["item_set_digest"],
        json!(first_design.item_set_digest())
    );
    assert_eq!(detail["group"]["closure"]["cause"], "replan");
    assert_eq!(
        detail["group"]["closure"]["source"],
        json!(recorded_choice.decision())
    );
    assert_eq!(
        detail["group"]["closure"]["feedback"]["text"],
        recorded_choice.feedback()
    );
    assert_eq!(detail["total"], 2);
    assert_eq!(detail["rows"][0]["entry_ordinal"], 2);
    assert_eq!(detail["rows"][0]["choice"], "redesign");
    assert_eq!(detail["rows"][0]["reason"], recorded_choice.reason());
    let expected_items = assessment_items("failure", "scored", 80, 2);
    assert_eq!(
        detail["rows"][0]["assessment"],
        json!([{
            "source": assessment_attempt, "items": expected_items,
        }])
    );
    assert_eq!(
        detail["rows"][0]["guard_bindings"],
        json!(expected_items.map(|item| json!({
            "source": assessment_attempt, "binding": item["binding"],
        })))
    );
    assert_eq!(logical_store(&fixture), before_reads);
    assert_eq!(fixture.state().0, selected);

    let cancel_reason = ReasonV2::new("End fixture execution.").unwrap();
    let original_cancel = issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &selected, "cancel"),
        SuccessorCursorMutation::Cancel(cancel_reason.clone()),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(matches!(
        list(&mut fixture, Current, page(1, Some(token))),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let (terminal_state, terminal_sources) = fixture.state();
    let terminal_revision = terminal_state.fence().revision;
    assert!(matches!(
        fixture.store.archive_fixture_session(
            &fixture.workspace, &fixture.session, selected.fence().revision, definition
        ),
        Err(CandidateStoreError::ArchiveRevisionMismatch { actual, .. }) if actual == terminal_revision
    ));
    let terminal = list(&mut fixture, Current, page(1, None)).unwrap();
    let before_archive = fixture.ledger();
    fixture
        .store
        .archive_fixture_session(
            &fixture.workspace,
            &fixture.session,
            terminal_revision,
            definition,
        )
        .unwrap();
    fixture.reopen();
    assert_eq!(fixture.ledger(), before_archive);
    assert!(matches!(
        list(&mut fixture, Current, page(1, None)),
        Err(SuccessorExecutionError::Store(
            CandidateStoreError::ArchivedSession
        ))
    ));
    let before_refusals = logical_store(&fixture);
    let mut raw = Connection::open(&fixture.file).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    let raw_write = raw.transaction().unwrap();
    assert!(matches!(
        persist_candidate_execution(&raw_write, &terminal_state, &terminal_sources),
        Err(CandidateStoreError::ArchivedSession)
    ));
    drop(raw_write);
    let late_item = item_request(&fixture, &selected, "late-item");
    assert!(matches!(
        record_successor_item(
            &mut fixture.store,
            late_item,
            GroupItemWriteV1 {
                item: ItemId::new("score").unwrap(),
                expected_revision: Revision::ZERO,
                value: None,
            },
            || TIME
        ),
        Err(SuccessorExecutionError::Store(
            CandidateStoreError::ArchivedSession
        ))
    ));
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            activation(&fixture.workspace, &redesigned, &group, "late-activation"),
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Store(
            CandidateStoreError::ArchivedSession
        ))
    ));
    assert_eq!(logical_store(&fixture), before_refusals);
    assert!(matches!(
        list(
            &mut fixture,
            Archived,
            page(1, terminal["next_page_token"].as_str())
        ),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));

    let archived_counts = fixture.ledger();
    let replay = issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &selected, "cancel"),
        SuccessorCursorMutation::Cancel(cancel_reason.clone()),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(replay.replayed());
    assert_eq!(
        replay.outcome().response(),
        original_cancel.outcome().response()
    );
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            mutation(&fixture.workspace, &selected, "late-cancel"),
            SuccessorCursorMutation::Cancel(cancel_reason),
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::Store(
            CandidateStoreError::ArchivedSession
        ))
    ));
    let archived = list(&mut fixture, Archived, page(1, None)).unwrap();
    assert_eq!(archived["rows"], terminal["rows"]);
    let remaining = list(
        &mut fixture,
        Archived,
        page(1, archived["next_page_token"].as_str()),
    )
    .unwrap();
    assert_eq!(remaining["rows"][0]["closure_cause"], "cancel");
    assert_eq!(remaining["rows"][0]["predecessor"], json!(first));
    let archived_first = show(&mut fixture, Archived, &first, page(1, None)).unwrap();
    let archived_detail = show(
        &mut fixture,
        Archived,
        &first,
        page(1, archived_first["next_page_token"].as_str()),
    )
    .unwrap();
    assert_eq!(archived_detail["rows"], detail["rows"]);
    assert_eq!(archived_detail["group"], detail["group"]);
    assert_eq!(fixture.ledger(), archived_counts);
}

#[test]
fn unguarded_choice_retains_its_exact_assessment_across_rework_and_archive() {
    use CandidateSessionLocation::{Archived, Current};

    let mut fixture = Fixture::new(&base_document(), true);
    let mut group = local_decision_group(true);
    group["manual_rework"] = json!({"allowed_targets": ["work"]});
    let group = canonical(&group);
    let (design, _) = fixture.state();
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &group, "activate"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();

    let mut sources = Vec::new();
    for (index, (stage, score)) in [("not-scored", 0), ("lightweight", 12)]
        .into_iter()
        .enumerate()
    {
        let (mut working, definitions) = fixture.state();
        sources.push(working.active_attempt().unwrap().target().clone());
        assessment(&mut working, "success", stage, Some(score));
        complete(&mut working);
        fixture.edit_fixture(&working, &definitions);
        issue_cursor_mutation(
            &mut fixture.store,
            mutation(&fixture.workspace, &working, &format!("finish-{index}")),
            SuccessorCursorMutation::Decide {
                option: OptionId::new("finish").unwrap(),
                reason: ReasonV2::new("Finish on the observed assessment.").unwrap(),
                goal_outcome: None,
            },
            || TIME,
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        if index == 0 {
            let (finishing, _) = fixture.state();
            issue_cursor_mutation(
                &mut fixture.store,
                mutation(&fixture.workspace, &finishing, "rework"),
                SuccessorCursorMutation::Rework {
                    target: GraphNodeId::new("work").unwrap(),
                    reason: ReasonV2::new("Recheck the earlier assessment.").unwrap(),
                },
                || TIME,
                &NativeDirectIdSourceV1,
            )
            .unwrap();
        }
    }

    let (finishing, _) = fixture.state();
    let execution = finishing.selected().unwrap().identity().execution().clone();
    let shown = show(&mut fixture, Current, &execution, page(64, None)).unwrap();
    assert_eq!(shown["total"], 2);
    for (index, (stage, score)) in [("not-scored", 0), ("lightweight", 12)]
        .into_iter()
        .enumerate()
    {
        let row = &shown["rows"][index];
        assert_eq!(row["entry_ordinal"], index + 1);
        assert_eq!(row["choice"], "finish");
        assert_eq!(row["guard_bindings"], json!([]));
        assert_eq!(
            row["assessment"],
            json!([{
                "source": sources[index], "items": assessment_items("success", stage, score, 1),
            }])
        );
    }

    issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &finishing, "cancel"),
        SuccessorCursorMutation::Cancel(ReasonV2::new("End fixture execution.").unwrap()),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let revision = fixture.state().0.fence().revision;
    fixture
        .store
        .archive_fixture_session(&fixture.workspace, &fixture.session, revision, definition)
        .unwrap();
    fixture.reopen();
    let archived = show(&mut fixture, Archived, &execution, page(64, None)).unwrap();
    assert_eq!(archived["rows"], shown["rows"]);

    // A valid later attempt of the same node and a matching row checksum do not
    // establish the first Decision's source. Cold admission must replay that binding.
    let decision = shown["rows"][0]["attempt"]["attempt"].as_str().unwrap();
    let connection = Connection::open(&fixture.file).unwrap();
    let retained: String = connection
        .query_row(
            "SELECT record_json FROM successor_attempts WHERE session_id = ?1 AND attempt_id = ?2",
            params![fixture.session.as_str(), decision],
            |row| row.get(0),
        )
        .unwrap();
    let mut retained: Value = serde_json::from_str(&retained).unwrap();
    retained["decision"]["evidence_sources"][0] = json!(sources[1]);
    let altered = canonicalize_json_v1(&retained).unwrap();
    let checksum = format!("sha256:{:x}", Sha256::digest(altered.as_bytes()));
    connection
        .execute(
            "UPDATE successor_attempts SET record_json = ?1, record_digest = ?2
         WHERE session_id = ?3 AND attempt_id = ?4",
            params![altered, checksum, fixture.session.as_str(), decision],
        )
        .unwrap();
    drop(connection);
    fixture.reopen();
    assert!(matches!(
        show(&mut fixture, Archived, &execution, page(64, None)),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Domain(
            DomainError::InvalidState {
                reason: "retained trace differs from replayed execution"
            }
        )))
    ));
}
