//! Bounded authoring advisories for admitted successor definitions.

use std::collections::{BTreeMap, BTreeSet};

use podway_core::{
    AuthoringDiagnostic, AuthoringDiagnosticCode as Code, DecisionDefinitionV2, OptionId,
};

use super::{DecisionRouteV3, NodeDefinitionV3, PlacementKindV3, ValidatedSuccessorDefinition};
use crate::procedure_lint_rules::{
    PAIRWISE_RULE_MAX_FINDINGS, WEAK_GUIDANCE_MIN_CHARS_PRIMARY, WEAK_GUIDANCE_MIN_CHARS_SECONDARY,
    are_confusable, attempt_content_budget_warning, is_weak_guidance, normalize_text,
};
use crate::procedure_v2_authoring::{definition_path, node_path};
use crate::procedure_v2_diagnostics::{AuthoringContext, diagnostic_hint};
use crate::procedure_v2_document::AuthoringValue;
use crate::procedure_v2_source::{FieldPath, field_string};

/// Computes warning-only findings. The caller merges them with other authoring stages.
pub fn lint_successor_definition(
    validated: &ValidatedSuccessorDefinition,
    context: &AuthoringContext<'_>,
) -> Vec<AuthoringDiagnostic> {
    let definition = validated.definition();
    let mut lint = Lint {
        context,
        document: &validated.authoring,
        findings: Vec::new(),
        label_findings: 0,
        criteria_findings: 0,
    };
    if is_weak_guidance(
        Some(definition.header().purpose()),
        WEAK_GUIDANCE_MIN_CHARS_PRIMARY,
    ) {
        let diagnostic = lint.finding(
            Code::WeakPurposeGuidance,
            &FieldPath::root().child_key("purpose"),
            "The purpose does not state what running the document is meant to accomplish.",
        );
        lint.findings.push(diagnostic);
    }
    let used = definition
        .graph()
        .nodes()
        .iter()
        .map(|node| node.definition().as_str())
        .collect::<BTreeSet<_>>();
    let definitions = definition
        .definitions()
        .iter()
        .map(|node| (node.id().as_str(), node))
        .collect::<BTreeMap<_, _>>();
    for node in definition.definitions() {
        let id = node.id().as_str();
        let path = definition_path(id);
        if !used.contains(id) {
            lint.definition_finding(
                Code::UnusedNodeDefinition,
                &path,
                id,
                format!("No graph node uses the node definition `{id}`."),
            );
        }
        match node {
            NodeDefinitionV3::Action(action) => {
                lint.guidance(
                    Code::WeakIntentGuidance,
                    &path.child_key("intent"),
                    id,
                    Some(action.intent()),
                    WEAK_GUIDANCE_MIN_CHARS_PRIMARY,
                    "The action should state the outcome the node must produce.",
                );
                if let Some(message) = attempt_content_budget_warning(action) {
                    lint.definition_finding(
                        Code::AttemptContentBudgetExceeded,
                        &path.child_key("items"),
                        id,
                        message,
                    );
                }
            }
            NodeDefinitionV3::Decision(decision) => lint.decision(decision.metadata()),
        }
    }

    let nodes = definition.graph().nodes();
    let mut confusable_findings = 0;
    for (index, node) in nodes.iter().enumerate() {
        let id = node.id().as_str();
        let path = node_path(index);
        for earlier in &nodes[..index] {
            if confusable_findings == PAIRWISE_RULE_MAX_FINDINGS {
                break;
            }
            let earlier_id = earlier.id().as_str();
            if are_confusable(earlier_id, id) {
                let diagnostic = lint
                    .finding(
                        Code::GraphNodeIdConfusing,
                        &path.child_key("id"),
                        format!("Graph node identifiers `{earlier_id}` and `{id}` are hard to tell apart."),
                    )
                    .with_graph_node_id(id)
                    .with_related_graph_node_ids([earlier_id.to_owned()]);
                lint.findings.push(diagnostic);
                confusable_findings += 1;
            }
        }
        let PlacementKindV3::Decision { routes } = node.kind() else {
            continue;
        };
        let definition_id = node.definition().as_str();
        let NodeDefinitionV3::Decision(decision) = definitions[definition_id] else {
            unreachable!("successor placement and definition kinds already match");
        };
        if node.evidence().is_empty() && decision.metadata().evidence_guidance().is_empty() {
            let diagnostic = lint
                .finding(
                    Code::EvidenceGuidanceMissing,
                    &path,
                    format!("Graph node `{id}` declares neither `evidence_from` nor `evidence_guidance`."),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(definition_id);
            lint.findings.push(diagnostic);
        }
        if let Some(reason) = confusing_rework_reason(id, routes, decision.metadata()) {
            let diagnostic = lint
                .finding(
                    Code::ReworkTopologyConfusing,
                    &path.child_key("routes"),
                    format!("Graph node `{id}` has confusing rework: {reason}."),
                )
                .with_graph_node_id(id);
            lint.findings.push(diagnostic);
        }

        // Compare the successor enum and scoped guards directly. Replan is an
        // explicit boundary effect and can never collapse into local rework.
        if decision.metadata().assessment().is_none()
            && routes.iter().enumerate().any(|(index, (option, route))| {
                routes[..index]
                    .iter()
                    .any(|(earlier_option, earlier_route)| {
                        earlier_route == route
                            && decision.guards(earlier_option) == decision.guards(option)
                    })
            })
        {
            let diagnostic = lint
                .finding(
                    Code::IdenticalEffectiveRoutes,
                    &path.child_key("routes"),
                    format!(
                        "Graph node `{id}` has options with the same route and evidence guards."
                    ),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(definition_id);
            lint.findings.push(diagnostic);
        }
    }
    lint.duplicated_definitions();
    lint.findings
}

fn confusing_rework_reason(
    id: &str,
    routes: &[(OptionId, DecisionRouteV3)],
    decision: &DecisionDefinitionV2,
) -> Option<&'static str> {
    let rework = routes
        .iter()
        .filter_map(|(option_id, route)| match route {
            DecisionRouteV3::Rework(target) => Some((option_id, target)),
            DecisionRouteV3::Advance(_) | DecisionRouteV3::Replan => None,
        })
        .collect::<Vec<_>>();
    if rework.iter().any(|(_, target)| target.as_str() == id) {
        return Some("a local route targets its own decision");
    }
    if rework.len() < 2 {
        return None;
    }
    let mut labels = BTreeSet::new();
    let mut criteria = BTreeSet::new();
    rework
        .into_iter()
        .any(|(id, _)| {
            let option = decision
                .options()
                .iter()
                .find(|option| option.id() == id)
                .expect("successor routes cover declared options");
            !labels.insert(normalize_text(option.label()))
                || is_weak_guidance(option.criteria(), WEAK_GUIDANCE_MIN_CHARS_SECONDARY)
                || option
                    .criteria()
                    .is_some_and(|value| !criteria.insert(normalize_text(value)))
        })
        .then_some("local rework routes lack distinct labels or concrete distinct criteria")
}

struct Lint<'a, 'source> {
    context: &'a AuthoringContext<'source>,
    document: &'a AuthoringValue,
    findings: Vec<AuthoringDiagnostic>,
    label_findings: usize,
    criteria_findings: usize,
}

impl Lint<'_, '_> {
    fn finding(
        &self,
        code: Code,
        path: &FieldPath,
        message: impl Into<String>,
    ) -> AuthoringDiagnostic {
        AuthoringDiagnostic::new(
            code,
            self.context.source_path(),
            self.context.locate(path),
            field_string(path, Some(self.document)),
            message,
            diagnostic_hint(code),
        )
    }

    fn definition_finding(
        &mut self,
        code: Code,
        path: &FieldPath,
        id: &str,
        message: impl Into<String>,
    ) {
        let diagnostic = self
            .finding(code, path, message)
            .with_node_definition_id(id);
        self.findings.push(diagnostic);
    }

    fn guidance(
        &mut self,
        code: Code,
        path: &FieldPath,
        id: &str,
        value: Option<&str>,
        minimum_chars: usize,
        message: &str,
    ) {
        if is_weak_guidance(value, minimum_chars) {
            self.definition_finding(code, path, id, message);
        }
    }

    fn decision(&mut self, decision: &DecisionDefinitionV2) {
        let id = decision.id().as_str();
        let path = definition_path(id);
        for (field, code, text, message) in [
            (
                "objective",
                Code::WeakObjectiveGuidance,
                decision.objective(),
                "The decision should state what it must determine.",
            ),
            (
                "prompt",
                Code::WeakPromptGuidance,
                decision.prompt(),
                "Ask the decision as a concrete question.",
            ),
        ] {
            self.guidance(
                code,
                &path.child_key(field),
                id,
                Some(text),
                WEAK_GUIDANCE_MIN_CHARS_PRIMARY,
                message,
            );
        }
        self.guidance(
            Code::WeakReasonGuidance,
            &path.child_key("reason").child_key("prompt"),
            id,
            decision.reason().prompt(),
            WEAK_GUIDANCE_MIN_CHARS_SECONDARY,
            "Ask for the rationale the recorded reason must explain.",
        );
        if decision.options().len() == 1 {
            self.definition_finding(
                Code::SingleOptionDecision,
                &path.child_key("options"),
                id,
                "The decision declares one option, leaving no alternative to weigh.",
            );
        }
        let mut labels = BTreeMap::new();
        let mut criteria = BTreeMap::new();
        for (index, option) in decision.options().iter().enumerate() {
            let option_id = option.id().as_str();
            let option_path = path.child_key("options").child_index(index);
            let earlier_label = labels
                .entry(normalize_text(option.label()))
                .or_insert(index);
            if *earlier_label != index && self.label_findings < PAIRWISE_RULE_MAX_FINDINGS {
                let earlier_id = decision.options()[*earlier_label].id().as_str();
                let message = format!(
                    "Option `{option_id}` reads the same as `{earlier_id}` after normalizing labels."
                );
                for code in [
                    Code::OptionLabelsNotDistinct,
                    Code::IndistinguishableOptionLabels,
                ] {
                    self.definition_finding(code, &option_path.child_key("label"), id, &message);
                }
                self.label_findings += 1;
            }

            let weak = is_weak_guidance(option.criteria(), WEAK_GUIDANCE_MIN_CHARS_SECONDARY);
            let duplicate = option
                .criteria()
                .map(|text| *criteria.entry(normalize_text(text)).or_insert(index))
                .filter(|earlier| *earlier != index);
            if (weak || duplicate.is_some()) && self.criteria_findings < PAIRWISE_RULE_MAX_FINDINGS
            {
                let criteria_path = option_path.child_key("criteria");
                let message = duplicate.map_or_else(
                    || format!("Option `{option_id}` does not state when it applies."),
                    |earlier| {
                        let earlier_id = decision.options()[earlier].id().as_str();
                        format!("Option `{option_id}` has the same normalized criteria as `{earlier_id}`.")
                    },
                );
                self.definition_finding(Code::OptionCriteriaWeak, &criteria_path, id, message);
                if weak {
                    self.definition_finding(
                        Code::WeakCriteriaGuidance,
                        &criteria_path,
                        id,
                        format!("Option `{option_id}` needs concrete guidance."),
                    );
                }
                self.criteria_findings += 1;
            }
        }
    }

    fn duplicated_definitions(&mut self) {
        let entries = match self.document {
            AuthoringValue::Map(fields) => fields,
            _ => unreachable!("admitted source projection is a mapping"),
        };
        let definitions = match entries.iter().find(|(key, _)| key == "node_definitions") {
            Some((_, AuthoringValue::Map(definitions))) => definitions,
            _ => unreachable!("admitted source projects its definitions"),
        };
        let mut emitted = 0;
        for (index, (id, definition)) in definitions.iter().enumerate() {
            if emitted == PAIRWISE_RULE_MAX_FINDINGS {
                break;
            }
            if let Some((earlier_id, _)) = definitions[..index]
                .iter()
                .find(|(_, earlier)| earlier == definition)
            {
                let diagnostic = self
                    .finding(
                        Code::DuplicatedNodeDefinition,
                        &definition_path(id),
                        format!("Node definition `{id}` is identical to `{earlier_id}` apart from its identifier."),
                    )
                    .with_node_definition_id(id.as_str());
                self.findings.push(diagnostic);
                emitted += 1;
            }
        }
    }
}
