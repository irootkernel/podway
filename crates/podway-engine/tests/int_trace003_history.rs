//! Bounded prior-work provenance through the isolated candidate Engine and Store.

use super::int_group002_transactions::{
    Fixture, TIME, activation, base_document, canonical, definition, mutation,
};
use super::int_group003_replan::local_decision_group;
use super::int_group004_fences::tracked_base;
use podway_core::execution_groups::*;
use podway_core::{
    CheckResultExecutorV2, CheckResultInputBasisV2, CheckResultOutcomeV2, CheckResultValueV2,
    CriterionId, DomainError, GoalCriterionV2, GoalDefinitionV2, GoalStatementV2, GraphNodeId,
    ItemId, OperationId, OptionId, ReasonV2, RecordedItemValueV2, Revision, Sha256Digest,
    recorded_value_digest_v2, workflow_v2::item_value_json_v2,
};
use podway_engine::direct_execution::NativeDirectIdSourceV1;
use podway_engine::successor_execution::*;
use podway_store::CanonicalExecutionJsonV1;
use podway_store::successor_store::CandidateStoreError;
use serde_json::{Value, json};

fn issue(fixture: &mut Fixture, key: &str, operation: SuccessorCursorMutation) {
    let state = fixture.state().0;
    issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &state, key),
        operation,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
}

fn record(
    fixture: &mut Fixture,
    key: &str,
    items: impl IntoIterator<Item = (&'static str, u64, RecordedItemValueV2)>,
) {
    let state = fixture.state().0;
    let writes = items
        .into_iter()
        .map(|(id, revision, value)| GroupItemWriteV1 {
            item: ItemId::new(id).unwrap(),
            expected_revision: Revision::new(revision),
            value: Some(value),
        })
        .collect();
    record_successor_many(
        &mut fixture.store,
        mutation(&fixture.workspace, &state, key),
        writes,
        || TIME,
    )
    .unwrap();
}

fn goal(statement: &str) -> ExecutionGoalV3 {
    ExecutionGoalV3 {
        statement: GoalStatementV2::new(statement).unwrap(),
        criteria: GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(
                CriterionId::new("remaining-work").unwrap(),
                "Verify the remaining work.",
            )
            .unwrap(),
        ])
        .unwrap(),
    }
}

fn plan(fixture: &mut Fixture, key: &str, label: &str, basis: &CheckResultInputBasisV2) {
    issue(
        fixture,
        &format!("goal-{key}"),
        SuccessorCursorMutation::ReviseGoal(ExecutionGoalChangeV3 {
            goal: goal(label),
            target: GraphNodeId::new("design").unwrap(),
            feedback: GroupClosureFeedbackV1::new(
                "Revise the scope of remaining work.".into(),
                basis.clone(),
                label.into(),
            )
            .unwrap(),
            reactivate: false,
        }),
    );
    record(
        fixture,
        &format!("plan-{key}"),
        [("plan", 0, RecordedItemValueV2::text(label).unwrap())],
    );
}

fn check(
    operation_digest: &Sha256Digest,
    basis: &CheckResultInputBasisV2,
    outcome: CheckResultOutcomeV2,
) -> RecordedItemValueV2 {
    RecordedItemValueV2::check_result(
        CheckResultValueV2::new(
            OperationId::new("verify").unwrap(),
            operation_digest.clone(),
            basis.clone(),
            CheckResultExecutorV2::new("fixture", "1").unwrap(),
            outcome,
            "Verification result for the exact scoped attempt.",
            basis.digest().clone(),
        )
        .unwrap(),
    )
}

fn value(page: CanonicalExecutionJsonV1) -> Value {
    assert!(page.as_str().len() <= podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1);
    serde_json::from_str(page.as_str()).unwrap()
}

fn page(limit: usize, continuation: Option<&str>) -> SuccessorHistoryPage<'_> {
    SuccessorHistoryPage {
        limit,
        continuation,
    }
}

fn work(
    group: &GroupExecutionIdV1,
    reference: Option<HistoricalReferenceV1>,
    choice: PriorWorkChoiceV1,
    rationale: &str,
) -> PriorWorkDispositionV1 {
    PriorWorkDispositionV1::new(group.clone(), reference, choice, rationale.to_owned()).unwrap()
}

fn binding(item: &str, revision: u64, value: &RecordedItemValueV2) -> Value {
    json!({"item": item, "revision": revision, "value_digest": recorded_value_digest_v2(value).unwrap()})
}

#[test]
fn third_plan_pages_exact_first_group_work_without_inheriting_gates() {
    use CandidateSessionLocation::{Archived, Current};
    use PriorWorkChoiceV1::{Modify, Retain, Revalidate, Revert};
    use SuccessorCursorMutation as Action;

    let mut fixture = Fixture::new(&tracked_base(), false);
    issue(&mut fixture, "prefix", Action::Complete);
    issue(
        &mut fixture,
        "first-goal",
        Action::DefineGoal(goal("Initial baseline")),
    );
    let first_plan = RecordedItemValueV2::text("First plan").unwrap();
    record(&mut fixture, "plan-one", [("plan", 0, first_plan.clone())]);
    let (design_one, _) = fixture.state();
    let design_one_attempt = design_one.active_attempt().unwrap().target().clone();
    let base_digest = design_one.base_digest();
    let first_basis =
        CheckResultInputBasisV2::new("First working-copy snapshot", base_digest.clone()).unwrap();
    let next_basis = CheckResultInputBasisV2::new(
        "Working copy with retained first-group changes",
        Sha256Digest::new(format!("sha256:{}", "3".repeat(64))).unwrap(),
    )
    .unwrap();
    let mut definition_json = local_decision_group(true);
    definition_json["node_definitions"]["work"]["items"].as_array_mut().unwrap().extend([
        json!({"id":"failure-details", "type":"text", "prompt":"Explain the finding.", "required":true}),
        json!({"id":"check", "type":"check_result", "prompt":"Run verification.", "required":true,
            "operation_id":"verify", "operation_digest":base_digest, "accepted_outcomes":["pass","fail"]}),
    ]);
    definition_json["graph"]["nodes"][1]["evidence_from"][0]["items"]
        .as_array_mut()
        .unwrap()
        .extend([json!("failure-details"), json!("check")]);
    let original_group = canonical(&definition_json);
    definition_json["name"] = json!("Revised execution definition");
    let group = canonical(&definition_json);
    let mut activate = activation(
        &fixture.workspace,
        &design_one,
        &original_group,
        "group-one",
    );
    activate.working_copy_basis = first_basis.clone();
    activate_group(
        &mut fixture.store,
        activate,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let first = fixture.state().0;
    let g1 = first.selected().unwrap().identity().execution();
    let assessment = first.active_attempt().unwrap().target();
    let failure = RecordedItemValueV2::choice("failure").unwrap();
    let stage = RecordedItemValueV2::choice("scored").unwrap();
    let score = RecordedItemValueV2::integer(84);
    let failure_details =
        RecordedItemValueV2::text("Evaluator identified a boundary mismatch.").unwrap();
    let old_check = check(base_digest, &first_basis, CheckResultOutcomeV2::Fail);
    record(
        &mut fixture,
        "scored-failure",
        [
            ("verdict", 0, failure.clone()),
            ("stage", 0, stage.clone()),
            ("score", 0, score.clone()),
            ("failure-details", 0, failure_details.clone()),
            (
                "check",
                0,
                check(base_digest, &first_basis, CheckResultOutcomeV2::Pass),
            ),
        ],
    );
    record(
        &mut fixture,
        "failed-check",
        [("check", 1, old_check.clone())],
    );
    issue(&mut fixture, "close-assessment", Action::Complete);
    let choice_state = fixture.state().0;
    let old_choice = choice_state.active_attempt().unwrap().target();
    let reasoning =
        "Continue the independent remaining work and keep the failing check for revalidation.";
    issue(
        &mut fixture,
        "co-choice",
        Action::Decide {
            option: OptionId::new("finish").unwrap(),
            reason: ReasonV2::new(reasoning).unwrap(),
            goal_outcome: None,
        },
    );
    let old_report = RecordedItemValueV2::text("The independent work completed.").unwrap();
    record(
        &mut fixture,
        "report-one",
        [("report", 0, old_report.clone())],
    );
    let finish = fixture.state().0;
    let old_terminal = finish.active_attempt().unwrap().target();
    return_from_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &finish, "return-one"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    plan(&mut fixture, "second", "Second plan", &next_basis);
    let (design_two, _) = fixture.state();
    let mut activate = activation(&fixture.workspace, &design_two, &group, "group-two");
    activate.prior_work = vec![work(
        g1,
        None,
        Retain,
        "Keep the recorded first-group work.",
    )];
    activate_group(
        &mut fixture.store,
        activate,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let empty = fixture.state().0;
    let g2 = empty.selected().unwrap().identity().execution();
    assert!(
        empty
            .active_attempt()
            .unwrap()
            .slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
    plan(&mut fixture, "third", "Third plan", &next_basis);

    // The request names a real group from another fixture, not a fabricated missing ID.
    let mut foreign = Fixture::new(&base_document(), true);
    let foreign_design = foreign.state().0;
    let request = activation(&foreign.workspace, &foreign_design, &group, "foreign");
    activate_group(
        &mut foreign.store,
        request,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let foreign_state = foreign.state().0;
    let foreign_group = foreign_state.selected().unwrap().identity().execution();
    let before_activation = fixture.state();
    let design_three = &before_activation.0;
    let failed_digest = recorded_value_digest_v2(&old_check).unwrap();
    let cite = |item, digest| {
        HistoricalReferenceV1::Item(HistoricalItemV1::new(
            assessment.attempt().clone(),
            ItemId::new(item).unwrap(),
            digest,
        ))
    };
    for (key, rejected, expected) in [
        (
            "bad-item-digest",
            work(
                g1,
                Some(cite("check", base_digest.clone())),
                Revalidate,
                "Wrong digest",
            ),
            "prior work value digest differs from retained history",
        ),
        (
            "cross-group-attempt",
            work(
                g2,
                Some(HistoricalReferenceV1::Attempt(assessment.attempt().clone())),
                Retain,
                "Wrong group",
            ),
            "prior work attempt is outside its retained group",
        ),
        (
            "foreign-group",
            work(foreign_group, None, Retain, "Wrong session"),
            "prior work does not identify a retained same-session group",
        ),
    ] {
        let mut request = activation(&fixture.workspace, design_three, &group, key);
        request.prior_work = vec![rejected];
        let ledger = fixture.ledger();
        assert!(matches!(
            activate_group(&mut fixture.store, request, || TIME, &NativeDirectIdSourceV1),
            Err(SuccessorExecutionError::GroupActivationPriorWorkInvalid(
                DomainError::InvalidState { reason }
            )) if reason == expected
        ));
        fixture.compare_rejected(&before_activation, key, ledger);
    }

    let design_three_attempt = design_three.active_attempt().unwrap().target();
    let expected_plan = design_three.plan_binding().unwrap();
    let mut activate = activation(&fixture.workspace, design_three, &group, "group-three");
    activate.working_copy_basis = next_basis.clone();
    activate.prior_work = vec![
        work(
            g1,
            Some(HistoricalReferenceV1::Item(HistoricalItemV1::new(
                old_terminal.attempt().clone(),
                ItemId::new("report").unwrap(),
                recorded_value_digest_v2(&old_report).unwrap(),
            ))),
            Retain,
            "Retain the first group's successful output.",
        ),
        work(
            g1,
            Some(cite("check", failed_digest.clone())),
            Revalidate,
            "Repeat the failed check.",
        ),
        work(
            g1,
            Some(HistoricalReferenceV1::Attempt(old_choice.attempt().clone())),
            Modify,
            "Change the follow-up to the selected decision.",
        ),
        work(
            g1,
            Some(cite(
                "failure-details",
                recorded_value_digest_v2(&failure_details).unwrap(),
            )),
            Revert,
            "Plan the reversal of the rejected change.",
        ),
    ];
    for index in activate.prior_work.len()..MAX_PRIOR_WORK_DISPOSITIONS_V1 {
        let rationale = format!("{index:03} {}", "x".repeat(1996));
        assert_eq!(rationale.chars().count(), 2000);
        activate.prior_work.push(work(g1, None, Retain, &rationale));
    }
    let expected_work = activate.prior_work.clone();
    activate_group(
        &mut fixture.store,
        activate,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let current = fixture.state();
    let g3 = current.0.selected().unwrap().identity().execution();
    let g3_assessment = current.0.active_attempt().unwrap().target();
    let workspace = fixture.workspace.clone();
    let session = fixture.session.clone();
    let target = SuccessorHistorySession {
        workspace: &workspace,
        session: &session,
        location: Current,
    };
    let digest = group.digest();
    let original_digest = original_group.digest();
    assert_ne!(digest, original_digest);
    let ledger = fixture.ledger();
    let first_page = value(
        list_successor_prior_work(
            &mut fixture.store,
            target,
            g1,
            original_digest,
            page(64, None),
        )
        .unwrap(),
    );
    assert_eq!(
        first_page["group"]["design_attempt"],
        json!(design_one_attempt)
    );
    assert_eq!(
        first_page["group"]["working_copy_basis"],
        json!(first_basis)
    );
    assert_eq!(
        first_page["group"]["plan"]["items"][0],
        binding("plan", 1, &first_plan)
    );
    assert_eq!(first_page["group"]["predecessor"], Value::Null);
    assert_eq!(first_page["total"], 0);
    assert_eq!(first_page["rows"], json!([]));
    assert_eq!(first_page["truncated"], false);
    assert_eq!(first_page["next_page_token"], Value::Null);
    let group_link =
        |execution, definition| json!({"execution":execution, "definition_digest":definition});
    let read_pages = |fixture: &mut Fixture, target| {
        let first = value(
            list_successor_prior_work(&mut fixture.store, target, g3, digest, page(64, None))
                .unwrap(),
        );
        assert_eq!(
            first["group"],
            json!({
                "activation_sequence": 3, "design_attempt": design_three_attempt, "definition_digest": digest,
                "execution": g3, "goal_revision": 3, "working_copy_basis": next_basis,
                "predecessor": group_link(g2, digest),
                "plan": {
                    "design_attempt": design_three_attempt.attempt(), "item_set_digest": expected_plan.item_set_digest(),
                    "items": [binding("plan", 1, &RecordedItemValueV2::text("Third plan").unwrap())],
                },
            })
        );
        assert_eq!(first["total"], MAX_PRIOR_WORK_DISPOSITIONS_V1);
        assert_eq!(first["truncated"], true);
        assert_eq!(first["rows"].as_array().unwrap().len(), 64);
        let token = first["next_page_token"].as_str().unwrap();
        assert!(matches!(
            list_successor_prior_work(
                &mut fixture.store,
                target,
                g2,
                digest,
                page(64, Some(token))
            ),
            Err(SuccessorExecutionError::StaleHistoryPageToken)
        ));
        assert!(matches!(
            show_successor_group(&mut fixture.store, target, g3, page(64, Some(token))),
            Err(SuccessorExecutionError::StaleHistoryPageToken)
        ));
        let invalid = format!(
            "{}:{:016x}",
            token.rsplit_once(':').unwrap().0,
            MAX_PRIOR_WORK_DISPOSITIONS_V1
        );
        assert!(matches!(
            list_successor_prior_work(
                &mut fixture.store,
                target,
                g3,
                digest,
                page(64, Some(&invalid))
            ),
            Err(SuccessorExecutionError::InvalidHistoryPageToken)
        ));
        let last = value(
            list_successor_prior_work(
                &mut fixture.store,
                target,
                g3,
                digest,
                page(64, Some(token)),
            )
            .unwrap(),
        );
        assert_eq!(last["group"], first["group"]);
        assert_eq!(last["total"], MAX_PRIOR_WORK_DISPOSITIONS_V1);
        assert_eq!(last["truncated"], false);
        assert_eq!(last["next_page_token"], Value::Null);
        let rows = first["rows"]
            .as_array()
            .unwrap()
            .iter()
            .chain(last["rows"].as_array().unwrap());
        let actual = rows.cloned().collect::<Vec<_>>();
        assert_eq!(actual.len(), expected_work.len());
        for (index, (row, expected)) in actual.iter().zip(&expected_work).enumerate() {
            assert_eq!(row["choice"], json!(expected.choice()));
            assert_eq!(row["rationale"], expected.rationale());
            assert_eq!(row["source"]["group"], group_link(g1, original_digest));
            let (attempt, item) = match index {
                0 => (Some(old_terminal), binding("report", 1, &old_report)),
                1 => (Some(assessment), binding("check", 2, &old_check)),
                2 => (Some(old_choice), Value::Null),
                3 => (
                    Some(assessment),
                    binding("failure-details", 1, &failure_details),
                ),
                _ => (None, Value::Null),
            };
            assert_eq!(row["source"]["attempt"], json!(attempt));
            assert_eq!(row["source"]["item"], item);
        }
        first
    };
    let seen = read_pages(&mut fixture, target);
    assert!(matches!(
        list_successor_prior_work(&mut fixture.store, target, g3, base_digest, page(64, None)),
        Err(SuccessorExecutionError::GroupDefinitionMismatch)
    ));
    assert!(matches!(
        list_successor_prior_work(
            &mut fixture.store,
            target,
            foreign_group,
            digest,
            page(64, None)
        ),
        Err(SuccessorExecutionError::HistoryGroupNotFound)
    ));
    assert!(matches!(
        list_successor_prior_work(&mut fixture.store, target, g3, digest, page(65, None)),
        Err(SuccessorExecutionError::InvalidHistoryPageLimit)
    ));

    let read_sources = |fixture: &mut Fixture, target| {
        for (row, expected) in [
            (0, json!(old_report.as_text().unwrap())),
            (1, item_value_json_v2(&old_check).unwrap()),
        ] {
            let selection = &seen["rows"][row]["source"];
            let cited_attempt: ScopedAttemptV1 =
                serde_json::from_value(selection["attempt"].clone()).unwrap();
            let cited_item: ItemId =
                serde_json::from_value(selection["item"]["item"].clone()).unwrap();
            let history = value(
                read_successor_history_item(
                    &mut fixture.store,
                    target,
                    &cited_attempt,
                    &cited_item,
                    None,
                )
                .unwrap(),
            );
            assert_eq!(
                history["detail"]["value_digest"],
                selection["item"]["value_digest"]
            );
            assert_eq!(
                history["detail"]["item_revision"],
                selection["item"]["revision"]
            );
            assert_eq!(history["data"], expected);
        }
        let cited_group: GroupExecutionIdV1 =
            serde_json::from_value(seen["rows"][2]["source"]["group"]["execution"].clone())
                .unwrap();
        let decision_detail = value(
            show_successor_group(&mut fixture.store, target, &cited_group, page(64, None)).unwrap(),
        );
        assert_eq!(decision_detail["group"]["summary"]["outcome"], "completed");
        let decision = &decision_detail["rows"][0];
        assert_eq!(decision["attempt"], seen["rows"][2]["source"]["attempt"]);
        assert_eq!(decision["choice"], "finish");
        assert_eq!(decision["reason"], reasoning);
        assert_eq!(
            decision["assessment"],
            json!([{
                "source": assessment, "items": [
                    {"binding":binding("verdict", 1, &failure), "scalar":"failure"},
                    {"binding":binding("stage", 1, &stage), "scalar":"scored"},
                    {"binding":binding("score", 1, &score), "scalar":84},
                    {"binding":binding("failure-details", 1, &failure_details), "scalar":null},
                    {"binding":binding("check", 2, &old_check), "scalar":null},
                ],
            }])
        );
        assert_eq!(decision["guard_bindings"], json!([]));
    };
    read_sources(&mut fixture, target);
    assert_eq!(fixture.state(), current);
    assert_eq!(fixture.ledger(), ledger);

    let visible_plan = read_current_evidence(
        &mut fixture.store,
        &workspace,
        &SuccessorCursor::from_state(&current.0),
        &EvidenceSourceV3::Plan,
        &ItemId::new("plan").unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(&visible_plan.source, design_three_attempt);
    assert_eq!(visible_plan.value.as_text(), Some("Third plan"));
    record(
        &mut fixture,
        "new-assessment-incomplete",
        [
            (
                "verdict",
                0,
                RecordedItemValueV2::choice("success").unwrap(),
            ),
            (
                "stage",
                0,
                RecordedItemValueV2::choice("not-scored").unwrap(),
            ),
            ("score", 0, RecordedItemValueV2::integer(0)),
            (
                "failure-details",
                0,
                RecordedItemValueV2::text("Reassessment on the new plan.").unwrap(),
            ),
        ],
    );
    let before_gate = fixture.state();
    let ledger = fixture.ledger();
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            mutation(&workspace, &before_gate.0, "missing-new-check"),
            Action::Complete,
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::RequiredItemsMissing
        ))
    ));
    fixture.compare_rejected(&before_gate, "missing-new-check", ledger);
    assert!(matches!(
        list_successor_prior_work(
            &mut fixture.store,
            target,
            g3,
            digest,
            page(64, seen["next_page_token"].as_str())
        ),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let fresh_check = check(base_digest, &next_basis, CheckResultOutcomeV2::Pass);
    record(
        &mut fixture,
        "new-check",
        [("check", 0, fresh_check.clone())],
    );
    issue(&mut fixture, "new-gate", Action::Complete);
    let choice = fixture.state().0;
    let visible = read_current_evidence(
        &mut fixture.store,
        &workspace,
        &SuccessorCursor::from_state(&choice),
        &EvidenceSourceV3::Local(GraphNodeId::new("work").unwrap()),
        &ItemId::new("check").unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(&visible.source, g3_assessment);
    assert_eq!(visible.value, fresh_check);
    issue(
        &mut fixture,
        "cancel",
        Action::Cancel(ReasonV2::new("End the isolated fixture.").unwrap()),
    );
    let terminal = fixture.state();
    let terminal_revision = terminal.0.fence().revision;
    let terminal_page = value(
        list_successor_prior_work(&mut fixture.store, target, g3, digest, page(64, None)).unwrap(),
    );
    fixture
        .store
        .archive_fixture_session(&workspace, &session, terminal_revision, definition)
        .unwrap();
    fixture.reopen();
    let archived = SuccessorHistorySession {
        location: Archived,
        ..target
    };
    let ledger = fixture.ledger();
    assert!(matches!(
        list_successor_prior_work(&mut fixture.store, target, g3, digest, page(64, None)),
        Err(SuccessorExecutionError::Store(
            CandidateStoreError::ArchivedSession
        ))
    ));
    let archived_page = read_pages(&mut fixture, archived);
    assert_eq!(
        archived_page["session_revision"],
        terminal_page["session_revision"]
    );
    assert_eq!(archived_page["rows"], seen["rows"]);
    assert!(matches!(
        list_successor_prior_work(
            &mut fixture.store,
            archived,
            g3,
            digest,
            page(64, terminal_page["next_page_token"].as_str())
        ),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    read_sources(&mut fixture, archived);
    let (archived_state, archived_sources, _) = fixture
        .store
        .history_snapshot(&workspace, &session, Archived, definition)
        .unwrap();
    assert_eq!((archived_state, archived_sources), terminal);
    assert_eq!(fixture.ledger(), ledger);
}
