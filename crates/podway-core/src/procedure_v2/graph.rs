//! Procedure v2 graph placements, declared routes, transition effects, evidence references,
//! manual rework targets, and the declarative procedure graph.

use std::collections::BTreeSet;

use crate::{DomainError, GraphNodeId, ItemId, NodeDefinitionId, OptionId};

use super::NodeKindV2;
use super::invalid;

const MIN_EVIDENCE_REFERENCES_PER_PLACEMENT: usize = 1;
const MAX_EVIDENCE_REFERENCES_PER_PLACEMENT: usize = 8;
const MIN_SELECTED_ITEMS_PER_REFERENCE: usize = 1;
const MAX_SELECTED_ITEMS_PER_REFERENCE: usize = 16;
const MIN_ROUTES_PER_DECISION: usize = 1;
const MAX_ROUTES_PER_DECISION: usize = 8;
const MIN_MANUAL_REWORK_TARGETS: usize = 1;
const MAX_MANUAL_REWORK_TARGETS: usize = 64;
const MAX_GRAPH_NODES_PER_PROCEDURE: usize = 64;

/// The exact `DomainError::InvalidState` reason [`ProcedureGraphV2::new`] reports when `graph.entry`
/// names no declared placement.
///
/// Published because `podway-config`'s authoring-diagnostic mapping switches on it to emit
/// `ENTRY_NODE_INVALID`: the rejection travels as a `DomainError::InvalidState` whose only
/// distinguishing content is this static reason, so the raise site and the classifier must read the
/// same constant. Two literals could drift; one constant cannot.
pub const GRAPH_ENTRY_ABSENT_REASON: &str = "the entry graph node must be present in the graph";

/// The exact `DomainError::InvalidState` reason [`ProcedureGraphV2::new`] reports when two
/// placements declare the same graph node identifier.
///
/// Published for the same reason as [`GRAPH_ENTRY_ABSENT_REASON`]: every reference naming that
/// identifier resolves to two placements, which is `AMBIGUOUS_GRAPH_REFERENCE`.
pub const GRAPH_NODE_ID_NOT_UNIQUE_REASON: &str = "graph node identifiers must be unique";

/// The transition effect carried by a declared decision route (dossier section 9.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionEffectV2 {
    Advance,
    Rework,
}

impl TransitionEffectV2 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Advance => "advance",
            Self::Rework => "rework",
        }
    }
}

impl std::str::FromStr for TransitionEffectV2 {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "advance" => Ok(Self::Advance),
            "rework" => Ok(Self::Rework),
            _ => Err(invalid("unknown transition effect")),
        }
    }
}

/// One declared route of a decision placement: the target graph node and transition effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRouteV2 {
    to: GraphNodeId,
    effect: TransitionEffectV2,
}

impl DecisionRouteV2 {
    pub const fn new(to: GraphNodeId, effect: TransitionEffectV2) -> Self {
        Self { to, effect }
    }

    pub fn to(&self) -> &GraphNodeId {
        &self.to
    }

    pub const fn effect(&self) -> TransitionEffectV2 {
        self.effect
    }
}

/// One option-to-route binding inside a decision placement route table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRouteEntryV2 {
    option_id: OptionId,
    route: DecisionRouteV2,
}

impl DecisionRouteEntryV2 {
    pub const fn new(option_id: OptionId, route: DecisionRouteV2) -> Self {
        Self { option_id, route }
    }

    pub fn option_id(&self) -> &OptionId {
        &self.option_id
    }

    pub fn route(&self) -> &DecisionRouteV2 {
        &self.route
    }
}

/// The bounded option-to-route table of a decision placement. Routes bind declared options to
/// graph targets; whether every option is routed is a graph-vetting concern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRouteMapV2 {
    entries: Vec<DecisionRouteEntryV2>,
}

impl DecisionRouteMapV2 {
    pub fn new(entries: Vec<DecisionRouteEntryV2>) -> Result<Self, DomainError> {
        if entries.len() < MIN_ROUTES_PER_DECISION || entries.len() > MAX_ROUTES_PER_DECISION {
            return Err(invalid("route count must be between one and eight"));
        }
        let mut seen = BTreeSet::new();
        for entry in &entries {
            if !seen.insert(entry.option_id()) {
                return Err(invalid("route option identifiers must be unique"));
            }
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[DecisionRouteEntryV2] {
        &self.entries
    }
}

/// The action-placement skip policy. A declared policy requires `allowed: true`; `false` is
/// invalid (dossier section 5.1). Decision placements must not declare skip; that cross-field rule
/// is enforced by graph vetting, not by this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkipPolicyV2 {
    allowed: bool,
    reason_required: bool,
}

impl SkipPolicyV2 {
    pub fn new(allowed: bool, reason_required: bool) -> Result<Self, DomainError> {
        if !allowed {
            return Err(invalid("a declared skip policy requires allowed: true"));
        }
        Ok(Self {
            allowed,
            reason_required,
        })
    }

    pub const fn allowed_with(reason_required: bool) -> Self {
        Self {
            allowed: true,
            reason_required,
        }
    }

    pub const fn is_allowed(&self) -> bool {
        self.allowed
    }

    pub const fn reason_required(&self) -> bool {
        self.reason_required
    }
}

/// The single normal outcome of an action placement. An action declares exactly one of a `next`
/// target or a terminal disposition; modeling the outcome as an enum makes "both or neither"
/// unrepresentable (dossier section 6.2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionOutcomeV2 {
    Next(GraphNodeId),
    Terminal,
}

impl ActionOutcomeV2 {
    pub fn next(to: GraphNodeId) -> Self {
        Self::Next(to)
    }

    pub const fn terminal() -> Self {
        Self::Terminal
    }

    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }

    pub fn next_target(&self) -> Option<&GraphNodeId> {
        match self {
            Self::Next(target) => Some(target),
            Self::Terminal => None,
        }
    }
}

/// One declared evidence reference on a graph placement.
///
/// `required` defaults to `true` at the authoring layer. Whether a required source may declare
/// `skip.allowed: true` is a graph-vetting concern and is not enforced by this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceReferenceV2 {
    source_node: GraphNodeId,
    required: bool,
    selected_items: Option<Vec<ItemId>>,
}

impl EvidenceReferenceV2 {
    pub fn new(
        source_node: GraphNodeId,
        required: bool,
        selected_items: Option<Vec<ItemId>>,
    ) -> Result<Self, DomainError> {
        if let Some(items) = &selected_items {
            if items.len() < MIN_SELECTED_ITEMS_PER_REFERENCE
                || items.len() > MAX_SELECTED_ITEMS_PER_REFERENCE
            {
                return Err(invalid(
                    "selected item count must be between one and sixteen",
                ));
            }
            let mut seen = BTreeSet::new();
            for item in items {
                if !seen.insert(item) {
                    return Err(invalid("selected item identifiers must be unique"));
                }
            }
        }
        Ok(Self {
            source_node,
            required,
            selected_items,
        })
    }

    pub fn source_node(&self) -> &GraphNodeId {
        &self.source_node
    }

    pub const fn required(&self) -> bool {
        self.required
    }

    pub fn selected_items(&self) -> Option<&[ItemId]> {
        self.selected_items.as_deref()
    }
}

/// The bounded `evidence_from` list of a graph placement. An absent list is represented by
/// `Option::None` at the placement level; a present list always carries at least one entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceFromListV2 {
    entries: Vec<EvidenceReferenceV2>,
}

impl EvidenceFromListV2 {
    pub fn new(entries: Vec<EvidenceReferenceV2>) -> Result<Self, DomainError> {
        if entries.len() < MIN_EVIDENCE_REFERENCES_PER_PLACEMENT
            || entries.len() > MAX_EVIDENCE_REFERENCES_PER_PLACEMENT
        {
            return Err(invalid(
                "evidence reference count must be between one and eight",
            ));
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[EvidenceReferenceV2] {
        &self.entries
    }
}

/// An action graph placement: one uniquely identified placement of an action definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionPlacementV2 {
    id: GraphNodeId,
    definition: NodeDefinitionId,
    evidence_from: Option<EvidenceFromListV2>,
    skip: Option<SkipPolicyV2>,
    outcome: ActionOutcomeV2,
}

impl ActionPlacementV2 {
    pub fn new(
        id: GraphNodeId,
        definition: NodeDefinitionId,
        evidence_from: Option<EvidenceFromListV2>,
        skip: Option<SkipPolicyV2>,
        outcome: ActionOutcomeV2,
    ) -> Self {
        Self {
            id,
            definition,
            evidence_from,
            skip,
            outcome,
        }
    }

    pub fn id(&self) -> &GraphNodeId {
        &self.id
    }

    pub fn definition(&self) -> &NodeDefinitionId {
        &self.definition
    }

    pub fn evidence_from(&self) -> Option<&EvidenceFromListV2> {
        self.evidence_from.as_ref()
    }

    pub fn skip(&self) -> Option<SkipPolicyV2> {
        self.skip
    }

    pub fn outcome(&self) -> &ActionOutcomeV2 {
        &self.outcome
    }
}

/// A decision graph placement: one uniquely identified placement of a decision definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionPlacementV2 {
    id: GraphNodeId,
    definition: NodeDefinitionId,
    evidence_from: Option<EvidenceFromListV2>,
    routes: DecisionRouteMapV2,
}

impl DecisionPlacementV2 {
    pub fn new(
        id: GraphNodeId,
        definition: NodeDefinitionId,
        evidence_from: Option<EvidenceFromListV2>,
        routes: DecisionRouteMapV2,
    ) -> Self {
        Self {
            id,
            definition,
            evidence_from,
            routes,
        }
    }

    pub fn id(&self) -> &GraphNodeId {
        &self.id
    }

    pub fn definition(&self) -> &NodeDefinitionId {
        &self.definition
    }

    pub fn evidence_from(&self) -> Option<&EvidenceFromListV2> {
        self.evidence_from.as_ref()
    }

    pub fn routes(&self) -> &DecisionRouteMapV2 {
        &self.routes
    }
}

/// The declared manual rework target list. Targets are graph node identifiers; whether each target
/// exists in the procedure graph is a graph-vetting concern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualReworkTargetListV2 {
    targets: Vec<GraphNodeId>,
}

impl ManualReworkTargetListV2 {
    pub fn new(targets: Vec<GraphNodeId>) -> Result<Self, DomainError> {
        if targets.len() < MIN_MANUAL_REWORK_TARGETS || targets.len() > MAX_MANUAL_REWORK_TARGETS {
            return Err(invalid(
                "manual rework target count must be between one and 64",
            ));
        }
        let mut seen = BTreeSet::new();
        for target in &targets {
            if !seen.insert(target) {
                return Err(invalid("manual rework targets must be unique"));
            }
        }
        Ok(Self { targets })
    }

    pub fn targets(&self) -> &[GraphNodeId] {
        &self.targets
    }
}

/// One placed node in a Procedure v2 graph. The enum has no fork, parallel, spawn, synchronizing
/// join, or executable variant: an action carries exactly one outcome and a decision maps each
/// option to exactly one declared route (ADR-0017, INV-V2S08).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphPlacementV2 {
    Action(ActionPlacementV2),
    Decision(DecisionPlacementV2),
}

impl GraphPlacementV2 {
    pub fn id(&self) -> &GraphNodeId {
        match self {
            Self::Action(placement) => placement.id(),
            Self::Decision(placement) => placement.id(),
        }
    }

    pub const fn node_kind(&self) -> NodeKindV2 {
        match self {
            Self::Action(_) => NodeKindV2::Action,
            Self::Decision(_) => NodeKindV2::Decision,
        }
    }
}

/// The declarative single-cursor Procedure v2 graph: one entry placement, the bounded set of placed
/// nodes, and the optional manual rework target list. Construction enforces only structural
/// assembly — one to 64 placements, unique graph node ids, and the entry present. Branches, declared
/// rework cycles, and convergence are accepted as declarative data; closed-reference validation
/// lives in podway-config (delivered), and graph vetting is owned by V2GRF-001.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureGraphV2 {
    entry: GraphNodeId,
    placements: Vec<GraphPlacementV2>,
    manual_rework: Option<ManualReworkTargetListV2>,
}

impl ProcedureGraphV2 {
    pub fn new(
        entry: GraphNodeId,
        placements: Vec<GraphPlacementV2>,
        manual_rework: Option<ManualReworkTargetListV2>,
    ) -> Result<Self, DomainError> {
        if placements.is_empty() || placements.len() > MAX_GRAPH_NODES_PER_PROCEDURE {
            return Err(invalid("graph node count must be between one and 64"));
        }
        let mut seen = BTreeSet::new();
        let mut entry_present = false;
        for placement in &placements {
            if !seen.insert(placement.id().clone()) {
                return Err(invalid(GRAPH_NODE_ID_NOT_UNIQUE_REASON));
            }
            if placement.id() == &entry {
                entry_present = true;
            }
        }
        if !entry_present {
            return Err(invalid(GRAPH_ENTRY_ABSENT_REASON));
        }
        Ok(Self {
            entry,
            placements,
            manual_rework,
        })
    }

    pub fn entry(&self) -> &GraphNodeId {
        &self.entry
    }

    pub fn placements(&self) -> &[GraphPlacementV2] {
        &self.placements
    }

    pub fn manual_rework(&self) -> Option<&ManualReworkTargetListV2> {
        self.manual_rework.as_ref()
    }

    pub fn node_count(&self) -> usize {
        self.placements.len()
    }

    pub fn placement(&self, id: &GraphNodeId) -> Option<&GraphPlacementV2> {
        self.placements
            .iter()
            .find(|placement| placement.id() == id)
    }

    pub fn node_kind(&self, id: &GraphNodeId) -> Option<NodeKindV2> {
        self.placement(id).map(GraphPlacementV2::node_kind)
    }
}
