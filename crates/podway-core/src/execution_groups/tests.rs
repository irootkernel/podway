use super::*;
use crate::*;
fn group(n: u8) -> GroupExecutionIdV1 {
    GroupExecutionIdV1::new(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
}
fn id(v: &str) -> GraphNodeId {
    GraphNodeId::new(v).unwrap()
}
fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}
fn attempt() -> AttemptId {
    AttemptId::new("00000000-0000-4000-8000-000000000099").unwrap()
}
fn common(prompt: &str) -> ItemCommonV2 {
    ItemCommonV2::new(ItemId::new("score").unwrap(), prompt, None, true).unwrap()
}
fn target(g: u8) -> ScopedAttemptV1 {
    ScopedAttemptV1::new(
        SessionId::new("00000000-0000-4000-8000-000000000098").unwrap(),
        digest(),
        ExecutionScopeV1::Group {
            group_execution_id: group(g),
            definition_digest: digest(),
        },
        id("assess"),
        attempt(),
    )
}
fn input(
    previous: Option<u8>,
    prior_work: Vec<PriorWorkDispositionV1>,
) -> ActivationIdentityInputV1 {
    ActivationIdentityInputV1 {
        execution: group(3),
        session: target(1).session().clone(),
        base_digest: digest(),
        definition_digest: digest(),
        anchor: id("design"),
        goal_revision: None,
        plan: PlanBindingV1::new(attempt(), digest(), vec![]).unwrap(),
        predecessor: previous.map(group),
        working_copy_basis: CheckResultInputBasisV2::new("dirty source", digest()).unwrap(),
        prior_work,
    }
}
#[test]
fn boundary_equality_ignores_only_prompts() {
    let a = ItemSpecV2::integer(common("A"), Some(0), Some(100)).unwrap();
    let b = ItemSpecV2::integer(common("B"), Some(0), Some(100)).unwrap();
    assert!(same_boundary_contract_v1(std::slice::from_ref(&a), &[b]));
    assert!(
        !a.same_boundary_contract(&ItemSpecV2::integer(common("A"), Some(0), Some(99)).unwrap())
    );
    let with_help = ItemCommonV2::new(
        ItemId::new("score").unwrap(),
        "A",
        Some("different".into()),
        true,
    )
    .unwrap();
    assert!(
        !a.same_boundary_contract(&ItemSpecV2::integer(with_help, Some(0), Some(100)).unwrap())
    );
    let mut common = common("A");
    common = common.with_required_when(None).unwrap();
    let c = ItemSpecV2::check_result(
        common,
        OperationId::new("check").unwrap(),
        digest(),
        vec![CheckResultOutcomeV2::Pass],
    )
    .unwrap();
    let d = ItemSpecV2::check_result(
        super::tests::common("B"),
        OperationId::new("check").unwrap(),
        digest(),
        vec![CheckResultOutcomeV2::Fail],
    )
    .unwrap();
    assert!(!c.same_boundary_contract(&d));
}
#[test]
fn activation_preserves_arbitrary_retained_prior_work_and_latest_predecessor() {
    assert!(ActivationIdentityV1::new(input(None, vec![])).is_ok());
    assert!(ActivationIdentityV1::new(input(Some(2), vec![])).is_err());
    let prior = PriorWorkDispositionV1::new(
        group(1),
        None,
        PriorWorkChoiceV1::Retain,
        "G1 code remains after empty G2".into(),
    )
    .unwrap();
    let value = ActivationIdentityV1::new(input(Some(2), vec![prior.clone()])).unwrap();
    assert_eq!(value.predecessor(), Some(&group(2)));
    assert_eq!(value.prior_work()[0].group(), &group(1));
    assert_eq!(value.working_copy_basis().descriptor(), "dirty source");
    assert!(ActivationIdentityV1::new(input(None, vec![prior.clone()])).is_err());
    assert!(ActivationIdentityV1::new(input(Some(2), vec![prior.clone(); 128])).is_ok());
    assert!(ActivationIdentityV1::new(input(Some(2), vec![prior; 129])).is_err());
}
#[test]
fn scoped_delegation_and_counters_distinguish_same_digest_activations() {
    let binding = ScopedDelegationBindingV1::new(target(1), digest());
    assert!(binding.applies_to(&target(1), &digest()));
    assert!(!binding.applies_to(&target(2), &digest()));
    assert!(DecisionEntryV1::new(target(1), group(2), 1).is_err());
    assert!(DecisionEntryV1::new(target(1), group(1), 0).is_err());
    assert_eq!(
        DecisionEntryV1::new(target(1), group(1), u64::MAX)
            .unwrap()
            .ordinal(),
        u64::MAX
    );
    assert!(GroupExecutionIdV1::new("not-an-id").is_err());
    assert!(serde_json::from_str::<GroupExecutionIdV1>("\"not-an-id\"").is_err());
}
#[test]
fn score_and_metadata_bounds_are_exact_without_a_scoring_engine() {
    let evidence = vec![
        PlanItemBindingV1::new(
            ItemId::new("score").unwrap(),
            Revision::new(u64::MAX),
            digest(),
        )
        .unwrap(),
    ];
    for score in [0, 79, 80, 100] {
        assert_eq!(
            AssessmentRecordV1::new(
                target(1),
                AssessmentVerdictV1::Failure,
                AssessmentStageV1::Scored,
                score,
                evidence.clone()
            )
            .unwrap()
            .score(),
            score
        );
    }
    assert!(
        AssessmentRecordV1::new(
            target(1),
            AssessmentVerdictV1::Failure,
            AssessmentStageV1::Scored,
            101,
            evidence
        )
        .is_err()
    );
    assert!(CheckResultInputBasisV2::new("한".repeat(512), digest()).is_ok());
    assert!(CheckResultInputBasisV2::new("한".repeat(513), digest()).is_err());
    assert!(
        PriorWorkDispositionV1::new(group(1), None, PriorWorkChoiceV1::Modify, " ".into()).is_err()
    );
    assert!(
        ReplanFeedbackV1::new(
            target(1),
            OptionId::new("replan").unwrap(),
            "x".repeat(2000),
            CheckResultInputBasisV2::new("basis", digest()).unwrap(),
            "x".repeat(4000)
        )
        .is_ok()
    );
}
#[test]
fn authoring_ports_and_routes_have_closed_local_bounds() {
    assert!(GroupBoundaryV1::new(id("design"), id("design"), vec![], vec![], vec![]).is_err());
    let item = ItemId::new("plan").unwrap();
    assert!(
        GroupBoundaryV1::new(
            id("design"),
            id("review"),
            vec![item.clone(), item],
            vec![],
            vec![]
        )
        .is_err()
    );
    assert!(EvidenceReferenceV3::new(EvidenceSourceV3::Plan, true, vec![]).is_err());
    assert!(EvidenceReferenceV3::new(EvidenceSourceV3::Local(id("work")), false, vec![]).is_ok());
    let p = PlacementV3::new(
        id("choose"),
        NodeDefinitionId::new("choose").unwrap(),
        vec![],
        PlacementKindV3::Decision {
            routes: vec![(OptionId::new("replan").unwrap(), DecisionRouteV3::Replan)],
        },
    )
    .unwrap();
    assert!(TopologyV3::new(id("choose"), vec![p.clone(), p.clone()], vec![]).is_err());
    assert_eq!(
        TopologyV3::new(id("choose"), vec![p], vec![])
            .unwrap()
            .nodes()
            .len(),
        1
    );
    assert_eq!(
        (
            MAX_GROUP_EXECUTIONS_V1,
            MAX_GROUP_PLACEMENTS_V1,
            MAX_GROUP_EDGES_V1,
            MAX_SESSION_PLACEMENTS_V1,
            MAX_SESSION_ATTEMPTS_V1
        ),
        (64, 1024, 8192, 16384, 65536)
    );
}

#[test]
fn boundary_contract_preserves_all_item_declaration_kinds_and_conditions() {
    let variants = |prompt: &str| {
        vec![
            ItemSpecV2::confirm(common(prompt)),
            ItemSpecV2::text(common(prompt), 1, 200, true).unwrap(),
            ItemSpecV2::choice(common(prompt), vec!["success".into(), "failure".into()]).unwrap(),
            ItemSpecV2::integer(common(prompt), Some(0), Some(100)).unwrap(),
            ItemSpecV2::list(common(prompt), 0, 100, 20, 1000, true).unwrap(),
            ItemSpecV2::artifact(common(prompt), vec!["text/plain".into()]).unwrap(),
            ItemSpecV2::check_result(
                common(prompt),
                OperationId::new("check").unwrap(),
                digest(),
                vec![CheckResultOutcomeV2::Pass],
            )
            .unwrap(),
        ]
    };
    for (a, b) in variants("First").iter().zip(variants("Second")) {
        assert!(a.same_boundary_contract(&b));
    }
    let optional = |condition| {
        ItemSpecV2::text(
            ItemCommonV2::new(ItemId::new("reason").unwrap(), "Why", None, false)
                .unwrap()
                .with_required_when(condition)
                .unwrap(),
            1,
            200,
            true,
        )
        .unwrap()
    };
    let condition = ItemConditionV2::new(vec![ItemPredicateV2::new(
        ItemId::new("flag").unwrap(),
        false,
        ItemPredicateOperatorV2::Equals(PredicateScalarV2::Boolean(true)),
    )])
    .unwrap();
    assert!(!optional(None).same_boundary_contract(&optional(Some(condition))));
    let mut zero_goal = input(None, vec![]);
    zero_goal.goal_revision = Some(GoalRevisionNumberV2::ZERO);
    assert!(ActivationIdentityV1::new(zero_goal).is_err());
    let nodes = (0..1024)
        .map(|n| {
            PlacementV3::new(
                id(&format!("node-{n}")),
                NodeDefinitionId::new("work").unwrap(),
                vec![],
                PlacementKindV3::Action {
                    outcome: ActionOutcomeV2::Terminal,
                    skip: None,
                    delegation: None,
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(TopologyV3::new(id("node-0"), nodes.clone(), vec![]).is_ok());
    let mut too_many = nodes;
    too_many.push(
        PlacementV3::new(
            id("extra"),
            NodeDefinitionId::new("work").unwrap(),
            vec![],
            PlacementKindV3::Action {
                outcome: ActionOutcomeV2::Terminal,
                skip: None,
                delegation: None,
            },
        )
        .unwrap(),
    );
    assert!(TopologyV3::new(id("node-0"), too_many, vec![]).is_err());
}

#[test]
fn scoped_guards_distinguish_local_plan_and_selected_current_result() {
    let item = ItemId::new("score").unwrap();
    let local = EvidencePredicateV3::new(
        EvidenceSourceV3::Local(id("assess")),
        item.clone(),
        false,
        ItemPredicateOperatorV2::AtLeast(80),
    );
    let plan = EvidencePredicateV3::new(
        EvidenceSourceV3::Plan,
        item.clone(),
        false,
        ItemPredicateOperatorV2::AtLeast(80),
    );
    let result = EvidencePredicateV3::new(
        EvidenceSourceV3::ExecutionResult,
        item,
        false,
        ItemPredicateOperatorV2::AtLeast(80),
    );
    assert_ne!(local.source(), result.source());
    assert_ne!(plan.source(), result.source());
    assert_eq!(
        result
            .evaluate(Some(&RecordedItemValueV2::integer(80)))
            .state(),
        ConditionStateV2::Met
    );
    assert_eq!(
        result
            .evaluate(Some(&RecordedItemValueV2::integer(79)))
            .state(),
        ConditionStateV2::Unmet
    );
    assert_eq!(result.evaluate(None).state(), ConditionStateV2::Unevaluable);
    assert!(OptionGuardV3::new(vec![]).is_err());
    assert!(OptionGuardV3::new(vec![result.clone(); 4]).is_ok());
    assert!(OptionGuardV3::new(vec![result.clone(); 5]).is_err());
    let metadata = DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
        id: NodeDefinitionId::new("review").unwrap(),
        title: "Review".into(),
        description: None,
        objective: "Assess group output".into(),
        prompt: "Choose a declared route".into(),
        evidence_guidance: vec![],
        items: vec![],
        options: vec![
            DecisionOptionV2::new(OptionId::new("replan").unwrap(), "Replan", None).unwrap(),
        ],
        reason: ReasonPolicyV2::new(true, None).unwrap(),
        assessment: None,
    })
    .unwrap();
    let guard = OptionGuardV3::new(vec![result]).unwrap();
    let definition = DecisionDefinitionV3::new(
        metadata.clone(),
        vec![(OptionId::new("replan").unwrap(), guard.clone())],
    )
    .unwrap();
    assert_eq!(
        definition
            .guards(&OptionId::new("replan").unwrap())
            .unwrap()
            .predicates()[0]
            .source(),
        &EvidenceSourceV3::ExecutionResult
    );
    assert!(
        DecisionDefinitionV3::new(
            metadata,
            vec![(OptionId::new("undeclared").unwrap(), guard)]
        )
        .is_err()
    );
}

#[test]
fn regression_header_preserves_existing_version_whitespace_semantics() {
    let header = DefinitionHeaderV3::new(
        NodeDefinitionId::new("base").unwrap(),
        " ".into(),
        "Base".into(),
        "Purpose".into(),
        None,
    )
    .expect("existing nonempty version text remains allowed");
    assert_eq!(header.version(), " ");
}

#[test]
fn regression_topology_rejects_missing_entry_placement() {
    let placement = PlacementV3::new(
        id("present"),
        NodeDefinitionId::new("work").unwrap(),
        vec![],
        PlacementKindV3::Action {
            outcome: ActionOutcomeV2::Terminal,
            skip: None,
            delegation: None,
        },
    )
    .unwrap();
    assert!(TopologyV3::new(id("absent"), vec![placement], vec![]).is_err());
}

#[test]
fn regression_activation_diagnostics_identify_goal_and_self_reference() {
    let mut value = input(None, vec![]);
    value.goal_revision = Some(GoalRevisionNumberV2::ZERO);
    assert_eq!(
        ActivationIdentityV1::new(value).unwrap_err(),
        DomainError::InvalidState {
            reason: "activation goal revision must be positive"
        }
    );
    assert_eq!(
        GroupExecutionIdV1::new("bad").unwrap_err(),
        DomainError::InvalidUuid {
            field: "GroupExecutionIdV1"
        }
    );
}

fn header(version: &str) -> Result<DefinitionHeaderV3, DomainError> {
    DefinitionHeaderV3::new(
        NodeDefinitionId::new("example").unwrap(),
        version.into(),
        "Example".into(),
        "Work".into(),
        None,
    )
}
fn action_definition(name: &str) -> NodeDefinitionV3 {
    NodeDefinitionV3::Action(
        ActionDefinitionV2::new(
            NodeDefinitionId::new(name).unwrap(),
            "Work",
            "Complete work",
            None,
            vec!["Perform the assigned work.".into()],
            vec![],
        )
        .unwrap(),
    )
}
fn action_placement(name: &str) -> PlacementV3 {
    PlacementV3::new(
        id(name),
        NodeDefinitionId::new("work").unwrap(),
        vec![],
        PlacementKindV3::Action {
            outcome: ActionOutcomeV2::Terminal,
            skip: None,
            delegation: None,
        },
    )
    .unwrap()
}
fn boundary() -> GroupBoundaryV1 {
    GroupBoundaryV1::new(
        id("design"),
        id("review"),
        vec![ItemId::new("plan").unwrap()],
        vec![ItemSpecV2::confirm(common("Result"))],
        vec![id("review")],
    )
    .unwrap()
}
#[test]
fn top_level_documents_and_headers_preserve_bounded_value_contracts() {
    let graph = TopologyV3::new(id("work"), vec![action_placement("work")], vec![]).unwrap();
    let base = ProcedureDefinitionV3::new(
        header("1").unwrap(),
        vec![action_definition("work")],
        graph.clone(),
        Some(GoalTrackingOptIn::enabled()),
        boundary(),
    )
    .unwrap();
    assert_eq!(base.header().version(), "1");
    assert!(base.goal_tracking().unwrap().is_enabled());
    assert_eq!(base.boundary().plan_inputs()[0].as_str(), "plan");
    assert_eq!(base.definitions().len(), 1);
    assert_eq!(base.graph().entry(), &id("work"));
    let group = ExecutionGroupDefinitionV1::new(
        header("1").unwrap(),
        vec![action_definition("work")],
        graph.clone(),
        vec![ItemSpecV2::confirm(common("Input"))],
    )
    .unwrap();
    assert_eq!(group.header().id().as_str(), "example");
    assert_eq!(group.definitions().len(), 1);
    assert_eq!(group.graph().nodes().len(), 1);
    assert_eq!(group.plan_inputs().len(), 1);
    assert!(
        ProcedureDefinitionV3::new(
            header("1").unwrap(),
            vec![],
            graph.clone(),
            None,
            boundary()
        )
        .is_err()
    );
    assert!(
        ExecutionGroupDefinitionV1::new(
            header("1").unwrap(),
            vec![action_definition("work"); 2],
            graph.clone(),
            vec![]
        )
        .is_err()
    );
    let nodes = (0..MAX_NODE_DEFINITIONS_PER_PROCEDURE_V2)
        .map(|n| action_definition(&format!("work-{n}")))
        .collect::<Vec<_>>();
    assert!(
        ExecutionGroupDefinitionV1::new(header("1").unwrap(), nodes.clone(), graph.clone(), vec![])
            .is_ok()
    );
    let mut extra = nodes;
    extra.push(action_definition("extra"));
    assert!(
        ProcedureDefinitionV3::new(header("1").unwrap(), extra, graph, None, boundary()).is_err()
    );
    assert!(header("").is_err());
    assert!(header(&"한".repeat(64)).is_ok());
    assert!(header(&"한".repeat(65)).is_err());
    for (name, purpose, description, ok) in [
        (
            "n".repeat(120),
            "p".repeat(500),
            Some("d".repeat(1000)),
            true,
        ),
        ("n".repeat(121), "p".into(), None, false),
        ("n".into(), "p".repeat(501), None, false),
        ("n".into(), "p".into(), Some("d".repeat(1001)), false),
        (" ".into(), "p".into(), None, false),
        ("n".into(), " ".into(), None, false),
    ] {
        assert_eq!(
            DefinitionHeaderV3::new(
                NodeDefinitionId::new("example").unwrap(),
                "1".into(),
                name,
                purpose,
                description
            )
            .is_ok(),
            ok
        );
    }
}
#[test]
fn port_route_collections_and_delegation_admit_exact_bounds_only() {
    assert!(TopologyV3::new(id("work"), vec![], vec![]).is_err());
    for count in [MAX_GROUP_PLACEMENTS_V1, MAX_GROUP_PLACEMENTS_V1 + 1] {
        let nodes = (0..count)
            .map(|n| action_placement(&format!("work-{n}")))
            .collect();
        assert_eq!(
            TopologyV3::new(id("work-0"), nodes, vec![]).is_ok(),
            count == MAX_GROUP_PLACEMENTS_V1
        );
    }
    let items = (0..16)
        .map(|n| ItemId::new(format!("item-{n}")).unwrap())
        .collect::<Vec<_>>();
    assert!(EvidenceReferenceV3::new(EvidenceSourceV3::Plan, true, items.clone()).is_ok());
    let mut extra = items;
    extra.push(ItemId::new("extra").unwrap());
    assert!(EvidenceReferenceV3::new(EvidenceSourceV3::ExecutionResult, true, extra).is_err());
    assert!(
        EvidenceReferenceV3::new(
            EvidenceSourceV3::Plan,
            true,
            vec![ItemId::new("same").unwrap(); 2]
        )
        .is_err()
    );
    let evidence =
        EvidenceReferenceV3::new(EvidenceSourceV3::Local(id("source")), true, vec![]).unwrap();
    for count in [
        MAX_EVIDENCE_REFERENCES_PER_PLACEMENT_V2,
        MAX_EVIDENCE_REFERENCES_PER_PLACEMENT_V2 + 1,
    ] {
        assert_eq!(
            PlacementV3::new(
                id("work"),
                NodeDefinitionId::new("work").unwrap(),
                vec![evidence.clone(); count],
                PlacementKindV3::Action {
                    outcome: ActionOutcomeV2::Terminal,
                    skip: None,
                    delegation: None
                }
            )
            .is_ok(),
            count == MAX_EVIDENCE_REFERENCES_PER_PLACEMENT_V2
        );
    }
    for count in [
        0,
        MAX_ROUTES_PER_DECISION_V2,
        MAX_ROUTES_PER_DECISION_V2 + 1,
    ] {
        let routes = (0..count)
            .map(|n| {
                (
                    OptionId::new(format!("option-{n}")).unwrap(),
                    DecisionRouteV3::Advance(id("finish")),
                )
            })
            .collect();
        assert_eq!(
            PlacementV3::new(
                id("choose"),
                NodeDefinitionId::new("choose").unwrap(),
                vec![],
                PlacementKindV3::Decision { routes }
            )
            .is_ok(),
            count == MAX_ROUTES_PER_DECISION_V2
        );
    }
    assert!(
        PlacementV3::new(
            id("choose"),
            NodeDefinitionId::new("choose").unwrap(),
            vec![],
            PlacementKindV3::Decision {
                routes: vec![(OptionId::new("same").unwrap(), DecisionRouteV3::Replan); 2]
            }
        )
        .is_err()
    );
    let delegation = DelegationDeclarationV1::new(
        DelegationBindingRefV1::new("native").unwrap(),
        DelegationRoleRefV1::new("worker".into()).unwrap(),
        digest(),
        1,
        DelegationAfterAcceptV1::Retain,
    )
    .unwrap();
    assert!(
        PlacementV3::new(
            id("work"),
            NodeDefinitionId::new("work").unwrap(),
            vec![],
            PlacementKindV3::Action {
                outcome: ActionOutcomeV2::Terminal,
                skip: None,
                delegation: Some(delegation.clone())
            }
        )
        .is_ok()
    );
    assert!(
        PlacementV3::new(
            id("work"),
            NodeDefinitionId::new("work").unwrap(),
            vec![],
            PlacementKindV3::Action {
                outcome: ActionOutcomeV2::Terminal,
                skip: Some(SkipPolicyV2::allowed_with(true)),
                delegation: Some(delegation)
            }
        )
        .is_err()
    );
    for count in [64, 65] {
        let targets = (0..count).map(|n| id(&format!("target-{n}"))).collect();
        assert_eq!(
            TopologyV3::new(id("work"), vec![action_placement("work")], targets).is_ok(),
            count == 64
        );
    }
    assert!(
        TopologyV3::new(
            id("work"),
            vec![action_placement("work")],
            vec![id("same"); 2]
        )
        .is_err()
    );
    for count in [MAX_ITEMS_PER_DEFINITION_V2, MAX_ITEMS_PER_DEFINITION_V2 + 1] {
        let inputs = (0..count)
            .map(|n| ItemId::new(format!("input-{n}")).unwrap())
            .collect();
        assert_eq!(
            GroupBoundaryV1::new(id("design"), id("review"), inputs, vec![], vec![]).is_ok(),
            count == MAX_ITEMS_PER_DEFINITION_V2
        );
    }
    for count in [MAX_GROUP_PLACEMENTS_V1, MAX_GROUP_PLACEMENTS_V1 + 1] {
        let decisions = (0..count).map(|n| id(&format!("review-{n}"))).collect();
        assert_eq!(
            GroupBoundaryV1::new(id("design"), id("review"), vec![], vec![], decisions).is_ok(),
            count == MAX_GROUP_PLACEMENTS_V1
        );
    }
    assert!(
        GroupBoundaryV1::new(
            id("design"),
            id("review"),
            vec![],
            vec![],
            vec![id("same"); 2]
        )
        .is_err()
    );
}
#[test]
fn recorded_bindings_base_scope_and_feedback_cover_rejection_boundaries() {
    let items = (0..MAX_ITEMS_PER_DEFINITION_V2)
        .map(|n| {
            ItemSpecV2::confirm(
                ItemCommonV2::new(
                    ItemId::new(format!("result-{n}")).unwrap(),
                    "Result",
                    None,
                    true,
                )
                .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        GroupBoundaryV1::new(id("design"), id("review"), vec![], items.clone(), vec![]).is_ok()
    );
    let mut extra = items;
    extra.push(ItemSpecV2::confirm(common("Extra")));
    assert_eq!(
        GroupBoundaryV1::new(id("design"), id("review"), vec![], extra, vec![]).unwrap_err(),
        DomainError::BoundExceeded {
            field: "boundary items",
            actual: (MAX_ITEMS_PER_DEFINITION_V2 + 1) as u64,
            maximum: MAX_ITEMS_PER_DEFINITION_V2 as u64,
            unit: BoundUnitV2::Items
        }
    );
    assert_eq!(
        GroupBoundaryV1::new(
            id("design"),
            id("review"),
            vec![],
            vec![ItemSpecV2::confirm(common("Duplicate")); 2],
            vec![]
        )
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "boundary item identifiers must be unique"
        }
    );
    assert!(
        PlanItemBindingV1::new(ItemId::new("score").unwrap(), Revision::ZERO, digest()).is_err()
    );
    for revision in [1, u64::MAX] {
        assert_eq!(
            PlanItemBindingV1::new(
                ItemId::new("score").unwrap(),
                Revision::new(revision),
                digest()
            )
            .unwrap()
            .revision()
            .get(),
            revision
        );
    }
    let base = ScopedAttemptV1::new(
        target(1).session().clone(),
        digest(),
        ExecutionScopeV1::Base,
        id("review"),
        attempt(),
    );
    let package = ScopedDelegationBindingV1::new(base.clone(), digest());
    assert!(package.applies_to(&base, &digest()));
    assert!(!package.applies_to(&target(1), &digest()));
    assert_eq!(
        DecisionEntryV1::new(base.clone(), group(1), 1)
            .unwrap()
            .target()
            .scope(),
        &ExecutionScopeV1::Base
    );
    for count in [128, 129] {
        let items = (0..count)
            .map(|n| {
                PlanItemBindingV1::new(
                    ItemId::new(format!("item-{n}")).unwrap(),
                    Revision::new(1),
                    digest(),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(
            PlanBindingV1::new(attempt(), digest(), items).is_ok(),
            count == 128
        );
    }
    let item =
        PlanItemBindingV1::new(ItemId::new("same").unwrap(), Revision::new(1), digest()).unwrap();
    assert!(PlanBindingV1::new(attempt(), digest(), vec![item.clone(); 2]).is_err());
    for count in [0, 16, 17] {
        let items = (0..count)
            .map(|n| {
                PlanItemBindingV1::new(
                    ItemId::new(format!("item-{n}")).unwrap(),
                    Revision::new(1),
                    digest(),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(
            AssessmentRecordV1::new(
                base.clone(),
                AssessmentVerdictV1::Failure,
                AssessmentStageV1::Scored,
                80,
                items
            )
            .is_ok(),
            count == 16
        );
    }
    assert!(
        AssessmentRecordV1::new(
            base.clone(),
            AssessmentVerdictV1::Failure,
            AssessmentStageV1::Scored,
            80,
            vec![item; 2]
        )
        .is_err()
    );
    let mut self_predecessor = input(Some(3), vec![]);
    self_predecessor.prior_work = vec![
        PriorWorkDispositionV1::new(group(1), None, PriorWorkChoiceV1::Retain, "retained".into())
            .unwrap(),
    ];
    assert_eq!(
        ActivationIdentityV1::new(self_predecessor).unwrap_err(),
        DomainError::InvalidState {
            reason: "activation predecessor cannot reference itself"
        }
    );
    let self_work = PriorWorkDispositionV1::new(
        group(3),
        None,
        PriorWorkChoiceV1::Retain,
        "invalid self".into(),
    )
    .unwrap();
    assert_eq!(
        ActivationIdentityV1::new(input(Some(2), vec![self_work])).unwrap_err(),
        DomainError::InvalidState {
            reason: "prior work cannot reference the new activation"
        }
    );
    for reference in [
        HistoricalReferenceV1::Attempt(attempt()),
        HistoricalReferenceV1::Item(HistoricalItemV1::new(
            attempt(),
            ItemId::new("score").unwrap(),
            digest(),
        )),
    ] {
        let value = PriorWorkDispositionV1::new(
            group(1),
            Some(reference),
            PriorWorkChoiceV1::Revalidate,
            "r".repeat(2000),
        )
        .unwrap();
        assert!(value.item().is_some());
    }
    assert!(
        PriorWorkDispositionV1::new(group(1), None, PriorWorkChoiceV1::Modify, "r".repeat(2001))
            .is_err()
    );
    for (reason, feedback, ok) in [(2000, 4000, true), (2001, 1, false), (1, 4001, false)] {
        assert_eq!(
            ReplanFeedbackV1::new(
                base.clone(),
                OptionId::new("replan").unwrap(),
                "r".repeat(reason),
                CheckResultInputBasisV2::new("source", digest()).unwrap(),
                "f".repeat(feedback)
            )
            .is_ok(),
            ok
        );
    }
    let serde_error = serde_json::from_str::<GroupExecutionIdV1>("\"bad\"").unwrap_err();
    assert!(serde_error.to_string().contains("GroupExecutionIdV1"));
}
#[test]
fn exact_boundary_contract_and_scoped_guard_metadata_reject_substitutions() {
    let item = |name: &str| {
        ItemSpecV2::check_result(
            ItemCommonV2::new(ItemId::new(name).unwrap(), "Result", None, true).unwrap(),
            OperationId::new("check").unwrap(),
            digest(),
            vec![CheckResultOutcomeV2::Pass],
        )
        .unwrap()
    };
    let original = item("result");
    let other = item("other");
    assert!(!same_boundary_contract_v1(std::slice::from_ref(&original), &[]));
    assert!(!same_boundary_contract_v1(
        &[original.clone(), other.clone()],
        &[other, original.clone()]
    ));
    let changed_operation = ItemSpecV2::check_result(
        common("Result"),
        OperationId::new("different").unwrap(),
        digest(),
        vec![CheckResultOutcomeV2::Pass],
    )
    .unwrap();
    let matching = ItemSpecV2::check_result(
        common("Result"),
        OperationId::new("check").unwrap(),
        digest(),
        vec![CheckResultOutcomeV2::Pass],
    )
    .unwrap();
    assert!(!matching.same_boundary_contract(&changed_operation));
    let changed_digest = ItemSpecV2::check_result(
        common("Result"),
        OperationId::new("check").unwrap(),
        Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
        vec![CheckResultOutcomeV2::Pass],
    )
    .unwrap();
    assert!(!matching.same_boundary_contract(&changed_digest));
    let guard = OptionGuardV3::new(vec![EvidencePredicateV3::new(
        EvidenceSourceV3::Plan,
        ItemId::new("score").unwrap(),
        false,
        ItemPredicateOperatorV2::AtLeast(80),
    )])
    .unwrap();
    let old_guard = OptionGuardV2::new(vec![EvidencePredicateV2::new(
        id("assess"),
        ItemId::new("score").unwrap(),
        false,
        ItemPredicateOperatorV2::AtLeast(80),
    )])
    .unwrap();
    let metadata = |legacy| {
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            id: NodeDefinitionId::new("choose").unwrap(),
            title: "Choose".into(),
            description: None,
            objective: "Choose route".into(),
            prompt: "Choose".into(),
            evidence_guidance: vec![],
            items: vec![],
            options: vec![
                DecisionOptionV2::new(OptionId::new("replan").unwrap(), "Replan", None)
                    .unwrap()
                    .with_guards(legacy),
            ],
            reason: ReasonPolicyV2::new(true, None).unwrap(),
            assessment: None,
        })
        .unwrap()
    };
    assert!(DecisionDefinitionV3::new(metadata(Some(old_guard)), vec![]).is_err());
    assert!(
        DecisionDefinitionV3::new(
            metadata(None),
            vec![(OptionId::new("replan").unwrap(), guard); 2]
        )
        .is_err()
    );
}
