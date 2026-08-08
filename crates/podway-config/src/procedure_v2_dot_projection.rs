//! Deterministic Graphviz DOT projection for a normalized Procedure v2 graph.

use std::fmt::Write as _;

use podway_core::Sha256Digest;
use sha2::{Digest as _, Sha256};

use crate::{
    ConfigError, GraphProjectionNodeTypeV2, ProcedureGraphModelV2,
    procedure_v2_graph_projection::validate_projection_characters,
};

/// A bounded DOT projection and SHA-256 digest over its exact UTF-8 bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureDotProjectionV2 {
    projection: String,
    projection_digest: Sha256Digest,
}

impl ProcedureDotProjectionV2 {
    /// DOT text without a trailing newline.
    pub fn projection(&self) -> &str {
        &self.projection
    }

    pub fn projection_digest(&self) -> &Sha256Digest {
        &self.projection_digest
    }
}

/// Renders a reviewable DOT graph from the normalized Procedure v2 graph.
///
/// The model is topology-only, so evidence wiring and manual rework cannot become flow edges.
/// Node and edge order remains the semantic author order captured by normalization.
pub fn project_procedure_v2_dot(
    graph: &ProcedureGraphModelV2,
) -> Result<ProcedureDotProjectionV2, ConfigError> {
    let mut projection = String::new();
    projection.push_str("digraph podway {\n    // ");
    projection.push_str(graph.procedure_schema());
    projection.push_str("\n    // procedure-digest: ");
    projection.push_str(graph.procedure_digest().as_str());
    projection.push_str("\n    rankdir=TB;\n");

    for node in graph.nodes() {
        projection.push_str("\n    \"");
        projection.push_str(node.graph_node_id());
        projection.push_str("\" [label=\"");
        projection.push_str(&node_label(node));
        projection.push_str("\", shape=");
        projection.push_str(match (node.node_type(), node.goal_assessment()) {
            (GraphProjectionNodeTypeV2::Action, _) => "box",
            (GraphProjectionNodeTypeV2::Decision, false) => "diamond",
            (GraphProjectionNodeTypeV2::Decision, true) => "hexagon",
        });
        if node.manual_rework_target() {
            projection.push_str(", style=dashed");
        }
        projection.push_str("];");
    }

    if !graph.edges().is_empty() {
        projection.push('\n');
        for edge in graph.edges() {
            projection.push_str("\n    \"");
            projection.push_str(edge.from_graph_node_id());
            projection.push_str("\" -> \"");
            projection.push_str(edge.to_graph_node_id());
            projection.push('"');
            if let Some(option_id) = edge.option_id() {
                projection.push_str(" [label=\"");
                projection.push_str(option_id);
                projection.push_str(" · ");
                projection.push_str(edge.effect());
                projection.push_str("\"]");
            }
            projection.push(';');
        }
    }

    projection.push_str("\n\n}");
    validate_projection_characters(projection.chars().count())?;
    let projection_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(projection.as_bytes())
    ))
    .map_err(|_| ConfigError::InvalidDigest)?;

    Ok(ProcedureDotProjectionV2 {
        projection,
        projection_digest,
    })
}

fn node_label(node: &crate::ProcedureGraphNodeV2) -> String {
    let mut label = encode_dot_scalar(node.title());
    for (enabled, annotation) in [
        (node.entry(), "entry"),
        (node.terminal(), "terminal"),
        (node.skippable(), "skippable"),
        (
            node.node_type() == GraphProjectionNodeTypeV2::Decision && !node.goal_assessment(),
            "decision",
        ),
        (node.goal_assessment(), "goal assessment"),
        (node.manual_rework_target(), "manual rework target"),
    ] {
        if enabled {
            label.push_str(" · ");
            label.push_str(annotation);
        }
    }
    label
}

fn encode_dot_scalar(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for character in value.chars() {
        let safe_ascii = character.is_ascii_alphanumeric()
            || character == ' '
            || matches!(character, '.' | ',' | ':' | ';' | '?' | '(' | ')');
        let safe_non_ascii =
            !character.is_ascii() && !character.is_control() && !character.is_whitespace();
        if safe_ascii || safe_non_ascii {
            encoded.push(character);
        } else {
            write!(encoded, "<U+{:04X}>", u32::from(character))
                .expect("writing to a String cannot fail");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use podway_core::SOURCE_PROJECTION_MAX_CHARACTERS;

    use super::*;

    #[test]
    fn dot_scalar_encoding_uses_a_closed_safe_character_set() {
        assert_eq!(
            encode_dot_scalar("Az 09.,:;?()_-\"<&\n\t界\u{a0}\u{1f600}"),
            "Az 09.,:;?()<U+005F><U+002D><U+0022><U+003C><U+0026><U+000A><U+0009>界<U+00A0>😀"
        );
    }

    #[test]
    fn dot_projection_budget_accepts_equality_and_rejects_the_next_character() {
        assert!(validate_projection_characters(SOURCE_PROJECTION_MAX_CHARACTERS).is_ok());
        assert!(matches!(
            validate_projection_characters(SOURCE_PROJECTION_MAX_CHARACTERS + 1),
            Err(ConfigError::OutOfBounds {
                field: "graph projection",
                min: 1,
                max: SOURCE_PROJECTION_MAX_CHARACTERS,
                actual,
            }) if actual == SOURCE_PROJECTION_MAX_CHARACTERS + 1
        ));
    }
}
