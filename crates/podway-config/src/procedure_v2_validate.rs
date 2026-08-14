//! Procedure v2 closed semantic validation (dossier section 11.2).
//!
//! This is the second authoring stage. Parsing (`procedure_v2_parse`) stays permissive about
//! everything that needs a second declaration to decide: it maps one document into the core v2
//! authoring model and enforces only the bounds each individual value owns. This module resolves
//! the document's own closed reference set — placement `use`, `next` and route targets, evidence
//! sources and selectors, assessment option mappings, and manual rework targets — against the
//! declarations the same document makes, and nothing else.
//!
//! Scope boundary. Section 11.2 admits only local, lookup-closed structure; every check that needs
//! a path through the graph belongs to vet (section 11.3, V2GRF-001/V2GRF-002): reachability,
//! terminal-path existence, the cycle rule, dominance (required evidence sources, declared rework
//! targets, goal-assessment dominance of terminal actions), skippable evidence sources, the
//! read-back and next-static budgets, and goal-assessment option/outcome coverage (section 7.4
//! assigns "leaves an option unmapped or an outcome unreachable" to vet explicitly). Checks the
//! core constructors already guarantee on a parsed model are not repeated here either: per-scope
//! identifier uniqueness, collection bounds, the action `next`-or-`terminal` disposition, the
//! decision "routes only" shape, and the presence of the entry placement in the graph.
//!
//! Determinism. Checks run in a fixed order — node definitions in author order, then graph
//! placements in author order (and, within a placement, `use`, then `evidence_from`, then the
//! outcome or route table, mirroring the authored field order), then `manual_rework` — and the
//! first failure wins. Lookups go through ordered maps that are read, never iterated, so two
//! validations of the same parsed model always produce the identical `ConfigError`.
//!
//! Diagnostic binding. Every rejection below is classified into
//! `assets/specifications/authoring-diagnostics.json` by
//! [`crate::procedure_v2_diagnostics::config_error_diagnostic`]. That classifier distinguishes the
//! checks by the `field` and `reason` constants declared here, so the codes it reports and the
//! rejections raised here are the same closed set by construction: a check cannot change the shape
//! it reports without moving the constant the classifier matches on.

use std::collections::BTreeMap;

use podway_core::{
    ActionOutcomeV2, EvidenceFromListV2, GraphNodeId, GraphPlacementV2, ItemSpecV2,
    NodeDefinitionId, Sha256Digest,
};

use crate::procedure_v2_canonical::canonical_projection;
use crate::{CanonicalJsonV1, ConfigError, ParsedNodeDefinition, ParsedProcedureV2};

/// A Procedure v2 model that passed closed semantic validation (section 11.2 closed references),
/// together with the canonical bytes and digest section 12.1 derives from it.
///
/// The wrapped model is private and the only constructor is [`validate_procedure_v2`], so the type
/// cannot be forged from an unvalidated document, and canonical bytes can never disagree with the
/// model they were produced from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedProcedureV2 {
    parsed: ParsedProcedureV2,
    canonical_json: CanonicalJsonV1,
    digest: Sha256Digest,
}

impl ValidatedProcedureV2 {
    pub fn parsed(&self) -> &ParsedProcedureV2 {
        &self.parsed
    }

    /// The model-derived Canonical JSON/IR document: the validation, digest, snapshot, and runtime
    /// authority of section 12.1.
    pub fn canonical_json(&self) -> &CanonicalJsonV1 {
        &self.canonical_json
    }

    /// The SHA-256 digest over the canonical bytes.
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

/// Runs closed semantic validation over an already-parsed Procedure v2 model, then canonicalizes it.
///
/// Parsing and validation are discrete stages (section 13.3 sequences them); this function never
/// re-decodes source bytes and never mutates the model. Canonicalization runs only after the closed
/// reference set resolves, so canonical bytes exist for exactly the models that are admissible, and
/// a closed-reference diagnostic always wins over a canonical-projection one.
pub fn validate_procedure_v2(
    parsed: ParsedProcedureV2,
) -> Result<ValidatedProcedureV2, ConfigError> {
    validate_closed_references(&parsed)?;
    let (canonical_json, digest) = canonical_projection(&parsed)?;
    Ok(ValidatedProcedureV2 {
        parsed,
        canonical_json,
        digest,
    })
}

// -------------------------------------------------------------------------------------------
// Shared rejection vocabulary
//
// The authored field shapes and the static reasons below are the only content a closed-reference
// rejection carries, and `procedure_v2_diagnostics` switches on exactly these constants to select
// the catalog code. They are declared once and read from both places so a raise site and its
// classification cannot drift apart.
// -------------------------------------------------------------------------------------------

/// A placement's `use`: unknown here is `GRAPH_DEFINITION_UNKNOWN`, a kind disagreement is a closed
/// shape violation.
pub(crate) const PLACEMENT_USE_FIELD: &str = "graph.nodes.use";
/// An action placement's `next` target: `ROUTE_TARGET_NOT_FOUND`.
pub(crate) const PLACEMENT_NEXT_FIELD: &str = "graph.nodes.next";
/// A decision placement's route table, as a whole: the two option/route coverage failures.
pub(crate) const PLACEMENT_ROUTES_FIELD: &str = "graph.nodes.routes";
/// One route's target graph node: `ROUTE_TARGET_NOT_FOUND`.
pub(crate) const PLACEMENT_ROUTE_TARGET_FIELD: &str = "graph.nodes.routes.to";
/// An evidence reference's source placement: unknown is `EVIDENCE_SOURCE_UNKNOWN`, naming the
/// consumer is `EVIDENCE_SOURCE_SELF_REFERENCE`.
pub(crate) const EVIDENCE_SOURCE_FIELD: &str = "graph.nodes.evidence_from.node";
/// One selected evidence item: `EVIDENCE_SELECTOR_UNKNOWN_ITEM`.
pub(crate) const EVIDENCE_SELECTOR_FIELD: &str = "graph.nodes.evidence_from.items";
/// A declared manual rework target: `MANUAL_REWORK_TARGET_UNKNOWN`.
pub(crate) const MANUAL_REWORK_TARGETS_FIELD: &str = "manual_rework.allowed_targets";
/// A session-goal assessment's outcome table.
pub(crate) const ASSESSMENT_OUTCOMES_FIELD: &str = "node_definitions.assessment.outcomes";
/// A session-goal assessment contract as a whole: `GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING`.
pub(crate) const ASSESSMENT_FIELD: &str = "node_definitions.assessment";

/// Check 2, action half. A closed-shape violation: no catalog code names a kind disagreement.
pub(crate) const ACTION_PLACEMENT_KIND_REASON: &str =
    "an action placement must use an action definition";
/// Check 2, decision half. As above.
pub(crate) const DECISION_PLACEMENT_KIND_REASON: &str =
    "a decision placement must use a decision definition";
/// Check 4a: `DECISION_ROUTE_OPTION_UNDEFINED`.
pub(crate) const ROUTE_OPTION_UNDECLARED_REASON: &str =
    "a route names an option the decision definition does not declare";
/// Check 4b: `DECISION_OPTION_ROUTE_MISSING`.
pub(crate) const OPTION_ROUTE_MISSING_REASON: &str =
    "every declared decision option must have exactly one route";
/// Check 5b: `EVIDENCE_SOURCE_SELF_REFERENCE`.
pub(crate) const EVIDENCE_SELF_REFERENCE_REASON: &str =
    "an evidence reference must not name its consuming placement";
/// Check 7a. A closed-shape violation; see `procedure_v2_diagnostics` for why no catalog code fits.
pub(crate) const ASSESSMENT_OUTCOME_UNDECLARED_OPTION_REASON: &str =
    "an assessment outcome names an option the decision definition does not declare";
/// Check 7b: `GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING`.
pub(crate) const ASSESSMENT_REQUIRES_GOAL_TRACKING_REASON: &str =
    "a session-goal assessment requires the procedure to declare goal_tracking: true";

/// Node definitions indexed by identifier. Read-only lookup: never iterated for diagnostics.
type DefinitionIndex<'a> = BTreeMap<&'a str, &'a ParsedNodeDefinition>;
/// Graph placements indexed by graph node identifier. Read-only lookup, as above.
type PlacementIndex<'a> = BTreeMap<&'a str, &'a GraphPlacementV2>;

fn unknown_reference(field: &'static str, value: &str) -> ConfigError {
    ConfigError::UnknownV2Reference {
        field,
        value: value.to_owned(),
    }
}

const fn shape_mismatch(field: &'static str, reason: &'static str) -> ConfigError {
    ConfigError::V2ShapeMismatch { field, reason }
}

fn validate_closed_references(parsed: &ParsedProcedureV2) -> Result<(), ConfigError> {
    let definitions: DefinitionIndex<'_> = parsed
        .node_definitions()
        .iter()
        .map(|definition| (definition.id().as_str(), definition))
        .collect();
    let placements: PlacementIndex<'_> = parsed
        .graph()
        .placements()
        .iter()
        .map(|placement| (placement.id().as_str(), placement))
        .collect();

    let goal_tracking = parsed
        .goal_tracking()
        .is_some_and(|opt_in| opt_in.is_enabled());
    for definition in parsed.node_definitions() {
        validate_definition(definition, goal_tracking)?;
    }
    for placement in parsed.graph().placements() {
        validate_placement(placement, &definitions, &placements)?;
    }
    if let Some(manual_rework) = parsed.graph().manual_rework() {
        // Check 8: every declared manual rework target names an existing placement
        // (MANUAL_REWORK_TARGET_UNKNOWN). Whether a target is on the current valid trace is a
        // runtime precondition (section 9.5), not an authoring one.
        for target in manual_rework.targets() {
            if !placements.contains_key(target.as_str()) {
                return Err(unknown_reference(
                    MANUAL_REWORK_TARGETS_FIELD,
                    target.as_str(),
                ));
            }
        }
    }
    Ok(())
}

/// Definition-level checks. Only a decision definition carrying a session-goal assessment contract
/// has any: an action definition is fully checked by its constructor.
fn validate_definition(
    definition: &ParsedNodeDefinition,
    goal_tracking: bool,
) -> Result<(), ConfigError> {
    let ParsedNodeDefinition::Decision(decision) = definition else {
        return Ok(());
    };
    let Some(assessment) = decision.assessment() else {
        return Ok(());
    };

    // Check 7a: an outcome mapping may only name an option this definition declares. The opposite
    // direction — every declared option mapped, and every one of the three goal outcomes reachable
    // — is coverage, which section 7.4 assigns to vet (GOAL_ASSESSMENT_OPTION_UNMAPPED,
    // GOAL_ASSESSMENT_OUTCOME_UNREACHABLE).
    for mapping in assessment.outcomes() {
        if !decision
            .options()
            .iter()
            .any(|option| option.id() == mapping.option_id())
        {
            return Err(shape_mismatch(
                ASSESSMENT_OUTCOMES_FIELD,
                ASSESSMENT_OUTCOME_UNDECLARED_OPTION_REASON,
            ));
        }
    }

    // Check 7b: a session-goal assessment requires the procedure-level opt-in
    // (GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING; sections 7.1 and 11.3). `goal_tracking` is a scalar
    // opt-in whose only accepted value is `true`, so its presence is its enablement.
    if !goal_tracking {
        return Err(shape_mismatch(
            ASSESSMENT_FIELD,
            ASSESSMENT_REQUIRES_GOAL_TRACKING_REASON,
        ));
    }
    Ok(())
}

fn validate_placement(
    placement: &GraphPlacementV2,
    definitions: &DefinitionIndex<'_>,
    placements: &PlacementIndex<'_>,
) -> Result<(), ConfigError> {
    let definition = lookup_definition(placement_definition(placement), definitions)?;

    match (placement, definition) {
        (GraphPlacementV2::Action(action), ParsedNodeDefinition::Action(_)) => {
            validate_evidence(action.id(), action.evidence_from(), definitions, placements)?;
            // Check 3 (action half): a declared `next` names an existing placement
            // (ROUTE_TARGET_NOT_FOUND). A terminal disposition has no target to resolve.
            if let ActionOutcomeV2::Next(target) = action.outcome()
                && !placements.contains_key(target.as_str())
            {
                return Err(unknown_reference(PLACEMENT_NEXT_FIELD, target.as_str()));
            }
            Ok(())
        }
        (GraphPlacementV2::Decision(decision), ParsedNodeDefinition::Decision(used)) => {
            validate_evidence(
                decision.id(),
                decision.evidence_from(),
                definitions,
                placements,
            )?;
            for entry in decision.routes().entries() {
                // Check 4a: no route may name an option the used definition does not declare
                // (DECISION_ROUTE_OPTION_UNDEFINED).
                if !used
                    .options()
                    .iter()
                    .any(|option| option.id() == entry.option_id())
                {
                    return Err(shape_mismatch(
                        PLACEMENT_ROUTES_FIELD,
                        ROUTE_OPTION_UNDECLARED_REASON,
                    ));
                }
                // Check 3 (decision half): every route target names an existing placement
                // (ROUTE_TARGET_NOT_FOUND).
                let target = entry.route().to();
                if !placements.contains_key(target.as_str()) {
                    return Err(unknown_reference(
                        PLACEMENT_ROUTE_TARGET_FIELD,
                        target.as_str(),
                    ));
                }
            }
            // Check 4b: every declared option is routed (DECISION_OPTION_ROUTE_MISSING). Route
            // option identifiers are unique by construction, so "at least one route" is "exactly
            // one route" here.
            for option in used.options() {
                if !decision
                    .routes()
                    .entries()
                    .iter()
                    .any(|entry| entry.option_id() == option.id())
                {
                    return Err(shape_mismatch(
                        PLACEMENT_ROUTES_FIELD,
                        OPTION_ROUTE_MISSING_REASON,
                    ));
                }
            }
            Ok(())
        }
        // Check 2: kind agreement. Parsing decides the placement kind from the authored shape (a
        // placement with `routes` is a decision, one with `next`/`terminal` is an action), so a
        // mismatch is only observable once the used definition is resolved.
        (GraphPlacementV2::Action(_), ParsedNodeDefinition::Decision(_)) => Err(shape_mismatch(
            PLACEMENT_USE_FIELD,
            ACTION_PLACEMENT_KIND_REASON,
        )),
        (GraphPlacementV2::Decision(_), ParsedNodeDefinition::Action(_)) => Err(shape_mismatch(
            PLACEMENT_USE_FIELD,
            DECISION_PLACEMENT_KIND_REASON,
        )),
    }
}

/// Checks 5 and 6 for one placement's `evidence_from` list, in authored entry order.
fn validate_evidence(
    placement_id: &GraphNodeId,
    evidence_from: Option<&EvidenceFromListV2>,
    definitions: &DefinitionIndex<'_>,
    placements: &PlacementIndex<'_>,
) -> Result<(), ConfigError> {
    let Some(entries) = evidence_from else {
        return Ok(());
    };
    for reference in entries.entries() {
        let source_id = reference.source_node();
        // Check 5a: the named source placement exists (EVIDENCE_SOURCE_UNKNOWN).
        let Some(source) = placements.get(source_id.as_str()) else {
            return Err(unknown_reference(EVIDENCE_SOURCE_FIELD, source_id.as_str()));
        };
        // Check 5b: an entry never names its own consuming placement
        // (EVIDENCE_SOURCE_SELF_REFERENCE, section 8.1). Reported as a shape mismatch rather than
        // an unknown reference because the named placement does exist.
        if source_id == placement_id {
            return Err(shape_mismatch(
                EVIDENCE_SOURCE_FIELD,
                EVIDENCE_SELF_REFERENCE_REASON,
            ));
        }
        let Some(selected) = reference.selected_items() else {
            continue;
        };
        // Check 6: every selected item is declared by the source placement's definition
        // (EVIDENCE_SELECTOR_UNKNOWN_ITEM, section 8.1). An entry without `items` selects every
        // item of the source definition and has nothing to resolve.
        //
        // Resolving the source's own definition can fail when that placement's `use` is itself
        // dangling. That is the source placement's check-1 failure, reported here with the same
        // `ConfigError` the source placement would produce, so the diagnostic does not depend on
        // which placement is visited first.
        let source_definition = lookup_definition(placement_definition(source), definitions)?;
        for item in selected {
            if !definition_items(source_definition)
                .iter()
                .any(|specification| specification.id() == item)
            {
                return Err(unknown_reference(EVIDENCE_SELECTOR_FIELD, item.as_str()));
            }
        }
    }
    Ok(())
}

/// Check 1: a placement's `use` names an existing node definition (GRAPH_DEFINITION_UNKNOWN).
fn lookup_definition<'a>(
    definition_id: &NodeDefinitionId,
    definitions: &DefinitionIndex<'a>,
) -> Result<&'a ParsedNodeDefinition, ConfigError> {
    definitions
        .get(definition_id.as_str())
        .copied()
        .ok_or_else(|| unknown_reference(PLACEMENT_USE_FIELD, definition_id.as_str()))
}

fn placement_definition(placement: &GraphPlacementV2) -> &NodeDefinitionId {
    match placement {
        GraphPlacementV2::Action(action) => action.definition(),
        GraphPlacementV2::Decision(decision) => decision.definition(),
    }
}

fn definition_items(definition: &ParsedNodeDefinition) -> &[ItemSpecV2] {
    match definition {
        ParsedNodeDefinition::Action(action) => action.items(),
        ParsedNodeDefinition::Decision(decision) => decision.items(),
    }
}
