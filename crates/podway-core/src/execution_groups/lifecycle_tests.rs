use super::*;
use crate::*;

const NOW: UnixMillis = UnixMillis::UNIX_EPOCH;

fn node(name: &str) -> GraphNodeId {
    GraphNodeId::new(name).unwrap()
}

fn item(name: &str) -> ItemId {
    ItemId::new(name).unwrap()
}

fn option(name: &str) -> OptionId {
    OptionId::new(name).unwrap()
}

fn digest(byte: &str) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.repeat(64))).unwrap()
}

fn attempt(n: usize) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
}

fn group_id(n: usize) -> GroupExecutionIdV1 {
    GroupExecutionIdV1::new(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
}

fn reason() -> ReasonV2 {
    ReasonV2::new("Current work and verification").unwrap()
}

fn basis() -> CheckResultInputBasisV2 {
    CheckResultInputBasisV2::new("Owned working copy", digest("e")).unwrap()
}

fn closure_feedback() -> GroupClosureFeedbackV1 {
    GroupClosureFeedbackV1::new("Goal changed".into(), basis(), "Keep verified work".into())
        .unwrap()
}

fn goal(statement: &str) -> ExecutionGoalV3 {
    ExecutionGoalV3 {
        statement: GoalStatementV2::new(statement).unwrap(),
        criteria: GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(CriterionId::new("criterion").unwrap(), statement).unwrap(),
        ])
        .unwrap(),
    }
}

fn goal_change(target: &str, reactivate: bool) -> ExecutionGoalChangeV3 {
    ExecutionGoalChangeV3 {
        goal: goal("Revised scope"),
        target: node(target),
        feedback: closure_feedback(),
        reactivate,
    }
}

fn common(name: &str, required: bool) -> ItemCommonV2 {
    ItemCommonV2::new(item(name), name, None, required).unwrap()
}

fn choices(name: &str, values: &[&str]) -> ItemSpecV2 {
    ItemSpecV2::choice(
        common(name, true),
        values.iter().map(|value| (*value).to_owned()).collect(),
    )
    .unwrap()
}

fn plan_input() -> Vec<ItemSpecV2> {
    let conditional = ItemConditionV2::new(vec![ItemPredicateV2::new(
        item("detail-mode"),
        false,
        ItemPredicateOperatorV2::Equals(PredicateScalarV2::Text("on".into())),
    )])
    .unwrap();
    vec![
        choices("detail-mode", &["off", "on"]),
        ItemSpecV2::text(
            common("detail", false)
                .with_required_when(Some(conditional))
                .unwrap(),
            1,
            20,
            false,
        )
        .unwrap(),
    ]
}

fn assessment_items(required_score: bool) -> Vec<ItemSpecV2> {
    vec![
        choices("verdict", &["failure", "success"]),
        choices("stage", &["not-scored", "lightweight", "scored"]),
        ItemSpecV2::integer(common("score", required_score), Some(0), Some(100)).unwrap(),
    ]
}

fn action(
    name: &str,
    next: Option<&str>,
    items: Vec<ItemSpecV2>,
    evidence: Vec<EvidenceReferenceV3>,
    delegation: Option<DelegationDeclarationV1>,
) -> (NodeDefinitionV3, PlacementV3) {
    let id = NodeDefinitionId::new(name).unwrap();
    (
        NodeDefinitionV3::Action(
            ActionDefinitionV2::new(id.clone(), name, "Record work", None, vec![], items).unwrap(),
        ),
        PlacementV3::new(
            node(name),
            id,
            evidence,
            PlacementKindV3::Action {
                outcome: next.map_or(ActionOutcomeV2::Terminal, |next| {
                    ActionOutcomeV2::Next(node(next))
                }),
                skip: None,
                delegation,
            },
        )
        .unwrap(),
    )
}

fn decision(
    name: &str,
    routes: Vec<(&str, DecisionRouteV3)>,
    assessment: Option<AssessmentContractV2>,
    guard_source: Option<EvidenceSourceV3>,
) -> (NodeDefinitionV3, PlacementV3) {
    let id = NodeDefinitionId::new(name).unwrap();
    let metadata = DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
        id: id.clone(),
        title: name.into(),
        description: None,
        objective: "Choose the declared route".into(),
        prompt: "Decide from the current result".into(),
        evidence_guidance: vec![],
        items: vec![],
        options: routes
            .iter()
            .map(|(name, _)| DecisionOptionV2::new(option(name), *name, None).unwrap())
            .collect(),
        reason: ReasonPolicyV2::new(true, None).unwrap(),
        assessment,
    })
    .unwrap();
    let evidence = guard_source
        .as_ref()
        .map(|source| {
            EvidenceReferenceV3::new(
                source.clone(),
                true,
                vec![item("verdict"), item("stage"), item("score")],
            )
            .unwrap()
        })
        .into_iter()
        .collect();
    let guards = guard_source
        .map(|source| {
            let guards = [
                (
                    "verdict",
                    ItemPredicateOperatorV2::Equals(PredicateScalarV2::Text("failure".into())),
                ),
                (
                    "stage",
                    ItemPredicateOperatorV2::Equals(PredicateScalarV2::Text("scored".into())),
                ),
                ("score", ItemPredicateOperatorV2::AtLeast(80)),
            ]
            .into_iter()
            .map(|(name, operator)| {
                EvidencePredicateV3::new(source.clone(), item(name), false, operator)
            })
            .collect();
            (option("redesign"), OptionGuardV3::new(guards).unwrap())
        })
        .into_iter()
        .collect();
    (
        NodeDefinitionV3::Decision(DecisionDefinitionV3::new(metadata, guards).unwrap()),
        PlacementV3::new(
            node(name),
            id,
            evidence,
            PlacementKindV3::Decision {
                routes: routes
                    .into_iter()
                    .map(|(name, target)| (option(name), target))
                    .collect(),
            },
        )
        .unwrap(),
    )
}

fn goal_assessment(name: &str, next: &str) -> (NodeDefinitionV3, PlacementV3) {
    let mappings = [
        ("achieved", GoalOutcome::Achieved),
        ("not-achieved", GoalOutcome::NotAchieved),
        ("superseded", GoalOutcome::Superseded),
    ];
    decision(
        name,
        mappings
            .iter()
            .map(|(id, _)| (*id, DecisionRouteV3::Advance(node(next))))
            .collect(),
        Some(
            AssessmentContractV2::new(
                mappings
                    .into_iter()
                    .map(|(id, outcome)| AssessmentOutcomeMappingV2::new(option(id), outcome))
                    .collect(),
            )
            .unwrap(),
        ),
        None,
    )
}

fn fixture_base(tracked: bool, delegated: bool) -> ProcedureDefinitionV3 {
    let delegation = delegated.then(|| {
        DelegationDeclarationV1::new(
            DelegationBindingRefV1::new("design").unwrap(),
            DelegationRoleRefV1::new("designer".into()).unwrap(),
            digest("d"),
            1,
            DelegationAfterAcceptV1::Retain,
        )
        .unwrap()
    });
    let mut anchor_items = plan_input();
    anchor_items.push(ItemSpecV2::confirm(common("approval", true)));
    let mut pairs = vec![
        action("prefix", Some("design"), vec![], vec![], None),
        action(
            "design",
            Some("review-work"),
            anchor_items,
            vec![],
            delegation,
        ),
        action("review-work", Some("review"), vec![], vec![], None),
        decision(
            "review",
            vec![
                (
                    "pass",
                    DecisionRouteV3::Advance(node(if tracked { "goal" } else { "final" })),
                ),
                ("redo", DecisionRouteV3::Rework(node("review-work"))),
                ("redesign", DecisionRouteV3::Replan),
            ],
            None,
            Some(EvidenceSourceV3::ExecutionResult),
        ),
    ];
    if tracked {
        pairs.push(goal_assessment("goal", "final"));
    }
    pairs.push(action("final", None, vec![], vec![], None));
    let (definitions, nodes) = pairs.into_iter().unzip();
    ProcedureDefinitionV3::new(
        DefinitionHeaderV3::new(
            NodeDefinitionId::new("base").unwrap(),
            "1".into(),
            "Base".into(),
            "Base purpose".into(),
            None,
        )
        .unwrap(),
        definitions,
        TopologyV3::new(
            node("prefix"),
            nodes,
            vec![node("prefix"), node("review-work")],
        )
        .unwrap(),
        tracked.then(GoalTrackingOptIn::enabled),
        GroupBoundaryV1::new(
            node("design"),
            node("review-work"),
            vec![item("detail-mode"), item("detail")],
            assessment_items(true),
            vec![node("review")],
        )
        .unwrap(),
    )
    .unwrap()
}

fn fixture_group(reversed_result: bool) -> ExecutionGroupDefinitionV1 {
    fixture_group_with(reversed_result, None, 3, 1)
}

fn fixture_group_with(
    reversed_result: bool,
    delegation: Option<DelegationDeclarationV1>,
    placements: usize,
    version: usize,
) -> ExecutionGroupDefinitionV1 {
    let mut result = assessment_items(true);
    if reversed_result {
        result.reverse();
    }
    let mut fixture = vec![
        action(
            "assess",
            Some("route"),
            assessment_items(false),
            vec![
                EvidenceReferenceV3::new(
                    EvidenceSourceV3::Plan,
                    true,
                    vec![item("detail-mode"), item("detail")],
                )
                .unwrap(),
            ],
            delegation,
        ),
        decision(
            "route",
            vec![
                (
                    "pass",
                    DecisionRouteV3::Advance(node(if placements == 3 { "exit" } else { "step-3" })),
                ),
                ("redo", DecisionRouteV3::Rework(node("assess"))),
                ("redesign", DecisionRouteV3::Replan),
            ],
            None,
            Some(EvidenceSourceV3::Local(node("assess"))),
        ),
        action("exit", None, result, vec![], None),
    ];
    for i in 3..placements {
        let next = if i + 1 == placements {
            "exit".to_owned()
        } else {
            format!("step-{}", i + 1)
        };
        fixture.push(action(
            &format!("step-{i}"),
            Some(&next),
            vec![],
            vec![],
            None,
        ));
    }
    let (definitions, nodes) = fixture.into_iter().unzip();
    ExecutionGroupDefinitionV1::new(
        DefinitionHeaderV3::new(
            NodeDefinitionId::new("group").unwrap(),
            version.to_string(),
            "Group".into(),
            "Group purpose".into(),
            None,
        )
        .unwrap(),
        definitions,
        TopologyV3::new(node("assess"), nodes, vec![node("assess")]).unwrap(),
        plan_input(),
    )
    .unwrap()
}

fn set(session: &mut ExecutionSessionV3, name: &str, value: RecordedItemValueV2) {
    let target = session.active_attempt().unwrap().target().clone();
    let revision = session
        .active_attempt()
        .unwrap()
        .slot(&item(name))
        .unwrap()
        .revision();
    session
        .record_item(&target, &item(name), revision, Some(value), NOW)
        .unwrap();
}

fn begin(tracked: bool, delegated: bool) -> ExecutionSessionV3 {
    begin_base(fixture_base(tracked, delegated))
}

fn start_base(base: ProcedureDefinitionV3) -> ExecutionSessionV3 {
    ExecutionSessionV3::begin(
        SessionId::new("00000000-0000-4000-8000-000000009999").unwrap(),
        base,
        digest("a"),
        None,
        attempt(1),
        NOW,
    )
    .unwrap()
}

fn begin_base(base: ProcedureDefinitionV3) -> ExecutionSessionV3 {
    let mut session = start_base(base);
    session
        .complete(&session.fence(), Some(attempt(2)), NOW)
        .unwrap();
    session
}

fn ready_design(session: &mut ExecutionSessionV3) {
    set(
        session,
        "detail-mode",
        RecordedItemValueV2::choice("off").unwrap(),
    );
    set(session, "approval", RecordedItemValueV2::confirm());
}

fn activation(
    session: &ExecutionSessionV3,
    run: usize,
    prior_work: Vec<PriorWorkDispositionV1>,
) -> ActivationIdentityV1 {
    activation_for(session, run, digest("b"), prior_work)
}

fn activation_for(
    session: &ExecutionSessionV3,
    run: usize,
    definition_digest: Sha256Digest,
    prior_work: Vec<PriorWorkDispositionV1>,
) -> ActivationIdentityV1 {
    ActivationIdentityV1::new(activation_input(
        session,
        run,
        definition_digest,
        prior_work,
    ))
    .unwrap()
}

fn activation_input(
    session: &ExecutionSessionV3,
    run: usize,
    definition_digest: Sha256Digest,
    prior_work: Vec<PriorWorkDispositionV1>,
) -> ActivationIdentityInputV1 {
    ActivationIdentityInputV1 {
        execution: group_id(run),
        session: session.session.clone(),
        base_digest: digest("a"),
        definition_digest,
        anchor: node("design"),
        goal_revision: session.fence().goal_revision,
        plan: session.plan_binding().unwrap(),
        predecessor: session.latest_group_id().cloned(),
        working_copy_basis: basis(),
        prior_work,
    }
}

fn launch(
    session: &mut ExecutionSessionV3,
    run: usize,
    next: usize,
    prior_work: Vec<PriorWorkDispositionV1>,
) {
    let identity = activation(session, run, prior_work);
    session
        .activate(
            &session.fence(),
            identity,
            fixture_group(false),
            &digest("b"),
            attempt(next),
            NOW,
        )
        .unwrap();
}

fn assessor(session: &mut ExecutionSessionV3, verdict: &str, stage: &str, score: Option<i64>) {
    set(
        session,
        "verdict",
        RecordedItemValueV2::choice(verdict).unwrap(),
    );
    set(
        session,
        "stage",
        RecordedItemValueV2::choice(stage).unwrap(),
    );
    if let Some(value) = score {
        set(session, "score", RecordedItemValueV2::integer(value));
    }
}

fn feedback(session: &ExecutionSessionV3) -> ReplanFeedbackV1 {
    ReplanFeedbackV1::new(
        session.active_attempt().unwrap().target().clone(),
        option("redesign"),
        "Current score and reasons support redesign".into(),
        basis(),
        "Retain separately verified work".into(),
    )
    .unwrap()
}

fn prior(run: usize, reference: Option<HistoricalReferenceV1>) -> PriorWorkDispositionV1 {
    PriorWorkDispositionV1::new(
        group_id(run),
        reference,
        PriorWorkChoiceV1::Retain,
        "Verified source stays".into(),
    )
    .unwrap()
}

fn reach_review(session: &mut ExecutionSessionV3, next: usize) {
    reach_review_with_score(session, next, 80);
}

fn reach_review_with_score(session: &mut ExecutionSessionV3, next: usize, score: i64) {
    assessor(session, "success", "not-scored", Some(0));
    session
        .complete(&session.fence(), Some(attempt(next)), NOW)
        .unwrap();
    session
        .decide(
            &session.fence(),
            option("pass"),
            reason(),
            None,
            attempt(next + 1),
            NOW,
        )
        .unwrap();
    assessor(session, "failure", "scored", Some(score));
    session
        .complete(&session.fence(), Some(attempt(next + 2)), NOW)
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(next + 3)), NOW)
        .unwrap();
}

#[test]
fn activation_enforces_exact_order_current_plan_goal_and_delegation() {
    let mut session = begin(true, true);
    ready_design(&mut session);
    let plan = session.plan_binding().unwrap();
    assert_eq!(
        plan.items().len(),
        1,
        "the false conditional selected item has no value"
    );
    assert_eq!(plan.items()[0].item(), &item("detail-mode"));
    let identity = activation(&session, 1, vec![]);
    let unchanged = session.clone();
    assert!(
        session
            .activate(
                &session.fence(),
                identity,
                fixture_group(false),
                &digest("b"),
                attempt(3),
                NOW
            )
            .is_err()
    );
    assert_eq!(session, unchanged);
    session
        .define_goal(&session.fence(), goal("Original scope"), NOW)
        .unwrap();
    let identity = activation(&session, 1, vec![]);
    assert!(
        session
            .activate(
                &session.fence(),
                identity,
                fixture_group(false),
                &digest("b"),
                attempt(3),
                NOW
            )
            .is_err()
    );
    let target = session.active_attempt().unwrap().target().clone();
    let binding = ScopedDelegationBindingV1::new(target, digest("c"));
    assert!(
        session
            .accept_delegation(&session.fence(), binding.clone(), &digest("b"), NOW)
            .is_err()
    );
    session
        .accept_delegation(&session.fence(), binding.clone(), &digest("c"), NOW)
        .unwrap();
    let old_identity = activation(&session, 1, vec![]);
    set(&mut session, "approval", RecordedItemValueV2::confirm());
    assert!(
        session
            .activate(
                &session.fence(),
                old_identity,
                fixture_group(false),
                &digest("b"),
                attempt(3),
                NOW
            )
            .is_err()
    );
    session
        .accept_delegation(&session.fence(), binding, &digest("c"), NOW)
        .unwrap();
    let unchanged = session.clone();
    assert!(
        session
            .complete(&session.fence(), Some(attempt(3)), NOW)
            .is_err()
    );
    assert!(
        session
            .skip(&session.fence(), None, attempt(3), NOW)
            .is_err()
    );
    assert!(
        session
            .activate(
                &session.fence(),
                activation(&session, 1, vec![]),
                fixture_group(true),
                &digest("b"),
                attempt(3),
                NOW
            )
            .is_err()
    );
    assert_eq!(session, unchanged);
    launch(&mut session, 1, 3, vec![]);
    assert_eq!(
        session.attempt_history()[1].lifecycle(),
        AttemptLifecycle::Completed
    );
    assert_eq!(
        session.active_attempt().unwrap().target().attempt(),
        &attempt(3)
    );
    assert!(
        session
            .current_evidence(&EvidenceSourceV3::Plan, &item("detail"))
            .is_none()
    );
    assert!(
        session
            .current_evidence(&EvidenceSourceV3::Plan, &item("approval"))
            .is_none()
    );
    assert_eq!(
        session
            .current_evidence(&EvidenceSourceV3::Plan, &item("detail-mode"))
            .unwrap()
            .0
            .attempt(),
        &attempt(2)
    );
}

#[test]
fn conditional_plan_and_complete_anchor_digest_are_checked_before_effects() {
    let mut session = begin(false, false);
    set(
        &mut session,
        "detail-mode",
        RecordedItemValueV2::choice("on").unwrap(),
    );
    set(&mut session, "approval", RecordedItemValueV2::confirm());
    assert_eq!(
        session.activate(
            &session.fence(),
            activation(&session, 1, vec![]),
            fixture_group(false),
            &digest("b"),
            attempt(3),
            NOW
        ),
        Err(DomainError::RequiredItemsMissing)
    );
    set(
        &mut session,
        "detail",
        RecordedItemValueV2::text("planned detail").unwrap(),
    );
    let plan = session.plan_binding().unwrap();
    assert_eq!(plan.items().len(), 2);
    let slots = session.active_attempt().unwrap().slots();
    let entire = RecordedItemSetV2::new(
        slots
            .iter()
            .map(|slot| RecordedItemV2::new(slot.item_id().clone(), slot.value().unwrap().clone()))
            .collect(),
    )
    .unwrap();
    assert_eq!(
        plan.item_set_digest(),
        &recorded_items_digest_v2(&entire).unwrap()
    );
    let stale = activation(&session, 1, vec![]);
    set(
        &mut session,
        "detail",
        RecordedItemValueV2::text("planned detail").unwrap(),
    );
    assert_eq!(
        session.plan_binding().unwrap().item_set_digest(),
        plan.item_set_digest()
    );
    let unchanged = session.clone();
    assert!(
        session
            .activate(
                &session.fence(),
                stale,
                fixture_group(false),
                &digest("b"),
                attempt(3),
                NOW
            )
            .is_err()
    );
    assert_eq!(session, unchanged);
    launch(&mut session, 1, 3, vec![]);
}

#[test]
fn default_guards_reject_missing_unscored_success_and_79_but_allow_exact_80() {
    for (verdict, stage, score, allowed) in [
        ("failure", "scored", None, false),
        ("failure", "not-scored", Some(0), false),
        ("failure", "lightweight", Some(0), false),
        ("failure", "scored", Some(79), false),
        ("success", "scored", Some(100), false),
        ("failure", "scored", Some(80), true),
    ] {
        let mut session = begin(false, false);
        ready_design(&mut session);
        launch(&mut session, 1, 3, vec![]);
        let unchanged = session.clone();
        assert!(
            session
                .replan(&session.fence(), feedback(&session), attempt(4), NOW)
                .is_err()
        );
        assert_eq!(session, unchanged);
        assessor(&mut session, verdict, stage, score);
        session
            .complete(&session.fence(), Some(attempt(4)), NOW)
            .unwrap();
        assert_eq!(session.active_attempt().unwrap().decision_entry(), Some(1));
        assert_eq!(session.next_decision_entry(&node("route")), Some(2));
        let decision = session.active_attempt().unwrap().target().clone();
        let stale = session.fence();
        session.retry(&stale, reason(), attempt(5), NOW).unwrap();
        assert_eq!(session.active_attempt().unwrap().decision_entry(), Some(2));
        let unchanged = session.clone();
        assert!(
            session
                .replan(
                    &stale,
                    ReplanFeedbackV1::new(
                        decision,
                        option("redesign"),
                        "Old Decision".into(),
                        basis(),
                        "Must not apply".into()
                    )
                    .unwrap(),
                    attempt(6),
                    NOW
                )
                .is_err()
        );
        assert!(
            session
                .decide(
                    &session.fence(),
                    option("redesign"),
                    reason(),
                    None,
                    attempt(6),
                    NOW
                )
                .is_err()
        );
        assert_eq!(session, unchanged);
        let supplied = feedback(&session);
        let result = session.replan(&session.fence(), supplied.clone(), attempt(6), NOW);
        assert_eq!(result.is_ok(), allowed, "{verdict} / {stage} / {score:?}");
        if allowed {
            assert!(session.selected().is_none());
            assert_eq!(
                session.group_history()[0].outcome(),
                GroupExecutionOutcomeV1::Abandoned
            );
            for (index, ordinal) in [(3, 1), (4, 2)] {
                let retained = &session.attempt_history()[index];
                let entry = retained.selected_decision_entry().unwrap();
                assert_eq!(entry.target(), retained.target());
                assert_eq!(entry.selected_group(), &group_id(1));
                assert_eq!(entry.ordinal(), ordinal);
            }
            let record = session.attempt_history()[4].decision().unwrap();
            assert_eq!(record.option, *supplied.option());
            assert_eq!(record.reason, supplied.reason());
            assert_eq!(record.guard_values.len(), 3);
            let source = session.history_attempt(&attempt(3)).unwrap();
            for (guard, name) in record
                .guard_values
                .iter()
                .zip(["verdict", "stage", "score"])
            {
                let slot = source.slot(&item(name)).unwrap();
                assert_eq!(guard.source, *source.target());
                assert_eq!(guard.item.item(), &item(name));
                assert_eq!(guard.item.revision(), slot.revision());
                assert_eq!(
                    guard.item.value_digest(),
                    &recorded_value_digest_v2(slot.value().unwrap()).unwrap()
                );
            }
            let closure = session.group_history()[0].closure().unwrap();
            assert_eq!(&closure.source, supplied.decision());
            let retained = closure.feedback.as_ref().unwrap();
            assert_eq!(retained.reason(), supplied.reason());
            assert_eq!(retained.working_copy_basis(), supplied.working_copy_basis());
            assert_eq!(retained.feedback(), supplied.feedback());
            assert_eq!(
                session.active_attempt().unwrap().target().node(),
                &node("design")
            );
        } else {
            assert_eq!(session, unchanged);
        }
    }
}

#[test]
fn completed_group_return_base_retry_and_replan_keep_exact_history() {
    let mut session = begin(false, false);
    ready_design(&mut session);
    launch(&mut session, 1, 3, vec![]);
    let original_target = session.active_attempt().unwrap().target().clone();
    reach_review(&mut session, 4);
    let original_terminal = session.history_attempt(&attempt(5)).unwrap().clone();
    let group = session.selected().unwrap();
    assert_eq!(group.outcome(), GroupExecutionOutcomeV1::Completed);
    assert_eq!(group.terminal_attempt(), Some(&attempt(5)));
    assert_eq!(session.active_attempt().unwrap().decision_entry(), Some(1));
    assert_eq!(
        session
            .current_evidence(&EvidenceSourceV3::ExecutionResult, &item("score"))
            .unwrap()
            .0
            .attempt(),
        &attempt(5)
    );
    let stale = session.fence();
    assert!(
        session
            .rework(&stale, &node("prefix"), reason(), attempt(8), NOW)
            .is_err()
    );
    session
        .decide(&stale, option("redo"), reason(), None, attempt(8), NOW)
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(9)), NOW)
        .unwrap();
    assert_eq!(session.active_attempt().unwrap().decision_entry(), Some(2));
    assert_eq!(
        session
            .current_evidence(&EvidenceSourceV3::ExecutionResult, &item("score"))
            .unwrap()
            .0
            .attempt(),
        &attempt(5)
    );
    session
        .replan(&session.fence(), feedback(&session), attempt(10), NOW)
        .unwrap();
    assert_eq!(
        session.group_history()[0].outcome(),
        GroupExecutionOutcomeV1::Completed
    );
    assert_eq!(
        session.group_history()[0].closure().unwrap().cause,
        SelectionClosureCauseV1::Replan
    );
    let before_prefix_rework = session.clone();
    let fence = session.fence();
    assert_eq!(
        session.rework(&fence, &node("prefix"), reason(), attempt(11), NOW),
        Err(invalid(
            "ordinary base routing cannot cross the execution-group boundary"
        ))
    );
    assert_eq!(session, before_prefix_rework);
    ready_design(&mut session);
    launch(&mut session, 2, 11, vec![prior(1, None)]);
    let current = session.active_attempt().unwrap().target().clone();
    let slot = session
        .active_attempt()
        .unwrap()
        .slot(&item("score"))
        .unwrap();
    assert_eq!(slot.revision(), Revision::ZERO);
    let unchanged = session.clone();
    let score = Some(RecordedItemValueV2::integer(99));
    let expected = DomainError::AttemptNotCurrent {
        expected: original_target.attempt().clone(),
        actual: Some(current.attempt().clone()),
    };
    assert_eq!(
        session.record_item(
            &original_target,
            &item("score"),
            Revision::ZERO,
            score.clone(),
            NOW
        ),
        Err(expected)
    );
    assert_eq!(session, unchanged);
    session
        .record_item(&current, &item("score"), Revision::ZERO, score, NOW)
        .unwrap();
    for (attempt_id, ordinal) in [(7, 1), (9, 2)] {
        let retained = session.history_attempt(&attempt(attempt_id)).unwrap();
        let entry = retained.selected_decision_entry().unwrap();
        assert_eq!(entry.target(), retained.target());
        assert_eq!(entry.selected_group(), &group_id(1));
        assert_eq!(entry.ordinal(), ordinal);
    }
    assert_eq!(session.definitions.len(), 1);
    assert_eq!(
        session.selected().unwrap().identity().definition_digest(),
        session.group_history()[0].identity().definition_digest()
    );
    assert_ne!(
        session.active_attempt().unwrap().target().scope(),
        original_target.scope()
    );
    assert_eq!(session.next_decision_entry(&node("route")), Some(1));
    let mut cancelled = session.clone();
    cancelled.cancel(&cancelled.fence(), reason(), NOW).unwrap();
    assert_eq!(
        cancelled.group_history()[1].outcome(),
        GroupExecutionOutcomeV1::TerminatedByCancel
    );
    reach_review_with_score(&mut session, 12, 93);
    let (source, current_score) = session
        .current_evidence(&EvidenceSourceV3::ExecutionResult, &item("score"))
        .unwrap();
    assert_eq!(source.attempt(), &attempt(13));
    assert_eq!(current_score.value().unwrap().as_integer(), Some(93));
    let entry = session
        .active_attempt()
        .unwrap()
        .selected_decision_entry()
        .unwrap();
    assert_eq!(entry.selected_group(), &group_id(2));
    assert_eq!(entry.ordinal(), 1);
    let retained = session.history_attempt(&attempt(5)).unwrap();
    assert_eq!(retained, &original_terminal);
    assert_eq!(retained.validity(), AttemptValidityV2::Valid);
    assert_eq!(
        retained
            .slot(&item("score"))
            .unwrap()
            .value()
            .unwrap()
            .as_integer(),
        Some(80)
    );
    session.cancel(&session.fence(), reason(), NOW).unwrap();
    assert_eq!(
        session.group_history()[1].outcome(),
        GroupExecutionOutcomeV1::Completed
    );
    assert!(session.selected().is_none());
}

// Core defends the formal terminal even when a direct typed caller supplies a route
// that the Config admission checks reject.
fn start_with_supplemental_prefix_assessment() -> ExecutionSessionV3 {
    let base = fixture_base(true, false);
    let (prefix, prefix_placement) = goal_assessment("prefix-goal", "prefix");
    let (review, review_placement) = decision(
        "review",
        vec![
            ("pass", DecisionRouteV3::Advance(node("goal"))),
            ("bypass", DecisionRouteV3::Advance(node("final"))),
            ("redo", DecisionRouteV3::Rework(node("review-work"))),
            ("redesign", DecisionRouteV3::Replan),
        ],
        None,
        Some(EvidenceSourceV3::ExecutionResult),
    );
    let definitions = base
        .definitions()
        .iter()
        .filter(|v| v.id().as_str() != "review")
        .cloned()
        .chain([prefix, review])
        .collect();
    let nodes = base
        .graph()
        .nodes()
        .iter()
        .filter(|v| v.id().as_str() != "review")
        .cloned()
        .chain([prefix_placement, review_placement])
        .collect();
    let base = ProcedureDefinitionV3::new(
        base.header().clone(),
        definitions,
        TopologyV3::new(
            node("prefix-goal"),
            nodes,
            base.graph().manual_rework().to_vec(),
        )
        .unwrap(),
        base.goal_tracking(),
        base.boundary().clone(),
    )
    .unwrap();
    let mut session = start_base(base);
    session
        .define_goal(&session.fence(), goal("Current work"), NOW)
        .unwrap();
    session
        .decide(
            &session.fence(),
            option("achieved"),
            reason(),
            Some(GoalOutcome::Achieved),
            attempt(2),
            NOW,
        )
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(3)), NOW)
        .unwrap();
    session
}

fn assert_prefix_outcome_does_not_complete(session: &mut ExecutionSessionV3) {
    let unchanged = session.clone();
    assert_eq!(
        session.complete(&session.fence(), None, NOW),
        Err(invalid(
            "session terminal requires the current completed group and any required goal assessment",
        ))
    );
    assert_eq!(session, &unchanged);
    assert_eq!(session.current_goal_outcome(), None);
    let prefix = session.history_attempt(&attempt(1)).unwrap();
    assert!(prefix.selected_decision_entry().is_none());
    assert_eq!(prefix.validity(), AttemptValidityV2::Valid);
    assert_eq!(
        prefix.decision().unwrap().goal_assessment,
        Some((GoalRevisionNumberV2::FIRST, GoalOutcome::Achieved))
    );
}

#[test]
fn frame_audit_core_refuses_terminal_with_only_prefix_goal_assessment() {
    let mut session = start_with_supplemental_prefix_assessment();
    ready_design(&mut session);
    launch(&mut session, 1, 4, vec![]);
    reach_review(&mut session, 5);
    session
        .decide(
            &session.fence(),
            option("bypass"),
            reason(),
            None,
            attempt(9),
            NOW,
        )
        .unwrap();
    assert_eq!(
        session.selected().unwrap().outcome(),
        GroupExecutionOutcomeV1::Completed
    );
    assert_prefix_outcome_does_not_complete(&mut session);
}

#[test]
fn frame_audit_core_requires_fresh_group_assessment_after_same_goal_replan() {
    let mut session = start_with_supplemental_prefix_assessment();
    let current_goal = session.current_goal().unwrap().clone();
    ready_design(&mut session);
    launch(&mut session, 1, 4, vec![]);
    reach_review(&mut session, 5);
    session
        .replan(&session.fence(), feedback(&session), attempt(9), NOW)
        .unwrap();
    let prior_group = session.group_history()[0].clone();
    ready_design(&mut session);
    launch(&mut session, 2, 10, vec![prior(1, None)]);
    reach_review(&mut session, 11);
    assert_eq!(
        session.fence().goal_revision,
        Some(GoalRevisionNumberV2::FIRST)
    );
    assert_eq!(session.current_goal(), Some(&current_goal));
    assert_eq!(session.group_history()[0], prior_group);
    assert_eq!(
        session.selected().unwrap().identity().execution(),
        &group_id(2)
    );

    let mut without = session.clone();
    without
        .decide(
            &without.fence(),
            option("bypass"),
            reason(),
            None,
            attempt(15),
            NOW,
        )
        .unwrap();
    assert_prefix_outcome_does_not_complete(&mut without);

    session
        .decide(
            &session.fence(),
            option("pass"),
            reason(),
            None,
            attempt(15),
            NOW,
        )
        .unwrap();
    session
        .decide(
            &session.fence(),
            option("not-achieved"),
            reason(),
            Some(GoalOutcome::NotAchieved),
            attempt(16),
            NOW,
        )
        .unwrap();
    let assessed = session
        .history_attempt(&attempt(15))
        .unwrap()
        .selected_decision_entry()
        .unwrap();
    assert_eq!(assessed.selected_group(), &group_id(2));
    assert_eq!(
        session.current_goal_outcome(),
        Some(GoalOutcome::NotAchieved)
    );

    let mut cancelled = session.clone();
    cancelled.cancel(&cancelled.fence(), reason(), NOW).unwrap();
    assert_eq!(cancelled.lifecycle(), SessionLifecycle::Cancelled);
    assert_eq!(
        cancelled.current_goal_outcome(),
        Some(GoalOutcome::NotAchieved)
    );

    session.complete(&session.fence(), None, NOW).unwrap();
    assert_eq!(session.lifecycle(), SessionLifecycle::Completed);
    assert!(session.selected().is_none());
    assert_eq!(
        session.current_goal_outcome(),
        Some(GoalOutcome::NotAchieved)
    );
    assert_eq!(session.group_history()[0], prior_group);
}

#[test]
fn goal_closure_reactivation_and_older_provenance_preserve_outcomes() {
    let mut session = begin(true, false);
    session
        .define_goal(&session.fence(), goal("Original scope"), NOW)
        .unwrap();
    ready_design(&mut session);
    launch(&mut session, 1, 3, vec![]);
    reach_review(&mut session, 4);
    session
        .decide(
            &session.fence(),
            option("pass"),
            reason(),
            None,
            attempt(8),
            NOW,
        )
        .unwrap();
    let unchanged = session.clone();
    assert!(
        session
            .decide(
                &session.fence(),
                option("achieved"),
                reason(),
                Some(GoalOutcome::NotAchieved),
                attempt(9),
                NOW
            )
            .is_err()
    );
    assert!(
        session
            .decide(
                &session.fence(),
                option("achieved"),
                reason(),
                None,
                attempt(9),
                NOW
            )
            .is_err()
    );
    assert_eq!(session, unchanged);
    session
        .decide(
            &session.fence(),
            option("achieved"),
            reason(),
            Some(GoalOutcome::Achieved),
            attempt(9),
            NOW,
        )
        .unwrap();
    session.complete(&session.fence(), None, NOW).unwrap();
    assert_eq!(session.lifecycle(), SessionLifecycle::Completed);
    assert_eq!(session.current_goal_outcome(), Some(GoalOutcome::Achieved));
    assert_eq!(
        session.group_history()[0].closure().unwrap().cause,
        SelectionClosureCauseV1::Completion
    );
    let original_group = session.group_history()[0].clone();
    let terminal = session.fence();
    assert!(
        session
            .revise_goal(&terminal, goal_change("design", false), attempt(10), NOW)
            .is_err()
    );
    assert!(
        session
            .revise_goal(&terminal, goal_change("prefix", true), attempt(10), NOW)
            .is_err()
    );
    session
        .revise_goal(&terminal, goal_change("design", true), attempt(10), NOW)
        .unwrap();
    assert_eq!(session.current_goal_outcome(), None);
    assert_eq!(session.initial_goal().unwrap(), &goal("Original scope"));
    assert_eq!(session.current_goal().unwrap(), &goal("Revised scope"));
    assert_eq!(
        session.goal_revision_history()[0].predecessor.as_ref(),
        Some(&group_id(1))
    );
    assert_eq!(
        session.goal_revision_history()[0].closure.cause,
        SelectionClosureCauseV1::Reactivation
    );
    assert_eq!(session.group_history()[0], original_group);
    let reactivated = &session.goal_revision_history()[0];
    assert_eq!(
        reactivated.closure.feedback.as_ref(),
        Some(&closure_feedback())
    );
    assert_eq!(reactivated.closure.source.attempt(), &attempt(9));
    assert_eq!(reactivated.successor.attempt(), &attempt(10));
    ready_design(&mut session);
    launch(&mut session, 2, 11, vec![prior(1, None)]);
    let working = session.fence();
    assert!(
        session
            .revise_goal(&working, goal_change("prefix", false), attempt(12), NOW)
            .is_err()
    );
    let mut revision = goal_change("design", false);
    let supplied = GroupClosureFeedbackV1::new(
        "Revise a selected running group".into(),
        CheckResultInputBasisV2::new("Updated worktree source", digest("f")).unwrap(),
        "Keep the earlier completed outcome".into(),
    )
    .unwrap();
    revision.feedback = supplied.clone();
    session
        .revise_goal(&working, revision, attempt(12), NOW)
        .unwrap();
    assert_eq!(
        session.group_history()[1].outcome(),
        GroupExecutionOutcomeV1::Abandoned
    );
    assert_eq!(
        session.group_history()[1].closure().unwrap().cause,
        SelectionClosureCauseV1::GoalRevision
    );
    let revised = &session.goal_revision_history()[1];
    assert_eq!(revised.closure.feedback.as_ref(), Some(&supplied));
    assert_eq!(revised.closure.source.attempt(), &attempt(11));
    assert_eq!(revised.successor.attempt(), &attempt(12));
    assert_eq!(session.group_history()[1].closure(), Some(&revised.closure));
    ready_design(&mut session);
    let retained = HistoricalItemV1::new(
        attempt(5),
        item("score"),
        recorded_value_digest_v2(&RecordedItemValueV2::integer(80)).unwrap(),
    );
    let wrong = HistoricalItemV1::new(attempt(5), item("score"), digest("f"));
    let unchanged = session.clone();
    let cases = [
        (
            prior(99, None),
            "prior work does not identify a retained same-session group",
        ),
        (
            prior(1, Some(HistoricalReferenceV1::Attempt(attempt(11)))),
            "prior work attempt is outside its retained group",
        ),
        (
            prior(1, Some(HistoricalReferenceV1::Item(wrong))),
            "prior work value digest differs from retained history",
        ),
    ];
    for (disposition, error) in cases {
        assert_eq!(
            session.activate(
                &session.fence(),
                activation(&session, 3, vec![disposition]),
                fixture_group(false),
                &digest("b"),
                attempt(13),
                NOW
            ),
            Err(invalid(error))
        );
        assert_eq!(session, unchanged);
    }
    let mut wrong_predecessor = activation_input(&session, 3, digest("b"), vec![prior(1, None)]);
    wrong_predecessor.predecessor = Some(group_id(1));
    let duplicate = activation_input(&session, 1, digest("b"), vec![prior(2, None)]);
    for input in [wrong_predecessor, duplicate] {
        assert_eq!(
            session.activate(
                &session.fence(),
                ActivationIdentityV1::new(input).unwrap(),
                fixture_group(false),
                &digest("b"),
                attempt(13),
                NOW
            ),
            Err(invalid(
                "activation identity does not bind the exact current design"
            ))
        );
        assert_eq!(session, unchanged);
    }
    launch(
        &mut session,
        3,
        13,
        vec![prior(1, Some(HistoricalReferenceV1::Item(retained)))],
    );
    assert_eq!(
        session.selected().unwrap().identity().predecessor(),
        Some(&group_id(2))
    );
    reach_review(&mut session, 14);
    session.cancel(&session.fence(), reason(), NOW).unwrap();
    assert_eq!(
        session.group_history()[2].outcome(),
        GroupExecutionOutcomeV1::Completed
    );
    assert!(
        session
            .revise_goal(
                &session.fence(),
                goal_change("design", true),
                attempt(18),
                NOW
            )
            .is_err()
    );
}

#[test]
fn exact_cumulative_placement_budget_counts_distinct_definitions() {
    let mut session = begin(false, false);
    let base = session.base.graph().nodes().len();
    let full = (MAX_SESSION_PLACEMENTS_V1 - base) / MAX_GROUP_PLACEMENTS_V1;
    let remainder = (MAX_SESSION_PLACEMENTS_V1 - base) % MAX_GROUP_PLACEMENTS_V1;
    let mut next = 3;
    for run in 1..=full + 1 {
        ready_design(&mut session);
        let id = Sha256Digest::new(format!("sha256:{run:064x}")).unwrap();
        let prior_work = if run == 1 {
            vec![]
        } else {
            vec![prior(1, None)]
        };
        let identity = activation_for(&session, run, id.clone(), prior_work);
        let placements = if run <= full {
            MAX_GROUP_PLACEMENTS_V1
        } else {
            remainder
        };
        session
            .activate(
                &session.fence(),
                identity,
                fixture_group_with(false, None, placements, run),
                &id,
                attempt(next),
                NOW,
            )
            .unwrap();
        assessor(&mut session, "failure", "scored", Some(80));
        session
            .complete(&session.fence(), Some(attempt(next + 1)), NOW)
            .unwrap();
        session
            .replan(&session.fence(), feedback(&session), attempt(next + 2), NOW)
            .unwrap();
        next += 3;
    }
    assert_eq!(session.group_history().len(), full + 1);
    assert_eq!(
        base + session
            .definitions
            .values()
            .map(|v| v.graph().nodes().len())
            .sum::<usize>(),
        MAX_SESSION_PLACEMENTS_V1
    );
    ready_design(&mut session);
    let fence = session.fence();
    let (definition, terminal) =
        action("only-terminal", None, assessment_items(true), vec![], None);
    let one = ExecutionGroupDefinitionV1::new(
        DefinitionHeaderV3::new(
            NodeDefinitionId::new("one").unwrap(),
            "1".into(),
            "One".into(),
            "One terminal".into(),
            None,
        )
        .unwrap(),
        vec![definition],
        TopologyV3::new(node("only-terminal"), vec![terminal], vec![]).unwrap(),
        plan_input(),
    )
    .unwrap();
    assert!(
        matches!(session.activate(&fence, activation(&session, full + 2, vec![prior(1, None)]),
        one, &digest("b"), attempt(next), NOW),
        Err(DomainError::BoundExceeded { field: "session placements", actual, .. }) if actual == MAX_SESSION_PLACEMENTS_V1 as u64 + 1)
    );
    assert_eq!(session.fence(), fence);
    let reused = Sha256Digest::new(format!("sha256:{:064x}", 1)).unwrap();
    let reuse = activation_for(&session, full + 2, reused.clone(), vec![prior(1, None)]);
    session
        .activate(
            &fence,
            reuse,
            fixture_group_with(false, None, MAX_GROUP_PLACEMENTS_V1, 1),
            &reused,
            attempt(next),
            NOW,
        )
        .unwrap();
    assert_eq!(session.definitions.len(), full + 1);
}

#[test]
fn same_digest_group_cannot_reuse_an_old_delegation_acceptance() {
    let mut session = begin(false, false);
    ready_design(&mut session);
    let fallback = || {
        DelegationDeclarationV1::new(
            DelegationBindingRefV1::new("assess").unwrap(),
            DelegationRoleRefV1::new("assessor".into()).unwrap(),
            digest("d"),
            1,
            DelegationAfterAcceptV1::Retain,
        )
        .unwrap()
    };
    let group = || fixture_group_with(false, Some(fallback()), 3, 1);
    session
        .activate(
            &session.fence(),
            activation(&session, 1, vec![]),
            group(),
            &digest("b"),
            attempt(3),
            NOW,
        )
        .unwrap();
    assessor(&mut session, "failure", "scored", Some(80));
    let old = ScopedDelegationBindingV1::new(
        session.active_attempt().unwrap().target().clone(),
        digest("c"),
    );
    session
        .accept_delegation(&session.fence(), old.clone(), &digest("c"), NOW)
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(4)), NOW)
        .unwrap();
    session
        .replan(&session.fence(), feedback(&session), attempt(5), NOW)
        .unwrap();
    ready_design(&mut session);
    session
        .activate(
            &session.fence(),
            activation(&session, 2, vec![prior(1, None)]),
            group(),
            &digest("b"),
            attempt(6),
            NOW,
        )
        .unwrap();
    assessor(&mut session, "failure", "scored", Some(80));
    let unchanged = session.clone();
    assert!(
        session
            .accept_delegation(&session.fence(), old, &digest("c"), NOW)
            .is_err()
    );
    assert!(
        session
            .complete(&session.fence(), Some(attempt(7)), NOW)
            .is_err()
    );
    assert_eq!(session, unchanged);
    let current = ScopedDelegationBindingV1::new(
        session.active_attempt().unwrap().target().clone(),
        digest("c"),
    );
    session
        .accept_delegation(&session.fence(), current, &digest("c"), NOW)
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(7)), NOW)
        .unwrap();
}

#[test]
fn activation_count_and_attempt_budgets_refuse_without_eviction() {
    let mut session = begin(false, false);
    let mut next = 3;
    for run in 1..=MAX_GROUP_EXECUTIONS_V1 {
        ready_design(&mut session);
        launch(
            &mut session,
            run,
            next,
            if run == 1 {
                vec![]
            } else {
                vec![prior(1, None)]
            },
        );
        assessor(&mut session, "failure", "scored", Some(80));
        session
            .complete(&session.fence(), Some(attempt(next + 1)), NOW)
            .unwrap();
        session
            .replan(&session.fence(), feedback(&session), attempt(next + 2), NOW)
            .unwrap();
        next += 3;
    }
    ready_design(&mut session);
    let unchanged = session.clone();
    assert!(matches!(
        session.activate(
            &session.fence(),
            activation(&session, MAX_GROUP_EXECUTIONS_V1 + 1, vec![prior(1, None)]),
            fixture_group(false),
            &digest("b"),
            attempt(next),
            NOW
        ),
        Err(DomainError::BoundExceeded {
            field: "group executions",
            ..
        })
    ));
    assert_eq!(session, unchanged);
    assert_eq!(session.definitions.len(), 1);

    let mut attempts = begin(false, false);
    for id in 3..=MAX_SESSION_ATTEMPTS_V1 {
        attempts
            .retry(&attempts.fence(), reason(), attempt(id), NOW)
            .unwrap();
    }
    let fence = attempts.fence();
    assert!(matches!(
        attempts.retry(&fence, reason(), attempt(MAX_SESSION_ATTEMPTS_V1 + 1), NOW),
        Err(DomainError::BoundExceeded {
            field: "session attempts",
            ..
        })
    ));
    assert_eq!(attempts.fence(), fence);
    attempts.cancel(&fence, reason(), NOW).unwrap();
    assert_eq!(attempts.attempt_history().len(), MAX_SESSION_ATTEMPTS_V1);
}

fn blocker(
    target: &ScopedAttemptV1,
    id: usize,
    reason: &str,
    state: BlockerState,
) -> BlockerStateV2 {
    BlockerStateV2::new(
        BlockerId::new(format!("00000000-0000-4000-8000-{id:012}")).unwrap(),
        target.attempt().clone(),
        reason,
        state,
        NOW,
        (state == BlockerState::Resolved).then_some(NOW),
    )
    .unwrap()
}

#[test]
fn frame003_w1_base_review_attempt_distinguishes_selected_execution() {
    let review = |run| {
        let mut session = begin(false, false);
        ready_design(&mut session);
        launch(&mut session, run, 3, vec![]);
        reach_review(&mut session, 4);
        session.active_attempt().unwrap().clone()
    };
    let first = review(1);
    let second = review(2);
    assert_eq!(first.target(), second.target());
    assert_eq!(first.decision_entry(), Some(1));
    assert_eq!(first.decision_entry(), second.decision_entry());
    let first_entry = first.selected_decision_entry().unwrap();
    let second_entry = second.selected_decision_entry().unwrap();
    assert_eq!(first_entry.target(), first.target());
    assert_eq!(second_entry.target(), second.target());
    assert_eq!(first_entry.selected_group(), &group_id(1));
    assert_eq!(second_entry.selected_group(), &group_id(2));
    assert_ne!(
        first, second,
        "retained base-review entry must identify its selected execution"
    );
}

#[test]
fn frame003_w1_prefix_entry_preview_does_not_acquire_a_group_binding() {
    let base = fixture_base(false, false);
    let (prefix, placement) = decision(
        "prefix-choice",
        vec![("go", DecisionRouteV3::Advance(node("prefix")))],
        None,
        None,
    );
    let topology = TopologyV3::new(
        node("prefix-choice"),
        std::iter::once(placement)
            .chain(base.graph().nodes().iter().cloned())
            .collect(),
        base.graph().manual_rework().to_vec(),
    )
    .unwrap();
    let base = ProcedureDefinitionV3::new(
        base.header().clone(),
        std::iter::once(prefix)
            .chain(base.definitions().iter().cloned())
            .collect(),
        topology,
        None,
        base.boundary().clone(),
    )
    .unwrap();
    let mut session = ExecutionSessionV3::begin(
        SessionId::new("00000000-0000-4000-8000-000000009999").unwrap(),
        base,
        digest("a"),
        None,
        attempt(1),
        NOW,
    )
    .unwrap();
    assert_eq!(session.next_decision_entry(&node("prefix-choice")), Some(2));
    assert!(
        session
            .active_attempt()
            .unwrap()
            .selected_decision_entry()
            .is_none()
    );
    session
        .decide(
            &session.fence(),
            option("go"),
            reason(),
            None,
            attempt(2),
            NOW,
        )
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(3)), NOW)
        .unwrap();
    ready_design(&mut session);
    launch(&mut session, 1, 4, vec![]);
    reach_review(&mut session, 5);
    assert_eq!(session.next_decision_entry(&node("prefix-choice")), Some(2));
}

#[test]
fn frame003_w2_refuses_omitted_open_blocker_without_advancing() {
    let mut session = begin(false, false);
    ready_design(&mut session);
    let target = session.active_attempt().unwrap().target().clone();
    let open = blocker(
        &target,
        1,
        "Waiting for approval evidence",
        BlockerState::Open,
    );
    session
        .replace_blockers(&session.fence(), vec![open.clone()], NOW)
        .unwrap();
    let fenced = session.fence();
    let unchanged = session.clone();
    assert_eq!(
        session.activate(
            &fenced,
            activation(&session, 1, vec![]),
            fixture_group(false),
            &digest("b"),
            attempt(3),
            NOW
        ),
        Err(DomainError::BlockersPresent)
    );
    assert!(session.replace_blockers(&fenced, vec![], NOW).is_err());
    assert_eq!(session, unchanged);
    let resolved = blocker(&target, 1, open.reason(), BlockerState::Resolved);
    session
        .replace_blockers(&fenced, vec![resolved.clone()], NOW)
        .unwrap();
    launch(&mut session, 1, 3, vec![]);
    assert_eq!(session.attempt_history()[1].blockers(), [resolved]);
}

#[test]
fn frame003_w2_rejects_a_new_already_resolved_blocker() {
    let mut session = begin(false, false);
    let target = session.active_attempt().unwrap().target().clone();
    let unchanged = session.clone();
    let illegal = blocker(&target, 1, "Never observed open", BlockerState::Resolved);
    assert!(
        session
            .replace_blockers(&session.fence(), vec![illegal], NOW)
            .is_err()
    );
    assert_eq!(session, unchanged);
}

#[test]
fn frame003_w2_preserves_blocker_identity_and_resolution() {
    let mut session = begin(false, false);
    let target = session.active_attempt().unwrap().target().clone();
    let open = blocker(&target, 1, "Original reason", BlockerState::Open);
    session
        .replace_blockers(&session.fence(), vec![open.clone()], NOW)
        .unwrap();
    let unchanged = session.clone();
    let rewritten = blocker(&target, 1, "Rewritten reason", BlockerState::Resolved);
    assert!(
        session
            .replace_blockers(&session.fence(), vec![rewritten], NOW)
            .is_err()
    );
    assert_eq!(session, unchanged);
    let resolved = blocker(&target, 1, open.reason(), BlockerState::Resolved);
    session
        .replace_blockers(&session.fence(), vec![resolved], NOW)
        .unwrap();
    let unchanged = session.clone();
    assert!(
        session
            .replace_blockers(&session.fence(), vec![open], NOW)
            .is_err()
    );
    assert_eq!(session, unchanged);
}

fn result_base(results: Vec<ItemSpecV2>, allow_replan: bool) -> ProcedureDefinitionV3 {
    let base = fixture_base(false, false);
    let boundary = base.boundary();
    ProcedureDefinitionV3::new(
        base.header().clone(),
        base.definitions().to_vec(),
        base.graph().clone(),
        None,
        GroupBoundaryV1::new(
            boundary.anchor().clone(),
            boundary.success_return().clone(),
            boundary.plan_inputs().to_vec(),
            results,
            if allow_replan {
                boundary.replan_decisions().to_vec()
            } else {
                vec![]
            },
        )
        .unwrap(),
    )
    .unwrap()
}

fn result_group(result_items: Vec<ItemSpecV2>) -> ExecutionGroupDefinitionV1 {
    let group = fixture_group(false);
    let mut definitions = group.definitions().to_vec();
    let terminal = definitions
        .iter_mut()
        .find(|definition| definition.id().as_str() == "exit")
        .unwrap();
    *terminal = action("exit", None, result_items, vec![], None).0;
    ExecutionGroupDefinitionV1::new(
        group.header().clone(),
        definitions,
        group.graph().clone(),
        group.plan_inputs().to_vec(),
    )
    .unwrap()
}

fn conditional_results() -> Vec<ItemSpecV2> {
    let required_when_scored = ItemConditionV2::new(vec![ItemPredicateV2::new(
        item("stage"),
        false,
        ItemPredicateOperatorV2::Equals(PredicateScalarV2::Text("scored".into())),
    )])
    .unwrap();
    vec![
        choices("verdict", &["failure", "success"]),
        choices("stage", &["not-scored", "lightweight", "scored"]),
        ItemSpecV2::integer(
            common("score", false)
                .with_required_when(Some(required_when_scored))
                .unwrap(),
            Some(0),
            Some(100),
        )
        .unwrap(),
        ItemSpecV2::text(common("explanation", false), 1, 20, false).unwrap(),
    ]
}

fn reach_conditional_terminal() -> ExecutionSessionV3 {
    let results = conditional_results();
    let mut session = begin_base(result_base(results.clone(), true));
    ready_design(&mut session);
    session
        .activate(
            &session.fence(),
            activation(&session, 1, vec![]),
            result_group(results),
            &digest("b"),
            attempt(3),
            NOW,
        )
        .unwrap();
    assessor(&mut session, "success", "not-scored", None);
    session
        .complete(&session.fence(), Some(attempt(4)), NOW)
        .unwrap();
    session
        .decide(
            &session.fence(),
            option("pass"),
            reason(),
            None,
            attempt(5),
            NOW,
        )
        .unwrap();
    session
}

#[test]
fn frame003_f6_false_conditional_and_optional_result_can_be_absent() {
    let mut session = reach_conditional_terminal();
    assessor(&mut session, "success", "not-scored", None);
    session
        .complete(&session.fence(), Some(attempt(6)), NOW)
        .unwrap();
    assert_eq!(
        session.active_attempt().unwrap().target().node(),
        &node("review-work")
    );
    assert_eq!(
        session.selected().unwrap().terminal_attempt(),
        Some(&attempt(5))
    );
    session
        .complete(&session.fence(), Some(attempt(7)), NOW)
        .unwrap();
    assert!(
        session
            .current_evidence(&EvidenceSourceV3::ExecutionResult, &item("score"))
            .is_none()
    );
    let (source, slot) = session
        .current_evidence(&EvidenceSourceV3::ExecutionResult, &item("verdict"))
        .unwrap();
    assert_eq!(source.attempt(), &attempt(5));
    assert_eq!(slot.value().unwrap().as_choice(), Some("success"));
    let terminal = session.history_attempt(&attempt(5)).unwrap();
    assert!(
        terminal
            .slot(&item("explanation"))
            .unwrap()
            .value()
            .is_none()
    );
}

#[test]
fn frame003_f6_true_conditional_result_is_required_before_return() {
    let mut session = reach_conditional_terminal();
    assessor(&mut session, "failure", "scored", None);
    let unchanged = session.clone();
    assert_eq!(
        session.complete(&session.fence(), Some(attempt(6)), NOW),
        Err(DomainError::RequiredItemsMissing)
    );
    assert_eq!(session, unchanged);
    set(&mut session, "score", RecordedItemValueV2::integer(81));
    set(
        &mut session,
        "explanation",
        RecordedItemValueV2::text("Checked outcome").unwrap(),
    );
    session
        .complete(&session.fence(), Some(attempt(6)), NOW)
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(7)), NOW)
        .unwrap();
    let (source, slot) = session
        .current_evidence(&EvidenceSourceV3::ExecutionResult, &item("score"))
        .unwrap();
    assert_eq!(source.attempt(), &attempt(5));
    assert_eq!(slot.value().unwrap().as_integer(), Some(81));
}

#[test]
fn frame003_f1_tracked_goal_and_activation_identity_refusals_are_independent() {
    let mut session = begin(true, false);
    ready_design(&mut session);
    let unchanged = session.clone();
    assert_eq!(
        session.activate(
            &session.fence(),
            activation(&session, 1, vec![]),
            fixture_group(false),
            &digest("b"),
            attempt(3),
            NOW
        ),
        Err(invalid(
            "activation requires an unselected ready anchor with its current goal",
        ))
    );
    assert_eq!(session, unchanged);
    session
        .define_goal(&session.fence(), goal("Original scope"), NOW)
        .unwrap();
    let unchanged = session.clone();
    for case in [
        "session",
        "base",
        "definition",
        "anchor",
        "goal",
        "design-attempt",
        "plan-digest",
    ] {
        let mut input = activation_input(&session, 1, digest("b"), vec![]);
        match case {
            "session" => {
                input.session = SessionId::new("00000000-0000-4000-8000-000000009998").unwrap()
            }
            "base" => input.base_digest = digest("d"),
            "definition" => input.definition_digest = digest("d"),
            "anchor" => input.anchor = node("prefix"),
            "goal" => input.goal_revision = Some(GoalRevisionNumberV2::new(2)),
            "design-attempt" => {
                input.plan = PlanBindingV1::new(
                    attempt(1),
                    input.plan.item_set_digest().clone(),
                    input.plan.items().to_vec(),
                )
                .unwrap()
            }
            "plan-digest" => {
                input.plan = PlanBindingV1::new(
                    input.plan.design_attempt().clone(),
                    digest("d"),
                    input.plan.items().to_vec(),
                )
                .unwrap()
            }
            _ => unreachable!(),
        }
        assert_eq!(
            session.activate(
                &session.fence(),
                ActivationIdentityV1::new(input).unwrap(),
                fixture_group(false),
                &digest("b"),
                attempt(3),
                NOW
            ),
            Err(invalid(
                "activation identity does not bind the exact current design",
            )),
            "{case}"
        );
        assert_eq!(session, unchanged, "{case}");
    }
    launch(&mut session, 1, 3, vec![]);
}

#[test]
fn frame003_f2_current_replan_checks_route_policy_and_feedback_target() {
    let mut session = begin(false, false);
    ready_design(&mut session);
    launch(&mut session, 1, 3, vec![]);
    reach_review(&mut session, 4);
    let wrong = session.active_attempt().unwrap().target().clone();
    session
        .retry(&session.fence(), reason(), attempt(8), NOW)
        .unwrap();
    let current = session.active_attempt().unwrap().target().clone();
    let rejected_transition =
        "replan requires a declared ordinary Decision at the current selected placement";
    let unchanged = session.clone();
    let cases = [
        (
            "unknown",
            current.clone(),
            "the selected option has no declared route",
        ),
        ("pass", current, rejected_transition),
        ("redesign", wrong, rejected_transition),
    ];
    for (route, target, error) in cases {
        let wrong_feedback = ReplanFeedbackV1::new(
            target,
            option(route),
            "Test replan identity".into(),
            basis(),
            "Keep scope".into(),
        )
        .unwrap();
        assert_eq!(
            session.replan(&session.fence(), wrong_feedback, attempt(9), NOW),
            Err(invalid(error))
        );
        assert_eq!(session, unchanged);
    }
    session
        .replan(&session.fence(), feedback(&session), attempt(9), NOW)
        .unwrap();

    // A deliberately incomplete authoring policy tests the independent Core defense. Config
    // rejects this base before production use. Only the return-policy membership is changed.
    let mut denied = begin_base(result_base(assessment_items(true), false));
    ready_design(&mut denied);
    launch(&mut denied, 1, 3, vec![]);
    reach_review(&mut denied, 4);
    let unchanged = denied.clone();
    assert_eq!(
        denied.replan(&denied.fence(), feedback(&denied), attempt(8), NOW),
        Err(invalid(rejected_transition))
    );
    assert_eq!(denied, unchanged);
}

fn base_with_prefix(next: &str, skip: Option<SkipPolicyV2>) -> ProcedureDefinitionV3 {
    let base = fixture_base(false, false);
    let mut placements = base.graph().nodes().to_vec();
    let prefix = placements
        .iter_mut()
        .find(|placement| placement.id() == &node("prefix"))
        .unwrap();
    *prefix = PlacementV3::new(
        prefix.id().clone(),
        prefix.definition().clone(),
        vec![],
        PlacementKindV3::Action {
            outcome: ActionOutcomeV2::Next(node(next)),
            skip,
            delegation: None,
        },
    )
    .unwrap();
    ProcedureDefinitionV3::new(
        base.header().clone(),
        base.definitions().to_vec(),
        TopologyV3::new(
            node("prefix"),
            placements,
            base.graph().manual_rework().to_vec(),
        )
        .unwrap(),
        None,
        base.boundary().clone(),
    )
    .unwrap()
}

#[test]
fn frame003_f3_local_rework_and_skip_respect_the_boundary() {
    let mut session = start_base(base_with_prefix(
        "design",
        Some(SkipPolicyV2::allowed_with(true)),
    ));
    let unchanged = session.clone();
    assert_eq!(
        session.skip(&session.fence(), None, attempt(2), NOW),
        Err(invalid("skip is not permitted at the current placement"))
    );
    assert_eq!(session, unchanged);
    session
        .skip(&session.fence(), Some(reason()), attempt(2), NOW)
        .unwrap();
    assert_eq!(
        session.history_attempt(&attempt(1)).unwrap().lifecycle(),
        AttemptLifecycle::Skipped
    );
    assert_eq!(
        session.active_attempt().unwrap().target().node(),
        &node("design")
    );
    session
        .rework(&session.fence(), &node("prefix"), reason(), attempt(3), NOW)
        .unwrap();
    session
        .complete(&session.fence(), Some(attempt(4)), NOW)
        .unwrap();
    assert_eq!(
        session.active_attempt().unwrap().target().node(),
        &node("design")
    );
    ready_design(&mut session);
    launch(&mut session, 1, 5, vec![]);
    assessor(&mut session, "failure", "scored", Some(79));
    session
        .complete(&session.fence(), Some(attempt(6)), NOW)
        .unwrap();
    session
        .decide(
            &session.fence(),
            option("redo"),
            reason(),
            None,
            attempt(7),
            NOW,
        )
        .unwrap();
    assert_eq!(
        session.active_attempt().unwrap().target().node(),
        &node("assess")
    );
    assert_eq!(
        session.selected().unwrap().identity().execution(),
        &group_id(1)
    );
    assert_eq!(
        session.history_attempt(&attempt(6)).unwrap().validity(),
        AttemptValidityV2::Stale
    );
    reach_review(&mut session, 8);

    // The shipped parser rejects this attempted boundary escape. Core refuses it independently.
    let mut bypass = start_base(base_with_prefix("review-work", None));
    let unchanged = bypass.clone();
    assert_eq!(
        bypass.complete(&bypass.fence(), Some(attempt(2)), NOW),
        Err(invalid(
            "ordinary base routing cannot cross the execution-group boundary"
        ))
    );
    assert_eq!(bypass, unchanged);
}

#[test]
fn frame003_f7_current_optional_guard_selection_is_unevaluable_without_value() {
    let mut session = begin(false, false);
    ready_design(&mut session);
    launch(&mut session, 1, 3, vec![]);
    assessor(&mut session, "failure", "scored", None);
    session
        .complete(&session.fence(), Some(attempt(4)), NOW)
        .unwrap();
    let (active, placement, _) = session.current_parts().unwrap();
    let (definition, _) = session
        .decision(placement, active.target().scope(), &option("redesign"))
        .unwrap();
    let guard = definition
        .guards(&option("redesign"))
        .unwrap()
        .predicates()
        .iter()
        .find(|guard| guard.predicate().item() == &item("score"))
        .unwrap();
    let value = session
        .current_evidence(guard.source(), &item("score"))
        .and_then(|(_, slot)| slot.value());
    assert_eq!(guard.evaluate(value).state(), ConditionStateV2::Unevaluable);
    let unchanged = session.clone();
    assert_eq!(
        session.replan(&session.fence(), feedback(&session), attempt(5), NOW),
        Err(invalid("guard is unmet or unevaluable in current evidence"))
    );
    assert_eq!(session, unchanged);
}

#[test]
fn frame003_f13_result_contract_is_rechecked_independently_of_readiness() {
    let mut session = reach_conditional_terminal();
    assessor(&mut session, "failure", "scored", Some(81));
    let mut mismatched = conditional_results();
    mismatched[2] = ItemSpecV2::integer(mismatched[2].common().clone(), Some(0), Some(99)).unwrap();
    // Simulate a mismatched immutable definition for the independent return defense.
    // The values satisfy both definitions and the input was valid at activation.
    session
        .definitions
        .insert(digest("b"), result_group(mismatched));
    let unchanged = session.clone();
    assert_eq!(
        session.complete(&session.fence(), Some(attempt(6)), NOW),
        Err(invalid(
            "group terminal declarations differ from the base result contract"
        ))
    );
    assert_eq!(session, unchanged);
}

#[test]
fn frame003_f13_stale_revision_time_and_open_cap_leave_state_unchanged() {
    let mut session = begin(false, false);
    let stale = session.fence();
    let later = UnixMillis::new(2);
    session
        .record_item(
            stale.attempt.as_ref().unwrap(),
            &item("approval"),
            Revision::ZERO,
            Some(RecordedItemValueV2::confirm()),
            later,
        )
        .unwrap();
    let current = session.fence();
    let unchanged = session.clone();
    assert_eq!(
        session.retry(&stale, reason(), attempt(3), later),
        Err(DomainError::PreconditionFailed {
            expected: stale.revision,
            actual: current.revision,
        })
    );
    assert_eq!(
        session.retry(&current, reason(), attempt(3), UnixMillis::new(1)),
        Err(invalid(
            "execution transition time predates the latest state"
        ))
    );
    assert_eq!(session, unchanged);

    let mut blocked = begin(false, false);
    let target = blocked.active_attempt().unwrap().target().clone();
    let mut rows: Vec<_> = (1..=MAX_OPEN_BLOCKERS_V2)
        .map(|id| blocker(&target, id, "Current blocker", BlockerState::Open))
        .collect();
    blocked
        .replace_blockers(&blocked.fence(), rows.clone(), NOW)
        .unwrap();
    let unchanged = blocked.clone();
    rows.push(blocker(
        &target,
        MAX_OPEN_BLOCKERS_V2 + 1,
        "One over",
        BlockerState::Open,
    ));
    assert_eq!(
        blocked.replace_blockers(&blocked.fence(), rows, NOW),
        Err(DomainError::BoundExceeded {
            field: "open attempt blockers",
            actual: (MAX_OPEN_BLOCKERS_V2 + 1) as u64,
            maximum: MAX_OPEN_BLOCKERS_V2 as u64,
            unit: BoundUnitV2::Entries,
        })
    );
    assert_eq!(blocked, unchanged);
}

#[test]
fn frame003_w2_replan_keeps_resolved_blocker_and_rejects_id_reuse() {
    let mut session = begin(false, false);
    ready_design(&mut session);
    launch(&mut session, 1, 3, vec![]);
    assessor(&mut session, "failure", "scored", Some(80));
    session
        .complete(&session.fence(), Some(attempt(4)), NOW)
        .unwrap();
    let decision = session.active_attempt().unwrap().target().clone();
    let open = blocker(&decision, 1, "Await a current fact", BlockerState::Open);
    session
        .replace_blockers(&session.fence(), vec![open], NOW)
        .unwrap();
    assert_eq!(
        session.replan(&session.fence(), feedback(&session), attempt(5), NOW),
        Err(DomainError::BlockersPresent)
    );
    let resolved = blocker(&decision, 1, "Await a current fact", BlockerState::Resolved);
    session
        .replace_blockers(&session.fence(), vec![resolved.clone()], NOW)
        .unwrap();
    session
        .replan(&session.fence(), feedback(&session), attempt(5), NOW)
        .unwrap();
    assert_eq!(session.attempt_history()[3].blockers(), [resolved]);
    ready_design(&mut session);
    launch(&mut session, 2, 6, vec![prior(1, None)]);
    let current = session.active_attempt().unwrap().target().clone();
    let reused = blocker(
        &current,
        1,
        "Reused historical identity",
        BlockerState::Open,
    );
    let unchanged = session.clone();
    assert!(
        session
            .replace_blockers(&session.fence(), vec![reused], NOW)
            .is_err()
    );
    assert_eq!(session, unchanged);
}
