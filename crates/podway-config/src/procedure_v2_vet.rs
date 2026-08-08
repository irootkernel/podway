//! Mandatory graph-wide semantic analysis for Procedure v2 (dossier section 11.3).
//!
//! Vet receives a closed-reference-validated model. It therefore owns only facts that require a
//! path through the complete graph: reachability, terminal-route existence, the advance-only cycle
//! rule, assessment coverage, the three dominance uses (evidence, declared rework, and goal
//! assessment), and the two per-placement wire-budget proofs.

use std::collections::{BTreeMap, BTreeSet};

use podway_core::{
    AuthoringDiagnostic, AuthoringDiagnosticCode, GoalOutcome, GraphPlacementV2, TransitionEffectV2,
};

use crate::procedure_v2_authoring::{
    definition_path, node_path, placement_definition_id, placement_evidence_from,
};
use crate::procedure_v2_budget::{
    NEXT_STATIC_BUDGET, READBACK_BUDGET, exceeds_budget, placement_budget,
};
use crate::procedure_v2_diagnostics::{AuthoringContext, diagnostic_hint};
use crate::procedure_v2_document::{AuthoringValue, authoring_document_value};
use crate::procedure_v2_graph::{GraphIndex, NodeSet};
use crate::procedure_v2_source::{FieldPath, field_string};
use crate::{ParsedNodeDefinition, ParsedProcedureV2, ValidatedProcedureV2};

/// Runs the section 11.3 structural graph rule set over a validated Procedure v2 model.
pub fn vet_procedure_v2(
    validated: &ValidatedProcedureV2,
    context: &AuthoringContext<'_>,
) -> Vec<AuthoringDiagnostic> {
    Vet::new(validated.parsed(), context).run()
}

struct Vet<'a, 'source> {
    parsed: &'a ParsedProcedureV2,
    context: &'a AuthoringContext<'source>,
    document: AuthoringValue,
    graph: GraphIndex<'a>,
    definitions: BTreeMap<&'a str, &'a ParsedNodeDefinition>,
    reachable: NodeSet,
    reaches_terminal: NodeSet,
    dominators: Vec<NodeSet>,
    findings: Vec<AuthoringDiagnostic>,
}

impl<'a> Vet<'a, '_> {
    fn new<'source>(
        parsed: &'a ParsedProcedureV2,
        context: &'a AuthoringContext<'source>,
    ) -> Vet<'a, 'source> {
        let graph = GraphIndex::new(parsed.graph());
        let reachable = graph.reachable_from(graph.entry());
        let reaches_terminal = graph.terminal_reachable_nodes();
        let dominators = graph.dominators();
        Vet {
            parsed,
            context,
            document: authoring_document_value(parsed),
            graph,
            definitions: parsed
                .node_definitions()
                .iter()
                .map(|definition| (definition.id().as_str(), definition))
                .collect(),
            reachable,
            reaches_terminal,
            dominators,
            findings: Vec::new(),
        }
    }

    fn run(mut self) -> Vec<AuthoringDiagnostic> {
        self.reachability();
        self.terminal_paths();
        self.assessment_coverage();
        self.goal_assessment_dominance();
        self.evidence_rules();
        self.resource_budgets();
        self.advance_only_cycles();
        self.rework_dominance();

        self.findings.sort_by(|left, right| {
            (
                left.location().line(),
                left.location().column(),
                left.code().as_str(),
                left.field(),
            )
                .cmp(&(
                    right.location().line(),
                    right.location().column(),
                    right.code().as_str(),
                    right.field(),
                ))
        });
        self.findings
    }

    fn reachability(&mut self) {
        for node in 0..self.graph.node_count() {
            if self.reachable.contains(node) {
                continue;
            }
            let Some(placement) = self.graph.placement(node) else {
                continue;
            };
            let id = placement.id().as_str();
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::UnreachableGraphNode,
                    &node_path(node),
                    format!("Graph node `{id}` is unreachable from the entry node."),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(placement_definition_id(placement));
            self.findings.push(finding);
        }
    }

    fn terminal_paths(&mut self) {
        for node in 0..self.graph.node_count() {
            if !self.reachable.contains(node) || self.reaches_terminal.contains(node) {
                continue;
            }
            let Some(placement) = self.graph.placement(node) else {
                continue;
            };
            let id = placement.id().as_str();
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::NoTerminalPath,
                    &node_path(node),
                    format!("Reachable graph node `{id}` has no finite path to a terminal action."),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(placement_definition_id(placement));
            self.findings.push(finding);
        }
    }

    fn assessment_coverage(&mut self) {
        for definition in self.parsed.node_definitions() {
            let ParsedNodeDefinition::Decision(decision) = definition else {
                continue;
            };
            let Some(assessment) = decision.assessment() else {
                continue;
            };
            let definition_id = decision.id().as_str();
            let path = definition_path(definition_id)
                .child_key("assessment")
                .child_key("outcomes");
            let mapped: BTreeSet<&str> = assessment
                .outcomes()
                .iter()
                .map(|mapping| mapping.option_id().as_str())
                .collect();
            for (option_index, option) in decision.options().iter().enumerate() {
                let option_id = option.id().as_str();
                if mapped.contains(option_id) {
                    continue;
                }
                let option_path = definition_path(definition_id)
                    .child_key("options")
                    .child_index(option_index);
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::GoalAssessmentOptionUnmapped,
                        &option_path,
                        format!(
                            "Goal-assessment option `{option_id}` in definition `{definition_id}` has no outcome mapping."
                        ),
                    )
                    .with_node_definition_id(definition_id)
                    .with_related_graph_node_ids(self.placements_using(definition_id));
                self.findings.push(finding);
            }

            let mut outcomes = [false; 3];
            for mapping in assessment.outcomes() {
                outcomes[outcome_index(mapping.outcome())] = true;
            }
            for missing in [
                GoalOutcome::Achieved,
                GoalOutcome::NotAchieved,
                GoalOutcome::Superseded,
            ]
            .into_iter()
            .filter(|outcome| !outcomes[outcome_index(*outcome)])
            {
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::GoalAssessmentOutcomeUnreachable,
                        &path,
                        format!(
                            "Goal-assessment definition `{definition_id}` cannot record outcome `{}`.",
                            missing.as_str(),
                        ),
                    )
                    .with_node_definition_id(definition_id)
                    .with_related_graph_node_ids(self.placements_using(definition_id));
                self.findings.push(finding);
            }
        }
    }

    fn goal_assessment_dominance(&mut self) {
        let goal_tracking = self
            .parsed
            .goal_tracking()
            .is_some_and(|policy| policy.is_enabled());
        if !goal_tracking {
            return;
        }
        let assessment_nodes: Vec<usize> = (0..self.graph.node_count())
            .filter(|node| self.is_assessment_placement(*node))
            .collect();
        let assessment_ids: Vec<String> = assessment_nodes
            .iter()
            .filter_map(|node| self.graph.placement(*node))
            .map(|placement| placement.id().as_str().to_owned())
            .collect();
        for terminal in 0..self.graph.node_count() {
            if !self.reachable.contains(terminal)
                || !self.graph.is_terminal(terminal)
                || assessment_nodes.iter().any(|assessment| {
                    self.graph
                        .strictly_dominates(&self.dominators, *assessment, terminal)
                })
            {
                continue;
            }
            let Some(placement) = self.graph.placement(terminal) else {
                continue;
            };
            let id = placement.id().as_str();
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::GoalAssessmentNotDominatingTerminal,
                    &node_path(terminal),
                    format!(
                        "Terminal graph node `{id}` is reachable without completing a session-goal assessment."
                    ),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(placement_definition_id(placement))
                .with_related_graph_node_ids(assessment_ids.clone());
            self.findings.push(finding);
        }
    }

    fn evidence_rules(&mut self) {
        for consumer in 0..self.graph.node_count() {
            let Some(placement) = self.graph.placement(consumer) else {
                continue;
            };
            let Some(references) = placement_evidence_from(placement) else {
                continue;
            };
            if !self.reachable.contains(consumer) {
                continue;
            }
            for (offset, reference) in references.entries().iter().enumerate() {
                if !reference.required() {
                    continue;
                }
                let Some(source) = self.graph.index_of(reference.source_node().as_str()) else {
                    continue;
                };
                let path = node_path(consumer)
                    .child_key("evidence_from")
                    .child_index(offset);
                if !self
                    .graph
                    .strictly_dominates(&self.dominators, source, consumer)
                {
                    let finding = self
                        .diagnostic(
                            AuthoringDiagnosticCode::EvidenceSourceDoesNotDominateConsumer,
                            &path,
                            format!(
                                "Required evidence source `{}` does not strictly dominate consumer `{}`.",
                                reference.source_node().as_str(),
                                placement.id().as_str(),
                            ),
                        )
                        .with_graph_node_id(placement.id().as_str())
                        .with_node_definition_id(placement_definition_id(placement))
                        .with_related_graph_node_ids([reference.source_node().as_str().to_owned()]);
                    self.findings.push(finding);
                }
                if self.source_is_skippable(source) {
                    let finding = self
                        .diagnostic(
                            AuthoringDiagnosticCode::SkippableEvidenceSource,
                            &path,
                            format!(
                                "Required evidence source `{}` may be skipped before consumer `{}` activates.",
                                reference.source_node().as_str(),
                                placement.id().as_str(),
                            ),
                        )
                        .with_graph_node_id(placement.id().as_str())
                        .with_node_definition_id(placement_definition_id(placement))
                        .with_related_graph_node_ids([reference.source_node().as_str().to_owned()]);
                    self.findings.push(finding);
                }
            }
        }
    }

    fn advance_only_cycles(&mut self) {
        for component in self.graph.advance_only_cycles() {
            let Some(first) = component.first().copied() else {
                continue;
            };
            let Some(placement) = self.graph.placement(first) else {
                continue;
            };
            let related: Vec<String> = component
                .iter()
                .filter_map(|node| self.graph.placement(*node))
                .map(|member| member.id().as_str().to_owned())
                .collect();
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::GraphCycleInvalid,
                    &node_path(first),
                    format!(
                        "The cycle containing graph node `{}` has an advance-only traversal.",
                        placement.id().as_str(),
                    ),
                )
                .with_graph_node_id(placement.id().as_str())
                .with_node_definition_id(placement_definition_id(placement))
                .with_related_graph_node_ids(related);
            self.findings.push(finding);
        }
    }

    fn resource_budgets(&mut self) {
        for node in 0..self.graph.node_count() {
            let Some(placement) = self.graph.placement(node) else {
                continue;
            };
            let budget = placement_budget(self.parsed, placement);
            let graph_node_id = placement.id().as_str().to_owned();
            let definition_id = placement_definition_id(placement).to_owned();
            let related_sources: Vec<String> = placement_evidence_from(placement)
                .into_iter()
                .flat_map(|references| references.entries())
                .map(|reference| reference.source_node().as_str().to_owned())
                .collect();

            if exceeds_budget(budget.next_static, NEXT_STATIC_BUDGET) {
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::NextStaticBudgetExceeded,
                        &node_path(node),
                        format!(
                            "Graph node `{graph_node_id}` charges {} bytes of procedure-static next content, over the {NEXT_STATIC_BUDGET}-byte budget.",
                            budget.next_static,
                        ),
                    )
                    .with_graph_node_id(graph_node_id.clone())
                    .with_node_definition_id(definition_id.clone());
                self.findings.push(finding);
            }

            if exceeds_budget(budget.readback, READBACK_BUDGET) {
                let path = node_path(node).child_key("evidence_from");
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::ReadbackBudgetExceeded,
                        &path,
                        format!(
                            "Graph node `{graph_node_id}` charges {} bytes of worst-case evidence read-back, over the {READBACK_BUDGET}-byte budget.",
                            budget.readback,
                        ),
                    )
                    .with_graph_node_id(graph_node_id)
                    .with_node_definition_id(definition_id)
                    .with_related_graph_node_ids(related_sources);
                self.findings.push(finding);
            }
        }
    }

    fn rework_dominance(&mut self) {
        for node in 0..self.graph.node_count() {
            let Some(GraphPlacementV2::Decision(decision)) = self.graph.placement(node) else {
                continue;
            };
            if !self.reachable.contains(node) {
                continue;
            }
            for entry in decision.routes().entries() {
                if entry.route().effect() != TransitionEffectV2::Rework {
                    continue;
                }
                let Some(target) = self.graph.index_of(entry.route().to().as_str()) else {
                    continue;
                };
                if self.graph.dominates(&self.dominators, target, node) {
                    continue;
                }
                let path = node_path(node)
                    .child_key("routes")
                    .child_key(entry.option_id().as_str())
                    .child_key("to");
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::ReworkTargetNotDominating,
                        &path,
                        format!(
                            "Rework target `{}` does not dominate decision `{}`.",
                            entry.route().to().as_str(),
                            decision.id().as_str(),
                        ),
                    )
                    .with_graph_node_id(decision.id().as_str())
                    .with_node_definition_id(decision.definition().as_str())
                    .with_related_graph_node_ids([entry.route().to().as_str().to_owned()]);
                self.findings.push(finding);
            }
        }
    }

    fn diagnostic(
        &self,
        code: AuthoringDiagnosticCode,
        path: &FieldPath,
        message: String,
    ) -> AuthoringDiagnostic {
        AuthoringDiagnostic::new(
            code,
            self.context.source_path(),
            self.context.locate(path),
            field_string(path, Some(&self.document)),
            message,
            diagnostic_hint(code),
        )
    }

    fn placements_using(&self, definition_id: &str) -> Vec<String> {
        self.parsed
            .graph()
            .placements()
            .iter()
            .filter(|placement| placement_definition_id(placement) == definition_id)
            .map(|placement| placement.id().as_str().to_owned())
            .collect()
    }

    fn is_assessment_placement(&self, node: usize) -> bool {
        let Some(GraphPlacementV2::Decision(placement)) = self.graph.placement(node) else {
            return false;
        };
        matches!(
            self.definitions.get(placement.definition().as_str()),
            Some(ParsedNodeDefinition::Decision(definition)) if definition.assessment().is_some()
        )
    }

    fn source_is_skippable(&self, node: usize) -> bool {
        matches!(
            self.graph.placement(node),
            Some(GraphPlacementV2::Action(action))
                if action.skip().is_some_and(|policy| policy.is_allowed())
        )
    }
}

const fn outcome_index(outcome: GoalOutcome) -> usize {
    match outcome {
        GoalOutcome::Achieved => 0,
        GoalOutcome::NotAchieved => 1,
        GoalOutcome::Superseded => 2,
    }
}
