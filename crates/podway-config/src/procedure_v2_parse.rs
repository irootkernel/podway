//! Procedure v2 YAML parsing: bounded-decode reuse and exact syntactic mapping of the closed
//! wire DTOs into the core v2 authoring values. No semantic, cross-reference, or
//! canonicalization work is performed here.

use std::str::FromStr;

use podway_core::{
    ActionDefinitionV2, ActionOutcomeV2, ActionPlacementV2, AssessmentContractV2,
    AssessmentOutcomeMappingV2, AssessmentTargetV2, DecisionDefinitionInputV2,
    DecisionDefinitionV2, DecisionOptionV2, DecisionPlacementV2, DecisionRouteEntryV2,
    DecisionRouteMapV2, DecisionRouteV2, DomainError, EvidenceFromListV2, EvidenceReferenceV2,
    GoalOutcome, GoalTrackingOptIn, GraphNodeId, GraphPlacementV2, ItemCommonV2, ItemId,
    ItemSpecV2, ManualReworkTargetListV2, NodeDefinitionId, OptionId, ProcedureGraphV2,
    ReasonPolicyV2, SkipPolicyV2, TransitionEffectV2,
};

use crate::procedure_v2_wire::*;
use crate::{ConfigError, validate_count, validate_identifier, validate_text};

const MAX_PROCEDURE_VERSION_CHARS: usize = 64;
const MAX_PROCEDURE_NAME_CHARS: usize = 120;
const MAX_PROCEDURE_PURPOSE_CHARS: usize = 500;
const MAX_PROCEDURE_DESCRIPTION_CHARS: usize = 1_000;
const MIN_NODE_DEFINITIONS: usize = 1;
const MAX_NODE_DEFINITIONS: usize = 64;

/// The `field` [`map_domain_error`] reports when a core constructor rejects a value without naming
/// an authored path.
///
/// It is a sentinel, not a path: the rejection's only distinguishing content is then its static
/// reason. `procedure_v2_diagnostics` reads this constant to recognize that case and to switch on
/// the reason instead.
pub(crate) const DOMAIN_SENTINEL_FIELD: &str = "procedure v2";

/// The authored shape a placement-level mapping failure reports.
///
/// The three constants below are read both here, where the rejection is raised, and by
/// `procedure_v2_diagnostics`, which switches on them to select the catalog code. Declaring them
/// once is what makes the raise site and its classification unable to drift.
pub(crate) const PLACEMENT_FIELD: &str = "graph.nodes";
/// `ACTION_DISPOSITION_INVALID`: an action placement declared both dispositions.
pub(crate) const ACTION_OUTCOME_BOTH_REASON: &str =
    "an action placement declares either next or terminal, not both";
/// `ACTION_DISPOSITION_INVALID`: an action placement declared neither disposition.
pub(crate) const ACTION_OUTCOME_ABSENT_REASON: &str =
    "an action placement must declare next or terminal";
/// `DECISION_SKIP_NOT_ALLOWED`: a decision placement declared a skip policy (dossier section 6.2 —
/// skip belongs to action placements only, and the schema rejects it here too).
pub(crate) const DECISION_SKIP_REASON: &str = "a decision placement does not declare skip";

/// One mapped reusable node definition, preserving the discriminated action/decision kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedNodeDefinition {
    Action(ActionDefinitionV2),
    Decision(DecisionDefinitionV2),
}

impl ParsedNodeDefinition {
    pub fn id(&self) -> &NodeDefinitionId {
        match self {
            Self::Action(definition) => definition.id(),
            Self::Decision(definition) => definition.id(),
        }
    }
}

/// A bounded Procedure v2 document mapped into the core v2 authoring model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedProcedureV2 {
    id: String,
    version: String,
    name: String,
    purpose: String,
    description: Option<String>,
    goal_tracking: Option<GoalTrackingOptIn>,
    node_definitions: Vec<ParsedNodeDefinition>,
    graph: ProcedureGraphV2,
}

impl ParsedProcedureV2 {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    pub const fn goal_tracking(&self) -> Option<GoalTrackingOptIn> {
        self.goal_tracking
    }
    pub fn node_definitions(&self) -> &[ParsedNodeDefinition] {
        &self.node_definitions
    }
    pub fn graph(&self) -> &ProcedureGraphV2 {
        &self.graph
    }
}

/// Maps an already-admitted Procedure v2 YAML `text` into the core v2 authoring model. The
/// caller (`parse_procedure_document`) is responsible for the single bounded admission; this
/// function performs only order-preserving DTO deserialization and constructor mapping.
pub(crate) fn parse_procedure_v2_yaml(text: &str) -> Result<ParsedProcedureV2, ConfigError> {
    let document: ProcedureV2DocumentWire = serde_yaml::from_str(text).map_err(wire_error)?;
    map_document(document)
}

/// Maps an already-admitted Procedure v2 JSON `text` into the core v2 authoring model, mirroring
/// [`parse_procedure_v2_yaml`] exactly. The caller (`parse_procedure_document`) is responsible
/// for the single bounded admission: duplicate keys, non-canonical numbers, oversize input, and
/// excess depth/node count are already rejected upstream by the shared bounded decoder before
/// this runs, so — like the YAML path — this function performs only order-preserving DTO
/// deserialization (`serde_json`'s streaming deserializer still drives [`OrderedMap`]'s
/// `MapAccess` visitor, so author order survives even though the workspace's `serde_json::Value`
/// is built without `preserve_order`) and constructor mapping.
pub(crate) fn parse_procedure_v2_json(text: &str) -> Result<ParsedProcedureV2, ConfigError> {
    let document: ProcedureV2DocumentWire = serde_json::from_str(text).map_err(wire_error)?;
    map_document(document)
}

/// Wraps a wire-deserialization failure from either serde front-end into
/// `ConfigError::InvalidDocument`, normalizing away format-specific location/context annotations
/// so the same structural violation against [`ProcedureV2DocumentWire`] produces the same reason
/// string for YAML and JSON input — the equivalence the acceptance gate requires.
///
/// Two independent, mechanical trims are applied, both derived from the two crates' own error
/// formatting code (not guessed):
///
/// - A trailing `" at line <n> column <n>"` suffix, which both `serde_yaml` and `serde_json`
///   append to every `Error::custom`-style message that has position info. `serde_yaml`'s
///   libyaml layer can separately format a bare `" at position <n>"` (when a mark carries a byte
///   offset but no line/column); it is not reachable from this call site given the upstream
///   bounded decoder already rejects raw YAML syntax hazards, but is still trimmed defensively.
/// - A leading structural-path prefix that `serde_yaml` (only) prepends whenever the failure
///   happens while deserializing the value of a struct field, e.g. `"node_definitions: unknown
///   field ..."` or `"graph.nodes[0].terminal: invalid type ..."`. This is `serde_yaml::Path`'s
///   own `ident(.ident|[<digits>])*` rendering (see its `Display` impl); `serde_json` never adds
///   it. The prefix is recognized by that exact closed grammar, so stripping it is a mechanical
///   trim of format-specific context, not a rewrite of the message body — but it is a real, if
///   narrow, deviation from trimming only a trailing suffix, made because without it the two
///   formats disagree on `ConfigError` for ordinary field-level violations (an unknown field
///   inside a node definition, a wrong-typed `goal_tracking`), which the acceptance gate treats
///   as one structural violation that must diagnose identically.
fn wire_error(error: impl std::fmt::Display) -> ConfigError {
    let message = error.to_string();
    let without_path = strip_yaml_path_prefix(&message);
    let reason = strip_location_suffix(without_path).to_owned();
    ConfigError::InvalidDocument { reason }
}

/// Strips a leading `serde_yaml` structural path (`ident(.ident|[<digits>])*`) immediately
/// followed by `": "`, when present; a no-op for `serde_json` messages, which never carry one.
fn strip_yaml_path_prefix(message: &str) -> &str {
    let Some(prefix_end) = message.find(": ") else {
        return message;
    };
    let (candidate, rest) = message.split_at(prefix_end);
    if is_yaml_structural_path(candidate) {
        &rest[2..]
    } else {
        message
    }
}

/// True for a non-empty `ident(.ident|[<digits>])*` token: an ASCII letter or underscore, then
/// any run of ASCII alphanumerics, underscores, `.` separators, and `[<digits>]` index groups.
/// This is exactly `serde_yaml::path::Path`'s `Display` grammar, so it can only match a path
/// `serde_yaml` itself generated — never organic content from either serde front-end's own
/// message text (verified against every message shape reachable here: `missing field`, `unknown
/// field`, `unknown variant`, `duplicate field`, `invalid length`, `invalid type`/`invalid
/// value`; the latter two are the only ones containing `": "` natively, and both fail this
/// grammar on their embedded space before reaching a colon).
fn is_yaml_structural_path(candidate: &str) -> bool {
    let Some(&first) = candidate.as_bytes().first() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    let mut in_index = false;
    for byte in candidate.bytes() {
        match (in_index, byte) {
            (true, b']') => in_index = false,
            (true, byte) if byte.is_ascii_digit() => {}
            (true, _) => return false,
            (false, b'[') => in_index = true,
            (false, byte) if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' => {}
            (false, _) => return false,
        }
    }
    !in_index
}

/// Strips a trailing `" at line <n> column <n>"` or `" at position <n>"` location suffix, when
/// present.
fn strip_location_suffix(message: &str) -> &str {
    strip_line_column_suffix(message)
        .or_else(|| strip_position_suffix(message))
        .unwrap_or(message)
}

fn strip_line_column_suffix(message: &str) -> Option<&str> {
    let (body, tail) = message.rsplit_once(" at line ")?;
    let (line, column) = tail.split_once(" column ")?;
    let valid = !line.is_empty()
        && line.bytes().all(|byte| byte.is_ascii_digit())
        && !column.is_empty()
        && column.bytes().all(|byte| byte.is_ascii_digit());
    valid.then_some(body)
}

fn strip_position_suffix(message: &str) -> Option<&str> {
    let (body, tail) = message.rsplit_once(" at position ")?;
    let valid = !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit());
    valid.then_some(body)
}

fn map_domain_error(error: DomainError) -> ConfigError {
    match error {
        DomainError::EmptyValue { field } => ConfigError::InvalidValue {
            field,
            reason: "must not be empty",
        },
        DomainError::ValueTooLong {
            field,
            maximum,
            actual,
        } => ConfigError::OutOfBounds {
            field,
            min: 1,
            max: maximum,
            actual,
        },
        DomainError::InvalidIdentifier { field } => ConfigError::InvalidValue {
            field,
            reason: "must be lowercase kebab-case",
        },
        // v2 constructors surface every text/collection/cross-field rejection (including media
        // types) as InvalidState, whose static reason is preserved verbatim. No other variant is
        // reachable from v2 mapping; the wildcard forwards its Display rather than guessing.
        DomainError::InvalidState { reason } => ConfigError::InvalidValue {
            field: DOMAIN_SENTINEL_FIELD,
            reason,
        },
        other => ConfigError::InvalidDocument {
            reason: other.to_string(),
        },
    }
}

/// Maps one wire document into the core v2 authoring model.
///
/// Published to the crate rather than kept private because `procedure_v2_convert` builds its
/// candidate as a [`ProcedureV2DocumentWire`] and enters here: a conversion is then admitted by
/// exactly the identifier rules, text bounds, and collection bounds an authored document is
/// admitted by, and there is no second mapping to drift from this one.
pub(crate) fn map_document(
    document: ProcedureV2DocumentWire,
) -> Result<ParsedProcedureV2, ConfigError> {
    if document.schema != PROCEDURE_SCHEMA_V2 {
        return Err(ConfigError::InvalidSchema {
            expected: PROCEDURE_SCHEMA_V2,
            actual: document.schema,
        });
    }

    validate_identifier("procedure.id", &document.id)?;
    validate_text(
        "procedure.version",
        &document.version,
        1,
        MAX_PROCEDURE_VERSION_CHARS,
        false,
    )?;
    validate_text(
        "procedure.name",
        &document.name,
        1,
        MAX_PROCEDURE_NAME_CHARS,
        true,
    )?;
    validate_text(
        "procedure.purpose",
        &document.purpose,
        1,
        MAX_PROCEDURE_PURPOSE_CHARS,
        true,
    )?;
    if let Some(description) = &document.description {
        validate_text(
            "procedure.description",
            description,
            0,
            MAX_PROCEDURE_DESCRIPTION_CHARS,
            false,
        )?;
    }

    let goal_tracking = match document.goal_tracking {
        None => Ok(None),
        Some(flag) => GoalTrackingOptIn::from_bool(flag)
            .map(Some)
            .map_err(map_domain_error),
    }?;

    let node_definitions = document
        .node_definitions
        .entries()
        .into_iter()
        .map(|(id, wire)| map_node_definition(&id, wire))
        .collect::<Result<Vec<_>, _>>()?;
    validate_count(
        "node_definitions",
        node_definitions.len(),
        MIN_NODE_DEFINITIONS,
        MAX_NODE_DEFINITIONS,
    )?;

    let manual_rework = document.manual_rework.map(map_manual_rework).transpose()?;
    let graph = map_graph(document.graph, manual_rework)?;

    Ok(ParsedProcedureV2 {
        id: document.id,
        version: document.version,
        name: document.name,
        purpose: document.purpose,
        description: document.description,
        goal_tracking,
        node_definitions,
        graph,
    })
}

fn map_node_definition(
    id: &str,
    wire: NodeDefinitionWire,
) -> Result<ParsedNodeDefinition, ConfigError> {
    let id = NodeDefinitionId::new(id).map_err(map_domain_error)?;
    match wire {
        NodeDefinitionWire::Action {
            title,
            intent,
            description,
            instructions,
            items,
        } => {
            let instructions = optional_nonempty("instructions", instructions)?;
            let items = map_items(optional_nonempty("items", items)?)?;
            let definition =
                ActionDefinitionV2::new(id, title, intent, description, instructions, items)
                    .map_err(map_domain_error)?;
            Ok(ParsedNodeDefinition::Action(definition))
        }
        NodeDefinitionWire::Decision {
            title,
            description,
            objective,
            prompt,
            evidence_guidance,
            items,
            options,
            reason,
            assessment,
        } => {
            let evidence_guidance = optional_nonempty("evidence_guidance", evidence_guidance)?;
            let items = map_items(optional_nonempty("items", items)?)?;
            let options = options
                .into_iter()
                .map(map_option)
                .collect::<Result<Vec<_>, _>>()?;
            let reason =
                ReasonPolicyV2::new(reason.required, reason.prompt).map_err(map_domain_error)?;
            let assessment = assessment.map(map_assessment).transpose()?;
            let definition = DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                id,
                title,
                description,
                objective,
                prompt,
                evidence_guidance,
                items,
                options,
                reason,
                assessment,
            })
            .map_err(map_domain_error)?;
            Ok(ParsedNodeDefinition::Decision(definition))
        }
    }
}

/// Honors the schema `minItems: 1` for optional collections: omission becomes empty, an
/// explicitly empty array is rejected, and a non-empty array is passed through unchanged.
fn optional_nonempty<T>(field: &'static str, value: Option<Vec<T>>) -> Result<Vec<T>, ConfigError> {
    match value {
        None => Ok(Vec::new()),
        Some(entries) if entries.is_empty() => Err(ConfigError::InvalidValue {
            field,
            reason: "must be omitted or contain at least one entry",
        }),
        Some(entries) => Ok(entries),
    }
}

fn map_items(items: Vec<ItemWire>) -> Result<Vec<ItemSpecV2>, ConfigError> {
    items.into_iter().map(map_item).collect()
}

fn map_item(wire: ItemWire) -> Result<ItemSpecV2, ConfigError> {
    match wire {
        ItemWire::Confirm {
            id,
            prompt,
            help,
            required,
        } => Ok(ItemSpecV2::confirm(item_common(
            id, prompt, help, required,
        )?)),
        ItemWire::Text {
            id,
            prompt,
            help,
            required,
            min_length,
            max_length,
            multiline,
        } => ItemSpecV2::text(
            item_common(id, prompt, help, required)?,
            min_length,
            max_length,
            multiline,
        )
        .map_err(map_domain_error),
        ItemWire::Choice {
            id,
            prompt,
            help,
            required,
            choices,
        } => ItemSpecV2::choice(item_common(id, prompt, help, required)?, choices)
            .map_err(map_domain_error),
        ItemWire::Integer {
            id,
            prompt,
            help,
            required,
            minimum,
            maximum,
        } => ItemSpecV2::integer(item_common(id, prompt, help, required)?, minimum, maximum)
            .map_err(map_domain_error),
        ItemWire::List {
            id,
            prompt,
            help,
            required,
            min_items,
            max_items,
            max_item_length,
            unique,
        } => ItemSpecV2::list(
            item_common(id, prompt, help, required)?,
            min_items,
            max_items,
            max_item_length,
            unique,
        )
        .map_err(map_domain_error),
        ItemWire::Artifact {
            id,
            prompt,
            help,
            required,
            allowed_media_types,
        } => {
            let allowed_media_types =
                optional_nonempty("allowed_media_types", allowed_media_types)?;
            ItemSpecV2::artifact(
                item_common(id, prompt, help, required)?,
                allowed_media_types,
            )
            .map_err(map_domain_error)
        }
    }
}

fn item_common(
    id: String,
    prompt: String,
    help: Option<String>,
    required: bool,
) -> Result<ItemCommonV2, ConfigError> {
    let id = ItemId::new(id).map_err(map_domain_error)?;
    ItemCommonV2::new(id, prompt, help, required).map_err(map_domain_error)
}

fn map_option(wire: DecisionOptionWire) -> Result<DecisionOptionV2, ConfigError> {
    let id = OptionId::new(wire.id).map_err(map_domain_error)?;
    DecisionOptionV2::new(id, wire.label, wire.criteria).map_err(map_domain_error)
}

fn map_assessment(wire: AssessmentWire) -> Result<AssessmentContractV2, ConfigError> {
    let target = AssessmentTargetV2::from_str(&wire.target).map_err(map_domain_error)?;
    let outcomes = wire
        .outcomes
        .entries()
        .into_iter()
        .map(|(option_id, outcome)| {
            let option_id = OptionId::new(option_id).map_err(map_domain_error)?;
            let outcome = GoalOutcome::from_str(&outcome).map_err(map_domain_error)?;
            Ok::<_, ConfigError>(AssessmentOutcomeMappingV2::new(option_id, outcome))
        })
        .collect::<Result<Vec<_>, _>>()?;
    AssessmentContractV2::with_target(target, outcomes).map_err(map_domain_error)
}

fn map_graph(
    wire: GraphWire,
    manual_rework: Option<ManualReworkTargetListV2>,
) -> Result<ProcedureGraphV2, ConfigError> {
    let entry = GraphNodeId::new(wire.entry).map_err(map_domain_error)?;
    let placements = wire
        .nodes
        .into_iter()
        .map(map_placement)
        .collect::<Result<Vec<_>, _>>()?;
    ProcedureGraphV2::new(entry, placements, manual_rework).map_err(map_domain_error)
}

fn map_placement(wire: GraphPlacementWire) -> Result<GraphPlacementV2, ConfigError> {
    let id = GraphNodeId::new(wire.id).map_err(map_domain_error)?;
    let definition = NodeDefinitionId::new(wire.use_).map_err(map_domain_error)?;
    let evidence_from = map_evidence_from(wire.evidence_from)?;

    if let Some(routes) = wire.routes {
        // Skip has its own catalogued rejection; a stray `next`/`terminal` stays the generic
        // only-routes shape violation because no catalog code claims those.
        if wire.skip.is_some() {
            return Err(ConfigError::InvalidValue {
                field: PLACEMENT_FIELD,
                reason: DECISION_SKIP_REASON,
            });
        }
        if wire.next.is_some() || wire.terminal.is_some() {
            return Err(ConfigError::InvalidValue {
                field: PLACEMENT_FIELD,
                reason: "a decision placement declares only routes",
            });
        }
        let entries = routes
            .entries()
            .into_iter()
            .map(|(option_id, route)| {
                let option_id = OptionId::new(option_id).map_err(map_domain_error)?;
                let to = GraphNodeId::new(route.to).map_err(map_domain_error)?;
                let effect =
                    TransitionEffectV2::from_str(&route.effect).map_err(map_domain_error)?;
                Ok::<_, ConfigError>(DecisionRouteEntryV2::new(
                    option_id,
                    DecisionRouteV2::new(to, effect),
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let route_map = DecisionRouteMapV2::new(entries).map_err(map_domain_error)?;
        return Ok(GraphPlacementV2::Decision(DecisionPlacementV2::new(
            id,
            definition,
            evidence_from,
            route_map,
        )));
    }

    let outcome = map_action_outcome(wire.next, wire.terminal)?;
    let skip = wire.skip.map(map_skip).transpose()?;
    Ok(GraphPlacementV2::Action(ActionPlacementV2::new(
        id,
        definition,
        evidence_from,
        skip,
        outcome,
    )))
}

fn map_action_outcome(
    next: Option<String>,
    terminal: Option<bool>,
) -> Result<ActionOutcomeV2, ConfigError> {
    match (next, terminal) {
        (Some(target), None) => {
            let to = GraphNodeId::new(target).map_err(map_domain_error)?;
            Ok(ActionOutcomeV2::next(to))
        }
        (None, Some(true)) => Ok(ActionOutcomeV2::terminal()),
        (None, Some(false)) => Err(ConfigError::InvalidValue {
            field: "graph.nodes.terminal",
            reason: "terminal must be true",
        }),
        (Some(_), Some(_)) => Err(ConfigError::InvalidValue {
            field: PLACEMENT_FIELD,
            reason: ACTION_OUTCOME_BOTH_REASON,
        }),
        (None, None) => Err(ConfigError::InvalidValue {
            field: PLACEMENT_FIELD,
            reason: ACTION_OUTCOME_ABSENT_REASON,
        }),
    }
}

fn map_skip(wire: SkipPolicyWire) -> Result<SkipPolicyV2, ConfigError> {
    SkipPolicyV2::new(wire.allowed, wire.reason_required).map_err(map_domain_error)
}

fn map_evidence_from(
    wire: Option<Vec<EvidenceReferenceWire>>,
) -> Result<Option<EvidenceFromListV2>, ConfigError> {
    let Some(entries) = wire else {
        return Ok(None);
    };
    let references = entries
        .into_iter()
        .map(map_evidence_reference)
        .collect::<Result<Vec<_>, _>>()?;
    EvidenceFromListV2::new(references)
        .map(Some)
        .map_err(map_domain_error)
}

fn map_evidence_reference(wire: EvidenceReferenceWire) -> Result<EvidenceReferenceV2, ConfigError> {
    let source_node = GraphNodeId::new(wire.node).map_err(map_domain_error)?;
    let selected_items = wire
        .items
        .map(|items| {
            items
                .into_iter()
                .map(|item| ItemId::new(item).map_err(map_domain_error))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    EvidenceReferenceV2::new(source_node, wire.required, selected_items).map_err(map_domain_error)
}

fn map_manual_rework(wire: ManualReworkWire) -> Result<ManualReworkTargetListV2, ConfigError> {
    let targets = wire
        .allowed_targets
        .into_iter()
        .map(|target| GraphNodeId::new(target).map_err(map_domain_error))
        .collect::<Result<Vec<_>, _>>()?;
    ManualReworkTargetListV2::new(targets).map_err(map_domain_error)
}

#[cfg(test)]
mod wire_error_tests {
    use super::*;

    // Exact `Display` strings captured from `serde_yaml` 0.9.34 / `serde_json` 1.0.150 for a
    // handful of structural violations against `ProcedureV2DocumentWire`. These pin the two
    // trims in `wire_error` against the real quirks they exist to normalize: `serde_yaml`
    // sometimes omits the trailing location (a root-level `missing field`), and prepends a
    // structural path for anything nested one struct-field deeper (`node_definitions: `,
    // `graph.nodes[0].terminal: `) that `serde_json` never adds.

    #[test]
    fn strips_line_column_suffix_shared_by_both_formats() {
        assert_eq!(
            strip_location_suffix(
                "unknown field `unknown`, expected one of `schema`, `id` at line 3 column 1"
            ),
            "unknown field `unknown`, expected one of `schema`, `id`"
        );
        assert_eq!(
            strip_location_suffix(
                "unknown field `unknown`, expected one of `schema`, `id` at line 1 column 50"
            ),
            "unknown field `unknown`, expected one of `schema`, `id`"
        );
    }

    #[test]
    fn leaves_root_level_missing_field_message_that_yaml_never_suffixes() {
        // serde_yaml: no Mark is available for a structural "field never appeared" check, so
        // there is no " at line/column" to trim in the first place.
        assert_eq!(
            strip_location_suffix("missing field `name`"),
            "missing field `name`"
        );
        // serde_json always attaches one; wire_error must still land on the same text.
        assert_eq!(
            strip_location_suffix("missing field `name` at line 1 column 207"),
            "missing field `name`"
        );
    }

    #[test]
    fn strips_position_suffix() {
        assert_eq!(
            strip_location_suffix("unexpected end at position 4"),
            "unexpected end"
        );
        assert_eq!(strip_location_suffix("no suffix here"), "no suffix here");
    }

    #[test]
    fn strips_single_segment_yaml_structural_path() {
        assert_eq!(
            strip_yaml_path_prefix(
                "node_definitions: unknown variant `bogus`, expected one of `confirm`, `text`"
            ),
            "unknown variant `bogus`, expected one of `confirm`, `text`"
        );
        assert_eq!(
            strip_yaml_path_prefix(
                "goal_tracking: invalid type: string \"true\", expected a boolean"
            ),
            "invalid type: string \"true\", expected a boolean"
        );
    }

    #[test]
    fn strips_dotted_and_indexed_yaml_structural_path() {
        assert_eq!(
            strip_yaml_path_prefix("graph.nodes[0]: missing field `id`"),
            "missing field `id`"
        );
        assert_eq!(
            strip_yaml_path_prefix(
                "graph.nodes[0].terminal: invalid type: string \"true\", expected a boolean"
            ),
            "invalid type: string \"true\", expected a boolean"
        );
    }

    #[test]
    fn never_strips_message_bodies_serde_json_actually_produces() {
        // serde_json's own "invalid type"/"invalid value" messages are the only ones containing
        // ": " natively; the embedded space in the phrase before the colon must fail the path
        // grammar so they pass through untouched.
        let json_like = "invalid type: string \"true\", expected a boolean";
        assert_eq!(strip_yaml_path_prefix(json_like), json_like);
        let no_colon = "unknown field `unknown`, expected one of `schema`, `id`";
        assert_eq!(strip_yaml_path_prefix(no_colon), no_colon);
    }

    #[test]
    fn yaml_and_json_wire_errors_agree_after_full_normalization() {
        let cases: &[(&str, &str)] = &[
            (
                "node_definitions: unknown variant `bogus`, expected one of `confirm`, `text` at line 7 column 3",
                "unknown variant `bogus`, expected one of `confirm`, `text` at line 1 column 161",
            ),
            (
                "goal_tracking: invalid type: string \"true\", expected a boolean at line 3 column 16",
                "invalid type: string \"true\", expected a boolean at line 1 column 63",
            ),
            (
                "graph.nodes[0].terminal: invalid type: string \"true\", expected a boolean at line 16 column 17",
                "invalid type: string \"true\", expected a boolean at line 1 column 200",
            ),
            (
                "missing field `name`",
                "missing field `name` at line 1 column 207",
            ),
        ];
        for (yaml_like, json_like) in cases {
            let yaml_reason = strip_location_suffix(strip_yaml_path_prefix(yaml_like));
            let json_reason = strip_location_suffix(strip_yaml_path_prefix(json_like));
            assert_eq!(
                yaml_reason, json_reason,
                "yaml: {yaml_like:?} json: {json_like:?}"
            );
        }
    }
}
