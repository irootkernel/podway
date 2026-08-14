//! Read-only Procedure v2 preview aggregation (dossier section 13.2).
//!
//! Preview owns no filesystem or runtime state. It receives complete source text and a path label,
//! runs validation, vet, and lint, then publishes review details only when the validated graph and
//! its required Mermaid projection are admissible. Lint remains advisory and never removes the
//! start suggestion.

use podway_core::{AuthoringDiagnostic, AuthoringSeverity, Sha256Digest};

use crate::procedure_v2_authoring::placement_evidence_from;
use crate::procedure_v2_diagnostics::{
    AuthoringContext, AuthoringStage, FinalizedDiagnostics, config_error_diagnostic,
    finalize_diagnostics,
};
use crate::procedure_v2_format::{FormatFailure, FormatRequest, admit_procedure_v2};
use crate::procedure_v2_graph::GraphIndex;
use crate::{
    ConfigError, GraphProjectionNodeTypeV2, ProcedureGraphModelV2, ProcedureMermaidProjectionV2,
    lint_procedure_v2, normalize_procedure_v2_graph, project_procedure_v2_mermaid,
    vet_procedure_v2,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewChecksV2 {
    validate: bool,
    vet: bool,
    lint: bool,
}

impl ProcedurePreviewChecksV2 {
    pub const fn validate(self) -> bool {
        self.validate
    }
    pub const fn vet(self) -> bool {
        self.vet
    }
    pub const fn lint(self) -> bool {
        self.lint
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewSummaryV2 {
    definition_count: u32,
    graph_node_count: u32,
    action_node_count: u32,
    decision_node_count: u32,
    route_count: u32,
    cycle_count: u32,
    evidence_reference_count: u32,
    skippable_node_count: u32,
    manual_rework_target_count: u32,
}

macro_rules! count_accessors {
    ($($name:ident),+ $(,)?) => { $(pub const fn $name(&self) -> u32 { self.$name })+ };
}

impl ProcedurePreviewSummaryV2 {
    count_accessors!(
        definition_count,
        graph_node_count,
        action_node_count,
        decision_node_count,
        route_count,
        cycle_count,
        evidence_reference_count,
        skippable_node_count,
        manual_rework_target_count,
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewGraphNodeV2 {
    graph_node_id: String,
    node_definition_id: String,
    node_type: GraphProjectionNodeTypeV2,
    terminal: bool,
    skippable: bool,
}

impl ProcedurePreviewGraphNodeV2 {
    pub fn graph_node_id(&self) -> &str {
        &self.graph_node_id
    }
    pub fn node_definition_id(&self) -> &str {
        &self.node_definition_id
    }
    pub const fn node_type(&self) -> GraphProjectionNodeTypeV2 {
        self.node_type
    }
    pub const fn terminal(&self) -> bool {
        self.terminal
    }
    pub const fn skippable(&self) -> bool {
        self.skippable
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewGraphEdgeV2 {
    from_graph_node_id: String,
    to_graph_node_id: String,
    effect: String,
    option_id: Option<String>,
}

impl ProcedurePreviewGraphEdgeV2 {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewGraphV2 {
    entry_graph_node_id: String,
    terminal_graph_node_ids: Vec<String>,
    nodes: Vec<ProcedurePreviewGraphNodeV2>,
    edges: Vec<ProcedurePreviewGraphEdgeV2>,
}

impl ProcedurePreviewGraphV2 {
    pub fn entry_graph_node_id(&self) -> &str {
        &self.entry_graph_node_id
    }
    pub fn terminal_graph_node_ids(&self) -> &[String] {
        &self.terminal_graph_node_ids
    }
    pub fn nodes(&self) -> &[ProcedurePreviewGraphNodeV2] {
        &self.nodes
    }
    pub fn edges(&self) -> &[ProcedurePreviewGraphEdgeV2] {
        &self.edges
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewStartSuggestionV2 {
    command: &'static str,
    argv: Vec<String>,
}

impl ProcedurePreviewStartSuggestionV2 {
    pub const fn command(&self) -> &'static str {
        self.command
    }
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewDetailsV2 {
    procedure_schema: &'static str,
    procedure_id: String,
    procedure_version: String,
    purpose: String,
    procedure_digest: Sha256Digest,
    goal_tracking: bool,
    goal_assessment_graph_node_ids: Vec<String>,
    summary: ProcedurePreviewSummaryV2,
    graph: ProcedurePreviewGraphV2,
    mermaid: String,
    start_suggestion: ProcedurePreviewStartSuggestionV2,
}

impl ProcedurePreviewDetailsV2 {
    pub const fn procedure_schema(&self) -> &'static str {
        self.procedure_schema
    }
    pub fn procedure_id(&self) -> &str {
        &self.procedure_id
    }
    pub fn procedure_version(&self) -> &str {
        &self.procedure_version
    }
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
    pub fn procedure_digest(&self) -> &Sha256Digest {
        &self.procedure_digest
    }
    pub const fn goal_tracking(&self) -> bool {
        self.goal_tracking
    }
    pub fn goal_assessment_graph_node_ids(&self) -> &[String] {
        &self.goal_assessment_graph_node_ids
    }
    pub const fn summary(&self) -> &ProcedurePreviewSummaryV2 {
        &self.summary
    }
    pub const fn graph(&self) -> &ProcedurePreviewGraphV2 {
        &self.graph
    }
    pub fn mermaid(&self) -> &str {
        &self.mermaid
    }
    pub const fn start_suggestion(&self) -> &ProcedurePreviewStartSuggestionV2 {
        &self.start_suggestion
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedurePreviewReportV2 {
    checks: ProcedurePreviewChecksV2,
    details: Option<ProcedurePreviewDetailsV2>,
    findings: FinalizedDiagnostics,
}

impl ProcedurePreviewReportV2 {
    pub const fn admissible(&self) -> bool {
        self.details.is_some()
    }
    pub const fn checks(&self) -> ProcedurePreviewChecksV2 {
        self.checks
    }
    pub const fn details(&self) -> Option<&ProcedurePreviewDetailsV2> {
        self.details.as_ref()
    }
    pub fn diagnostics(&self) -> &[AuthoringDiagnostic] {
        self.findings.diagnostics()
    }
    pub const fn diagnostics_total(&self) -> u32 {
        self.findings.total()
    }
    pub const fn diagnostics_truncated(&self) -> bool {
        self.findings.truncated()
    }
}

/// Aggregates the complete read-only preview for one source document.
pub fn preview_procedure_v2(request: FormatRequest<'_>) -> ProcedurePreviewReportV2 {
    let context = AuthoringContext::new(request.source_path, request.source, request.format);
    let validated = match admit_procedure_v2(&context) {
        Ok(validated) => validated,
        Err(failure) => return stopped(failure, &context),
    };

    let vet_diagnostics = vet_procedure_v2(&validated, &context);
    let lint_diagnostics = lint_procedure_v2(&validated, &context);
    let mut entries = vet_diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| (AuthoringStage::Vet, diagnostic))
        .chain(
            lint_diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| (AuthoringStage::Lint, diagnostic)),
        )
        .collect::<Vec<_>>();

    let projection = normalize_procedure_v2_graph(&validated)
        .and_then(|graph| project_procedure_v2_mermaid(&graph).map(|mermaid| (graph, mermaid)));
    let projection = match projection {
        Ok(projection) => Some(projection),
        Err(error) => {
            entries.push((
                AuthoringStage::Vet,
                config_error_diagnostic(&error, &context),
            ));
            None
        }
    };

    let vet_ok = !entries.iter().any(|(stage, diagnostic)| {
        *stage == AuthoringStage::Vet && diagnostic.severity() == AuthoringSeverity::Error
    });
    let lint_ok = !lint_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity() == AuthoringSeverity::Warning);
    let details = projection
        .filter(|_| vet_ok)
        .map(|(graph, mermaid)| details(&validated, graph, mermaid, request.source_path));

    ProcedurePreviewReportV2 {
        checks: ProcedurePreviewChecksV2 {
            validate: true,
            vet: vet_ok,
            lint: lint_ok,
        },
        details,
        findings: finalize_diagnostics(entries),
    }
}

fn stopped(failure: FormatFailure, context: &AuthoringContext<'_>) -> ProcedurePreviewReportV2 {
    let diagnostics = match failure {
        FormatFailure::NotProcedureV2 => vec![config_error_diagnostic(
            &ConfigError::InvalidSchema {
                expected: podway_core::PROCEDURE_SCHEMA_V2,
                actual: "unsupported procedure schema".to_owned(),
            },
            context,
        )],
        FormatFailure::Diagnostics(diagnostics) => diagnostics,
    };
    ProcedurePreviewReportV2 {
        checks: ProcedurePreviewChecksV2 {
            validate: false,
            vet: false,
            lint: false,
        },
        details: None,
        findings: finalize_diagnostics(
            diagnostics
                .into_iter()
                .map(|diagnostic| (AuthoringStage::Validate, diagnostic))
                .collect(),
        ),
    }
}

fn details(
    validated: &crate::ValidatedProcedureV2,
    graph: ProcedureGraphModelV2,
    mermaid: ProcedureMermaidProjectionV2,
    source_path: &str,
) -> ProcedurePreviewDetailsV2 {
    let parsed = validated.parsed();
    let graph_index = GraphIndex::new(parsed.graph());
    let summary = ProcedurePreviewSummaryV2 {
        definition_count: count(parsed.node_definitions().len()),
        graph_node_count: count(graph.nodes().len()),
        action_node_count: count(
            graph
                .nodes()
                .iter()
                .filter(|node| node.node_type() == GraphProjectionNodeTypeV2::Action)
                .count(),
        ),
        decision_node_count: count(
            graph
                .nodes()
                .iter()
                .filter(|node| node.node_type() == GraphProjectionNodeTypeV2::Decision)
                .count(),
        ),
        route_count: count(
            graph
                .edges()
                .iter()
                .filter(|edge| edge.option_id().is_some())
                .count(),
        ),
        cycle_count: count(graph_index.cyclic_component_count()),
        evidence_reference_count: count(
            parsed
                .graph()
                .placements()
                .iter()
                .map(|placement| {
                    placement_evidence_from(placement)
                        .map_or(0, |evidence| evidence.entries().len())
                })
                .sum(),
        ),
        skippable_node_count: count(graph.nodes().iter().filter(|node| node.skippable()).count()),
        manual_rework_target_count: count(
            parsed
                .graph()
                .manual_rework()
                .map_or(0, |targets| targets.targets().len()),
        ),
    };
    let goal_assessment_graph_node_ids = graph
        .nodes()
        .iter()
        .filter(|node| node.goal_assessment())
        .map(|node| node.graph_node_id().to_owned())
        .collect();
    let preview_graph = ProcedurePreviewGraphV2 {
        entry_graph_node_id: graph.entry_graph_node_id().to_owned(),
        terminal_graph_node_ids: graph.terminal_graph_node_ids().to_vec(),
        nodes: graph
            .nodes()
            .iter()
            .map(|node| ProcedurePreviewGraphNodeV2 {
                graph_node_id: node.graph_node_id().to_owned(),
                node_definition_id: node.node_definition_id().to_owned(),
                node_type: node.node_type(),
                terminal: node.terminal(),
                skippable: node.skippable(),
            })
            .collect(),
        edges: graph
            .edges()
            .iter()
            .map(|edge| ProcedurePreviewGraphEdgeV2 {
                from_graph_node_id: edge.from_graph_node_id().to_owned(),
                to_graph_node_id: edge.to_graph_node_id().to_owned(),
                effect: edge.effect().to_owned(),
                option_id: edge.option_id().map(str::to_owned),
            })
            .collect(),
    };
    let digest = validated.digest().clone();
    ProcedurePreviewDetailsV2 {
        procedure_schema: podway_core::PROCEDURE_SCHEMA_V2,
        procedure_id: parsed.id().to_owned(),
        procedure_version: parsed.version().to_owned(),
        purpose: parsed.purpose().to_owned(),
        procedure_digest: digest.clone(),
        goal_tracking: parsed
            .goal_tracking()
            .is_some_and(|tracking| tracking.is_enabled()),
        goal_assessment_graph_node_ids,
        summary,
        graph: preview_graph,
        mermaid: mermaid.projection().to_owned(),
        start_suggestion: ProcedurePreviewStartSuggestionV2 {
            command: "session.start",
            argv: [
                "podway",
                "start",
                "--procedure",
                source_path,
                "--expect-procedure-digest",
                digest.as_str(),
                "--task",
                "<title>",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        },
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
