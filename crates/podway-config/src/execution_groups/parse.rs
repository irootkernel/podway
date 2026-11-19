use super::*;
use crate::procedure_v2_parse::{
    map_action_outcome, map_delegation, map_item_predicate, map_items, map_node_definition,
    map_skip,
};
use crate::procedure_v2_wire::*;
use crate::{ParsedNodeDefinition, ProcedureDocumentFormat, decode_procedure_document};
use podway_core::{
    GoalTrackingOptIn, GraphNodeId, ItemId, ManualReworkTargetListV2, NodeDefinitionId, OptionId,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: String,
    id: String,
    version: String,
    name: String,
    purpose: String,
    description: Option<String>,
    goal_tracking: Option<bool>,
    node_definitions: OrderedMap<String, Definition>,
    graph: Graph,
    manual_rework: Option<ManualReworkWire>,
    execution_group: Option<Boundary>,
    plan_inputs: Option<Vec<ItemWire>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Boundary {
    anchor: String,
    success_return: String,
    plan_inputs: Vec<String>,
    result_items: Vec<ItemWire>,
    replan_decisions: Vec<String>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Definition {
    Action {
        title: String,
        intent: String,
        description: Option<String>,
        instructions: Option<Vec<String>>,
        items: Option<Vec<ItemWire>>,
    },
    Decision {
        title: String,
        description: Option<String>,
        objective: String,
        prompt: String,
        evidence_guidance: Option<Vec<String>>,
        items: Option<Vec<ItemWire>>,
        options: Vec<OptionWire>,
        reason: ReasonPolicyWire,
        assessment: Option<AssessmentWire>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionWire {
    id: String,
    label: String,
    criteria: Option<String>,
    guards: Option<Vec<Predicate>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Predicate {
    evidence: Source,
    field: Option<String>,
    equals: Option<PredicateScalarWire>,
    not_equals: Option<PredicateScalarWire>,
    empty: Option<bool>,
    non_empty: Option<bool>,
    at_least: Option<i64>,
    at_most: Option<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    scope: String,
    node: Option<String>,
    item: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    scope: String,
    node: Option<String>,
    #[serde(default = "required")]
    required: bool,
    items: Option<Vec<String>>,
}
fn required() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Graph {
    entry: String,
    nodes: Vec<Placement>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    id: String,
    #[serde(rename = "use")]
    definition: String,
    evidence_from: Option<Vec<Reference>>,
    next: Option<String>,
    terminal: Option<bool>,
    routes: Option<OrderedMap<String, Route>>,
    skip: Option<SkipPolicyWire>,
    delegation: Option<DelegationWire>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Route {
    effect: String,
    to: Option<String>,
}

/// Dedicated internal parser; public parse_procedure_document continues rejecting both schemas.
pub fn parse_successor_document(
    input: &[u8],
    format: ProcedureDocumentFormat,
) -> Result<SuccessorDefinition, ConfigError> {
    parse_document(input, format, false)
}

/// Admit authored sources for session start, which accepts only a base definition.
pub fn parse_successor_base_document(
    input: &[u8],
    format: ProcedureDocumentFormat,
) -> Result<SuccessorDefinition, ConfigError> {
    parse_document(input, format, true)
}

fn parse_document(
    input: &[u8],
    format: ProcedureDocumentFormat,
    require_base: bool,
) -> Result<SuccessorDefinition, ConfigError> {
    let decoded = decode_procedure_document(input, format)?;
    if format == ProcedureDocumentFormat::Yaml && crate::parser::begins_with_flow_collection(input)
    {
        return Err(invalid(
            "source",
            "JSON-form documents are not accepted by YAML dispatch",
        ));
    }
    let schema = decoded
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("schema", "a string is required"))?;
    match (schema, require_base) {
        (PROCEDURE_SCHEMA_V3, _) | (EXECUTION_GROUP_SCHEMA_V1, false) => {}
        (actual, _) => {
            return Err(ConfigError::InvalidSchema {
                actual: actual.to_owned(),
                expected: PROCEDURE_SCHEMA_V3,
            });
        }
    }
    let text = std::str::from_utf8(input).map_err(|e| ConfigError::InvalidDocument {
        reason: e.to_string(),
    })?;
    let document: Document = match format {
        ProcedureDocumentFormat::Json => {
            serde_json::from_str(text).map_err(|e| ConfigError::InvalidDocument {
                reason: e.to_string(),
            })?
        }
        ProcedureDocumentFormat::Yaml => {
            serde_norway::from_str(text).map_err(|e| ConfigError::InvalidDocument {
                reason: e.to_string(),
            })?
        }
    };
    let header = DefinitionHeaderV3::new(
        NodeDefinitionId::new(document.id).map_err(core)?,
        document.version,
        document.name,
        document.purpose,
        document.description,
    )
    .map_err(core)?;
    let definitions = document
        .node_definitions
        .entries()
        .into_iter()
        .map(|(id, wire)| definition(&id, wire))
        .collect::<Result<Vec<_>, _>>()?;
    let manual = document
        .manual_rework
        .map(|wire| {
            let targets = wire
                .allowed_targets
                .into_iter()
                .map(|v| GraphNodeId::new(v).map_err(core))
                .collect::<Result<Vec<_>, _>>()?;
            ManualReworkTargetListV2::new(targets).map_err(core)
        })
        .transpose()?
        .map_or_else(Vec::new, |v| v.targets().to_vec());
    let graph = TopologyV3::new(
        GraphNodeId::new(document.graph.entry).map_err(core)?,
        document
            .graph
            .nodes
            .into_iter()
            .map(placement)
            .collect::<Result<Vec<_>, _>>()?,
        manual,
    )
    .map_err(core)?;
    match document.schema.as_str() {
        PROCEDURE_SCHEMA_V3 => {
            if document.plan_inputs.is_some() {
                return Err(invalid(
                    "plan_inputs",
                    "plan inputs belong only to group documents",
                ));
            }
            let boundary = document.execution_group.ok_or_else(|| {
                invalid("execution_group", "a base requires exactly one boundary")
            })?;
            let boundary = GroupBoundaryV1::new(
                GraphNodeId::new(boundary.anchor).map_err(core)?,
                GraphNodeId::new(boundary.success_return).map_err(core)?,
                boundary
                    .plan_inputs
                    .into_iter()
                    .map(|v| ItemId::new(v).map_err(core))
                    .collect::<Result<_, _>>()?,
                map_items(boundary.result_items)?,
                boundary
                    .replan_decisions
                    .into_iter()
                    .map(|v| GraphNodeId::new(v).map_err(core))
                    .collect::<Result<_, _>>()?,
            )
            .map_err(core)?;
            let goal = document
                .goal_tracking
                .map(GoalTrackingOptIn::from_bool)
                .transpose()
                .map_err(core)?;
            ProcedureDefinitionV3::new(header, definitions, graph, goal, boundary)
                .map(SuccessorDefinition::Base)
                .map_err(core)
        }
        EXECUTION_GROUP_SCHEMA_V1 => {
            if document.execution_group.is_some() || document.goal_tracking.is_some() {
                return Err(invalid(
                    "execution_group",
                    "groups cannot own a boundary or session goal",
                ));
            }
            let inputs = map_items(document.plan_inputs.ok_or_else(|| {
                invalid("plan_inputs", "a group must declare its plan interface")
            })?)?;
            ExecutionGroupDefinitionV1::new(header, definitions, graph, inputs)
                .map(SuccessorDefinition::Group)
                .map_err(core)
        }
        _ => Err(ConfigError::InvalidSchema {
            expected: PROCEDURE_SCHEMA_V3,
            actual: document.schema,
        }),
    }
}
fn source(scope: String, node: Option<String>) -> Result<EvidenceSourceV3, ConfigError> {
    match (scope.as_str(), node) {
        ("local", Some(node)) => GraphNodeId::new(node)
            .map(EvidenceSourceV3::Local)
            .map_err(core),
        ("plan", None) => Ok(EvidenceSourceV3::Plan),
        ("execution_result", None) => Ok(EvidenceSourceV3::ExecutionResult),
        _ => Err(invalid(
            "evidence.scope",
            "scope must name one closed local, plan or execution-result source",
        )),
    }
}
fn definition(id: &str, wire: Definition) -> Result<NodeDefinitionV3, ConfigError> {
    let mut guards = Vec::new();
    let wire = match wire {
        Definition::Action {
            title,
            intent,
            description,
            instructions,
            items,
        } => NodeDefinitionWire::Action {
            title,
            intent,
            description,
            instructions,
            items,
        },
        Definition::Decision {
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
            let options = options
                .into_iter()
                .map(|v| {
                    if let Some(predicates) = v.guards {
                        let predicates = predicates
                            .into_iter()
                            .map(|p| {
                                let item = p.evidence.item.ok_or_else(|| {
                                    invalid(
                                        "guards.evidence.item",
                                        "guard evidence must select one item",
                                    )
                                })?;
                                let source = source(p.evidence.scope, p.evidence.node)?;
                                let predicate = map_item_predicate(ItemPredicateWire {
                                    item,
                                    field: p.field,
                                    equals: p.equals,
                                    not_equals: p.not_equals,
                                    empty: p.empty,
                                    non_empty: p.non_empty,
                                    at_least: p.at_least,
                                    at_most: p.at_most,
                                })?;
                                Ok(EvidencePredicateV3::new(
                                    source,
                                    predicate.item().clone(),
                                    predicate.field_outcome(),
                                    predicate.operator().clone(),
                                ))
                            })
                            .collect::<Result<Vec<_>, ConfigError>>()?;
                        guards.push((
                            OptionId::new(v.id.clone()).map_err(core)?,
                            OptionGuardV3::new(predicates).map_err(core)?,
                        ));
                    }
                    Ok(DecisionOptionWire {
                        id: v.id,
                        label: v.label,
                        criteria: v.criteria,
                        guards: None,
                    })
                })
                .collect::<Result<Vec<_>, ConfigError>>()?;
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
            }
        }
    };
    match map_node_definition(id, wire)? {
        ParsedNodeDefinition::Action(v) => Ok(NodeDefinitionV3::Action(v)),
        ParsedNodeDefinition::Decision(v) => DecisionDefinitionV3::new(v, guards)
            .map(NodeDefinitionV3::Decision)
            .map_err(core),
    }
}
fn placement(wire: Placement) -> Result<PlacementV3, ConfigError> {
    let mut evidence = Vec::new();
    if let Some(references) = wire.evidence_from {
        if references.is_empty() {
            return Err(invalid(
                "evidence_from",
                "optional evidence references must be nonempty",
            ));
        }
        for r in references {
            if r.items.as_ref().is_some_and(Vec::is_empty) {
                return Err(invalid(
                    "evidence_from.items",
                    "explicit item selection must be nonempty",
                ));
            }
            evidence.push(
                EvidenceReferenceV3::new(
                    source(r.scope, r.node)?,
                    r.required,
                    r.items
                        .unwrap_or_default()
                        .into_iter()
                        .map(|v| ItemId::new(v).map_err(core))
                        .collect::<Result<_, _>>()?,
                )
                .map_err(core)?,
            );
        }
    }
    let kind = if let Some(routes) = wire.routes {
        if wire.next.is_some()
            || wire.terminal.is_some()
            || wire.skip.is_some()
            || wire.delegation.is_some()
        {
            return Err(invalid("graph.nodes", "decisions declare only routes"));
        }
        let routes = routes
            .entries()
            .into_iter()
            .map(|(id, r)| {
                let route = match (r.effect.as_str(), r.to) {
                    ("advance", Some(to)) => {
                        DecisionRouteV3::Advance(GraphNodeId::new(to).map_err(core)?)
                    }
                    ("rework", Some(to)) => {
                        DecisionRouteV3::Rework(GraphNodeId::new(to).map_err(core)?)
                    }
                    ("replan", None) => DecisionRouteV3::Replan,
                    _ => {
                        return Err(invalid(
                            "graph.nodes.routes",
                            "route effect and target must agree",
                        ));
                    }
                };
                Ok((OptionId::new(id).map_err(core)?, route))
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;
        PlacementKindV3::Decision { routes }
    } else {
        PlacementKindV3::Action {
            outcome: map_action_outcome(wire.next, wire.terminal)?,
            skip: wire.skip.map(map_skip).transpose()?,
            delegation: wire.delegation.map(map_delegation).transpose()?,
        }
    };
    PlacementV3::new(
        GraphNodeId::new(wire.id).map_err(core)?,
        NodeDefinitionId::new(wire.definition).map_err(core)?,
        evidence,
        kind,
    )
    .map_err(core)
}
