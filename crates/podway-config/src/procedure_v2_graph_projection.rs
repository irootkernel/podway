//! Canonical, read-only Procedure v2 graph projection.
//!
//! The source Procedure remains authoritative. This module derives only stable placement identity,
//! topology, and review flags from a validated model, serializes that normalized graph as Podway
//! Canonical JSON v1, and hashes the exact emitted bytes. Definition bodies and runtime/session
//! state never enter the projection.

use podway_core::{
    ActionOutcomeV2, GraphPlacementV2, PROCEDURE_SCHEMA_V2, SOURCE_PROJECTION_MAX_CHARACTERS,
    Sha256Digest,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::{
    CanonicalJsonV1, ConfigError, ParsedNodeDefinition, ValidatedProcedureV2,
    canonical_json_from_serializable,
};

/// The `ConfigError` field used when the generated graph projection exceeds its output budget.
pub(crate) const GRAPH_PROJECTION_FIELD: &str = "graph projection";

const VALIDATED_PROCEDURE_FIELD: &str = "validated procedure v2";
const MISSING_DEFINITION_REASON: &str =
    "a graph placement must reference a present node definition";
const MISSING_ROUTE_REASON: &str = "a decision option must have a validated route";

/// The normalized node kind written into every graph projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphProjectionNodeTypeV2 {
    Action,
    Decision,
}

impl GraphProjectionNodeTypeV2 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Decision => "decision",
        }
    }
}

/// One immutable node in the normalized graph shared by all generated projections.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProcedureGraphNodeV2 {
    graph_node_id: String,
    node_definition_id: String,
    #[serde(skip)]
    title: String,
    node_type: GraphProjectionNodeTypeV2,
    goal_assessment: bool,
    entry: bool,
    terminal: bool,
    skippable: bool,
    manual_rework_target: bool,
}

impl ProcedureGraphNodeV2 {
    pub fn graph_node_id(&self) -> &str {
        &self.graph_node_id
    }

    pub fn node_definition_id(&self) -> &str {
        &self.node_definition_id
    }

    /// Human label retained for the text renderers but deliberately excluded from graph JSON.
    pub fn title(&self) -> &str {
        &self.title
    }

    pub const fn node_type(&self) -> GraphProjectionNodeTypeV2 {
        self.node_type
    }

    pub const fn goal_assessment(&self) -> bool {
        self.goal_assessment
    }

    pub const fn entry(&self) -> bool {
        self.entry
    }

    pub const fn terminal(&self) -> bool {
        self.terminal
    }

    pub const fn skippable(&self) -> bool {
        self.skippable
    }

    pub const fn manual_rework_target(&self) -> bool {
        self.manual_rework_target
    }
}

/// One immutable transition edge in the normalized graph shared by all generated projections.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProcedureGraphEdgeV2 {
    from_graph_node_id: String,
    to_graph_node_id: String,
    effect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    option_id: Option<String>,
}

impl ProcedureGraphEdgeV2 {
    pub fn from_graph_node_id(&self) -> &str {
        &self.from_graph_node_id
    }

    pub fn to_graph_node_id(&self) -> &str {
        &self.to_graph_node_id
    }

    pub fn effect(&self) -> &str {
        &self.effect
    }

    pub fn option_id(&self) -> Option<&str> {
        self.option_id.as_deref()
    }
}

/// The normalized graph model. Array order is semantic and follows placement and option author
/// order; all fields are immutable so later renderers consume exactly the model that was hashed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProcedureGraphModelV2 {
    procedure_schema: String,
    procedure_digest: Sha256Digest,
    entry_graph_node_id: String,
    terminal_graph_node_ids: Vec<String>,
    nodes: Vec<ProcedureGraphNodeV2>,
    edges: Vec<ProcedureGraphEdgeV2>,
}

impl ProcedureGraphModelV2 {
    pub fn procedure_schema(&self) -> &str {
        &self.procedure_schema
    }

    pub fn procedure_digest(&self) -> &Sha256Digest {
        &self.procedure_digest
    }

    pub fn entry_graph_node_id(&self) -> &str {
        &self.entry_graph_node_id
    }

    pub fn terminal_graph_node_ids(&self) -> &[String] {
        &self.terminal_graph_node_ids
    }

    pub fn nodes(&self) -> &[ProcedureGraphNodeV2] {
        &self.nodes
    }

    pub fn edges(&self) -> &[ProcedureGraphEdgeV2] {
        &self.edges
    }
}

/// The bounded canonical JSON graph projection and SHA-256 over its exact bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureGraphProjectionV2 {
    graph: ProcedureGraphModelV2,
    projection: CanonicalJsonV1,
    projection_digest: Sha256Digest,
}

impl ProcedureGraphProjectionV2 {
    pub fn graph(&self) -> &ProcedureGraphModelV2 {
        &self.graph
    }

    /// Canonical JSON v1 with no trailing newline.
    pub fn projection(&self) -> &str {
        self.projection.as_str()
    }

    pub fn projection_digest(&self) -> &Sha256Digest {
        &self.projection_digest
    }
}

/// Builds the canonical graph projection from a validated Procedure v2 model.
///
/// The caller must successfully vet `procedure` before publishing this projection. Validation
/// makes all references closed; vetting establishes the graph-wide topology and liveness claims
/// that this read-only builder intentionally does not repeat.
pub fn project_procedure_v2_graph(
    procedure: &ValidatedProcedureV2,
) -> Result<ProcedureGraphProjectionV2, ConfigError> {
    let graph = normalized_graph(procedure)?;
    let projection = canonical_json_from_serializable(&graph)?;
    let characters = projection.as_str().chars().count();
    validate_projection_characters(characters)?;
    let projection_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(projection.as_bytes())
    ))
    .map_err(|_| ConfigError::InvalidDigest)?;

    Ok(ProcedureGraphProjectionV2 {
        graph,
        projection,
        projection_digest,
    })
}

fn validate_projection_characters(characters: usize) -> Result<(), ConfigError> {
    if characters <= SOURCE_PROJECTION_MAX_CHARACTERS {
        return Ok(());
    }
    Err(ConfigError::OutOfBounds {
        field: GRAPH_PROJECTION_FIELD,
        min: 1,
        max: SOURCE_PROJECTION_MAX_CHARACTERS,
        actual: characters,
    })
}

fn normalized_graph(
    procedure: &ValidatedProcedureV2,
) -> Result<ProcedureGraphModelV2, ConfigError> {
    let parsed = procedure.parsed();
    let graph = parsed.graph();
    let manual_rework = graph.manual_rework().map(|targets| targets.targets());
    let mut terminal_graph_node_ids = Vec::new();
    let mut nodes = Vec::with_capacity(graph.placements().len());
    let mut edges = Vec::new();

    for placement in graph.placements() {
        let placement_definition_id = match placement {
            GraphPlacementV2::Action(action) => action.definition().as_str(),
            GraphPlacementV2::Decision(decision) => decision.definition().as_str(),
        };
        let definition = parsed
            .node_definitions()
            .iter()
            .find(|definition| definition.id().as_str() == placement_definition_id)
            .ok_or(ConfigError::InvalidValue {
                field: VALIDATED_PROCEDURE_FIELD,
                reason: MISSING_DEFINITION_REASON,
            })?;

        let (node_definition_id, node_type, goal_assessment, terminal, skippable) = match placement
        {
            GraphPlacementV2::Action(action) => {
                let terminal = action.outcome().is_terminal();
                if terminal {
                    terminal_graph_node_ids.push(action.id().as_str().to_owned());
                }
                if let ActionOutcomeV2::Next(target) = action.outcome() {
                    edges.push(ProcedureGraphEdgeV2 {
                        from_graph_node_id: action.id().as_str().to_owned(),
                        to_graph_node_id: target.as_str().to_owned(),
                        effect: "advance".to_owned(),
                        option_id: None,
                    });
                }
                (
                    action.definition().as_str(),
                    GraphProjectionNodeTypeV2::Action,
                    false,
                    terminal,
                    action.skip().is_some_and(|skip| skip.is_allowed()),
                )
            }
            GraphPlacementV2::Decision(decision) => {
                let ParsedNodeDefinition::Decision(definition) = definition else {
                    return Err(ConfigError::InvalidValue {
                        field: VALIDATED_PROCEDURE_FIELD,
                        reason: MISSING_DEFINITION_REASON,
                    });
                };
                for option in definition.options() {
                    let route = decision
                        .routes()
                        .entries()
                        .iter()
                        .find(|entry| entry.option_id() == option.id())
                        .ok_or(ConfigError::InvalidValue {
                            field: VALIDATED_PROCEDURE_FIELD,
                            reason: MISSING_ROUTE_REASON,
                        })?;
                    edges.push(ProcedureGraphEdgeV2 {
                        from_graph_node_id: decision.id().as_str().to_owned(),
                        to_graph_node_id: route.route().to().as_str().to_owned(),
                        effect: route.route().effect().as_str().to_owned(),
                        option_id: Some(option.id().as_str().to_owned()),
                    });
                }
                (
                    decision.definition().as_str(),
                    GraphProjectionNodeTypeV2::Decision,
                    definition.assessment().is_some(),
                    false,
                    false,
                )
            }
        };
        let graph_node_id = placement.id().as_str();
        let title = match definition {
            ParsedNodeDefinition::Action(definition) => definition.title(),
            ParsedNodeDefinition::Decision(definition) => definition.title(),
        };
        nodes.push(ProcedureGraphNodeV2 {
            graph_node_id: graph_node_id.to_owned(),
            node_definition_id: node_definition_id.to_owned(),
            title: title.to_owned(),
            node_type,
            goal_assessment,
            entry: graph.entry().as_str() == graph_node_id,
            terminal,
            skippable,
            manual_rework_target: manual_rework.is_some_and(|targets| {
                targets
                    .iter()
                    .any(|target| target.as_str() == graph_node_id)
            }),
        });
    }

    Ok(ProcedureGraphModelV2 {
        procedure_schema: PROCEDURE_SCHEMA_V2.to_owned(),
        procedure_digest: procedure.digest().clone(),
        entry_graph_node_id: graph.entry().as_str().to_owned(),
        terminal_graph_node_ids,
        nodes,
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_projection_budget_accepts_equality_and_rejects_the_next_character() {
        assert!(validate_projection_characters(SOURCE_PROJECTION_MAX_CHARACTERS).is_ok());
        assert!(matches!(
            validate_projection_characters(SOURCE_PROJECTION_MAX_CHARACTERS + 1),
            Err(ConfigError::OutOfBounds {
                field: GRAPH_PROJECTION_FIELD,
                min: 1,
                max: SOURCE_PROJECTION_MAX_CHARACTERS,
                actual,
            }) if actual == SOURCE_PROJECTION_MAX_CHARACTERS + 1
        ));
    }
}
