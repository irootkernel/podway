use crate::procedure_v2::invalid;
use crate::{
    ActionDefinitionV2, AssessmentContractV2, AssessmentOutcomeMappingV2, AssessmentTargetV2,
    DecisionDefinitionInputV2, DecisionDefinitionV2, DecisionOptionV2, GoalOutcome, ItemSpecV2,
    NodeKindV2, ReasonPolicyV2,
};

use super::helpers::{def_id, item, opt_id};

fn reason() -> ReasonPolicyV2 {
    ReasonPolicyV2::new(true, None).unwrap()
}

fn option_with(id: &str) -> DecisionOptionV2 {
    DecisionOptionV2::new(opt_id(id), format!("Label {id}"), None).unwrap()
}

fn decision_input() -> DecisionDefinitionInputV2 {
    DecisionDefinitionInputV2 {
        id: def_id("dec"),
        title: "title".to_owned(),
        description: None,
        objective: "objective".to_owned(),
        prompt: "prompt".to_owned(),
        evidence_guidance: Vec::new(),
        items: Vec::new(),
        options: vec![option_with("only")],
        reason: reason(),
        assessment: None,
    }
}

fn valid_action_definition() -> ActionDefinitionV2 {
    ActionDefinitionV2::new(
        def_id("implement"),
        "Implement the change",
        "Produce an implementation.",
        None,
        vec!["Do the work.".to_owned()],
        vec![item("implementation-summary")],
    )
    .unwrap()
}

fn valid_decision_definition() -> DecisionDefinitionV2 {
    DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
        id: def_id("evaluate"),
        title: "Evaluate the result".to_owned(),
        description: None,
        objective: "Only acceptable evidence may proceed.".to_owned(),
        prompt: "Is the result acceptable?".to_owned(),
        evidence_guidance: Vec::new(),
        items: Vec::new(),
        options: vec![option_with("passed"), option_with("failed")],
        reason: reason(),
        assessment: None,
    })
    .unwrap()
}

#[test]
fn action_definition_accepts_at_limit_scalars_and_collections() {
    let title = "t".repeat(120);
    let intent = "i".repeat(300);
    let description = "d".repeat(1000);
    let instructions = vec!["x".repeat(1000); 16];
    let mut items: Vec<ItemSpecV2> = (0..64)
        .map(|index| item(&format!("item-{index}")))
        .collect();
    let at_limit = ActionDefinitionV2::new(
        def_id("act"),
        title.clone(),
        intent.clone(),
        Some(description.clone()),
        instructions.clone(),
        items.clone(),
    )
    .unwrap();
    assert_eq!(at_limit.title(), title);
    assert_eq!(at_limit.intent(), intent);
    assert_eq!(at_limit.description(), Some(description.as_str()));
    assert_eq!(at_limit.instructions().len(), 16);
    assert_eq!(at_limit.items().len(), 64);
    assert_eq!(at_limit.node_kind(), NodeKindV2::Action);

    assert_eq!(
        ActionDefinitionV2::new(
            def_id("act"),
            "t".repeat(121),
            "intent",
            None,
            Vec::new(),
            Vec::new(),
        ),
        Err(invalid("definition title"))
    );
    assert_eq!(
        ActionDefinitionV2::new(
            def_id("act"),
            "title",
            "i".repeat(301),
            None,
            Vec::new(),
            Vec::new(),
        ),
        Err(invalid("action intent"))
    );
    assert_eq!(
        ActionDefinitionV2::new(
            def_id("act"),
            "title",
            "intent",
            Some("d".repeat(1001)),
            Vec::new(),
            Vec::new(),
        ),
        Err(invalid("definition description"))
    );
    items.push(item("extra-item"));
    assert_eq!(
        ActionDefinitionV2::new(def_id("act"), "title", "intent", None, Vec::new(), items,),
        Err(invalid("too many definition items"))
    );
    // instruction count one-over the sixteen-entry ceiling.
    let seventeen_instructions = vec!["x".repeat(1000); 17];
    assert_eq!(
        ActionDefinitionV2::new(
            def_id("act"),
            "title",
            "intent",
            None,
            seventeen_instructions,
            Vec::new(),
        ),
        Err(invalid("too many definition instructions"))
    );
}

#[test]
fn action_definition_rejects_blank_required_text_and_empty_instruction_entries() {
    assert_eq!(
        ActionDefinitionV2::new(def_id("act"), "   ", "intent", None, Vec::new(), Vec::new(),),
        Err(invalid("definition title"))
    );
    assert_eq!(
        ActionDefinitionV2::new(
            def_id("act"),
            "title",
            "intent",
            None,
            vec![String::new()],
            Vec::new(),
        ),
        Err(invalid("definition instruction"))
    );
    assert_eq!(
        ActionDefinitionV2::new(
            def_id("act"),
            "title",
            "intent",
            None,
            vec!["x".repeat(1001)],
            Vec::new(),
        ),
        Err(invalid("definition instruction"))
    );
}

#[test]
fn definition_item_identifiers_must_be_unique_within_a_definition() {
    let duplicate_items = vec![item("dup"), item("dup")];
    assert_eq!(
        ActionDefinitionV2::new(
            def_id("act"),
            "title",
            "intent",
            None,
            Vec::new(),
            duplicate_items,
        ),
        Err(invalid("definition item identifiers must be unique"))
    );
}

#[test]
fn decision_definition_enforces_decision_shape_and_bounds() {
    let decision = valid_decision_definition();
    assert_eq!(decision.node_kind(), NodeKindV2::Decision);
    assert_eq!(decision.options().len(), 2);
    assert!(decision.reason().required());

    // the one-option floor is accepted (decision_input's default single option).
    let single_option = DecisionDefinitionV2::new(decision_input()).unwrap();
    assert_eq!(single_option.options().len(), 1);

    let objective_at_limit = "o".repeat(300);
    let prompt_at_limit = "p".repeat(500);
    let guidance = vec!["g".repeat(200); 8];
    let options: Vec<DecisionOptionV2> = (0..8).map(|i| option_with(&format!("opt-{i}"))).collect();
    DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
        objective: objective_at_limit,
        prompt: prompt_at_limit,
        evidence_guidance: guidance,
        options,
        ..decision_input()
    })
    .unwrap();

    assert_eq!(
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            objective: "o".repeat(301),
            ..decision_input()
        }),
        Err(invalid("decision objective"))
    );
    assert_eq!(
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            prompt: "p".repeat(501),
            ..decision_input()
        }),
        Err(invalid("decision prompt"))
    );
    assert_eq!(
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            evidence_guidance: vec!["g".repeat(201)],
            ..decision_input()
        }),
        Err(invalid("evidence guidance"))
    );
    assert_eq!(
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            options: Vec::new(),
            ..decision_input()
        }),
        Err(invalid(
            "decision option count must be between one and eight"
        ))
    );
    // evidence guidance count one-over the eight-entry ceiling.
    assert_eq!(
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            evidence_guidance: vec!["g".repeat(200); 9],
            ..decision_input()
        })
        .unwrap_err(),
        invalid("too many evidence guidance entries")
    );
    let too_many: Vec<DecisionOptionV2> = (0..9).map(|i| option_with(&format!("o-{i}"))).collect();
    assert_eq!(
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            options: too_many,
            ..decision_input()
        })
        .unwrap_err(),
        invalid("decision option count must be between one and eight")
    );
}

#[test]
fn decision_option_identifiers_must_be_unique() {
    let options = vec![
        DecisionOptionV2::new(opt_id("dup"), "Label one", None).unwrap(),
        DecisionOptionV2::new(opt_id("dup"), "Label two", None).unwrap(),
    ];
    assert_eq!(
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            options,
            ..decision_input()
        }),
        Err(invalid("decision option identifiers must be unique"))
    );
}

#[test]
fn decision_option_enforces_label_and_criteria_length_bounds() {
    let label_at_limit = DecisionOptionV2::new(opt_id("opt"), "l".repeat(120), None).unwrap();
    assert_eq!(label_at_limit.label(), "l".repeat(120));
    assert_eq!(
        DecisionOptionV2::new(opt_id("opt"), "l".repeat(121), None),
        Err(invalid("option label"))
    );

    let criteria_at_limit =
        DecisionOptionV2::new(opt_id("opt"), "label", Some("c".repeat(500))).unwrap();
    assert_eq!(criteria_at_limit.criteria(), Some("c".repeat(500).as_str()));
    assert_eq!(
        DecisionOptionV2::new(opt_id("opt"), "label", Some("c".repeat(501))),
        Err(invalid("option criteria"))
    );
}

#[test]
fn decision_and_action_definitions_are_shape_distinct() {
    let action = valid_action_definition();
    let decision = valid_decision_definition();
    assert_eq!(action.node_kind(), NodeKindV2::Action);
    assert_eq!(decision.node_kind(), NodeKindV2::Decision);
    // Action definitions carry an intent and never an assessment contract.
    assert!(!action.intent().is_empty());
    // Decision definitions carry an objective and a reason policy, not an intent.
    assert!(!decision.objective().is_empty());
    assert!(decision.reason().required());
    assert!(decision.assessment().is_none());
}

#[test]
fn reason_policy_requires_true() {
    assert_eq!(
        ReasonPolicyV2::new(false, None),
        Err(invalid("a declared reason policy requires required: true"))
    );
    let with_prompt = ReasonPolicyV2::new(true, Some("p".repeat(300))).unwrap();
    assert_eq!(with_prompt.prompt(), Some("p".repeat(300).as_str()));
    assert_eq!(
        ReasonPolicyV2::new(true, Some("p".repeat(301))),
        Err(invalid("reason policy prompt"))
    );
}

#[test]
fn assessment_contract_enforces_target_mapping_count_and_unique_options() {
    let outcomes = vec![
        AssessmentOutcomeMappingV2::new(opt_id("achieved-option"), GoalOutcome::Achieved),
        AssessmentOutcomeMappingV2::new(opt_id("failed-option"), GoalOutcome::NotAchieved),
        AssessmentOutcomeMappingV2::new(opt_id("superseded-option"), GoalOutcome::Superseded),
    ];
    let contract = AssessmentContractV2::new(outcomes.clone()).unwrap();
    assert_eq!(contract.target(), AssessmentTargetV2::new());
    assert_eq!(contract.outcomes().len(), 3);

    let eight: Vec<AssessmentOutcomeMappingV2> = (0..8)
        .map(|i| AssessmentOutcomeMappingV2::new(opt_id(&format!("o-{i}")), GoalOutcome::Achieved))
        .collect();
    assert!(AssessmentContractV2::new(eight).is_ok());

    // one-over the eight-mapping ceiling.
    let nine: Vec<AssessmentOutcomeMappingV2> = (0..9)
        .map(|i| AssessmentOutcomeMappingV2::new(opt_id(&format!("o-{i}")), GoalOutcome::Achieved))
        .collect();
    assert_eq!(
        AssessmentContractV2::new(nine).unwrap_err(),
        invalid("assessment outcome mapping count must be between three and eight")
    );

    assert_eq!(
        AssessmentContractV2::new(outcomes[..2].to_vec()).unwrap_err(),
        invalid("assessment outcome mapping count must be between three and eight")
    );
    let duplicate_option = vec![
        AssessmentOutcomeMappingV2::new(opt_id("dup"), GoalOutcome::Achieved),
        AssessmentOutcomeMappingV2::new(opt_id("dup"), GoalOutcome::NotAchieved),
        AssessmentOutcomeMappingV2::new(opt_id("other"), GoalOutcome::Superseded),
    ];
    assert_eq!(
        AssessmentContractV2::new(duplicate_option).unwrap_err(),
        invalid("assessment outcome option identifiers must be unique")
    );
}

#[test]
fn assessment_target_and_goal_outcome_round_trip_strings() {
    assert_eq!(
        "session_goal".parse::<AssessmentTargetV2>().unwrap(),
        AssessmentTargetV2::new()
    );
    assert_eq!(
        "procedure_goal".parse::<AssessmentTargetV2>(),
        Err(invalid("assessment target must be session_goal"))
    );
    assert_eq!(
        "achieved".parse::<GoalOutcome>().unwrap(),
        GoalOutcome::Achieved
    );
    assert_eq!(
        "not_achieved".parse::<GoalOutcome>().unwrap(),
        GoalOutcome::NotAchieved
    );
    assert_eq!(
        "superseded".parse::<GoalOutcome>().unwrap(),
        GoalOutcome::Superseded
    );
    assert_eq!(
        "partial".parse::<GoalOutcome>(),
        Err(invalid("unknown goal outcome"))
    );
    assert_eq!(GoalOutcome::Achieved.as_str(), "achieved");
    assert_eq!(GoalOutcome::NotAchieved.as_str(), "not_achieved");
    assert_eq!(GoalOutcome::Superseded.as_str(), "superseded");
}
