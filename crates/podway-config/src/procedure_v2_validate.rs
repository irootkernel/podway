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
//! Future stable diagnostic codes are named in comments only; binding `ConfigError` values to the
//! catalog in `assets/specifications/authoring-diagnostics.json` is V2AUT-008's task.

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
    /// authority of section 12.1. Mirrors `ValidatedProcedureV1::canonical_json`.
    pub fn canonical_json(&self) -> &CanonicalJsonV1 {
        &self.canonical_json
    }

    /// The SHA-256 digest over the canonical bytes. Mirrors `ValidatedProcedureV1::digest`.
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
                    "manual_rework.allowed_targets",
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
                "node_definitions.assessment.outcomes",
                "an assessment outcome names an option the decision definition does not declare",
            ));
        }
    }

    // Check 7b: a session-goal assessment requires the procedure-level opt-in
    // (GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING; sections 7.1 and 11.3). `goal_tracking` is a scalar
    // opt-in whose only accepted value is `true`, so its presence is its enablement.
    if !goal_tracking {
        return Err(shape_mismatch(
            "node_definitions.assessment",
            "a session-goal assessment requires the procedure to declare goal_tracking: true",
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
                return Err(unknown_reference("graph.nodes.next", target.as_str()));
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
                        "graph.nodes.routes",
                        "a route names an option the decision definition does not declare",
                    ));
                }
                // Check 3 (decision half): every route target names an existing placement
                // (ROUTE_TARGET_NOT_FOUND).
                let target = entry.route().to();
                if !placements.contains_key(target.as_str()) {
                    return Err(unknown_reference("graph.nodes.routes.to", target.as_str()));
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
                        "graph.nodes.routes",
                        "every declared decision option must have exactly one route",
                    ));
                }
            }
            Ok(())
        }
        // Check 2: kind agreement. Parsing decides the placement kind from the authored shape (a
        // placement with `routes` is a decision, one with `next`/`terminal` is an action), so a
        // mismatch is only observable once the used definition is resolved.
        (GraphPlacementV2::Action(_), ParsedNodeDefinition::Decision(_)) => Err(shape_mismatch(
            "graph.nodes.use",
            "an action placement must use an action definition",
        )),
        (GraphPlacementV2::Decision(_), ParsedNodeDefinition::Action(_)) => Err(shape_mismatch(
            "graph.nodes.use",
            "a decision placement must use a decision definition",
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
            return Err(unknown_reference(
                "graph.nodes.evidence_from.node",
                source_id.as_str(),
            ));
        };
        // Check 5b: an entry never names its own consuming placement
        // (EVIDENCE_SOURCE_SELF_REFERENCE, section 8.1). Reported as a shape mismatch rather than
        // an unknown reference because the named placement does exist.
        if source_id == placement_id {
            return Err(shape_mismatch(
                "graph.nodes.evidence_from.node",
                "an evidence reference must not name its consuming placement",
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
                return Err(unknown_reference(
                    "graph.nodes.evidence_from.items",
                    item.as_str(),
                ));
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
        .ok_or_else(|| unknown_reference("graph.nodes.use", definition_id.as_str()))
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
