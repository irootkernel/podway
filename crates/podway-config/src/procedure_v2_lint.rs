//! Procedure v2 authoring lint: the 23 advisory rules of dossier section 11.4.
//!
//! **Lint runs on validated models only.** `podway procedure lint` parses, validates, and only then
//! lints; a document whose closed references do not resolve reports the validation error and is not
//! linted at all. Every rule below therefore reads a resolved model — definitions look up by
//! identifier, routes match declared options, evidence sources name real placements — instead of
//! carrying a defensive lookup that would have to invent an answer for a document that is already
//! being rejected.
//!
//! Three properties hold by construction:
//!
//! - **Every finding is a warning.** Severity is bound to the code in `podway-core`, so
//!   `--warnings-as-errors` moves an exit code and never a `severity` field, and lint can never
//!   turn a valid document invalid.
//! - **The report is deterministic.** Rules run in the fixed table order of section 11.4, each
//!   iterating model slices and ordered maps, and the collected vector is *stably* sorted by
//!   `(line, column, code, field)`. Two lints of the same document produce byte-identical output.
//! - **Nothing is unbounded.** The pairwise rules — indistinguishable labels, duplicated
//!   definitions, confusable identifiers — are quadratic in a 64-node, 64-definition model and are
//!   capped at [`PAIRWISE_RULE_MAX_FINDINGS`] findings each, so a pathological document produces a
//!   report a human can still read.
//!
//! Rules that need a path through the graph use [`crate::procedure_v2_graph`], which vet also
//! reads. Lint deliberately reports only the advisory half: an *optional*
//! evidence reference no path can resolve is lint's, the required case is vet's
//! `EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER`; a region with no terminal at all is vet's
//! `NO_TERMINAL_PATH`, never a lint advisory about a distant assessment.

use std::collections::{BTreeMap, BTreeSet};

use podway_core::{
    AuthoringDiagnostic, AuthoringDiagnosticCode, DecisionDefinitionV2, DecisionPlacementV2,
    GraphPlacementV2, TransitionEffectV2,
};

use crate::procedure_v2_authoring::{
    definition_path, node_path, placement_definition_id, placement_evidence_from,
};
use crate::procedure_v2_diagnostics::{AuthoringContext, diagnostic_hint};
use crate::procedure_v2_document::{
    AuthoringValue, authoring_document_value, node_definition_value,
};
use crate::procedure_v2_graph::GraphIndex;
use crate::procedure_v2_source::{FieldPath, field_string};
use crate::{ParsedNodeDefinition, ParsedProcedureV2, ValidatedProcedureV2};

// ---------------------------------------------------------------------------------------------
// Thresholds
// ---------------------------------------------------------------------------------------------
//
// Every number a rule compares against lives in this one table. A threshold spelled inline in a
// rule body is a threshold nobody can review against the specification, and two rules sharing a
// concept ("weak guidance") must share its number or the report contradicts itself.

/// Minimum characters before the primary guidance fields — `purpose`, action `intent`, decision
/// `objective`, decision `prompt` — count as saying anything.
const WEAK_GUIDANCE_MIN_CHARS_PRIMARY: usize = 12;
/// Minimum characters for the secondary guidance fields — option `criteria`, `reason.prompt` —
/// which qualify an already-stated decision rather than introducing one.
const WEAK_GUIDANCE_MIN_CHARS_SECONDARY: usize = 8;
/// Minimum alphanumeric-bearing words in any guidance field. One word is a label, not guidance.
const WEAK_GUIDANCE_MIN_WORDS: usize = 2;
/// How many breadth-first levels count as the "early" prefix a goal-clarification path must reach:
/// the entry placement plus two more levels.
const GOAL_CLARIFICATION_PREFIX_NODES: u32 = 3;
/// The greatest nearest-terminal distance a session-goal assessment may sit at before it is placed
/// long before the work it assesses. The graph is capped at 64 nodes, so this is a real prefix.
const GOAL_ASSESSMENT_TERMINAL_DISTANCE: u32 = 4;
/// The fewest manual rework targets that can count as broad, combined with covering at least half
/// the graph.
const MANUAL_REWORK_BROAD_MINIMUM: usize = 8;
/// The largest option set that still reads as a choice. The schema caps options at eight, so this
/// fires only for six, seven, and eight.
const LARGE_OPTION_SET_MAXIMUM: usize = 5;
/// The largest strongly connected component that still reads as one rework loop.
const LARGE_CYCLE_MAXIMUM: usize = 8;
/// The fewest characters two graph node identifiers must both have before a one-edit difference
/// between them is confusing rather than simply short.
const CONFUSABLE_ID_MIN_CHARS: usize = 4;
/// The per-rule finding cap for the pairwise (quadratic) rules.
const PAIRWISE_RULE_MAX_FINDINGS: usize = 8;

/// Prefixes that mark a field as an unfinished note when followed by punctuation.
///
/// There is deliberately no single-token placeholder table: any one-word value — `placeholder`,
/// `n/a`, `...` — is already weak under the two-word minimum, so a table there would be dead code
/// pretending to carry weight.
const WEAK_GUIDANCE_PREFIXES: &[&str] = &["todo", "tbd", "fixme", "xxx"];

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// Runs every section 11.4 lint rule over a validated Procedure v2 model.
///
/// `context` must wrap the source the model was parsed from: locations are resolved against its
/// path index, and a path the source does not carry (an absent `manual_rework`, an omitted
/// `criteria`) degrades to its longest present ancestor exactly as every other authoring stage
/// does.
pub fn lint_procedure_v2(
    validated: &ValidatedProcedureV2,
    context: &AuthoringContext<'_>,
) -> Vec<AuthoringDiagnostic> {
    Lint::new(validated.parsed(), context).run()
}

/// One lint run: the model, its rendered field-path vocabulary, and the graph index every
/// path-sensitive rule shares.
struct Lint<'a, 'source> {
    parsed: &'a ParsedProcedureV2,
    context: &'a AuthoringContext<'source>,
    document: AuthoringValue,
    graph: GraphIndex<'a>,
    definitions: BTreeMap<&'a str, &'a ParsedNodeDefinition>,
    goal_tracking: bool,
    findings: Vec<AuthoringDiagnostic>,
}

impl<'a> Lint<'a, '_> {
    fn new<'source>(
        parsed: &'a ParsedProcedureV2,
        context: &'a AuthoringContext<'source>,
    ) -> Lint<'a, 'source> {
        Lint {
            parsed,
            context,
            document: authoring_document_value(parsed),
            graph: GraphIndex::new(parsed.graph()),
            definitions: parsed
                .node_definitions()
                .iter()
                .map(|definition| (definition.id().as_str(), definition))
                .collect(),
            goal_tracking: parsed
                .goal_tracking()
                .is_some_and(|opt_in| opt_in.is_enabled()),
            findings: Vec::new(),
        }
    }

    /// Runs the rules in section 11.4 table order, then sorts the report.
    ///
    /// The sort is stable, so a rule that emits several findings in a meaningful order (a cycle's
    /// members, a target list) keeps that order on a full tie, and rule order breaks ties between
    /// two rules that anchor at the same character.
    fn run(mut self) -> Vec<AuthoringDiagnostic> {
        self.unused_node_definitions();
        self.single_option_decisions();
        self.indistinguishable_option_labels();
        self.identical_effective_routes();
        self.weak_purpose_guidance();
        self.weak_definition_guidance();
        self.weak_option_criteria_guidance();
        self.weak_reason_guidance();
        self.evidence_guidance_missing();
        self.optional_evidence_unresolvable();
        self.goal_clarification_path_missing();
        self.goal_assessment_too_early();
        self.manual_rework_targets_broad();
        self.large_option_sets();
        self.large_cycles();
        self.duplicated_node_definitions();
        self.confusable_graph_node_ids();
        self.rework_topology_confusing();
        self.no_reactivation_path();
        self.goal_revision_target_unsafe();
        self.multiple_goal_assessment_sources();

        let mut findings = self.findings;
        findings.sort_by(|left, right| {
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
        findings
    }

    // -----------------------------------------------------------------------------------------
    // Rule 1: UNUSED_NODE_DEFINITION
    // -----------------------------------------------------------------------------------------

    fn unused_node_definitions(&mut self) {
        let used: BTreeSet<&str> = self
            .parsed
            .graph()
            .placements()
            .iter()
            .map(placement_definition_id)
            .collect();
        for definition in self.parsed.node_definitions() {
            let id = definition.id().as_str();
            if used.contains(id) {
                continue;
            }
            let path = definition_path(id);
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::UnusedNodeDefinition,
                    &path,
                    format!("No graph node uses the node definition `{id}`."),
                )
                .with_node_definition_id(id);
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 2: SINGLE_OPTION_DECISION
    // -----------------------------------------------------------------------------------------

    fn single_option_decisions(&mut self) {
        for (id, decision) in self.decision_definitions() {
            if decision.options().len() != 1 {
                continue;
            }
            let path = definition_path(id).child_key("options");
            let related = self.placements_using(id);
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::SingleOptionDecision,
                    &path,
                    format!(
                        "The decision definition `{id}` declares one option, so the decision has no alternative to weigh."
                    ),
                )
                .with_node_definition_id(id)
                .with_related_graph_node_ids(related);
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 3: INDISTINGUISHABLE_OPTION_LABELS
    // -----------------------------------------------------------------------------------------

    fn indistinguishable_option_labels(&mut self) {
        let mut emitted = 0_usize;
        for (id, decision) in self.decision_definitions() {
            for (offset, option) in decision.options().iter().enumerate() {
                if emitted == PAIRWISE_RULE_MAX_FINDINGS {
                    return;
                }
                let normalized = normalize_text(option.label());
                let Some(earlier) = decision.options()[..offset]
                    .iter()
                    .find(|candidate| normalize_text(candidate.label()) == normalized)
                else {
                    continue;
                };
                let path = definition_path(id).child_key("options").child_index(offset);
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::IndistinguishableOptionLabels,
                        &path,
                        format!(
                            "Option `{}` of `{id}` reads the same as option `{}` once case, spacing, and trailing punctuation are normalized.",
                            option.id().as_str(),
                            earlier.id().as_str(),
                        ),
                    )
                    .with_node_definition_id(id);
                self.findings.push(finding);
                emitted = emitted.saturating_add(1);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 4: IDENTICAL_EFFECTIVE_ROUTES
    // -----------------------------------------------------------------------------------------

    /// A goal-assessment decision is exempt: section 7.4 lets all three goal outcomes converge on
    /// one terminal action, and the options remain distinguishable by the outcome each records.
    fn identical_effective_routes(&mut self) {
        for (index, decision) in self.decision_placements() {
            if self.declares_assessment(decision.definition().as_str()) {
                continue;
            }
            let entries = decision.routes().entries();
            if entries.len() < 2 {
                continue;
            }
            let distinct: BTreeSet<(&str, &str)> = entries
                .iter()
                .map(|entry| (entry.route().to().as_str(), entry.route().effect().as_str()))
                .collect();
            if distinct.len() == entries.len() {
                continue;
            }
            let id = decision.id().as_str();
            let path = node_path(index).child_key("routes");
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::IdenticalEffectiveRoutes,
                    &path,
                    format!(
                        "Graph node `{id}` routes {} options to {} distinct target-and-effect pairs, so choosing between some of them changes nothing.",
                        entries.len(),
                        distinct.len(),
                    ),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(decision.definition().as_str());
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 5: WEAK_PURPOSE_GUIDANCE
    // -----------------------------------------------------------------------------------------

    fn weak_purpose_guidance(&mut self) {
        if !is_weak_guidance(Some(self.parsed.purpose()), WEAK_GUIDANCE_MIN_CHARS_PRIMARY) {
            return;
        }
        let path = FieldPath::root().child_key("purpose");
        let finding = self.diagnostic(
            AuthoringDiagnosticCode::WeakPurposeGuidance,
            &path,
            "The procedure `purpose` does not state what running the procedure is meant to accomplish.".to_owned(),
        );
        self.findings.push(finding);
    }

    // -----------------------------------------------------------------------------------------
    // Rules 6, 7, 8: WEAK_INTENT_GUIDANCE, WEAK_OBJECTIVE_GUIDANCE, WEAK_PROMPT_GUIDANCE
    // -----------------------------------------------------------------------------------------
    //
    // One pass over the definitions in author order emits the three primary per-definition
    // guidance rules. Splitting them into three passes would report the same definition three
    // times in three places; the report's own sort restores the section 11.4 order regardless.

    fn weak_definition_guidance(&mut self) {
        for definition in self.parsed.node_definitions() {
            let id = definition.id().as_str();
            match definition {
                ParsedNodeDefinition::Action(action) => {
                    if is_weak_guidance(Some(action.intent()), WEAK_GUIDANCE_MIN_CHARS_PRIMARY) {
                        let path = definition_path(id).child_key("intent");
                        let finding = self
                            .diagnostic(
                                AuthoringDiagnosticCode::WeakIntentGuidance,
                                &path,
                                format!(
                                    "The action definition `{id}` does not state the outcome its node must produce."
                                ),
                            )
                            .with_node_definition_id(id);
                        self.findings.push(finding);
                    }
                }
                ParsedNodeDefinition::Decision(decision) => {
                    if is_weak_guidance(Some(decision.objective()), WEAK_GUIDANCE_MIN_CHARS_PRIMARY)
                    {
                        let path = definition_path(id).child_key("objective");
                        let finding = self
                            .diagnostic(
                                AuthoringDiagnosticCode::WeakObjectiveGuidance,
                                &path,
                                format!(
                                    "The decision definition `{id}` does not state what the decision must determine."
                                ),
                            )
                            .with_node_definition_id(id);
                        self.findings.push(finding);
                    }
                    if is_weak_guidance(Some(decision.prompt()), WEAK_GUIDANCE_MIN_CHARS_PRIMARY) {
                        let path = definition_path(id).child_key("prompt");
                        let finding = self
                            .diagnostic(
                                AuthoringDiagnosticCode::WeakPromptGuidance,
                                &path,
                                format!(
                                    "The decision definition `{id}` does not ask the decision as a concrete question."
                                ),
                            )
                            .with_node_definition_id(id);
                        self.findings.push(finding);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 9: WEAK_CRITERIA_GUIDANCE
    // -----------------------------------------------------------------------------------------

    /// Absent criteria count as weak: an option with no criteria tells the decision-maker nothing
    /// about when to pick it, which is the same failure as criteria that say nothing.
    fn weak_option_criteria_guidance(&mut self) {
        for (id, decision) in self.decision_definitions() {
            for (offset, option) in decision.options().iter().enumerate() {
                if !is_weak_guidance(option.criteria(), WEAK_GUIDANCE_MIN_CHARS_SECONDARY) {
                    continue;
                }
                let path = definition_path(id)
                    .child_key("options")
                    .child_index(offset)
                    .child_key("criteria");
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::WeakCriteriaGuidance,
                        &path,
                        format!(
                            "Option `{}` of `{id}` does not state when it applies.",
                            option.id().as_str(),
                        ),
                    )
                    .with_node_definition_id(id);
                self.findings.push(finding);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 10: WEAK_REASON_GUIDANCE
    // -----------------------------------------------------------------------------------------

    fn weak_reason_guidance(&mut self) {
        for (id, decision) in self.decision_definitions() {
            if !is_weak_guidance(
                decision.reason().prompt(),
                WEAK_GUIDANCE_MIN_CHARS_SECONDARY,
            ) {
                continue;
            }
            let path = definition_path(id).child_key("reason").child_key("prompt");
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::WeakReasonGuidance,
                    &path,
                    format!(
                        "The decision definition `{id}` requires a reason without asking for the rationale it must record."
                    ),
                )
                .with_node_definition_id(id);
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 11: EVIDENCE_GUIDANCE_MISSING
    // -----------------------------------------------------------------------------------------

    fn evidence_guidance_missing(&mut self) {
        for (index, decision) in self.decision_placements() {
            if decision.evidence_from().is_some() {
                continue;
            }
            let definition_id = decision.definition().as_str();
            let has_guidance = self
                .decision_definition(definition_id)
                .is_some_and(|definition| !definition.evidence_guidance().is_empty());
            if has_guidance {
                continue;
            }
            let id = decision.id().as_str();
            let path = node_path(index);
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::EvidenceGuidanceMissing,
                    &path,
                    format!(
                        "Graph node `{id}` declares neither `evidence_from` nor `evidence_guidance`, so the decision-maker is told nothing to consult."
                    ),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(definition_id);
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 12: OPTIONAL_EVIDENCE_UNRESOLVABLE
    // -----------------------------------------------------------------------------------------

    /// Only optional references. A required reference that a path can miss is vet's
    /// `EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER`, a hard error; reporting it here too would
    /// make the same defect appear twice with two severities.
    fn optional_evidence_unresolvable(&mut self) {
        let reachable_from_entry = self.graph.reachable_from(self.graph.entry());
        for index in 0..self.graph.node_count() {
            let Some(placement) = self.graph.placement(index) else {
                continue;
            };
            let Some(entries) = placement_evidence_from(placement) else {
                continue;
            };
            for (offset, reference) in entries.entries().iter().enumerate() {
                if reference.required() {
                    continue;
                }
                let source_id = reference.source_node().as_str();
                let Some(source) = self.graph.index_of(source_id) else {
                    continue;
                };
                let resolvable = reachable_from_entry.contains(source)
                    && self.graph.reachable_from(source).contains(index);
                if resolvable {
                    continue;
                }
                let consumer = placement.id().as_str();
                let path = node_path(index)
                    .child_key("evidence_from")
                    .child_index(offset);
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::OptionalEvidenceUnresolvable,
                        &path,
                        format!(
                            "No path from the graph entry reaches `{source_id}` and then `{consumer}`, so this optional evidence reference can never resolve."
                        ),
                    )
                    .with_graph_node_id(consumer)
                    .with_related_graph_node_ids([source_id.to_owned()]);
                self.findings.push(finding);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 13: GOAL_CLARIFICATION_PATH_MISSING
    // -----------------------------------------------------------------------------------------

    /// The prefix is `GOAL_CLARIFICATION_PREFIX_NODES` breadth-first levels — the entry placement
    /// plus two more — so a qualifying node sits at distance `0`, `1`, or `2`.
    fn goal_clarification_path_missing(&mut self) {
        if !self.goal_tracking {
            return;
        }
        let distances = self.graph.distances_from(self.graph.entry());
        let clarified = (0..self.graph.node_count()).any(|index| {
            let within_prefix = distances
                .get(index)
                .copied()
                .flatten()
                .is_some_and(|distance| distance < GOAL_CLARIFICATION_PREFIX_NODES);
            within_prefix && self.records_or_decides(index)
        });
        if clarified {
            return;
        }
        let path = FieldPath::root().child_key("goal_tracking");
        let finding = self.diagnostic(
            AuthoringDiagnosticCode::GoalClarificationPathMissing,
            &path,
            format!(
                "Goal tracking is enabled, but the first {GOAL_CLARIFICATION_PREFIX_NODES} graph levels hold no decision and no action that records a required item, so the session goal is never clarified early."
            ),
        );
        self.findings.push(finding);
    }

    /// Whether a placement can clarify a goal: a decision asks, and an action with at least one
    /// required item records.
    fn records_or_decides(&self, index: usize) -> bool {
        match self.graph.placement(index) {
            Some(GraphPlacementV2::Decision(_)) => true,
            Some(GraphPlacementV2::Action(action)) => self
                .definitions
                .get(action.definition().as_str())
                .is_some_and(|definition| match definition {
                    ParsedNodeDefinition::Action(action) => {
                        action.items().iter().any(|item| item.common().required())
                    }
                    ParsedNodeDefinition::Decision(_) => false,
                }),
            None => false,
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 14: GOAL_ASSESSMENT_TOO_EARLY
    // -----------------------------------------------------------------------------------------

    /// An assessment that reaches no terminal at all is not reported here: that is vet's
    /// `NO_TERMINAL_PATH`, a hard error about the region, not an advisory about placement.
    fn goal_assessment_too_early(&mut self) {
        for (index, decision) in self.decision_placements() {
            if !self.declares_assessment(decision.definition().as_str()) {
                continue;
            }
            let distances = self.graph.distances_from(index);
            let Some(nearest) = (0..self.graph.node_count())
                .filter(|candidate| self.graph.is_terminal(*candidate))
                .filter_map(|candidate| distances.get(candidate).copied().flatten())
                .min()
            else {
                continue;
            };
            if nearest <= GOAL_ASSESSMENT_TERMINAL_DISTANCE {
                continue;
            }
            let id = decision.id().as_str();
            let path = node_path(index);
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::GoalAssessmentTooEarly,
                    &path,
                    format!(
                        "The session-goal assessment at `{id}` is {nearest} transitions from its nearest terminal action, so it judges the goal long before the work ends."
                    ),
                )
                .with_graph_node_id(id)
                .with_node_definition_id(decision.definition().as_str());
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 15: MANUAL_REWORK_TARGETS_BROAD
    // -----------------------------------------------------------------------------------------

    fn manual_rework_targets_broad(&mut self) {
        let Some(manual_rework) = self.parsed.graph().manual_rework() else {
            return;
        };
        let targets = manual_rework.targets();
        let node_count = self.graph.node_count();
        let broad = targets.len() >= MANUAL_REWORK_BROAD_MINIMUM
            && targets.len().saturating_mul(2) >= node_count;
        if !broad {
            return;
        }
        let path = manual_rework_path().child_key("allowed_targets");
        let finding = self
            .diagnostic(
                AuthoringDiagnosticCode::ManualReworkTargetsBroad,
                &path,
                format!(
                    "`manual_rework.allowed_targets` names {} of the procedure's {node_count} graph nodes, so rework can restart almost anywhere.",
                    targets.len(),
                ),
            )
            .with_related_graph_node_ids(
                targets
                    .iter()
                    .map(|target| target.as_str().to_owned())
                    .collect::<Vec<_>>(),
            );
        self.findings.push(finding);
    }

    // -----------------------------------------------------------------------------------------
    // Rule 16: LARGE_OPTION_SET
    // -----------------------------------------------------------------------------------------

    fn large_option_sets(&mut self) {
        for (id, decision) in self.decision_definitions() {
            let count = decision.options().len();
            if count <= LARGE_OPTION_SET_MAXIMUM {
                continue;
            }
            let path = definition_path(id).child_key("options");
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::LargeOptionSet,
                    &path,
                    format!(
                        "The decision definition `{id}` declares {count} options, more than one decision can be weighed against at once."
                    ),
                )
                .with_node_definition_id(id);
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 17: LARGE_CYCLE
    // -----------------------------------------------------------------------------------------

    /// Strongly connected components, not simple cycles: enumerating simple cycles is exponential,
    /// while a component is the bounded, deterministic "loop region" a reviewer reasons about. A
    /// single-node component is a loop only when it routes to itself; the size threshold already
    /// excludes it, and the guard states the definition rather than relying on that coincidence.
    fn large_cycles(&mut self) {
        for component in self.graph.strongly_connected_components() {
            if !self.is_cyclic_component(&component) || component.len() <= LARGE_CYCLE_MAXIMUM {
                continue;
            }
            let Some(anchor) = component.first().copied() else {
                continue;
            };
            let Some(placement) = self.graph.placement(anchor) else {
                continue;
            };
            let members = component
                .iter()
                .filter_map(|member| self.graph.placement(*member))
                .map(|member| member.id().as_str().to_owned())
                .collect::<Vec<_>>();
            let id = placement.id().as_str();
            let path = node_path(anchor);
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::LargeCycle,
                    &path,
                    format!(
                        "A rework region of {} graph nodes forms one strongly connected component starting at `{id}`.",
                        component.len(),
                    ),
                )
                .with_graph_node_id(id)
                .with_related_graph_node_ids(members);
            self.findings.push(finding);
        }
    }

    fn is_cyclic_component(&self, component: &[usize]) -> bool {
        match component {
            [] => false,
            [single] => self
                .graph
                .successors(*single)
                .iter()
                .any(|successor| successor.target() == *single),
            _ => true,
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 18: DUPLICATED_NODE_DEFINITION
    // -----------------------------------------------------------------------------------------

    /// Equality is over each definition's Canonical JSON v1 fingerprint. The authoring subtree of a
    /// definition does not contain its own identifier — the identifier is the `node_definitions`
    /// map key — so two equal fingerprints mean "identical except for the id" exactly, with no
    /// hand-written field comparison to fall out of date when the model grows a field.
    fn duplicated_node_definitions(&mut self) {
        let fingerprints = self
            .parsed
            .node_definitions()
            .iter()
            .map(|definition| {
                crate::canonical_json_from_serializable(
                    &node_definition_value(definition).into_json(),
                )
                .ok()
                .map(crate::CanonicalJsonV1::into_string)
            })
            .collect::<Vec<_>>();

        let mut emitted = 0_usize;
        for (offset, definition) in self.parsed.node_definitions().iter().enumerate() {
            if emitted == PAIRWISE_RULE_MAX_FINDINGS {
                return;
            }
            let Some(fingerprint) = fingerprints.get(offset).and_then(Option::as_ref) else {
                continue;
            };
            let Some(earlier) = (0..offset).find(|candidate| {
                fingerprints
                    .get(*candidate)
                    .and_then(Option::as_ref)
                    .is_some_and(|other| other == fingerprint)
            }) else {
                continue;
            };
            let Some(earlier_id) = self
                .parsed
                .node_definitions()
                .get(earlier)
                .map(|other| other.id().as_str())
            else {
                continue;
            };
            let id = definition.id().as_str();
            let path = definition_path(id);
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::DuplicatedNodeDefinition,
                    &path,
                    format!(
                        "The node definition `{id}` is identical to `{earlier_id}` apart from its identifier."
                    ),
                )
                .with_node_definition_id(id);
            self.findings.push(finding);
            emitted = emitted.saturating_add(1);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 19: GRAPH_NODE_ID_CONFUSING
    // -----------------------------------------------------------------------------------------

    /// Two shapes of confusion: identifiers that differ only in hyphenation, and identifiers one
    /// edit apart. The edit distance is computed only when the lengths differ by at most one, which
    /// is the precondition for a distance of one anyway and keeps the quadratic scan cheap.
    fn confusable_graph_node_ids(&mut self) {
        let ids = (0..self.graph.node_count())
            .filter_map(|index| self.graph.placement(index))
            .map(|placement| placement.id().as_str())
            .collect::<Vec<_>>();

        let mut emitted = 0_usize;
        for (later, id) in ids.iter().enumerate() {
            for earlier in ids[..later].iter() {
                if emitted == PAIRWISE_RULE_MAX_FINDINGS {
                    return;
                }
                if !are_confusable(earlier, id) {
                    continue;
                }
                let path = node_path(later);
                let finding = self
                    .diagnostic(
                        AuthoringDiagnosticCode::GraphNodeIdConfusing,
                        &path,
                        format!(
                            "The graph node identifiers `{earlier}` and `{id}` are hard to tell apart for a reader or a script."
                        ),
                    )
                    .with_graph_node_id(*id)
                    .with_related_graph_node_ids([(*earlier).to_owned()]);
                self.findings.push(finding);
                emitted = emitted.saturating_add(1);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 20: REWORK_TOPOLOGY_CONFUSING
    // -----------------------------------------------------------------------------------------

    /// Two shapes: a decision whose rework options land on different targets, and a rework route
    /// back to the deciding placement itself. A rework *in*-degree of two or more is deliberately
    /// not a trigger — "verify reworks to implement" and "review reworks to implement" is the
    /// ordinary software-development shape, not a defect.
    fn rework_topology_confusing(&mut self) {
        for (index, decision) in self.decision_placements() {
            let rework_targets = self
                .graph
                .successors(index)
                .iter()
                .filter(|successor| successor.effect() == Some(TransitionEffectV2::Rework))
                .map(|successor| successor.target())
                .collect::<Vec<_>>();
            let distinct = rework_targets.iter().copied().collect::<BTreeSet<_>>();
            let self_rework = distinct.contains(&index);
            let divergent = distinct.len() >= 2;
            if !self_rework && !divergent {
                continue;
            }
            let reason = if self_rework {
                "one rework route points back at the deciding node itself"
            } else {
                "its rework options resume from different graph nodes"
            };
            let id = decision.id().as_str();
            let related = distinct
                .iter()
                .filter_map(|target| self.graph.placement(*target))
                .map(|placement| placement.id().as_str().to_owned())
                .collect::<Vec<_>>();
            let path = node_path(index).child_key("routes");
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::ReworkTopologyConfusing,
                    &path,
                    format!(
                        "The rework topology of graph node `{id}` is legal but confusing: {reason}."
                    ),
                )
                .with_graph_node_id(id)
                .with_related_graph_node_ids(related);
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 21: NO_REACTIVATION_PATH
    // -----------------------------------------------------------------------------------------

    /// A legal authoring choice, reported so the author confirms it was intended. "Absent or empty"
    /// collapses to absent here: `ManualReworkTargetListV2::new` rejects an empty list, so an empty
    /// target list has no representable form.
    fn no_reactivation_path(&mut self) {
        if self.parsed.graph().manual_rework().is_some() {
            return;
        }
        let mut message = "`manual_rework.allowed_targets` is absent, so a completed session can never be reactivated and the procedure is terminal by design.".to_owned();
        if self.goal_tracking {
            message.push_str(
                " The procedure also declares `goal_tracking: true`, so the session goal can never be revised after start.",
            );
        }
        let path = manual_rework_path();
        let finding = self.diagnostic(AuthoringDiagnosticCode::NoReactivationPath, &path, message);
        self.findings.push(finding);
    }

    // -----------------------------------------------------------------------------------------
    // Rule 22: GOAL_REVISION_TARGET_UNSAFE
    // -----------------------------------------------------------------------------------------

    /// Section 7.2's revision-safe target: every path from the target to any terminal action passes
    /// at least one session-goal assessment. The complement is the assessment-free-to-terminal
    /// fixpoint, and "a path from the target includes the target itself", so an assessment
    /// placement is safe through its own placement.
    fn goal_revision_target_unsafe(&mut self) {
        if !self.goal_tracking {
            return;
        }
        let Some(manual_rework) = self.parsed.graph().manual_rework() else {
            return;
        };
        // Materialized before the fixpoint so the closure borrows a plain slice rather than the
        // whole lint state, which is being mutated by the findings this rule appends.
        let assessments = (0..self.graph.node_count())
            .map(|index| self.is_assessment_placement(index))
            .collect::<Vec<_>>();
        let unsafe_nodes = self
            .graph
            .assessment_free_to_terminal(|index| assessments.get(index).copied().unwrap_or(false));
        for (offset, target) in manual_rework.targets().iter().enumerate() {
            let id = target.as_str();
            let Some(index) = self.graph.index_of(id) else {
                continue;
            };
            if !unsafe_nodes.contains(index) {
                continue;
            }
            let path = manual_rework_path()
                .child_key("allowed_targets")
                .child_index(offset);
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::GoalRevisionTargetUnsafe,
                    &path,
                    format!(
                        "The manual rework target `{id}` is not revision-safe: a path from it reaches a terminal action without passing a session-goal assessment, so `goal revise --rework-to {id}` is rejected at runtime."
                    ),
                )
                .with_graph_node_id(id);
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Rule 23: MULTIPLE_GOAL_ASSESSMENT_SOURCES
    // -----------------------------------------------------------------------------------------

    fn multiple_goal_assessment_sources(&mut self) {
        for index in 0..self.graph.node_count() {
            let Some(placement) = self.graph.placement(index) else {
                continue;
            };
            let Some(entries) = placement_evidence_from(placement) else {
                continue;
            };
            let mut sources: Vec<&str> = Vec::new();
            for reference in entries.entries() {
                let source_id = reference.source_node().as_str();
                if sources.contains(&source_id) {
                    continue;
                }
                if self
                    .graph
                    .index_of(source_id)
                    .is_some_and(|source| self.is_assessment_placement(source))
                {
                    sources.push(source_id);
                }
            }
            if sources.len() < 2 {
                continue;
            }
            let id = placement.id().as_str();
            let path = node_path(index).child_key("evidence_from");
            let finding = self
                .diagnostic(
                    AuthoringDiagnosticCode::MultipleGoalAssessmentSources,
                    &path,
                    format!(
                        "Graph node `{id}` reads back {} session-goal assessment sources; two cannot fit the read-back budget together and vet rejects the placement.",
                        sources.len(),
                    ),
                )
                .with_graph_node_id(id)
                .with_related_graph_node_ids(
                    sources
                        .iter()
                        .map(|source| (*source).to_owned())
                        .collect::<Vec<_>>(),
                );
            self.findings.push(finding);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Shared model queries and diagnostic construction
    // -----------------------------------------------------------------------------------------

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

    /// Decision definitions in author order, with their identifiers.
    fn decision_definitions(&self) -> Vec<(&'a str, &'a DecisionDefinitionV2)> {
        self.parsed
            .node_definitions()
            .iter()
            .filter_map(|definition| match definition {
                ParsedNodeDefinition::Decision(decision) => {
                    Some((definition.id().as_str(), decision))
                }
                ParsedNodeDefinition::Action(_) => None,
            })
            .collect()
    }

    /// Decision placements in author order, with their graph index.
    fn decision_placements(&self) -> Vec<(usize, &'a DecisionPlacementV2)> {
        (0..self.graph.node_count())
            .filter_map(|index| match self.graph.placement(index) {
                Some(GraphPlacementV2::Decision(decision)) => Some((index, decision)),
                _ => None,
            })
            .collect()
    }

    fn decision_definition(&self, id: &str) -> Option<&'a DecisionDefinitionV2> {
        match self.definitions.get(id).copied()? {
            ParsedNodeDefinition::Decision(decision) => Some(decision),
            ParsedNodeDefinition::Action(_) => None,
        }
    }

    fn declares_assessment(&self, definition_id: &str) -> bool {
        self.decision_definition(definition_id)
            .is_some_and(|definition| definition.assessment().is_some())
    }

    /// Whether a placement is a session-goal assessment decision.
    fn is_assessment_placement(&self, index: usize) -> bool {
        matches!(
            self.graph.placement(index),
            Some(GraphPlacementV2::Decision(decision))
                if self.declares_assessment(decision.definition().as_str())
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
}

fn manual_rework_path() -> FieldPath {
    FieldPath::root().child_key("manual_rework")
}

// ---------------------------------------------------------------------------------------------
// Shared predicates
// ---------------------------------------------------------------------------------------------

/// Whether a guidance field says nothing useful.
///
/// Five independent clauses, any of which is enough. An absent field is weak: for `criteria` and
/// `reason.prompt` the absence and an empty gesture are the same failure from the reader's side.
fn is_weak_guidance(value: Option<&str>, minimum_chars: usize) -> bool {
    let Some(value) = value else {
        return true;
    };
    let trimmed = value.trim();
    if trimmed.chars().count() < minimum_chars {
        return true;
    }
    let words = trimmed
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count();
    if words < WEAK_GUIDANCE_MIN_WORDS {
        return true;
    }
    let normalized = normalize_text(trimmed);
    if WEAK_GUIDANCE_PREFIXES.iter().any(|prefix| {
        normalized
            .strip_prefix(prefix)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|next| !next.is_alphanumeric())
    }) {
        return true;
    }
    is_unfilled_placeholder_value(trimmed)
}

/// Whether the whole value is one `<…>` span: the shape of an authoring template nobody filled in.
///
/// The rule is deliberately whole-value, not substring: `Verify the page renders <h1> first.` and
/// `Confirm 0 < latency and p99 > 1s.` are ordinary prose that a substring scan would misreport,
/// while a field whose entire content is `<describe the outcome>` carries no guidance at all.
fn is_unfilled_placeholder_value(value: &str) -> bool {
    let mut characters = value.chars();
    if characters.next() != Some('<') {
        return false;
    }
    let Some(last) = value.chars().next_back() else {
        return false;
    };
    if last != '>' {
        return false;
    }
    // Exactly one span: no interior delimiter may close or reopen it.
    let interior: Vec<char> = value.chars().skip(1).collect();
    let interior = &interior[..interior.len().saturating_sub(1)];
    !interior
        .iter()
        .any(|character| *character == '<' || *character == '>')
}

/// The comparison form of an authored text: lowercase, whitespace-collapsed, trimmed, and stripped
/// of trailing sentence punctuation. Two labels that differ only in those respects read the same.
fn normalize_text(value: &str) -> String {
    let collapsed = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed
        .trim_end_matches(['.', '!', '?'])
        .trim()
        .to_owned()
}

/// Whether two graph node identifiers are easy to confuse.
fn are_confusable(left: &str, right: &str) -> bool {
    if left.replace('-', "").to_lowercase() == right.replace('-', "").to_lowercase() {
        return true;
    }
    let left_characters: Vec<char> = left.chars().collect();
    let right_characters: Vec<char> = right.chars().collect();
    if left_characters.len() < CONFUSABLE_ID_MIN_CHARS
        || right_characters.len() < CONFUSABLE_ID_MIN_CHARS
    {
        return false;
    }
    if left_characters.len().abs_diff(right_characters.len()) > 1 {
        return false;
    }
    edit_distance(&left_characters, &right_characters) <= 1
}

/// Levenshtein distance over two character slices, computed with one rolling row.
///
/// Hand-rolled rather than pulled in: `podway-config`'s dependency set is a contract surface, and a
/// 64-identifier comparison does not justify widening it.
fn edit_distance(left: &[char], right: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0_usize; right.len().saturating_add(1)];
    for (row, left_character) in left.iter().enumerate() {
        current[0] = row.saturating_add(1);
        for (column, right_character) in right.iter().enumerate() {
            let substitution = usize::from(left_character != right_character);
            current[column.saturating_add(1)] = previous[column]
                .saturating_add(substitution)
                .min(previous[column.saturating_add(1)].saturating_add(1))
                .min(current[column].saturating_add(1));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}
