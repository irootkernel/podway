//! Bounded immutable successor authoring values. Semantic reference/dominance checks belong to Config.
use super::{MAX_GROUP_PLACEMENTS_V1, bounded_text, invalid, item_contract};
use crate::{
    ActionDefinitionV2, ActionOutcomeV2, DecisionDefinitionV2, DelegationDeclarationV1,
    DomainError, GoalTrackingOptIn, GraphNodeId, ItemId, ItemSpecV2, NodeDefinitionId, OptionId,
    SkipPolicyV2,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeDefinitionV3 {
    Action(ActionDefinitionV2),
    Decision(DecisionDefinitionV3),
}
impl NodeDefinitionV3 {
    pub fn id(&self) -> &NodeDefinitionId {
        match self {
            Self::Action(v) => v.id(),
            Self::Decision(v) => v.metadata().id(),
        }
    }
}

/// Ports identify current admitted sources. Historical groups cannot be evidence sources here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceSourceV3 {
    Local(GraphNodeId),
    Plan,
    ExecutionResult,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceReferenceV3 {
    source: EvidenceSourceV3,
    required: bool,
    items: Vec<ItemId>,
}
impl EvidenceReferenceV3 {
    pub fn new(
        source: EvidenceSourceV3,
        required: bool,
        items: Vec<ItemId>,
    ) -> Result<Self, DomainError> {
        if items.len() > 16
            || items.iter().collect::<BTreeSet<_>>().len() != items.len()
            || (!matches!(source, EvidenceSourceV3::Local(_)) && items.is_empty())
        {
            return Err(invalid(
                "port evidence requires one to sixteen unique selected items",
            ));
        }
        Ok(Self {
            source,
            required,
            items,
        })
    }
    pub fn source(&self) -> &EvidenceSourceV3 {
        &self.source
    }
    pub const fn required(&self) -> bool {
        self.required
    }
    pub fn items(&self) -> &[ItemId] {
        &self.items
    }
}

/// Replan has no ordinary graph target: only the boundary operation can consume this route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionRouteV3 {
    Advance(GraphNodeId),
    Rework(GraphNodeId),
    Replan,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlacementKindV3 {
    Action {
        outcome: ActionOutcomeV2,
        skip: Option<SkipPolicyV2>,
        delegation: Option<DelegationDeclarationV1>,
    },
    Decision {
        routes: Vec<(OptionId, DecisionRouteV3)>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementV3 {
    id: GraphNodeId,
    definition: NodeDefinitionId,
    evidence: Vec<EvidenceReferenceV3>,
    kind: PlacementKindV3,
}
impl PlacementV3 {
    pub fn new(
        id: GraphNodeId,
        definition: NodeDefinitionId,
        evidence: Vec<EvidenceReferenceV3>,
        kind: PlacementKindV3,
    ) -> Result<Self, DomainError> {
        if evidence.len() > crate::MAX_EVIDENCE_REFERENCES_PER_PLACEMENT_V2 {
            return Err(invalid("too many successor evidence references"));
        }
        match &kind {
            PlacementKindV3::Action {
                skip: Some(_),
                delegation: Some(_),
                ..
            } => return Err(invalid("delegated actions cannot skip")),
            PlacementKindV3::Decision { routes }
                if routes.is_empty()
                    || routes.len() > crate::MAX_ROUTES_PER_DECISION_V2
                    || routes
                        .iter()
                        .map(|(id, _)| id)
                        .collect::<BTreeSet<_>>()
                        .len()
                        != routes.len() =>
            {
                return Err(invalid(
                    "decision routes require one to eight unique options",
                ));
            }
            _ => {}
        }
        Ok(Self {
            id,
            definition,
            evidence,
            kind,
        })
    }
    pub fn id(&self) -> &GraphNodeId {
        &self.id
    }
    pub fn definition(&self) -> &NodeDefinitionId {
        &self.definition
    }
    pub fn evidence(&self) -> &[EvidenceReferenceV3] {
        &self.evidence
    }
    pub fn kind(&self) -> &PlacementKindV3 {
        &self.kind
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopologyV3 {
    entry: GraphNodeId,
    nodes: Vec<PlacementV3>,
    manual_rework: Vec<GraphNodeId>,
}
impl TopologyV3 {
    pub fn new(
        entry: GraphNodeId,
        nodes: Vec<PlacementV3>,
        manual_rework: Vec<GraphNodeId>,
    ) -> Result<Self, DomainError> {
        if nodes.is_empty()
            || nodes.len() > MAX_GROUP_PLACEMENTS_V1
            || nodes.iter().map(|n| n.id()).collect::<BTreeSet<_>>().len() != nodes.len()
        {
            return Err(invalid(
                "successor topology requires one to 1024 unique placements",
            ));
        }
        if !nodes.iter().any(|node| node.id() == &entry) {
            return Err(invalid(
                "successor topology entry must name a declared placement",
            ));
        }
        if manual_rework.len() > 64
            || manual_rework.iter().collect::<BTreeSet<_>>().len() != manual_rework.len()
        {
            return Err(invalid("invalid successor manual rework declarations"));
        }
        Ok(Self {
            entry,
            nodes,
            manual_rework,
        })
    }
    pub fn entry(&self) -> &GraphNodeId {
        &self.entry
    }
    pub fn nodes(&self) -> &[PlacementV3] {
        &self.nodes
    }
    pub fn manual_rework(&self) -> &[GraphNodeId] {
        &self.manual_rework
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupBoundaryV1 {
    anchor: GraphNodeId,
    success_return: GraphNodeId,
    plan_inputs: Vec<ItemId>,
    result_items: Vec<ItemSpecV2>,
    replan_decisions: Vec<GraphNodeId>,
}
impl GroupBoundaryV1 {
    pub fn new(
        anchor: GraphNodeId,
        success_return: GraphNodeId,
        plan_inputs: Vec<ItemId>,
        result_items: Vec<ItemSpecV2>,
        replan_decisions: Vec<GraphNodeId>,
    ) -> Result<Self, DomainError> {
        item_contract(&result_items)?;
        if anchor == success_return
            || plan_inputs.len() > crate::MAX_ITEMS_PER_DEFINITION_V2
            || plan_inputs.iter().collect::<BTreeSet<_>>().len() != plan_inputs.len()
            || replan_decisions.len() > MAX_GROUP_PLACEMENTS_V1
            || replan_decisions.iter().collect::<BTreeSet<_>>().len() != replan_decisions.len()
        {
            return Err(invalid(
                "invalid group boundary ports or anchor return policy",
            ));
        }
        Ok(Self {
            anchor,
            success_return,
            plan_inputs,
            result_items,
            replan_decisions,
        })
    }
    pub fn anchor(&self) -> &GraphNodeId {
        &self.anchor
    }
    pub fn success_return(&self) -> &GraphNodeId {
        &self.success_return
    }
    pub fn plan_inputs(&self) -> &[ItemId] {
        &self.plan_inputs
    }
    pub fn result_items(&self) -> &[ItemSpecV2] {
        &self.result_items
    }
    pub fn replan_decisions(&self) -> &[GraphNodeId] {
        &self.replan_decisions
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionHeaderV3 {
    id: NodeDefinitionId,
    version: String,
    name: String,
    purpose: String,
    description: Option<String>,
}
impl DefinitionHeaderV3 {
    pub fn new(
        id: NodeDefinitionId,
        version: String,
        name: String,
        purpose: String,
        description: Option<String>,
    ) -> Result<Self, DomainError> {
        crate::validate_text("procedure version", &version, 1, 64, false)?;
        bounded_text(&name, 120)?;
        bounded_text(&purpose, 500)?;
        if let Some(v) = &description
            && v.chars().count() > 1000
        {
            return Err(invalid("successor description exceeds 1000 scalars"));
        }
        Ok(Self {
            id,
            version,
            name,
            purpose,
            description,
        })
    }
    pub fn id(&self) -> &NodeDefinitionId {
        &self.id
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}
fn definitions(values: &[NodeDefinitionV3]) -> Result<(), DomainError> {
    if values.is_empty()
        || values.len() > crate::MAX_NODE_DEFINITIONS_PER_PROCEDURE_V2
        || values.iter().map(|v| v.id()).collect::<BTreeSet<_>>().len() != values.len()
    {
        return Err(invalid(
            "successor definitions require one to 1024 unique identifiers",
        ));
    }
    Ok(())
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureDefinitionV3 {
    header: DefinitionHeaderV3,
    definitions: Vec<NodeDefinitionV3>,
    graph: TopologyV3,
    goal_tracking: Option<GoalTrackingOptIn>,
    boundary: GroupBoundaryV1,
}
impl ProcedureDefinitionV3 {
    pub fn new(
        header: DefinitionHeaderV3,
        nodes: Vec<NodeDefinitionV3>,
        graph: TopologyV3,
        goal_tracking: Option<GoalTrackingOptIn>,
        boundary: GroupBoundaryV1,
    ) -> Result<Self, DomainError> {
        definitions(&nodes)?;
        Ok(Self {
            header,
            definitions: nodes,
            graph,
            goal_tracking,
            boundary,
        })
    }
    pub fn header(&self) -> &DefinitionHeaderV3 {
        &self.header
    }
    pub fn definitions(&self) -> &[NodeDefinitionV3] {
        &self.definitions
    }
    pub fn graph(&self) -> &TopologyV3 {
        &self.graph
    }
    pub const fn goal_tracking(&self) -> Option<GoalTrackingOptIn> {
        self.goal_tracking
    }
    pub fn boundary(&self) -> &GroupBoundaryV1 {
        &self.boundary
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionGroupDefinitionV1 {
    header: DefinitionHeaderV3,
    definitions: Vec<NodeDefinitionV3>,
    graph: TopologyV3,
    plan_inputs: Vec<ItemSpecV2>,
}
impl ExecutionGroupDefinitionV1 {
    pub fn new(
        header: DefinitionHeaderV3,
        nodes: Vec<NodeDefinitionV3>,
        graph: TopologyV3,
        plan_inputs: Vec<ItemSpecV2>,
    ) -> Result<Self, DomainError> {
        definitions(&nodes)?;
        item_contract(&plan_inputs)?;
        Ok(Self {
            header,
            definitions: nodes,
            graph,
            plan_inputs,
        })
    }
    pub fn header(&self) -> &DefinitionHeaderV3 {
        &self.header
    }
    pub fn definitions(&self) -> &[NodeDefinitionV3] {
        &self.definitions
    }
    pub fn graph(&self) -> &TopologyV3 {
        &self.graph
    }
    pub fn plan_inputs(&self) -> &[ItemSpecV2] {
        &self.plan_inputs
    }
}

/// One current-port predicate. It reuses the original predicate vocabulary and value semantics;
/// the admitted source resolver supplies only the selected current port, never retained history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidencePredicateV3 {
    source: EvidenceSourceV3,
    predicate: crate::ItemPredicateV2,
}
impl EvidencePredicateV3 {
    pub const fn new(
        source: EvidenceSourceV3,
        item: ItemId,
        field_outcome: bool,
        operator: crate::ItemPredicateOperatorV2,
    ) -> Self {
        Self {
            source,
            predicate: crate::ItemPredicateV2::new(item, field_outcome, operator),
        }
    }
    pub fn source(&self) -> &EvidenceSourceV3 {
        &self.source
    }
    pub fn predicate(&self) -> &crate::ItemPredicateV2 {
        &self.predicate
    }
    pub fn evaluate(&self, value: Option<&crate::RecordedItemValueV2>) -> crate::PredicateStatusV2 {
        self.predicate.evaluate(value)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionGuardV3 {
    predicates: Vec<EvidencePredicateV3>,
}
impl OptionGuardV3 {
    pub fn new(predicates: Vec<EvidencePredicateV3>) -> Result<Self, DomainError> {
        if predicates.is_empty() || predicates.len() > 4 {
            return Err(invalid("successor guards require one to four predicates"));
        }
        Ok(Self { predicates })
    }
    pub fn predicates(&self) -> &[EvidencePredicateV3] {
        &self.predicates
    }
}
/// Shared decision metadata with successor-scoped guards. Legacy node-only guards cannot coexist
/// with these ports, so one authored guard always has one unambiguous source contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionDefinitionV3 {
    metadata: DecisionDefinitionV2,
    guards: BTreeMap<OptionId, OptionGuardV3>,
}
impl DecisionDefinitionV3 {
    pub fn new(
        metadata: DecisionDefinitionV2,
        guards: Vec<(OptionId, OptionGuardV3)>,
    ) -> Result<Self, DomainError> {
        if metadata
            .options()
            .iter()
            .any(|option| option.guards().is_some())
            || guards.len() > crate::MAX_ROUTES_PER_DECISION_V2
            || guards
                .iter()
                .map(|(id, _)| id)
                .collect::<BTreeSet<_>>()
                .len()
                != guards.len()
            || guards
                .iter()
                .any(|(id, _)| !metadata.options().iter().any(|option| option.id() == id))
        {
            return Err(invalid(
                "successor scoped guards must uniquely name declared unguarded metadata options",
            ));
        }
        Ok(Self {
            metadata,
            guards: guards.into_iter().collect(),
        })
    }
    pub fn metadata(&self) -> &DecisionDefinitionV2 {
        &self.metadata
    }
    pub fn guards(&self, option: &OptionId) -> Option<&OptionGuardV3> {
        self.guards.get(option)
    }
}
