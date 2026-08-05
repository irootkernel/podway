//! Procedure v2 YAML parsing: bounded-decode reuse and exact syntactic mapping of the closed
//! wire DTOs into the core v2 authoring values. No semantic, cross-reference, or
//! canonicalization work is performed here.

use std::str::FromStr;

use podway_core::{
    ActionDefinitionV2, ActionOutcomeV2, ActionPlacementV2, AssessmentContractV2,
    AssessmentOutcomeMappingV2, AssessmentTargetV2, DecisionDefinitionInputV2,
    DecisionDefinitionV2, DecisionOptionV2, DecisionPlacementV2, DecisionRouteEntryV2,
    DecisionRouteMapV2, DecisionRouteV2, DomainError, EvidenceFromListV2, EvidenceReferenceV2,
    GoalOutcome, GoalTrackingOptIn, GraphNodeId, GraphPlacementV2, ItemCommonV2, ItemId,
    ItemSpecV2, ManualReworkTargetListV2, NodeDefinitionId, OptionId, ProcedureGraphV2,
    ReasonPolicyV2, SkipPolicyV2, TransitionEffectV2,
};

use crate::procedure_v2_wire::*;
use crate::{ConfigError, validate_count, validate_identifier, validate_text};

const MAX_PROCEDURE_VERSION_CHARS: usize = 64;
const MAX_PROCEDURE_NAME_CHARS: usize = 120;
const MAX_PROCEDURE_PURPOSE_CHARS: usize = 500;
const MAX_PROCEDURE_DESCRIPTION_CHARS: usize = 1_000;
const MIN_NODE_DEFINITIONS: usize = 1;
const MAX_NODE_DEFINITIONS: usize = 64;

/// One mapped reusable node definition, preserving the discriminated action/decision kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedNodeDefinition {
    Action(ActionDefinitionV2),
    Decision(DecisionDefinitionV2),
}

impl ParsedNodeDefinition {
    pub fn id(&self) -> &NodeDefinitionId {
        match self {
            Self::Action(definition) => definition.id(),
            Self::Decision(definition) => definition.id(),
        }
    }
}

/// A bounded Procedure v2 document mapped into the core v2 authoring model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedProcedureV2 {
    id: String,
    version: String,
    name: String,
    purpose: String,
    description: Option<String>,
    goal_tracking: Option<GoalTrackingOptIn>,
    node_definitions: Vec<ParsedNodeDefinition>,
    graph: ProcedureGraphV2,
}

impl ParsedProcedureV2 {
    pub fn id(&self) -> &str {
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
    pub const fn goal_tracking(&self) -> Option<GoalTrackingOptIn> {
        self.goal_tracking
    }
    pub fn node_definitions(&self) -> &[ParsedNodeDefinition] {
        &self.node_definitions
    }
    pub fn graph(&self) -> &ProcedureGraphV2 {
        &self.graph
    }
}

/// Maps an already-admitted Procedure v2 YAML `text` into the core v2 authoring model. The
/// caller (`parse_procedure_yaml`) is responsible for the single bounded admission; this
/// function performs only order-preserving DTO deserialization and constructor mapping.
pub(crate) fn parse_procedure_v2_yaml(text: &str) -> Result<ParsedProcedureV2, ConfigError> {
    let document: ProcedureV2DocumentWire =
        serde_yaml::from_str(text).map_err(|error| ConfigError::InvalidDocument {
            reason: error.to_string(),
        })?;
    map_document(document)
}

fn map_domain_error(error: DomainError) -> ConfigError {
    match error {
        DomainError::EmptyValue { field } => ConfigError::InvalidValue {
            field,
            reason: "must not be empty",
        },
        DomainError::ValueTooLong {
            field,
            maximum,
            actual,
        } => ConfigError::OutOfBounds {
            field,
            min: 1,
            max: maximum,
            actual,
        },
        DomainError::InvalidIdentifier { field } => ConfigError::InvalidValue {
            field,
            reason: "must be lowercase kebab-case",
        },
        // v2 constructors surface every text/collection/cross-field rejection (including media
        // types) as InvalidState, whose static reason is preserved verbatim. No other variant is
        // reachable from v2 mapping; the wildcard forwards its Display rather than guessing.
        DomainError::InvalidState { reason } => ConfigError::InvalidValue {
            field: "procedure v2",
            reason,
        },
        other => ConfigError::InvalidDocument {
            reason: other.to_string(),
        },
    }
}

fn map_document(document: ProcedureV2DocumentWire) -> Result<ParsedProcedureV2, ConfigError> {
    if document.schema != PROCEDURE_SCHEMA_V2 {
        return Err(ConfigError::InvalidSchema {
            expected: PROCEDURE_SCHEMA_V2,
            actual: document.schema,
        });
    }

    validate_identifier("procedure.id", &document.id)?;
    validate_text(
        "procedure.version",
        &document.version,
        1,
        MAX_PROCEDURE_VERSION_CHARS,
        false,
    )?;
    validate_text(
        "procedure.name",
        &document.name,
        1,
        MAX_PROCEDURE_NAME_CHARS,
        true,
    )?;
    validate_text(
        "procedure.purpose",
        &document.purpose,
        1,
        MAX_PROCEDURE_PURPOSE_CHARS,
        true,
    )?;
    if let Some(description) = &document.description {
        validate_text(
            "procedure.description",
            description,
            0,
            MAX_PROCEDURE_DESCRIPTION_CHARS,
            false,
        )?;
    }

    let goal_tracking = match document.goal_tracking {
        None => Ok(None),
        Some(flag) => GoalTrackingOptIn::from_bool(flag)
            .map(Some)
            .map_err(map_domain_error),
    }?;

    let node_definitions = document
        .node_definitions
        .entries()
        .into_iter()
        .map(|(id, wire)| map_node_definition(&id, wire))
        .collect::<Result<Vec<_>, _>>()?;
    validate_count(
        "node_definitions",
        node_definitions.len(),
        MIN_NODE_DEFINITIONS,
        MAX_NODE_DEFINITIONS,
    )?;

    let manual_rework = document.manual_rework.map(map_manual_rework).transpose()?;
    let graph = map_graph(document.graph, manual_rework)?;

    Ok(ParsedProcedureV2 {
        id: document.id,
        version: document.version,
        name: document.name,
        purpose: document.purpose,
        description: document.description,
        goal_tracking,
        node_definitions,
        graph,
    })
}

fn map_node_definition(
    id: &str,
    wire: NodeDefinitionWire,
) -> Result<ParsedNodeDefinition, ConfigError> {
    let id = NodeDefinitionId::new(id).map_err(map_domain_error)?;
    match wire {
        NodeDefinitionWire::Action {
            title,
            intent,
            description,
            instructions,
            items,
        } => {
            let instructions = optional_nonempty("instructions", instructions)?;
            let items = map_items(optional_nonempty("items", items)?)?;
            let definition =
                ActionDefinitionV2::new(id, title, intent, description, instructions, items)
                    .map_err(map_domain_error)?;
            Ok(ParsedNodeDefinition::Action(definition))
        }
        NodeDefinitionWire::Decision {
            title,
            description,
            objective,
            prompt,
            evidence_guidance,
            items,
            options,
            reason,
            assessment,
        } => {
            let evidence_guidance = optional_nonempty("evidence_guidance", evidence_guidance)?;
            let items = map_items(optional_nonempty("items", items)?)?;
            let options = options
                .into_iter()
                .map(map_option)
                .collect::<Result<Vec<_>, _>>()?;
            let reason =
                ReasonPolicyV2::new(reason.required, reason.prompt).map_err(map_domain_error)?;
            let assessment = assessment.map(map_assessment).transpose()?;
            let definition = DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                id,
                title,
                description,
                objective,
                prompt,
                evidence_guidance,
                items,
                options,
                reason,
                assessment,
            })
            .map_err(map_domain_error)?;
            Ok(ParsedNodeDefinition::Decision(definition))
        }
    }
}

/// Honors the schema `minItems: 1` for optional collections: omission becomes empty, an
/// explicitly empty array is rejected, and a non-empty array is passed through unchanged.
fn optional_nonempty<T>(field: &'static str, value: Option<Vec<T>>) -> Result<Vec<T>, ConfigError> {
    match value {
        None => Ok(Vec::new()),
        Some(entries) if entries.is_empty() => Err(ConfigError::InvalidValue {
            field,
            reason: "must be omitted or contain at least one entry",
        }),
        Some(entries) => Ok(entries),
    }
}

fn map_items(items: Vec<ItemWire>) -> Result<Vec<ItemSpecV2>, ConfigError> {
    items.into_iter().map(map_item).collect()
}

fn map_item(wire: ItemWire) -> Result<ItemSpecV2, ConfigError> {
    match wire {
        ItemWire::Confirm {
            id,
            prompt,
            help,
            required,
        } => Ok(ItemSpecV2::confirm(item_common(
            id, prompt, help, required,
        )?)),
        ItemWire::Text {
            id,
            prompt,
            help,
            required,
            min_length,
            max_length,
            multiline,
        } => ItemSpecV2::text(
            item_common(id, prompt, help, required)?,
            min_length,
            max_length,
            multiline,
        )
        .map_err(map_domain_error),
        ItemWire::Choice {
            id,
            prompt,
            help,
            required,
            choices,
        } => ItemSpecV2::choice(item_common(id, prompt, help, required)?, choices)
            .map_err(map_domain_error),
        ItemWire::Integer {
            id,
            prompt,
            help,
            required,
            minimum,
            maximum,
        } => ItemSpecV2::integer(item_common(id, prompt, help, required)?, minimum, maximum)
            .map_err(map_domain_error),
        ItemWire::List {
            id,
            prompt,
            help,
            required,
            min_items,
            max_items,
            max_item_length,
            unique,
        } => ItemSpecV2::list(
            item_common(id, prompt, help, required)?,
            min_items,
            max_items,
            max_item_length,
            unique,
        )
        .map_err(map_domain_error),
        ItemWire::Artifact {
            id,
            prompt,
            help,
            required,
            allowed_media_types,
        } => {
            let allowed_media_types =
                optional_nonempty("allowed_media_types", allowed_media_types)?;
            ItemSpecV2::artifact(
                item_common(id, prompt, help, required)?,
                allowed_media_types,
            )
            .map_err(map_domain_error)
        }
    }
}

fn item_common(
    id: String,
    prompt: String,
    help: Option<String>,
    required: bool,
) -> Result<ItemCommonV2, ConfigError> {
    let id = ItemId::new(id).map_err(map_domain_error)?;
    ItemCommonV2::new(id, prompt, help, required).map_err(map_domain_error)
}

fn map_option(wire: DecisionOptionWire) -> Result<DecisionOptionV2, ConfigError> {
    let id = OptionId::new(wire.id).map_err(map_domain_error)?;
    DecisionOptionV2::new(id, wire.label, wire.criteria).map_err(map_domain_error)
}

fn map_assessment(wire: AssessmentWire) -> Result<AssessmentContractV2, ConfigError> {
    let target = AssessmentTargetV2::from_str(&wire.target).map_err(map_domain_error)?;
    let outcomes = wire
        .outcomes
        .entries()
        .into_iter()
        .map(|(option_id, outcome)| {
            let option_id = OptionId::new(option_id).map_err(map_domain_error)?;
            let outcome = GoalOutcome::from_str(&outcome).map_err(map_domain_error)?;
            Ok::<_, ConfigError>(AssessmentOutcomeMappingV2::new(option_id, outcome))
        })
        .collect::<Result<Vec<_>, _>>()?;
    AssessmentContractV2::with_target(target, outcomes).map_err(map_domain_error)
}

fn map_graph(
    wire: GraphWire,
    manual_rework: Option<ManualReworkTargetListV2>,
) -> Result<ProcedureGraphV2, ConfigError> {
    let entry = GraphNodeId::new(wire.entry).map_err(map_domain_error)?;
    let placements = wire
        .nodes
        .into_iter()
        .map(map_placement)
        .collect::<Result<Vec<_>, _>>()?;
    ProcedureGraphV2::new(entry, placements, manual_rework).map_err(map_domain_error)
}

fn map_placement(wire: GraphPlacementWire) -> Result<GraphPlacementV2, ConfigError> {
    let id = GraphNodeId::new(wire.id).map_err(map_domain_error)?;
    let definition = NodeDefinitionId::new(wire.use_).map_err(map_domain_error)?;
    let evidence_from = map_evidence_from(wire.evidence_from)?;

    if let Some(routes) = wire.routes {
        if wire.skip.is_some() || wire.next.is_some() || wire.terminal.is_some() {
            return Err(ConfigError::InvalidValue {
                field: "graph.nodes",
                reason: "a decision placement declares only routes",
            });
        }
        let entries = routes
            .entries()
            .into_iter()
            .map(|(option_id, route)| {
                let option_id = OptionId::new(option_id).map_err(map_domain_error)?;
                let to = GraphNodeId::new(route.to).map_err(map_domain_error)?;
                let effect =
                    TransitionEffectV2::from_str(&route.effect).map_err(map_domain_error)?;
                Ok::<_, ConfigError>(DecisionRouteEntryV2::new(
                    option_id,
                    DecisionRouteV2::new(to, effect),
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let route_map = DecisionRouteMapV2::new(entries).map_err(map_domain_error)?;
        return Ok(GraphPlacementV2::Decision(DecisionPlacementV2::new(
            id,
            definition,
            evidence_from,
            route_map,
        )));
    }

    let outcome = map_action_outcome(wire.next, wire.terminal)?;
    let skip = wire.skip.map(map_skip).transpose()?;
    Ok(GraphPlacementV2::Action(ActionPlacementV2::new(
        id,
        definition,
        evidence_from,
        skip,
        outcome,
    )))
}

fn map_action_outcome(
    next: Option<String>,
    terminal: Option<bool>,
) -> Result<ActionOutcomeV2, ConfigError> {
    match (next, terminal) {
        (Some(target), None) => {
            let to = GraphNodeId::new(target).map_err(map_domain_error)?;
            Ok(ActionOutcomeV2::next(to))
        }
        (None, Some(true)) => Ok(ActionOutcomeV2::terminal()),
        (None, Some(false)) => Err(ConfigError::InvalidValue {
            field: "graph.nodes.terminal",
            reason: "terminal must be true",
        }),
        (Some(_), Some(_)) => Err(ConfigError::InvalidValue {
            field: "graph.nodes",
            reason: "an action placement declares either next or terminal, not both",
        }),
        (None, None) => Err(ConfigError::InvalidValue {
            field: "graph.nodes",
            reason: "an action placement must declare next or terminal",
        }),
    }
}

fn map_skip(wire: SkipPolicyWire) -> Result<SkipPolicyV2, ConfigError> {
    SkipPolicyV2::new(wire.allowed, wire.reason_required).map_err(map_domain_error)
}

fn map_evidence_from(
    wire: Option<Vec<EvidenceReferenceWire>>,
) -> Result<Option<EvidenceFromListV2>, ConfigError> {
    let Some(entries) = wire else {
        return Ok(None);
    };
    let references = entries
        .into_iter()
        .map(map_evidence_reference)
        .collect::<Result<Vec<_>, _>>()?;
    EvidenceFromListV2::new(references)
        .map(Some)
        .map_err(map_domain_error)
}

fn map_evidence_reference(wire: EvidenceReferenceWire) -> Result<EvidenceReferenceV2, ConfigError> {
    let source_node = GraphNodeId::new(wire.node).map_err(map_domain_error)?;
    let selected_items = wire
        .items
        .map(|items| {
            items
                .into_iter()
                .map(|item| ItemId::new(item).map_err(map_domain_error))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    EvidenceReferenceV2::new(source_node, wire.required, selected_items).map_err(map_domain_error)
}

fn map_manual_rework(wire: ManualReworkWire) -> Result<ManualReworkTargetListV2, ConfigError> {
    let targets = wire
        .allowed_targets
        .into_iter()
        .map(|target| GraphNodeId::new(target).map_err(map_domain_error))
        .collect::<Result<Vec<_>, _>>()?;
    ManualReworkTargetListV2::new(targets).map_err(map_domain_error)
}
