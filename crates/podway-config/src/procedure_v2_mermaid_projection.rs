//! Deterministic Mermaid review projection for a normalized Procedure v2 graph.

use std::fmt::Write as _;

use podway_core::Sha256Digest;
use sha2::{Digest as _, Sha256};

use crate::{
    ConfigError, GraphProjectionNodeTypeV2, ProcedureGraphModelV2,
    procedure_v2_graph_projection::validate_projection_characters,
};

/// A bounded Mermaid projection and SHA-256 digest over its exact UTF-8 bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureMermaidProjectionV2 {
    projection: String,
    projection_digest: Sha256Digest,
}

impl ProcedureMermaidProjectionV2 {
    /// Mermaid text without a trailing newline.
    pub fn projection(&self) -> &str {
        &self.projection
    }

    pub fn projection_digest(&self) -> &Sha256Digest {
        &self.projection_digest
    }
}

/// Renders the human-review Mermaid projection of a normalized Procedure v2 graph.
///
/// The model is already topology-only: evidence wiring and definition guidance cannot become
/// flow edges or labels here. Node and edge order remains the semantic author order captured by
/// the normalized model.
pub fn project_procedure_v2_mermaid(
    graph: &ProcedureGraphModelV2,
) -> Result<ProcedureMermaidProjectionV2, ConfigError> {
    let mut projection = String::new();
    projection.push_str("%% ");
    projection.push_str(graph.procedure_schema());
    projection.push_str("\n%% procedure-digest: ");
    projection.push_str(graph.procedure_digest().as_str());
    projection.push_str("\n\nflowchart TD");

    for node in graph.nodes() {
        projection.push_str("\n    ");
        projection.push_str(&mermaid_node_id(node.graph_node_id()));

        let label = node_label(node);
        match (node.node_type(), node.goal_assessment()) {
            (GraphProjectionNodeTypeV2::Action, _) => {
                projection.push_str("[\"");
                projection.push_str(&label);
                projection.push_str("\"]");
            }
            (GraphProjectionNodeTypeV2::Decision, false) => {
                projection.push_str("{\"");
                projection.push_str(&label);
                projection.push_str("\"}");
            }
            (GraphProjectionNodeTypeV2::Decision, true) => {
                projection.push_str("{{\"");
                projection.push_str(&label);
                projection.push_str("\"}}");
            }
        }
    }

    if !graph.edges().is_empty() {
        projection.push('\n');
        for edge in graph.edges() {
            projection.push_str("\n    ");
            projection.push_str(&mermaid_node_id(edge.from_graph_node_id()));
            projection.push_str(" -->");
            if let Some(option_id) = edge.option_id() {
                projection.push('|');
                projection.push_str(option_id);
                projection.push_str(" · ");
                projection.push_str(edge.effect());
                projection.push('|');
            }
            projection.push(' ');
            projection.push_str(&mermaid_node_id(edge.to_graph_node_id()));
        }
    }

    let manual_rework_targets = graph
        .nodes()
        .iter()
        .filter(|node| node.manual_rework_target())
        .collect::<Vec<_>>();
    if !manual_rework_targets.is_empty() {
        projection
            .push_str("\n\n    classDef manual_rework_target stroke-dasharray:4 3\n    class ");
        for (index, node) in manual_rework_targets.iter().enumerate() {
            if index > 0 {
                projection.push(',');
            }
            projection.push_str(&mermaid_node_id(node.graph_node_id()));
        }
        projection.push_str(" manual_rework_target");
    }

    validate_projection_characters(projection.chars().count())?;
    let projection_digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(projection.as_bytes())
    ))
    .map_err(|_| ConfigError::InvalidDigest)?;

    Ok(ProcedureMermaidProjectionV2 {
        projection,
        projection_digest,
    })
}

fn mermaid_node_id(graph_node_id: &str) -> String {
    let normalized = graph_node_id.replace('-', "_");
    if matches!(
        normalized.as_str(),
        "call"
            | "click"
            | "href"
            | "style"
            | "interpolate"
            | "class"
            | "graph"
            | "flowchart"
            | "subgraph"
            | "end"
    ) {
        // A double underscore cannot result from the closed kebab-case Procedure identifier
        // grammar, so this reserved-word escape is injective with ordinary normalized IDs.
        format!("n__{normalized}")
    } else {
        normalized
    }
}

fn node_label(node: &crate::ProcedureGraphNodeV2) -> String {
    let mut label = escape_mermaid_label(node.title());
    for (enabled, annotation) in [
        (node.entry(), "entry"),
        (node.terminal(), "terminal"),
        (node.skippable(), "skippable"),
    ] {
        if enabled {
            label.push_str(" · ");
            label.push_str(annotation);
        }
    }
    label
}

fn escape_mermaid_label(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\n' => escaped.push_str("&#10;"),
            '\r' => escaped.push_str("&#13;"),
            '\t' => escaped.push_str("&#9;"),
            '\u{2028}' => escaped.push_str("&#8232;"),
            '\u{2029}' => escaped.push_str("&#8233;"),
            character if character.is_control() => {
                write!(escaped, "&#{};", u32::from(character))
                    .expect("writing to a String cannot fail");
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use podway_core::SOURCE_PROJECTION_MAX_CHARACTERS;

    use super::*;

    #[test]
    fn mermaid_label_escaping_covers_syntax_and_line_controls() {
        assert_eq!(
            escape_mermaid_label("A & \"B\" <C>\nD\r\t\u{2028}\u{2029}"),
            "A &amp; &quot;B&quot; &lt;C&gt;&#10;D&#13;&#9;&#8232;&#8233;"
        );
    }

    #[test]
    fn mermaid_node_ids_escape_flowchart_keywords_without_collisions() {
        for reserved in [
            "call",
            "click",
            "href",
            "style",
            "interpolate",
            "class",
            "graph",
            "flowchart",
            "subgraph",
            "end",
        ] {
            assert_eq!(mermaid_node_id(reserved), format!("n__{reserved}"));
        }
        assert_eq!(mermaid_node_id("n-end"), "n_end");
        assert_eq!(mermaid_node_id("flowchart-elk"), "flowchart_elk");
        assert_eq!(mermaid_node_id("swimlane-beta"), "swimlane_beta");
        assert_eq!(mermaid_node_id("review-change"), "review_change");
    }

    #[test]
    fn mermaid_projection_budget_accepts_equality_and_rejects_the_next_character() {
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
