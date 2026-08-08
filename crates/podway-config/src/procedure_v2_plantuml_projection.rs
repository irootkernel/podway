//! Deterministic PlantUML state-diagram projection for a normalized Procedure v2 graph.

use std::fmt::Write as _;

use podway_core::Sha256Digest;
use sha2::{Digest as _, Sha256};

use crate::{
    ConfigError, GraphProjectionNodeTypeV2, ProcedureGraphModelV2,
    procedure_v2_graph_projection::validate_projection_characters,
};

/// A bounded PlantUML projection and SHA-256 digest over its exact UTF-8 bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePlantUmlProjectionV2 {
    projection: String,
    projection_digest: Sha256Digest,
}

impl ProcedurePlantUmlProjectionV2 {
    /// PlantUML text without a trailing newline.
    pub fn projection(&self) -> &str {
        &self.projection
    }

    pub fn projection_digest(&self) -> &Sha256Digest {
        &self.projection_digest
    }
}

/// Renders a reviewable PlantUML state diagram from the normalized Procedure v2 graph.
///
/// The model is topology-only, so evidence wiring and definition guidance cannot become flow
/// edges. Node and edge order remains the semantic author order captured by normalization.
pub fn project_procedure_v2_plantuml(
    graph: &ProcedureGraphModelV2,
) -> Result<ProcedurePlantUmlProjectionV2, ConfigError> {
    let mut projection = String::new();
    projection.push_str("@startuml\n' ");
    projection.push_str(graph.procedure_schema());
    projection.push_str("\n' procedure-digest: ");
    projection.push_str(graph.procedure_digest().as_str());
    projection.push_str("\nhide empty description\ntop to bottom direction\n");

    for node in graph.nodes() {
        projection.push_str("\nstate \"");
        projection.push_str(&node_label(node));
        projection.push_str("\" as ");
        projection.push_str(&plantuml_node_alias(node.graph_node_id()));
        if node.goal_assessment() {
            projection.push_str(" <<goal_assessment>>");
        } else if node.node_type() == GraphProjectionNodeTypeV2::Decision {
            projection.push_str(" <<decision>>");
        }
    }

    if !graph.edges().is_empty() {
        projection.push('\n');
        for edge in graph.edges() {
            projection.push('\n');
            projection.push_str(&plantuml_node_alias(edge.from_graph_node_id()));
            projection.push_str(" --> ");
            projection.push_str(&plantuml_node_alias(edge.to_graph_node_id()));
            if let Some(option_id) = edge.option_id() {
                projection.push_str(" : ");
                projection.push_str(option_id);
                projection.push_str(" · ");
                projection.push_str(edge.effect());
            }
        }
    }

    projection.push_str("\n\n@enduml");
    validate_projection_characters(projection.chars().count())?;
    let projection_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(projection.as_bytes())
    ))
    .map_err(|_| ConfigError::InvalidDigest)?;

    Ok(ProcedurePlantUmlProjectionV2 {
        projection,
        projection_digest,
    })
}

fn plantuml_node_alias(graph_node_id: &str) -> String {
    format!("n_{}", graph_node_id.replace('-', "_"))
}

fn node_label(node: &crate::ProcedureGraphNodeV2) -> String {
    let mut label = escape_plantuml_title(node.title());
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

fn escape_plantuml_title(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        let safe_ascii = character.is_ascii_alphanumeric()
            || character == ' '
            || matches!(character, '.' | ',' | ':' | ';' | '?' | '(' | ')');
        let safe_non_ascii =
            !character.is_ascii() && !character.is_control() && !character.is_whitespace();
        if safe_ascii || safe_non_ascii {
            escaped.push(character);
        } else {
            write!(escaped, "<U+{:04X}>", u32::from(character))
                .expect("writing to a String cannot fail");
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use podway_core::SOURCE_PROJECTION_MAX_CHARACTERS;

    use super::*;

    #[test]
    fn plantuml_title_escaping_uses_a_closed_safe_character_set() {
        assert_eq!(
            escape_plantuml_title("Az 09.,:;?()_-\"<&\n\t界\u{a0}\u{1f600}"),
            "Az 09.,:;?()<U+005F><U+002D><U+0022><U+003C><U+0026><U+000A><U+0009>界<U+00A0>😀"
        );
    }

    #[test]
    fn plantuml_aliases_are_prefixed_and_preserve_placement_identity() {
        assert_eq!(plantuml_node_alias("review-change"), "n_review_change");
        assert_eq!(plantuml_node_alias("state"), "n_state");
        assert_eq!(plantuml_node_alias("end"), "n_end");
    }

    #[test]
    fn plantuml_projection_budget_accepts_equality_and_rejects_the_next_character() {
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
