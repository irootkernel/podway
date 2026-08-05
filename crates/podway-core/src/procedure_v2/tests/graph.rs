use crate::procedure_v2::invalid;
use crate::{
    ActionOutcomeV2, ActionPlacementV2, DecisionPlacementV2, DecisionRouteEntryV2,
    DecisionRouteMapV2, DecisionRouteV2, EvidenceFromListV2, EvidenceReferenceV2, GraphNodeId,
    GraphPlacementV2, ItemId, ManualReworkTargetListV2, NodeKindV2, ProcedureGraphV2, SkipPolicyV2,
    TransitionEffectV2,
};

use super::helpers::{def_id, opt_id};

fn node_id(value: &str) -> GraphNodeId {
    GraphNodeId::new(value).unwrap()
}

#[test]
fn skip_policy_requires_allowed_true() {
    assert_eq!(
        SkipPolicyV2::new(false, false),
        Err(invalid("a declared skip policy requires allowed: true"))
    );
    assert_eq!(
        SkipPolicyV2::new(false, true),
        Err(invalid("a declared skip policy requires allowed: true"))
    );
    let policy = SkipPolicyV2::new(true, true).unwrap();
    assert!(policy.is_allowed());
    assert!(policy.reason_required());
    let no_reason = SkipPolicyV2::allowed_with(false);
    assert!(no_reason.is_allowed());
    assert!(!no_reason.reason_required());
}

#[test]
fn action_outcome_is_exactly_next_or_terminal() {
    let next = ActionOutcomeV2::next(node_id("target"));
    assert!(!next.is_terminal());
    assert_eq!(next.next_target(), Some(&node_id("target")));
    let terminal = ActionOutcomeV2::terminal();
    assert!(terminal.is_terminal());
    assert_eq!(terminal.next_target(), None);
}

#[test]
fn decision_route_map_enforces_bounds_and_unique_option_keys() {
    let route = DecisionRouteV2::new(node_id("target"), TransitionEffectV2::Advance);
    let one = vec![DecisionRouteEntryV2::new(opt_id("a"), route.clone()); 1];
    assert!(DecisionRouteMapV2::new(one).is_ok());
    let eight: Vec<DecisionRouteEntryV2> = (0..8)
        .map(|i| DecisionRouteEntryV2::new(opt_id(&format!("o-{i}")), route.clone()))
        .collect();
    assert!(DecisionRouteMapV2::new(eight).is_ok());

    assert_eq!(
        DecisionRouteMapV2::new(Vec::new()).unwrap_err(),
        invalid("route count must be between one and eight")
    );
    let nine: Vec<DecisionRouteEntryV2> = (0..9)
        .map(|i| DecisionRouteEntryV2::new(opt_id(&format!("o-{i}")), route.clone()))
        .collect();
    assert_eq!(
        DecisionRouteMapV2::new(nine).unwrap_err(),
        invalid("route count must be between one and eight")
    );
    let duplicate = vec![
        DecisionRouteEntryV2::new(opt_id("dup"), route.clone()),
        DecisionRouteEntryV2::new(opt_id("dup"), route),
    ];
    assert_eq!(
        DecisionRouteMapV2::new(duplicate).unwrap_err(),
        invalid("route option identifiers must be unique")
    );
}

#[test]
fn transition_effect_round_trips_authoring_strings() {
    assert_eq!(
        "advance".parse::<TransitionEffectV2>().unwrap(),
        TransitionEffectV2::Advance
    );
    assert_eq!(
        "rework".parse::<TransitionEffectV2>().unwrap(),
        TransitionEffectV2::Rework
    );
    assert_eq!(
        "branch".parse::<TransitionEffectV2>(),
        Err(invalid("unknown transition effect"))
    );
}

#[test]
fn evidence_reference_enforces_selected_item_bounds_and_uniqueness() {
    let required = EvidenceReferenceV2::new(node_id("source"), true, None).unwrap();
    assert!(required.required());
    assert_eq!(required.selected_items(), None);

    let items: Vec<ItemId> = (0..16)
        .map(|i| ItemId::new(format!("item-{i}")).unwrap())
        .collect();
    let at_limit = EvidenceReferenceV2::new(node_id("source"), false, Some(items.clone())).unwrap();
    assert!(!at_limit.required());
    assert_eq!(at_limit.selected_items(), Some(items.as_slice()));

    assert_eq!(
        EvidenceReferenceV2::new(node_id("source"), true, Some(Vec::new())).unwrap_err(),
        invalid("selected item count must be between one and sixteen")
    );
    let too_many: Vec<ItemId> = (0..17)
        .map(|i| ItemId::new(format!("item-{i}")).unwrap())
        .collect();
    assert_eq!(
        EvidenceReferenceV2::new(node_id("source"), true, Some(too_many)).unwrap_err(),
        invalid("selected item count must be between one and sixteen")
    );
    let duplicate = vec![ItemId::new("dup").unwrap(), ItemId::new("dup").unwrap()];
    assert_eq!(
        EvidenceReferenceV2::new(node_id("source"), true, Some(duplicate)).unwrap_err(),
        invalid("selected item identifiers must be unique")
    );
}

#[test]
fn evidence_from_list_enforces_one_to_eight_entries() {
    let one = vec![EvidenceReferenceV2::new(node_id("a"), true, None).unwrap()];
    assert!(EvidenceFromListV2::new(one).is_ok());
    let eight: Vec<EvidenceReferenceV2> = (0..8)
        .map(|i| EvidenceReferenceV2::new(node_id(&format!("n-{i}")), true, None).unwrap())
        .collect();
    assert!(EvidenceFromListV2::new(eight).is_ok());
    assert_eq!(
        EvidenceFromListV2::new(Vec::new()).unwrap_err(),
        invalid("evidence reference count must be between one and eight")
    );
    // one-over the eight-entry ceiling.
    let nine: Vec<EvidenceReferenceV2> = (0..9)
        .map(|i| EvidenceReferenceV2::new(node_id(&format!("n-{i}")), true, None).unwrap())
        .collect();
    assert_eq!(
        EvidenceFromListV2::new(nine).unwrap_err(),
        invalid("evidence reference count must be between one and eight")
    );
}

#[test]
fn manual_rework_targets_enforce_bounds_and_uniqueness() {
    let one = vec![node_id("implement")];
    assert!(ManualReworkTargetListV2::new(one).is_ok());
    let sixty_four: Vec<GraphNodeId> = (0..64).map(|i| node_id(&format!("n-{i}"))).collect();
    assert!(ManualReworkTargetListV2::new(sixty_four).is_ok());
    assert_eq!(
        ManualReworkTargetListV2::new(Vec::new()).unwrap_err(),
        invalid("manual rework target count must be between one and 64")
    );
    let sixty_five: Vec<GraphNodeId> = (0..65).map(|i| node_id(&format!("n-{i}"))).collect();
    assert_eq!(
        ManualReworkTargetListV2::new(sixty_five).unwrap_err(),
        invalid("manual rework target count must be between one and 64")
    );
    let duplicate = vec![node_id("implement"), node_id("implement")];
    assert_eq!(
        ManualReworkTargetListV2::new(duplicate).unwrap_err(),
        invalid("manual rework targets must be unique")
    );
}

#[test]
fn graph_assembles_entry_placements_and_manual_rework() {
    let entry = node_id("implement");
    let middle = node_id("review");
    let closeout = node_id("closeout");
    let routes = DecisionRouteMapV2::new(vec![
        DecisionRouteEntryV2::new(
            opt_id("branch-a"),
            DecisionRouteV2::new(closeout.clone(), TransitionEffectV2::Advance),
        ),
        DecisionRouteEntryV2::new(
            opt_id("branch-b"),
            DecisionRouteV2::new(closeout.clone(), TransitionEffectV2::Advance),
        ),
    ])
    .unwrap();
    // Two decision routes converge on `closeout`; convergence is ordinary declarative data
    // reached by one cursor, never a synchronizing join (INV-V2S02, INV-V2S08).
    let graph = ProcedureGraphV2::new(
        entry.clone(),
        vec![
            GraphPlacementV2::Action(ActionPlacementV2::new(
                entry.clone(),
                def_id("implement"),
                None,
                None,
                ActionOutcomeV2::Next(middle.clone()),
            )),
            GraphPlacementV2::Decision(DecisionPlacementV2::new(
                middle,
                def_id("evaluate"),
                None,
                routes,
            )),
            GraphPlacementV2::Action(ActionPlacementV2::new(
                closeout.clone(),
                def_id("close"),
                None,
                None,
                ActionOutcomeV2::Terminal,
            )),
        ],
        Some(ManualReworkTargetListV2::new(vec![entry.clone()]).unwrap()),
    )
    .unwrap();

    assert_eq!(graph.entry(), &entry);
    assert_eq!(graph.node_count(), 3);
    assert_eq!(graph.node_kind(&closeout), Some(NodeKindV2::Action));
    assert_eq!(
        graph.node_kind(&node_id("review")),
        Some(NodeKindV2::Decision),
    );
    assert!(graph.placement(&closeout).is_some());
    assert!(graph.manual_rework().is_some());
}

#[test]
fn graph_assembly_rejects_duplicates_missing_entry_and_overflow() {
    let placement = |id: &str| {
        GraphPlacementV2::Action(ActionPlacementV2::new(
            node_id(id),
            def_id("d"),
            None,
            None,
            ActionOutcomeV2::Terminal,
        ))
    };
    let n1 = node_id("n1");
    assert_eq!(
        ProcedureGraphV2::new(n1.clone(), vec![placement("n1"), placement("n1")], None),
        Err(invalid("graph node identifiers must be unique"))
    );
    assert_eq!(
        ProcedureGraphV2::new(
            node_id("missing"),
            vec![placement("n1"), placement("n2")],
            None
        ),
        Err(invalid("the entry graph node must be present in the graph"))
    );
    assert_eq!(
        ProcedureGraphV2::new(n1, Vec::new(), None),
        Err(invalid("graph node count must be between one and 64"))
    );
    let sixty_five: Vec<GraphPlacementV2> = (0..65).map(|i| placement(&format!("n-{i}"))).collect();
    let entry_sixty_five = sixty_five[0].id().clone();
    assert_eq!(
        ProcedureGraphV2::new(entry_sixty_five, sixty_five, None),
        Err(invalid("graph node count must be between one and 64"))
    );
}
