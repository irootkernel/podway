use super::*;
use crate::ParsedNodeDefinition;
use crate::procedure_v2_graph::GraphIndex;
use crate::procedure_v2_validate::{
    predicate_matches_item, validate_definition, validate_required_when,
};
use podway_core::{ActionOutcomeV2, GoalOutcome, GraphNodeId, ItemSpecV2};
use std::collections::{BTreeMap, BTreeSet};

pub fn validate_successor_definition(
    definition: SuccessorDefinition,
) -> Result<ValidatedSuccessorDefinition, ConfigError> {
    admit(&definition)?;
    let authoring = super::projection::authoring(&definition);
    let canonical_json = crate::canonical_json_from_serializable(&authoring.clone().into_json())?;
    super::projection::bound(canonical_json.as_str())?;
    let digest = super::projection::digest(canonical_json.as_bytes())?;
    Ok(ValidatedSuccessorDefinition {
        definition,
        authoring,
        canonical_json,
        digest,
    })
}
fn lookup<'a>(
    definition: &'a SuccessorDefinition,
    id: &podway_core::NodeDefinitionId,
) -> Result<&'a NodeDefinitionV3, ConfigError> {
    definition
        .definitions()
        .iter()
        .find(|v| v.id() == id)
        .ok_or_else(|| {
            invalid(
                "graph.nodes.use",
                "placement references an unknown definition",
            )
        })
}
fn placement<'a>(
    definition: &'a SuccessorDefinition,
    id: &GraphNodeId,
) -> Result<&'a PlacementV3, ConfigError> {
    definition
        .graph()
        .nodes()
        .iter()
        .find(|v| v.id() == id)
        .ok_or_else(|| invalid("graph.nodes", "reference names an unknown local placement"))
}
fn source_items<'a>(
    definition: &'a SuccessorDefinition,
    source: &EvidenceSourceV3,
) -> Result<&'a [ItemSpecV2], ConfigError> {
    match source {
        EvidenceSourceV3::Local(node) => Ok(items(lookup(
            definition,
            placement(definition, node)?.definition(),
        )?)),
        EvidenceSourceV3::Plan => match definition {
            SuccessorDefinition::Group(v) => Ok(v.plan_inputs()),
            _ => Err(invalid(
                "evidence.scope",
                "plan ports belong only to a group",
            )),
        },
        EvidenceSourceV3::ExecutionResult => match definition {
            SuccessorDefinition::Base(v) => Ok(v.boundary().result_items()),
            _ => Err(invalid(
                "evidence.scope",
                "execution results belong only to the base",
            )),
        },
    }
}
fn has_replan(placement: &PlacementV3) -> bool {
    matches!(placement.kind(),PlacementKindV3::Decision{routes} if routes.iter().any(|(_,r)|matches!(r,DecisionRouteV3::Replan)))
}
fn admit(definition: &SuccessorDefinition) -> Result<(), ConfigError> {
    let topology = definition.graph();
    let index: BTreeMap<&str, usize> = topology
        .nodes()
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id().as_str(), i))
        .collect();
    let goal = matches!(definition,SuccessorDefinition::Base(v) if v.goal_tracking().is_some_and(|v|v.is_enabled()));
    for value in definition.definitions() {
        let shared = match value {
            NodeDefinitionV3::Action(v) => ParsedNodeDefinition::Action(v.clone()),
            NodeDefinitionV3::Decision(v) => ParsedNodeDefinition::Decision(v.metadata().clone()),
        };
        validate_definition(&shared, goal)?;
        if let NodeDefinitionV3::Decision(v) = value
            && let Some(assessment) = v.metadata().assessment()
        {
            let mapped = assessment
                .outcomes()
                .iter()
                .map(|v| v.option_id())
                .collect::<BTreeSet<_>>();
            if v.metadata()
                .options()
                .iter()
                .any(|v| !mapped.contains(v.id()))
                || [
                    GoalOutcome::Achieved,
                    GoalOutcome::NotAchieved,
                    GoalOutcome::Superseded,
                ]
                .iter()
                .any(|outcome| {
                    !assessment
                        .outcomes()
                        .iter()
                        .any(|v| v.outcome() == *outcome)
                })
            {
                return Err(invalid(
                    "assessment.outcomes",
                    "session-goal assessment must cover every option and outcome",
                ));
            }
        }
    }
    match definition {
        SuccessorDefinition::Base(base) => {
            let anchor = placement(definition, base.boundary().anchor())?;
            let anchor_items = items(lookup(definition, anchor.definition())?);
            if !matches!(anchor.kind(),PlacementKindV3::Action{outcome:ActionOutcomeV2::Next(to),skip,..} if to==base.boundary().success_return() && !skip.is_some_and(|v|v.is_allowed()))
            {
                return Err(invalid(
                    "execution_group.anchor",
                    "anchor must be an unskippable action advancing to success_return",
                ));
            }
            placement(definition, base.boundary().success_return())?;
            let selected_inputs = base
                .boundary()
                .plan_inputs()
                .iter()
                .map(|item| {
                    anchor_items
                        .iter()
                        .find(|v| v.id() == item)
                        .cloned()
                        .ok_or_else(|| {
                            invalid(
                                "execution_group.plan_inputs",
                                "plan input must name an anchor item",
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            validate_required_when(&selected_inputs)?;
            validate_required_when(base.boundary().result_items())?;
            for id in base.boundary().replan_decisions() {
                if !matches!(
                    placement(definition, id)?.kind(),
                    PlacementKindV3::Decision { .. }
                ) {
                    return Err(invalid(
                        "execution_group.replan_decisions",
                        "base replan policy must name decisions",
                    ));
                }
            }
        }
        SuccessorDefinition::Group(group) => validate_required_when(group.plan_inputs())?,
    }
    let mut edges = 0;
    for node in topology.nodes() {
        let declared = lookup(definition, node.definition())?;
        match (node.kind(), declared) {
            (PlacementKindV3::Action { outcome, .. }, NodeDefinitionV3::Action(_)) => {
                if let ActionOutcomeV2::Next(to) = outcome {
                    placement(definition, to)?;
                    edges += 1;
                }
            }
            (PlacementKindV3::Decision { routes }, NodeDefinitionV3::Decision(decision)) => {
                let options = decision
                    .metadata()
                    .options()
                    .iter()
                    .map(|v| v.id())
                    .collect::<BTreeSet<_>>();
                if routes.len() != options.len()
                    || routes.iter().any(|(id, _)| !options.contains(id))
                {
                    return Err(invalid(
                        "graph.nodes.routes",
                        "routes must exactly cover declared options",
                    ));
                }
                for (option, route) in routes {
                    match route {
                        DecisionRouteV3::Advance(to) | DecisionRouteV3::Rework(to) => {
                            placement(definition, to)?;
                            edges += 1;
                        }
                        DecisionRouteV3::Replan => {
                            if decision.metadata().assessment().is_some() {
                                return Err(invalid(
                                    "graph.nodes.routes",
                                    "session-goal assessment cannot use a replan route",
                                ));
                            }
                            if decision.guards(option).is_none() {
                                return Err(invalid(
                                    "graph.nodes.routes",
                                    "replan requires an eligible selected-evidence guard",
                                ));
                            }
                            if let SuccessorDefinition::Base(base) = definition
                                && !base.boundary().replan_decisions().contains(node.id())
                            {
                                return Err(invalid(
                                    "execution_group.replan_decisions",
                                    "base replan requires explicit anchor-return policy",
                                ));
                            }
                        }
                    }
                }
            }
            _ => {
                return Err(invalid(
                    "graph.nodes.use",
                    "placement and definition kinds disagree",
                ));
            }
        }
        for reference in node.evidence() {
            let selected = source_items(definition, reference.source())?;
            if matches!(reference.source(),EvidenceSourceV3::Local(id) if id==node.id()) {
                return Err(invalid(
                    "evidence.node",
                    "evidence cannot reference its own consumer",
                ));
            }
            for id in reference.items() {
                if !selected.iter().any(|v| v.id() == id) {
                    return Err(invalid(
                        "evidence.items",
                        "evidence selector names an unknown item",
                    ));
                }
            }
        }
        if let NodeDefinitionV3::Decision(decision) = declared {
            for option in decision.metadata().options() {
                if let Some(guards) = decision.guards(option.id()) {
                    for guard in guards.predicates() {
                        let predicate = guard.predicate();
                        if !node.evidence().iter().any(|r| {
                            r.source() == guard.source()
                                && r.required()
                                && r.items().contains(predicate.item())
                        }) {
                            return Err(invalid(
                                "guards.evidence",
                                "guard requires required explicitly selected evidence",
                            ));
                        }
                        let item = source_items(definition, guard.source())?
                            .iter()
                            .find(|v| v.id() == predicate.item())
                            .ok_or_else(|| {
                                invalid("guards.evidence.item", "guard selects an unknown item")
                            })?;
                        if !predicate_matches_item(
                            predicate.field_outcome(),
                            predicate.operator(),
                            item,
                        ) {
                            return Err(invalid(
                                "guards",
                                "guard operator and literal must match the selected item",
                            ));
                        }
                    }
                }
            }
        }
    }
    crate::validate_count("successor graph edges", edges, 0, MAX_GROUP_EDGES_V1)?;
    for id in topology.manual_rework() {
        placement(definition, id)?;
    }
    let graph = GraphIndex::successor(topology);
    let reachable = graph.reachable_from(graph.entry());
    if (0..graph.node_count()).any(|i| !reachable.contains(i)) {
        return Err(invalid(
            "graph.nodes",
            "every placement must be reachable from entry",
        ));
    }
    if !graph.advance_only_cycles().is_empty() {
        return Err(invalid("graph", "forward cycles are forbidden"));
    }
    let terminals = (0..graph.node_count())
        .filter(|i| graph.is_terminal(*i))
        .collect::<Vec<_>>();
    if terminals.is_empty()
        || matches!(definition, SuccessorDefinition::Group(_)) && terminals.len() != 1
    {
        return Err(invalid(
            "graph",
            "a group requires exactly one success-terminal action; a base requires a terminal",
        ));
    }
    // Explicit replan terminates this scope without pretending to complete its success terminal.
    let exits =
        graph.exit_reachable_nodes(|i| graph.is_terminal(i) || has_replan(&topology.nodes()[i]));
    if (0..graph.node_count()).any(|i| !exits.contains(i)) {
        return Err(invalid(
            "graph",
            "every placement must have a finite terminal or replan exit",
        ));
    }
    let dominators = graph.dominators();
    let base_indices = if let SuccessorDefinition::Base(base) = definition {
        let anchor = index[base.boundary().anchor().as_str()];
        let returned = index[base.boundary().success_return().as_str()];
        if terminals.iter().any(|t| {
            !graph.dominates(&dominators, anchor, *t) || !graph.dominates(&dominators, returned, *t)
        }) {
            return Err(invalid(
                "execution_group",
                "anchor and success_return must dominate every terminal",
            ));
        }
        if topology.manual_rework().contains(base.boundary().anchor()) {
            return Err(invalid(
                "manual_rework",
                "ordinary manual rework cannot target the anchor",
            ));
        }
        for allowed in base.boundary().replan_decisions() {
            if !graph.dominates(&dominators, returned, index[allowed.as_str()]) {
                return Err(invalid(
                    "execution_group.replan_decisions",
                    "base replan reviews must follow the group return",
                ));
            }
        }
        Some((anchor, returned))
    } else {
        None
    };
    for (consumer, node) in topology.nodes().iter().enumerate() {
        if let Some((anchor, returned)) = base_indices {
            let crosses_boundary = |to: &GraphNodeId| {
                let target = index[to.as_str()];
                graph.dominates(&dominators, returned, consumer)
                    && (target == anchor || !graph.dominates(&dominators, returned, target))
            };
            match node.kind() {
                PlacementKindV3::Action { outcome: ActionOutcomeV2::Next(to), .. }
                    if crosses_boundary(to) => return Err(invalid(
                        "graph.nodes.next",
                        "ordinary action progression cannot cross the anchor boundary",
                    )),
                PlacementKindV3::Decision { routes }
                    if routes.iter().any(|(_, route)| matches!(route, DecisionRouteV3::Advance(to) if crosses_boundary(to))) =>
                        return Err(invalid(
                            "graph.nodes.routes",
                            "ordinary decision advance cannot cross the anchor boundary",
                        )),
                _ => {}
            }
        }
        for reference in node.evidence() {
            match reference.source() {
                EvidenceSourceV3::Local(id) if reference.required() => {
                    let source = index[id.as_str()];
                    if !graph.strictly_dominates(&dominators, source, consumer) {
                        return Err(invalid(
                            "evidence",
                            "required local evidence must strictly dominate its consumer",
                        ));
                    }
                    if matches!(topology.nodes()[source].kind(),PlacementKindV3::Action{skip:Some(skip),..} if skip.is_allowed())
                    {
                        return Err(invalid(
                            "evidence",
                            "required evidence source cannot be skippable",
                        ));
                    }
                }
                EvidenceSourceV3::ExecutionResult => {
                    let Some((_, returned)) = base_indices else {
                        return Err(invalid("evidence", "execution results require a base"));
                    };
                    if !graph.dominates(&dominators, returned, consumer) {
                        return Err(invalid(
                            "evidence",
                            "current execution results are available only after group return",
                        ));
                    }
                }
                _ => {}
            }
        }
        if let PlacementKindV3::Decision { routes } = node.kind() {
            for (_, route) in routes {
                if let DecisionRouteV3::Rework(to) = route {
                    let target = index[to.as_str()];
                    if !graph.dominates(&dominators, target, consumer) {
                        return Err(invalid(
                            "graph.nodes.routes",
                            "rework target must dominate its decision",
                        ));
                    }
                    if let Some((anchor, returned)) = base_indices
                        && (target == anchor
                            || graph.dominates(&dominators, returned, consumer)
                                != graph.dominates(&dominators, returned, target))
                    {
                        return Err(invalid(
                            "graph.nodes.routes",
                            "ordinary decision rework cannot target or cross the anchor boundary",
                        ));
                    }
                }
            }
        }
    }
    if let Some((_, returned)) = base_indices
        && goal
    {
        let assessments = topology
            .nodes()
            .iter()
            .enumerate()
            .filter(|(i, _)| graph.dominates(&dominators, returned, *i))
            .filter_map(|(i, p)| match lookup(definition, p.definition()).ok()? {
                NodeDefinitionV3::Decision(v) if v.metadata().assessment().is_some() => Some(i),
                _ => None,
            })
            .collect::<Vec<_>>();
        if terminals.iter().any(|t| {
            !assessments
                .iter()
                .any(|a| graph.strictly_dominates(&dominators, *a, *t))
        }) {
            return Err(invalid(
                "assessment",
                "each base terminal requires a dominating session-goal assessment at or after group return",
            ));
        }
    }
    Ok(())
}

/// Shared pure interface comparison for preparation and later terminal-return admission.
/// Exact declaration order/defaults are preserved; only prompts are presentation-only.
pub fn validate_group_interface(
    base: &ValidatedSuccessorDefinition,
    group: &ValidatedSuccessorDefinition,
) -> Result<(), ConfigError> {
    let (SuccessorDefinition::Base(base), SuccessorDefinition::Group(group)) =
        (&base.definition, &group.definition)
    else {
        return Err(invalid(
            "execution_group",
            "interface comparison requires an admitted base and group",
        ));
    };
    let anchor = base
        .graph()
        .nodes()
        .iter()
        .find(|p| p.id() == base.boundary().anchor())
        .ok_or_else(|| invalid("execution_group.anchor", "anchor is absent"))?;
    let definition = base
        .definitions()
        .iter()
        .find(|v| v.id() == anchor.definition())
        .ok_or_else(|| invalid("execution_group.anchor", "anchor definition is absent"))?;
    let selected = base
        .boundary()
        .plan_inputs()
        .iter()
        .map(|id| {
            items(definition)
                .iter()
                .find(|v| v.id() == id)
                .cloned()
                .ok_or_else(|| invalid("plan_inputs", "anchor plan input is absent"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !same_boundary_contract_v1(&selected, group.plan_inputs()) {
        return Err(invalid(
            "plan_inputs",
            "group input declarations differ from selected anchor declarations",
        ));
    }
    let terminal = group
        .graph()
        .nodes()
        .iter()
        .find(|p| {
            matches!(
                p.kind(),
                PlacementKindV3::Action {
                    outcome: ActionOutcomeV2::Terminal,
                    ..
                }
            )
        })
        .ok_or_else(|| invalid("result_items", "group success terminal is absent"))?;
    let definition = group
        .definitions()
        .iter()
        .find(|v| v.id() == terminal.definition())
        .ok_or_else(|| invalid("result_items", "terminal definition is absent"))?;
    let selected = items(definition)
        .iter()
        .filter(|item| {
            base.boundary()
                .result_items()
                .iter()
                .any(|expected| expected.id() == item.id())
        })
        .cloned()
        .collect::<Vec<_>>();
    if !same_boundary_contract_v1(base.boundary().result_items(), &selected) {
        return Err(invalid(
            "result_items",
            "terminal result declarations differ from the base contract",
        ));
    }
    Ok(())
}
