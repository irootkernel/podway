//! Pure Procedure v2 authoring domain values.
//!
//! This module owns the additive, individually-bounded value types that compose a Procedure v2
//! model: node definitions, graph placements, routes, transition effects, evidence references,
//! manual rework targets, and session-goal definition contracts. Each constructor enforces only
//! the identifier, scalar, collection, uniqueness, and cross-field bounds owned by that single
//! value. Graph topology, cursor transitions, immutable workflow records, parsing,
//! cross-reference validation, and canonicalization are owned by later tasks and are deliberately
//! not enforced here.

use std::collections::BTreeSet;
use std::fmt;

use crate::procedure::validate_media_type;
use crate::{
    CriterionId, DomainError, GraphNodeId, ItemId, ItemTypeV1, NodeDefinitionId, OptionId,
    validate_text,
};

/// The exact Procedure v2 schema identifier.
pub const PROCEDURE_SCHEMA_V2: &str = "podway.procedure/v2";

// Scalar character bounds fixed by dossier section 5.1.
const MAX_DEFINITION_TITLE_CHARS: usize = 120;
const MAX_ACTION_INTENT_CHARS: usize = 300;
const MAX_DEFINITION_DESCRIPTION_CHARS: usize = 1000;
const MAX_DECISION_OBJECTIVE_CHARS: usize = 300;
const MAX_DECISION_PROMPT_CHARS: usize = 500;
const MAX_REASON_POLICY_PROMPT_CHARS: usize = 300;
const MAX_INSTRUCTION_CHARS: usize = 1000;
const MAX_ITEM_PROMPT_CHARS: usize = 300;
const MAX_ITEM_HELP_CHARS: usize = 1000;
const MAX_OPTION_LABEL_CHARS: usize = 120;
const MAX_OPTION_CRITERIA_CHARS: usize = 500;
const MAX_EVIDENCE_GUIDANCE_ENTRY_CHARS: usize = 200;

// Collection bounds fixed by dossier section 5.1.
const MAX_INSTRUCTIONS_PER_DEFINITION: usize = 16;
const MAX_ITEMS_PER_DEFINITION: usize = 64;
const MIN_OPTIONS_PER_DECISION: usize = 1;
const MAX_OPTIONS_PER_DECISION: usize = 8;
const MAX_EVIDENCE_GUIDANCE_ENTRIES: usize = 8;
const MIN_EVIDENCE_REFERENCES_PER_PLACEMENT: usize = 1;
const MAX_EVIDENCE_REFERENCES_PER_PLACEMENT: usize = 8;
const MIN_SELECTED_ITEMS_PER_REFERENCE: usize = 1;
const MAX_SELECTED_ITEMS_PER_REFERENCE: usize = 16;
const MIN_ROUTES_PER_DECISION: usize = 1;
const MAX_ROUTES_PER_DECISION: usize = 8;
const MIN_MANUAL_REWORK_TARGETS: usize = 1;
const MAX_MANUAL_REWORK_TARGETS: usize = 64;
const MIN_ASSESSMENT_OUTCOME_MAPPINGS: usize = 3;
const MAX_ASSESSMENT_OUTCOME_MAPPINGS: usize = 8;

// Item type bounds fixed by dossier section 5.1.
const MAX_V2_TEXT_LENGTH: u32 = 16_384;
const MIN_V2_CHOICE_COUNT: usize = 1;
const MAX_V2_CHOICE_COUNT: usize = 32;
const MAX_V2_CHOICE_VALUE_CHARS: usize = 120;
const MAX_V2_LIST_ENTRIES: u16 = 100;
const MAX_V2_LIST_ENTRY_CHARS: u16 = 1_000;
const MAX_V2_ARTIFACT_MEDIA_TYPES: usize = 64;

// Session-goal field bounds fixed by dossier section 4.5.
const MAX_GOAL_STATEMENT_CHARS: usize = 1_000;
const MAX_CRITERION_STATEMENT_CHARS: usize = 300;
const MAX_GOAL_REVISION_REASON_CHARS: usize = 1_000;
const MAX_CRITERION_ASSESSMENT_REASON_CHARS: usize = 2_000;
const MIN_GOAL_CRITERIA: usize = 1;
const MAX_GOAL_CRITERIA: usize = 16;

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}

/// The reusable node contract kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKindV2 {
    Action,
    Decision,
}

impl NodeKindV2 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Decision => "decision",
        }
    }
}

/// Common immutable metadata shared by every Procedure v2 item type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemCommonV2 {
    id: ItemId,
    prompt: String,
    help: Option<String>,
    required: bool,
}

impl ItemCommonV2 {
    pub fn new(
        id: ItemId,
        prompt: impl Into<String>,
        help: Option<String>,
        required: bool,
    ) -> Result<Self, DomainError> {
        let prompt = prompt.into();
        validate_text("item prompt", &prompt, 1, MAX_ITEM_PROMPT_CHARS, true)?;
        if let Some(help) = &help {
            validate_text("item help", help, 0, MAX_ITEM_HELP_CHARS, false)?;
        }
        Ok(Self {
            id,
            prompt,
            help,
            required,
        })
    }

    pub fn id(&self) -> &ItemId {
        &self.id
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }

    pub const fn required(&self) -> bool {
        self.required
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmItemSpecV2 {
    common: ItemCommonV2,
}

impl ConfirmItemSpecV2 {
    pub fn new(common: ItemCommonV2) -> Self {
        Self { common }
    }

    pub fn common(&self) -> &ItemCommonV2 {
        &self.common
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextItemSpecV2 {
    common: ItemCommonV2,
    min_length: u32,
    max_length: u32,
    multiline: bool,
}

impl TextItemSpecV2 {
    pub fn new(
        common: ItemCommonV2,
        min_length: u32,
        max_length: u32,
        multiline: bool,
    ) -> Result<Self, DomainError> {
        if min_length > max_length || max_length > MAX_V2_TEXT_LENGTH {
            return Err(invalid("invalid text length constraints"));
        }
        Ok(Self {
            common,
            min_length,
            max_length,
            multiline,
        })
    }

    pub fn common(&self) -> &ItemCommonV2 {
        &self.common
    }

    pub const fn min_length(&self) -> u32 {
        self.min_length
    }

    pub const fn max_length(&self) -> u32 {
        self.max_length
    }

    pub const fn multiline(&self) -> bool {
        self.multiline
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChoiceItemSpecV2 {
    common: ItemCommonV2,
    choices: Vec<String>,
}

impl ChoiceItemSpecV2 {
    pub fn new(common: ItemCommonV2, choices: Vec<String>) -> Result<Self, DomainError> {
        if choices.len() < MIN_V2_CHOICE_COUNT || choices.len() > MAX_V2_CHOICE_COUNT {
            return Err(invalid("choice count must be between one and 32"));
        }
        let mut seen = BTreeSet::new();
        for choice in &choices {
            validate_text("choice", choice, 1, MAX_V2_CHOICE_VALUE_CHARS, true)?;
            if !seen.insert(choice.as_str()) {
                return Err(invalid("choice values must be unique"));
            }
        }
        Ok(Self { common, choices })
    }

    pub fn common(&self) -> &ItemCommonV2 {
        &self.common
    }

    pub fn choices(&self) -> &[String] {
        &self.choices
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegerItemSpecV2 {
    common: ItemCommonV2,
    minimum: Option<i64>,
    maximum: Option<i64>,
}

impl IntegerItemSpecV2 {
    pub fn new(
        common: ItemCommonV2,
        minimum: Option<i64>,
        maximum: Option<i64>,
    ) -> Result<Self, DomainError> {
        if matches!((minimum, maximum), (Some(minimum), Some(maximum)) if minimum > maximum) {
            return Err(invalid("integer minimum must not exceed maximum"));
        }
        Ok(Self {
            common,
            minimum,
            maximum,
        })
    }

    pub fn common(&self) -> &ItemCommonV2 {
        &self.common
    }

    pub const fn minimum(&self) -> Option<i64> {
        self.minimum
    }

    pub const fn maximum(&self) -> Option<i64> {
        self.maximum
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListItemSpecV2 {
    common: ItemCommonV2,
    min_items: u16,
    max_items: u16,
    max_item_length: u16,
    unique: bool,
}

impl ListItemSpecV2 {
    pub fn new(
        common: ItemCommonV2,
        min_items: u16,
        max_items: u16,
        max_item_length: u16,
        unique: bool,
    ) -> Result<Self, DomainError> {
        if max_items == 0 || max_items > MAX_V2_LIST_ENTRIES {
            return Err(invalid("invalid list item count constraints"));
        }
        if min_items > max_items {
            return Err(invalid("invalid list item count constraints"));
        }
        if max_item_length == 0 || max_item_length > MAX_V2_LIST_ENTRY_CHARS {
            return Err(invalid("invalid list entry length constraint"));
        }
        Ok(Self {
            common,
            min_items,
            max_items,
            max_item_length,
            unique,
        })
    }

    pub fn common(&self) -> &ItemCommonV2 {
        &self.common
    }

    pub const fn min_items(&self) -> u16 {
        self.min_items
    }

    pub const fn max_items(&self) -> u16 {
        self.max_items
    }

    pub const fn max_item_length(&self) -> u16 {
        self.max_item_length
    }

    pub const fn unique(&self) -> bool {
        self.unique
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactItemSpecV2 {
    common: ItemCommonV2,
    allowed_media_types: Vec<String>,
}

impl ArtifactItemSpecV2 {
    pub fn new(
        common: ItemCommonV2,
        allowed_media_types: Vec<String>,
    ) -> Result<Self, DomainError> {
        if allowed_media_types.len() > MAX_V2_ARTIFACT_MEDIA_TYPES {
            return Err(invalid("too many allowed media types"));
        }
        let mut seen = BTreeSet::new();
        for media_type in &allowed_media_types {
            validate_media_type(media_type)?;
            if !seen.insert(media_type.as_str()) {
                return Err(invalid("allowed media types must be unique"));
            }
        }
        Ok(Self {
            common,
            allowed_media_types,
        })
    }

    pub fn common(&self) -> &ItemCommonV2 {
        &self.common
    }

    pub fn allowed_media_types(&self) -> &[String] {
        &self.allowed_media_types
    }
}

/// An immutable specification for one of the six supported Procedure v2 item types, reusing the v1
/// item-type taxonomy under the tightened v2 bounds of section 5.1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemSpecV2 {
    Confirm(ConfirmItemSpecV2),
    Text(TextItemSpecV2),
    Choice(ChoiceItemSpecV2),
    Integer(IntegerItemSpecV2),
    List(ListItemSpecV2),
    Artifact(ArtifactItemSpecV2),
}

impl ItemSpecV2 {
    pub fn confirm(common: ItemCommonV2) -> Self {
        Self::Confirm(ConfirmItemSpecV2::new(common))
    }

    pub fn text(
        common: ItemCommonV2,
        min_length: u32,
        max_length: u32,
        multiline: bool,
    ) -> Result<Self, DomainError> {
        Ok(Self::Text(TextItemSpecV2::new(
            common, min_length, max_length, multiline,
        )?))
    }

    pub fn choice(common: ItemCommonV2, choices: Vec<String>) -> Result<Self, DomainError> {
        Ok(Self::Choice(ChoiceItemSpecV2::new(common, choices)?))
    }

    pub fn integer(
        common: ItemCommonV2,
        minimum: Option<i64>,
        maximum: Option<i64>,
    ) -> Result<Self, DomainError> {
        Ok(Self::Integer(IntegerItemSpecV2::new(
            common, minimum, maximum,
        )?))
    }

    pub fn list(
        common: ItemCommonV2,
        min_items: u16,
        max_items: u16,
        max_item_length: u16,
        unique: bool,
    ) -> Result<Self, DomainError> {
        Ok(Self::List(ListItemSpecV2::new(
            common,
            min_items,
            max_items,
            max_item_length,
            unique,
        )?))
    }

    pub fn artifact(
        common: ItemCommonV2,
        allowed_media_types: Vec<String>,
    ) -> Result<Self, DomainError> {
        Ok(Self::Artifact(ArtifactItemSpecV2::new(
            common,
            allowed_media_types,
        )?))
    }

    pub fn common(&self) -> &ItemCommonV2 {
        match self {
            Self::Confirm(specification) => specification.common(),
            Self::Text(specification) => specification.common(),
            Self::Choice(specification) => specification.common(),
            Self::Integer(specification) => specification.common(),
            Self::List(specification) => specification.common(),
            Self::Artifact(specification) => specification.common(),
        }
    }

    pub fn id(&self) -> &ItemId {
        self.common().id()
    }

    pub const fn item_type(&self) -> ItemTypeV1 {
        match self {
            Self::Confirm(_) => ItemTypeV1::Confirm,
            Self::Text(_) => ItemTypeV1::Text,
            Self::Choice(_) => ItemTypeV1::Choice,
            Self::Integer(_) => ItemTypeV1::Integer,
            Self::List(_) => ItemTypeV1::List,
            Self::Artifact(_) => ItemTypeV1::Artifact,
        }
    }
}

fn validate_instructions(instructions: &[String]) -> Result<(), DomainError> {
    if instructions.len() > MAX_INSTRUCTIONS_PER_DEFINITION {
        return Err(invalid("too many definition instructions"));
    }
    for instruction in instructions {
        validate_text(
            "definition instruction",
            instruction,
            1,
            MAX_INSTRUCTION_CHARS,
            true,
        )?;
    }
    Ok(())
}

fn validate_item_specs(items: &[ItemSpecV2]) -> Result<(), DomainError> {
    if items.len() > MAX_ITEMS_PER_DEFINITION {
        return Err(invalid("too many definition items"));
    }
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(item.id()) {
            return Err(invalid("definition item identifiers must be unique"));
        }
    }
    Ok(())
}

/// A reusable action node definition. Work and its recorded items belong to the definition;
/// evidence wiring belongs to placements and is not represented here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionDefinitionV2 {
    id: NodeDefinitionId,
    title: String,
    intent: String,
    description: Option<String>,
    instructions: Vec<String>,
    items: Vec<ItemSpecV2>,
}

impl ActionDefinitionV2 {
    pub fn new(
        id: NodeDefinitionId,
        title: impl Into<String>,
        intent: impl Into<String>,
        description: Option<String>,
        instructions: Vec<String>,
        items: Vec<ItemSpecV2>,
    ) -> Result<Self, DomainError> {
        let title = title.into();
        let intent = intent.into();
        validate_text(
            "definition title",
            &title,
            1,
            MAX_DEFINITION_TITLE_CHARS,
            true,
        )?;
        validate_text("action intent", &intent, 1, MAX_ACTION_INTENT_CHARS, true)?;
        if let Some(description) = &description {
            validate_text(
                "definition description",
                description,
                0,
                MAX_DEFINITION_DESCRIPTION_CHARS,
                false,
            )?;
        }
        validate_instructions(&instructions)?;
        validate_item_specs(&items)?;
        Ok(Self {
            id,
            title,
            intent,
            description,
            instructions,
            items,
        })
    }

    pub fn id(&self) -> &NodeDefinitionId {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn intent(&self) -> &str {
        &self.intent
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn instructions(&self) -> &[String] {
        &self.instructions
    }

    pub fn items(&self) -> &[ItemSpecV2] {
        &self.items
    }

    pub const fn node_kind(&self) -> NodeKindV2 {
        NodeKindV2::Action
    }
}

/// The reason policy for a decision definition. A declared policy requires `required: true`;
/// `false` is invalid (dossier section 5.1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasonPolicyV2 {
    required: bool,
    prompt: Option<String>,
}

impl ReasonPolicyV2 {
    pub fn new(required: bool, prompt: Option<String>) -> Result<Self, DomainError> {
        if !required {
            return Err(invalid("a declared reason policy requires required: true"));
        }
        if let Some(prompt) = &prompt {
            validate_text(
                "reason policy prompt",
                prompt,
                0,
                MAX_REASON_POLICY_PROMPT_CHARS,
                false,
            )?;
        }
        Ok(Self { required, prompt })
    }

    pub const fn required(&self) -> bool {
        self.required
    }

    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }
}

/// One selectable option of a decision definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionOptionV2 {
    id: OptionId,
    label: String,
    criteria: Option<String>,
}

impl DecisionOptionV2 {
    pub fn new(
        id: OptionId,
        label: impl Into<String>,
        criteria: Option<String>,
    ) -> Result<Self, DomainError> {
        let label = label.into();
        validate_text("option label", &label, 1, MAX_OPTION_LABEL_CHARS, true)?;
        if let Some(criteria) = &criteria {
            validate_text(
                "option criteria",
                criteria,
                0,
                MAX_OPTION_CRITERIA_CHARS,
                false,
            )?;
        }
        Ok(Self {
            id,
            label,
            criteria,
        })
    }

    pub fn id(&self) -> &OptionId {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn criteria(&self) -> Option<&str> {
        self.criteria.as_deref()
    }
}

fn validate_options(options: &[DecisionOptionV2]) -> Result<(), DomainError> {
    if options.len() < MIN_OPTIONS_PER_DECISION || options.len() > MAX_OPTIONS_PER_DECISION {
        return Err(invalid(
            "decision option count must be between one and eight",
        ));
    }
    let mut seen = BTreeSet::new();
    for option in options {
        if !seen.insert(option.id()) {
            return Err(invalid("decision option identifiers must be unique"));
        }
    }
    Ok(())
}

fn validate_evidence_guidance(entries: &[String]) -> Result<(), DomainError> {
    if entries.len() > MAX_EVIDENCE_GUIDANCE_ENTRIES {
        return Err(invalid("too many evidence guidance entries"));
    }
    for entry in entries {
        validate_text(
            "evidence guidance",
            entry,
            1,
            MAX_EVIDENCE_GUIDANCE_ENTRY_CHARS,
            true,
        )?;
    }
    Ok(())
}

/// The single assessment target defined by Procedure v2. The only accepted value is
/// `session_goal`; any other authored target is invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssessmentTargetV2;

impl AssessmentTargetV2 {
    pub const SESSION_GOAL: Self = Self;

    pub const fn new() -> Self {
        Self
    }

    pub const SESSION_GOAL_STR: &'static str = "session_goal";

    pub const fn as_str(self) -> &'static str {
        Self::SESSION_GOAL_STR
    }
}

impl std::str::FromStr for AssessmentTargetV2 {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == Self::SESSION_GOAL_STR {
            Ok(Self)
        } else {
            Err(invalid("assessment target must be session_goal"))
        }
    }
}

impl Default for AssessmentTargetV2 {
    fn default() -> Self {
        Self::new()
    }
}

/// The recorded outcome of a session-goal assessment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoalOutcome {
    Achieved,
    NotAchieved,
    Superseded,
}

impl GoalOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Achieved => "achieved",
            Self::NotAchieved => "not_achieved",
            Self::Superseded => "superseded",
        }
    }
}

impl std::str::FromStr for GoalOutcome {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "achieved" => Ok(Self::Achieved),
            "not_achieved" => Ok(Self::NotAchieved),
            "superseded" => Ok(Self::Superseded),
            _ => Err(invalid("unknown goal outcome")),
        }
    }
}

/// One option-to-outcome mapping inside a session-goal assessment contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssessmentOutcomeMappingV2 {
    option_id: OptionId,
    outcome: GoalOutcome,
}

impl AssessmentOutcomeMappingV2 {
    pub fn new(option_id: OptionId, outcome: GoalOutcome) -> Self {
        Self { option_id, outcome }
    }

    pub fn option_id(&self) -> &OptionId {
        &self.option_id
    }

    pub const fn outcome(&self) -> GoalOutcome {
        self.outcome
    }
}

/// The session-goal assessment contract attached to a decision definition. It always targets
/// `session_goal` and maps decision options to goal outcomes. Outcome coverage (every goal outcome
/// reachable) is a graph-vetting concern and is not enforced by this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssessmentContractV2 {
    target: AssessmentTargetV2,
    outcomes: Vec<AssessmentOutcomeMappingV2>,
}

impl AssessmentContractV2 {
    pub fn new(outcomes: Vec<AssessmentOutcomeMappingV2>) -> Result<Self, DomainError> {
        Self::with_target(AssessmentTargetV2::new(), outcomes)
    }

    /// Reconstructs an assessment contract from an authored target and outcome mappings. The target
    /// must be `session_goal`; other values are rejected so the domain never silently accepts an
    /// unsupported assessment target.
    pub fn with_target(
        target: AssessmentTargetV2,
        outcomes: Vec<AssessmentOutcomeMappingV2>,
    ) -> Result<Self, DomainError> {
        if outcomes.len() < MIN_ASSESSMENT_OUTCOME_MAPPINGS
            || outcomes.len() > MAX_ASSESSMENT_OUTCOME_MAPPINGS
        {
            return Err(invalid(
                "assessment outcome mapping count must be between three and eight",
            ));
        }
        let mut seen = BTreeSet::new();
        for mapping in &outcomes {
            if !seen.insert(mapping.option_id()) {
                return Err(invalid(
                    "assessment outcome option identifiers must be unique",
                ));
            }
        }
        Ok(Self { target, outcomes })
    }

    pub const fn target(&self) -> AssessmentTargetV2 {
        self.target
    }

    pub fn outcomes(&self) -> &[AssessmentOutcomeMappingV2] {
        &self.outcomes
    }
}

/// Caller-supplied parts for assembling a reusable decision node definition. Named fields remove
/// the silent-transpose hazard that distinct `title` / `objective` / `prompt` string parameters
/// would otherwise create at construction sites.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionDefinitionInputV2 {
    pub id: NodeDefinitionId,
    pub title: String,
    pub description: Option<String>,
    pub objective: String,
    pub prompt: String,
    pub evidence_guidance: Vec<String>,
    pub items: Vec<ItemSpecV2>,
    pub options: Vec<DecisionOptionV2>,
    pub reason: ReasonPolicyV2,
    pub assessment: Option<AssessmentContractV2>,
}

/// A reusable decision node definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionDefinitionV2 {
    id: NodeDefinitionId,
    title: String,
    description: Option<String>,
    objective: String,
    prompt: String,
    evidence_guidance: Vec<String>,
    items: Vec<ItemSpecV2>,
    options: Vec<DecisionOptionV2>,
    reason: ReasonPolicyV2,
    assessment: Option<AssessmentContractV2>,
}

impl DecisionDefinitionV2 {
    pub fn new(input: DecisionDefinitionInputV2) -> Result<Self, DomainError> {
        let DecisionDefinitionInputV2 {
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
        } = input;
        validate_text(
            "definition title",
            &title,
            1,
            MAX_DEFINITION_TITLE_CHARS,
            true,
        )?;
        if let Some(description) = &description {
            validate_text(
                "definition description",
                description,
                0,
                MAX_DEFINITION_DESCRIPTION_CHARS,
                false,
            )?;
        }
        validate_text(
            "decision objective",
            &objective,
            1,
            MAX_DECISION_OBJECTIVE_CHARS,
            true,
        )?;
        validate_text(
            "decision prompt",
            &prompt,
            1,
            MAX_DECISION_PROMPT_CHARS,
            true,
        )?;
        validate_evidence_guidance(&evidence_guidance)?;
        validate_item_specs(&items)?;
        validate_options(&options)?;
        Ok(Self {
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
    }

    pub fn id(&self) -> &NodeDefinitionId {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn objective(&self) -> &str {
        &self.objective
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn evidence_guidance(&self) -> &[String] {
        &self.evidence_guidance
    }

    pub fn items(&self) -> &[ItemSpecV2] {
        &self.items
    }

    pub fn options(&self) -> &[DecisionOptionV2] {
        &self.options
    }

    pub fn reason(&self) -> &ReasonPolicyV2 {
        &self.reason
    }

    pub fn assessment(&self) -> Option<&AssessmentContractV2> {
        self.assessment.as_ref()
    }

    pub const fn node_kind(&self) -> NodeKindV2 {
        NodeKindV2::Decision
    }
}

/// The transition effect carried by a declared decision route (dossier section 9.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionEffectV2 {
    Advance,
    Rework,
}

impl TransitionEffectV2 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Advance => "advance",
            Self::Rework => "rework",
        }
    }
}

impl std::str::FromStr for TransitionEffectV2 {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "advance" => Ok(Self::Advance),
            "rework" => Ok(Self::Rework),
            _ => Err(invalid("unknown transition effect")),
        }
    }
}

/// One declared route of a decision placement: the target graph node and transition effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRouteV2 {
    to: GraphNodeId,
    effect: TransitionEffectV2,
}

impl DecisionRouteV2 {
    pub const fn new(to: GraphNodeId, effect: TransitionEffectV2) -> Self {
        Self { to, effect }
    }

    pub fn to(&self) -> &GraphNodeId {
        &self.to
    }

    pub const fn effect(&self) -> TransitionEffectV2 {
        self.effect
    }
}

/// One option-to-route binding inside a decision placement route table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRouteEntryV2 {
    option_id: OptionId,
    route: DecisionRouteV2,
}

impl DecisionRouteEntryV2 {
    pub const fn new(option_id: OptionId, route: DecisionRouteV2) -> Self {
        Self { option_id, route }
    }

    pub fn option_id(&self) -> &OptionId {
        &self.option_id
    }

    pub fn route(&self) -> &DecisionRouteV2 {
        &self.route
    }
}

/// The bounded option-to-route table of a decision placement. Routes bind declared options to
/// graph targets; whether every option is routed is a graph-vetting concern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRouteMapV2 {
    entries: Vec<DecisionRouteEntryV2>,
}

impl DecisionRouteMapV2 {
    pub fn new(entries: Vec<DecisionRouteEntryV2>) -> Result<Self, DomainError> {
        if entries.len() < MIN_ROUTES_PER_DECISION || entries.len() > MAX_ROUTES_PER_DECISION {
            return Err(invalid("route count must be between one and eight"));
        }
        let mut seen = BTreeSet::new();
        for entry in &entries {
            if !seen.insert(entry.option_id()) {
                return Err(invalid("route option identifiers must be unique"));
            }
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[DecisionRouteEntryV2] {
        &self.entries
    }
}

/// The action-placement skip policy. A declared policy requires `allowed: true`; `false` is
/// invalid (dossier section 5.1). Decision placements must not declare skip; that cross-field rule
/// is enforced by graph vetting, not by this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkipPolicyV2 {
    allowed: bool,
    reason_required: bool,
}

impl SkipPolicyV2 {
    pub fn new(allowed: bool, reason_required: bool) -> Result<Self, DomainError> {
        if !allowed {
            return Err(invalid("a declared skip policy requires allowed: true"));
        }
        Ok(Self {
            allowed,
            reason_required,
        })
    }

    pub const fn allowed_with(reason_required: bool) -> Self {
        Self {
            allowed: true,
            reason_required,
        }
    }

    pub const fn is_allowed(&self) -> bool {
        self.allowed
    }

    pub const fn reason_required(&self) -> bool {
        self.reason_required
    }
}

/// The single normal outcome of an action placement. An action declares exactly one of a `next`
/// target or a terminal disposition; modeling the outcome as an enum makes "both or neither"
/// unrepresentable (dossier section 6.2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionOutcomeV2 {
    Next(GraphNodeId),
    Terminal,
}

impl ActionOutcomeV2 {
    pub fn next(to: GraphNodeId) -> Self {
        Self::Next(to)
    }

    pub const fn terminal() -> Self {
        Self::Terminal
    }

    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }

    pub fn next_target(&self) -> Option<&GraphNodeId> {
        match self {
            Self::Next(target) => Some(target),
            Self::Terminal => None,
        }
    }
}

/// One declared evidence reference on a graph placement.
///
/// `required` defaults to `true` at the authoring layer. Whether a required source may declare
/// `skip.allowed: true` is a graph-vetting concern and is not enforced by this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceReferenceV2 {
    source_node: GraphNodeId,
    required: bool,
    selected_items: Option<Vec<ItemId>>,
}

impl EvidenceReferenceV2 {
    pub fn new(
        source_node: GraphNodeId,
        required: bool,
        selected_items: Option<Vec<ItemId>>,
    ) -> Result<Self, DomainError> {
        if let Some(items) = &selected_items {
            if items.len() < MIN_SELECTED_ITEMS_PER_REFERENCE
                || items.len() > MAX_SELECTED_ITEMS_PER_REFERENCE
            {
                return Err(invalid(
                    "selected item count must be between one and sixteen",
                ));
            }
            let mut seen = BTreeSet::new();
            for item in items {
                if !seen.insert(item) {
                    return Err(invalid("selected item identifiers must be unique"));
                }
            }
        }
        Ok(Self {
            source_node,
            required,
            selected_items,
        })
    }

    pub fn source_node(&self) -> &GraphNodeId {
        &self.source_node
    }

    pub const fn required(&self) -> bool {
        self.required
    }

    pub fn selected_items(&self) -> Option<&[ItemId]> {
        self.selected_items.as_deref()
    }
}

/// The bounded `evidence_from` list of a graph placement. An absent list is represented by
/// `Option::None` at the placement level; a present list always carries at least one entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceFromListV2 {
    entries: Vec<EvidenceReferenceV2>,
}

impl EvidenceFromListV2 {
    pub fn new(entries: Vec<EvidenceReferenceV2>) -> Result<Self, DomainError> {
        if entries.len() < MIN_EVIDENCE_REFERENCES_PER_PLACEMENT
            || entries.len() > MAX_EVIDENCE_REFERENCES_PER_PLACEMENT
        {
            return Err(invalid(
                "evidence reference count must be between one and eight",
            ));
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[EvidenceReferenceV2] {
        &self.entries
    }
}

/// An action graph placement: one uniquely identified placement of an action definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionPlacementV2 {
    id: GraphNodeId,
    definition: NodeDefinitionId,
    evidence_from: Option<EvidenceFromListV2>,
    skip: Option<SkipPolicyV2>,
    outcome: ActionOutcomeV2,
}

impl ActionPlacementV2 {
    pub fn new(
        id: GraphNodeId,
        definition: NodeDefinitionId,
        evidence_from: Option<EvidenceFromListV2>,
        skip: Option<SkipPolicyV2>,
        outcome: ActionOutcomeV2,
    ) -> Self {
        Self {
            id,
            definition,
            evidence_from,
            skip,
            outcome,
        }
    }

    pub fn id(&self) -> &GraphNodeId {
        &self.id
    }

    pub fn definition(&self) -> &NodeDefinitionId {
        &self.definition
    }

    pub fn evidence_from(&self) -> Option<&EvidenceFromListV2> {
        self.evidence_from.as_ref()
    }

    pub fn skip(&self) -> Option<SkipPolicyV2> {
        self.skip
    }

    pub fn outcome(&self) -> &ActionOutcomeV2 {
        &self.outcome
    }
}

/// A decision graph placement: one uniquely identified placement of a decision definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionPlacementV2 {
    id: GraphNodeId,
    definition: NodeDefinitionId,
    evidence_from: Option<EvidenceFromListV2>,
    routes: DecisionRouteMapV2,
}

impl DecisionPlacementV2 {
    pub fn new(
        id: GraphNodeId,
        definition: NodeDefinitionId,
        evidence_from: Option<EvidenceFromListV2>,
        routes: DecisionRouteMapV2,
    ) -> Self {
        Self {
            id,
            definition,
            evidence_from,
            routes,
        }
    }

    pub fn id(&self) -> &GraphNodeId {
        &self.id
    }

    pub fn definition(&self) -> &NodeDefinitionId {
        &self.definition
    }

    pub fn evidence_from(&self) -> Option<&EvidenceFromListV2> {
        self.evidence_from.as_ref()
    }

    pub fn routes(&self) -> &DecisionRouteMapV2 {
        &self.routes
    }
}

/// The declared manual rework target list. Targets are graph node identifiers; whether each target
/// exists in the procedure graph is a graph-vetting concern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualReworkTargetListV2 {
    targets: Vec<GraphNodeId>,
}

impl ManualReworkTargetListV2 {
    pub fn new(targets: Vec<GraphNodeId>) -> Result<Self, DomainError> {
        if targets.len() < MIN_MANUAL_REWORK_TARGETS || targets.len() > MAX_MANUAL_REWORK_TARGETS {
            return Err(invalid(
                "manual rework target count must be between one and 64",
            ));
        }
        let mut seen = BTreeSet::new();
        for target in &targets {
            if !seen.insert(target) {
                return Err(invalid("manual rework targets must be unique"));
            }
        }
        Ok(Self { targets })
    }

    pub fn targets(&self) -> &[GraphNodeId] {
        &self.targets
    }
}

/// The procedure-level session-goal tracking opt-in.
///
/// Session goal tracking is enabled only when a procedure declares exactly `goal_tracking: true`.
/// The only accepted value is `true`; absence of the key disables tracking. `false`, and non-boolean
/// authored values, are rejected. Non-boolean values are refused by the parser before reaching the
/// domain; the domain value enforces the boolean boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GoalTrackingOptIn {
    enabled: bool,
}

impl GoalTrackingOptIn {
    pub fn from_bool(value: bool) -> Result<Self, DomainError> {
        if !value {
            return Err(invalid(
                "goal_tracking accepts only true; omit the key to disable",
            ));
        }
        Ok(Self { enabled: true })
    }

    pub const fn enabled() -> Self {
        Self { enabled: true }
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }
}

/// A task-specific session goal statement (dossier section 4.5).
///
/// The bounded value is non-empty and holds at most 1,000 Unicode characters. Whether a statement
/// is required is a goal-revision record concern (V2MOD-003); this value enforces only the bound
/// that applies when a statement is present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalStatementV2(String);

impl GoalStatementV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text("goal statement", &value, 1, MAX_GOAL_STATEMENT_CHARS, true)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for GoalStatementV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for GoalStatementV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One stable-ID success criterion of a session goal (dossier section 4.5). The statement is
/// non-empty and holds at most 300 Unicode characters; the identifier bound is owned by
/// `CriterionId`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalCriterionV2 {
    id: CriterionId,
    statement: String,
}

impl GoalCriterionV2 {
    pub fn new(id: CriterionId, statement: impl Into<String>) -> Result<Self, DomainError> {
        let statement = statement.into();
        validate_text(
            "criterion statement",
            &statement,
            1,
            MAX_CRITERION_STATEMENT_CHARS,
            true,
        )?;
        Ok(Self { id, statement })
    }

    pub fn id(&self) -> &CriterionId {
        &self.id
    }

    pub fn statement(&self) -> &str {
        &self.statement
    }
}

/// The ordered set of success criteria that define a session goal (dossier section 4.5). A
/// definition holds one to sixteen criteria with unique `CriterionId` values and preserves author
/// order. Revision metadata is owned by the goal-revision record (V2MOD-003).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalDefinitionV2 {
    criteria: Vec<GoalCriterionV2>,
}

impl GoalDefinitionV2 {
    pub fn new(criteria: Vec<GoalCriterionV2>) -> Result<Self, DomainError> {
        if criteria.len() < MIN_GOAL_CRITERIA || criteria.len() > MAX_GOAL_CRITERIA {
            return Err(invalid("goal criterion count must be between one and 16"));
        }
        let mut seen = BTreeSet::new();
        for criterion in &criteria {
            if !seen.insert(criterion.id()) {
                return Err(invalid("goal criterion identifiers must be unique"));
            }
        }
        Ok(Self { criteria })
    }

    pub fn criteria(&self) -> &[GoalCriterionV2] {
        &self.criteria
    }
}

/// A bounded goal revision reason (dossier section 4.5). Non-empty, at most 1,000 Unicode
/// characters. A revision reason is supplied for revisions after the first; whether one is present
/// is a goal-revision record concern (V2MOD-003), distinct from this bounded value used when
/// present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalRevisionReasonV2(String);

impl GoalRevisionReasonV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "goal revision reason",
            &value,
            1,
            MAX_GOAL_REVISION_REASON_CHARS,
            true,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for GoalRevisionReasonV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for GoalRevisionReasonV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A bounded criterion assessment reason (dossier section 4.5). Non-empty, at most 2,000 Unicode
/// characters. The reason is always required for every criterion assessment status; this value
/// enforces the bound used when a reason is recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CriterionAssessmentReasonV2(String);

impl CriterionAssessmentReasonV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "criterion assessment reason",
            &value,
            1,
            MAX_CRITERION_ASSESSMENT_REASON_CHARS,
            true,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for CriterionAssessmentReasonV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for CriterionAssessmentReasonV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CriterionId, ItemId, NodeDefinitionId};

    fn def_id(value: &str) -> NodeDefinitionId {
        NodeDefinitionId::new(value).unwrap()
    }

    fn node_id(value: &str) -> GraphNodeId {
        GraphNodeId::new(value).unwrap()
    }

    fn opt_id(value: &str) -> OptionId {
        OptionId::new(value).unwrap()
    }

    fn item(id: &str) -> ItemSpecV2 {
        ItemSpecV2::confirm(
            ItemCommonV2::new(
                ItemId::new(id).unwrap(),
                format!("Prompt for {id}"),
                None,
                true,
            )
            .unwrap(),
        )
    }

    fn common(id: &str) -> ItemCommonV2 {
        ItemCommonV2::new(
            ItemId::new(id).unwrap(),
            format!("Prompt for {id}"),
            None,
            true,
        )
        .unwrap()
    }

    fn reason() -> ReasonPolicyV2 {
        ReasonPolicyV2::new(true, None).unwrap()
    }

    fn option_with(id: &str) -> DecisionOptionV2 {
        DecisionOptionV2::new(opt_id(id), format!("Label {id}"), None).unwrap()
    }

    fn criterion_id(value: &str) -> CriterionId {
        CriterionId::new(value).unwrap()
    }

    fn decision_input() -> DecisionDefinitionInputV2 {
        DecisionDefinitionInputV2 {
            id: def_id("dec"),
            title: "title".to_owned(),
            description: None,
            objective: "objective".to_owned(),
            prompt: "prompt".to_owned(),
            evidence_guidance: Vec::new(),
            items: Vec::new(),
            options: vec![option_with("only")],
            reason: reason(),
            assessment: None,
        }
    }

    fn valid_action_definition() -> ActionDefinitionV2 {
        ActionDefinitionV2::new(
            def_id("implement"),
            "Implement the change",
            "Produce an implementation.",
            None,
            vec!["Do the work.".to_owned()],
            vec![item("implementation-summary")],
        )
        .unwrap()
    }

    fn valid_decision_definition() -> DecisionDefinitionV2 {
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            id: def_id("evaluate"),
            title: "Evaluate the result".to_owned(),
            description: None,
            objective: "Only acceptable evidence may proceed.".to_owned(),
            prompt: "Is the result acceptable?".to_owned(),
            evidence_guidance: Vec::new(),
            items: Vec::new(),
            options: vec![option_with("passed"), option_with("failed")],
            reason: reason(),
            assessment: None,
        })
        .unwrap()
    }

    #[test]
    fn identifiers_enforce_kebab_bounds_and_reuse_v1_rule() {
        assert!(NodeDefinitionId::new("a").is_ok());
        assert!(GraphNodeId::new("implement-change-2").is_ok());
        assert_eq!(
            GraphNodeId::new(""),
            Err(DomainError::EmptyValue {
                field: "GraphNodeId"
            })
        );
        let overlong = "a".repeat(65);
        assert_eq!(
            OptionId::new(overlong.clone()),
            Err(DomainError::ValueTooLong {
                field: "OptionId",
                maximum: 64,
                actual: 65,
            })
        );
        assert_eq!(
            CriterionId::new("Bad-Case"),
            Err(DomainError::InvalidIdentifier {
                field: "CriterionId"
            })
        );
        assert_eq!(
            CriterionId::new("trailing-"),
            Err(DomainError::InvalidIdentifier {
                field: "CriterionId"
            })
        );
    }

    #[test]
    fn action_definition_accepts_at_limit_scalars_and_collections() {
        let title = "t".repeat(120);
        let intent = "i".repeat(300);
        let description = "d".repeat(1000);
        let instructions = vec!["x".repeat(1000); 16];
        let mut items: Vec<ItemSpecV2> = (0..64)
            .map(|index| item(&format!("item-{index}")))
            .collect();
        let at_limit = ActionDefinitionV2::new(
            def_id("act"),
            title.clone(),
            intent.clone(),
            Some(description.clone()),
            instructions.clone(),
            items.clone(),
        )
        .unwrap();
        assert_eq!(at_limit.title(), title);
        assert_eq!(at_limit.intent(), intent);
        assert_eq!(at_limit.description(), Some(description.as_str()));
        assert_eq!(at_limit.instructions().len(), 16);
        assert_eq!(at_limit.items().len(), 64);
        assert_eq!(at_limit.node_kind(), NodeKindV2::Action);

        assert_eq!(
            ActionDefinitionV2::new(
                def_id("act"),
                "t".repeat(121),
                "intent",
                None,
                Vec::new(),
                Vec::new(),
            ),
            Err(invalid("definition title"))
        );
        assert_eq!(
            ActionDefinitionV2::new(
                def_id("act"),
                "title",
                "i".repeat(301),
                None,
                Vec::new(),
                Vec::new(),
            ),
            Err(invalid("action intent"))
        );
        assert_eq!(
            ActionDefinitionV2::new(
                def_id("act"),
                "title",
                "intent",
                Some("d".repeat(1001)),
                Vec::new(),
                Vec::new(),
            ),
            Err(invalid("definition description"))
        );
        items.push(item("extra-item"));
        assert_eq!(
            ActionDefinitionV2::new(def_id("act"), "title", "intent", None, Vec::new(), items,),
            Err(invalid("too many definition items"))
        );
        // instruction count one-over the sixteen-entry ceiling.
        let seventeen_instructions = vec!["x".repeat(1000); 17];
        assert_eq!(
            ActionDefinitionV2::new(
                def_id("act"),
                "title",
                "intent",
                None,
                seventeen_instructions,
                Vec::new(),
            ),
            Err(invalid("too many definition instructions"))
        );
    }

    #[test]
    fn action_definition_rejects_blank_required_text_and_empty_instruction_entries() {
        assert_eq!(
            ActionDefinitionV2::new(def_id("act"), "   ", "intent", None, Vec::new(), Vec::new(),),
            Err(invalid("definition title"))
        );
        assert_eq!(
            ActionDefinitionV2::new(
                def_id("act"),
                "title",
                "intent",
                None,
                vec![String::new()],
                Vec::new(),
            ),
            Err(invalid("definition instruction"))
        );
        assert_eq!(
            ActionDefinitionV2::new(
                def_id("act"),
                "title",
                "intent",
                None,
                vec!["x".repeat(1001)],
                Vec::new(),
            ),
            Err(invalid("definition instruction"))
        );
    }

    #[test]
    fn definition_item_identifiers_must_be_unique_within_a_definition() {
        let duplicate_items = vec![item("dup"), item("dup")];
        assert_eq!(
            ActionDefinitionV2::new(
                def_id("act"),
                "title",
                "intent",
                None,
                Vec::new(),
                duplicate_items,
            ),
            Err(invalid("definition item identifiers must be unique"))
        );
    }

    #[test]
    fn decision_definition_enforces_decision_shape_and_bounds() {
        let decision = valid_decision_definition();
        assert_eq!(decision.node_kind(), NodeKindV2::Decision);
        assert_eq!(decision.options().len(), 2);
        assert_eq!(decision.reason().required(), true);

        let objective_at_limit = "o".repeat(300);
        let prompt_at_limit = "p".repeat(500);
        let guidance = vec!["g".repeat(200); 8];
        let options: Vec<DecisionOptionV2> =
            (0..8).map(|i| option_with(&format!("opt-{i}"))).collect();
        DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
            objective: objective_at_limit,
            prompt: prompt_at_limit,
            evidence_guidance: guidance,
            options,
            ..decision_input()
        })
        .unwrap();

        assert_eq!(
            DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                objective: "o".repeat(301),
                ..decision_input()
            }),
            Err(invalid("decision objective"))
        );
        assert_eq!(
            DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                prompt: "p".repeat(501),
                ..decision_input()
            }),
            Err(invalid("decision prompt"))
        );
        assert_eq!(
            DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                evidence_guidance: vec!["g".repeat(201)],
                ..decision_input()
            }),
            Err(invalid("evidence guidance"))
        );
        assert_eq!(
            DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                options: Vec::new(),
                ..decision_input()
            }),
            Err(invalid(
                "decision option count must be between one and eight"
            ))
        );
        // evidence guidance count one-over the eight-entry ceiling.
        assert_eq!(
            DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                evidence_guidance: vec!["g".repeat(200); 9],
                ..decision_input()
            })
            .unwrap_err(),
            invalid("too many evidence guidance entries")
        );
        let too_many: Vec<DecisionOptionV2> =
            (0..9).map(|i| option_with(&format!("o-{i}"))).collect();
        assert_eq!(
            DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                options: too_many,
                ..decision_input()
            })
            .unwrap_err(),
            invalid("decision option count must be between one and eight")
        );
    }

    #[test]
    fn decision_option_identifiers_must_be_unique() {
        let options = vec![
            DecisionOptionV2::new(opt_id("dup"), "Label one", None).unwrap(),
            DecisionOptionV2::new(opt_id("dup"), "Label two", None).unwrap(),
        ];
        assert_eq!(
            DecisionDefinitionV2::new(DecisionDefinitionInputV2 {
                options,
                ..decision_input()
            }),
            Err(invalid("decision option identifiers must be unique"))
        );
    }

    #[test]
    fn decision_and_action_definitions_are_shape_distinct() {
        let action = valid_action_definition();
        let decision = valid_decision_definition();
        assert_eq!(action.node_kind(), NodeKindV2::Action);
        assert_eq!(decision.node_kind(), NodeKindV2::Decision);
        // Action definitions carry an intent and never an assessment contract.
        assert!(!action.intent().is_empty());
        // Decision definitions carry an objective and a reason policy, not an intent.
        assert!(!decision.objective().is_empty());
        assert!(decision.reason().required());
        assert!(decision.assessment().is_none());
    }

    #[test]
    fn reason_policy_requires_true() {
        assert_eq!(
            ReasonPolicyV2::new(false, None),
            Err(invalid("a declared reason policy requires required: true"))
        );
        let with_prompt = ReasonPolicyV2::new(true, Some("p".repeat(300))).unwrap();
        assert_eq!(with_prompt.prompt(), Some("p".repeat(300).as_str()));
        assert_eq!(
            ReasonPolicyV2::new(true, Some("p".repeat(301))),
            Err(invalid("reason policy prompt"))
        );
    }

    #[test]
    fn skip_policy_requires_allowed_true() {
        assert_eq!(
            SkipPolicyV2::new(false, false),
            Err(invalid("a declared skip policy requires allowed: true"))
        );
        assert_eq!(
            SkipPolicyV2::new(false, true),
            Err(invalid("a declared skip policy requires allowed: true"))
        );
        let policy = SkipPolicyV2::new(true, true).unwrap();
        assert!(policy.is_allowed());
        assert!(policy.reason_required());
        let no_reason = SkipPolicyV2::allowed_with(false);
        assert!(no_reason.is_allowed());
        assert!(!no_reason.reason_required());
    }

    #[test]
    fn action_outcome_is_exactly_next_or_terminal() {
        let next = ActionOutcomeV2::next(node_id("target"));
        assert!(!next.is_terminal());
        assert_eq!(next.next_target(), Some(&node_id("target")));
        let terminal = ActionOutcomeV2::terminal();
        assert!(terminal.is_terminal());
        assert_eq!(terminal.next_target(), None);
    }

    #[test]
    fn decision_route_map_enforces_bounds_and_unique_option_keys() {
        let route = DecisionRouteV2::new(node_id("target"), TransitionEffectV2::Advance);
        let one = vec![DecisionRouteEntryV2::new(opt_id("a"), route.clone()); 1];
        assert!(DecisionRouteMapV2::new(one).is_ok());
        let eight: Vec<DecisionRouteEntryV2> = (0..8)
            .map(|i| DecisionRouteEntryV2::new(opt_id(&format!("o-{i}")), route.clone()))
            .collect();
        assert!(DecisionRouteMapV2::new(eight).is_ok());

        assert_eq!(
            DecisionRouteMapV2::new(Vec::new()).unwrap_err(),
            invalid("route count must be between one and eight")
        );
        let nine: Vec<DecisionRouteEntryV2> = (0..9)
            .map(|i| DecisionRouteEntryV2::new(opt_id(&format!("o-{i}")), route.clone()))
            .collect();
        assert_eq!(
            DecisionRouteMapV2::new(nine).unwrap_err(),
            invalid("route count must be between one and eight")
        );
        let duplicate = vec![
            DecisionRouteEntryV2::new(opt_id("dup"), route.clone()),
            DecisionRouteEntryV2::new(opt_id("dup"), route),
        ];
        assert_eq!(
            DecisionRouteMapV2::new(duplicate).unwrap_err(),
            invalid("route option identifiers must be unique")
        );
    }

    #[test]
    fn transition_effect_round_trips_authoring_strings() {
        assert_eq!(
            "advance".parse::<TransitionEffectV2>().unwrap(),
            TransitionEffectV2::Advance
        );
        assert_eq!(
            "rework".parse::<TransitionEffectV2>().unwrap(),
            TransitionEffectV2::Rework
        );
        assert_eq!(
            "branch".parse::<TransitionEffectV2>(),
            Err(invalid("unknown transition effect"))
        );
    }

    #[test]
    fn evidence_reference_enforces_selected_item_bounds_and_uniqueness() {
        let required = EvidenceReferenceV2::new(node_id("source"), true, None).unwrap();
        assert!(required.required());
        assert_eq!(required.selected_items(), None);

        let items: Vec<ItemId> = (0..16)
            .map(|i| ItemId::new(format!("item-{i}")).unwrap())
            .collect();
        let at_limit =
            EvidenceReferenceV2::new(node_id("source"), false, Some(items.clone())).unwrap();
        assert!(!at_limit.required());
        assert_eq!(at_limit.selected_items(), Some(items.as_slice()));

        assert_eq!(
            EvidenceReferenceV2::new(node_id("source"), true, Some(Vec::new())).unwrap_err(),
            invalid("selected item count must be between one and sixteen")
        );
        let too_many: Vec<ItemId> = (0..17)
            .map(|i| ItemId::new(format!("item-{i}")).unwrap())
            .collect();
        assert_eq!(
            EvidenceReferenceV2::new(node_id("source"), true, Some(too_many)).unwrap_err(),
            invalid("selected item count must be between one and sixteen")
        );
        let duplicate = vec![ItemId::new("dup").unwrap(), ItemId::new("dup").unwrap()];
        assert_eq!(
            EvidenceReferenceV2::new(node_id("source"), true, Some(duplicate)).unwrap_err(),
            invalid("selected item identifiers must be unique")
        );
    }

    #[test]
    fn evidence_from_list_enforces_one_to_eight_entries() {
        let one = vec![EvidenceReferenceV2::new(node_id("a"), true, None).unwrap()];
        assert!(EvidenceFromListV2::new(one).is_ok());
        let eight: Vec<EvidenceReferenceV2> = (0..8)
            .map(|i| EvidenceReferenceV2::new(node_id(&format!("n-{i}")), true, None).unwrap())
            .collect();
        assert!(EvidenceFromListV2::new(eight).is_ok());
        assert_eq!(
            EvidenceFromListV2::new(Vec::new()).unwrap_err(),
            invalid("evidence reference count must be between one and eight")
        );
        // one-over the eight-entry ceiling.
        let nine: Vec<EvidenceReferenceV2> = (0..9)
            .map(|i| EvidenceReferenceV2::new(node_id(&format!("n-{i}")), true, None).unwrap())
            .collect();
        assert_eq!(
            EvidenceFromListV2::new(nine).unwrap_err(),
            invalid("evidence reference count must be between one and eight")
        );
    }

    #[test]
    fn manual_rework_targets_enforce_bounds_and_uniqueness() {
        let one = vec![node_id("implement")];
        assert!(ManualReworkTargetListV2::new(one).is_ok());
        let sixty_four: Vec<GraphNodeId> = (0..64).map(|i| node_id(&format!("n-{i}"))).collect();
        assert!(ManualReworkTargetListV2::new(sixty_four).is_ok());
        assert_eq!(
            ManualReworkTargetListV2::new(Vec::new()).unwrap_err(),
            invalid("manual rework target count must be between one and 64")
        );
        let sixty_five: Vec<GraphNodeId> = (0..65).map(|i| node_id(&format!("n-{i}"))).collect();
        assert_eq!(
            ManualReworkTargetListV2::new(sixty_five).unwrap_err(),
            invalid("manual rework target count must be between one and 64")
        );
        let duplicate = vec![node_id("implement"), node_id("implement")];
        assert_eq!(
            ManualReworkTargetListV2::new(duplicate).unwrap_err(),
            invalid("manual rework targets must be unique")
        );
    }

    #[test]
    fn assessment_contract_enforces_target_mapping_count_and_unique_options() {
        let outcomes = vec![
            AssessmentOutcomeMappingV2::new(opt_id("achieved-option"), GoalOutcome::Achieved),
            AssessmentOutcomeMappingV2::new(opt_id("failed-option"), GoalOutcome::NotAchieved),
            AssessmentOutcomeMappingV2::new(opt_id("superseded-option"), GoalOutcome::Superseded),
        ];
        let contract = AssessmentContractV2::new(outcomes.clone()).unwrap();
        assert_eq!(contract.target(), AssessmentTargetV2::new());
        assert_eq!(contract.outcomes().len(), 3);

        let eight: Vec<AssessmentOutcomeMappingV2> = (0..8)
            .map(|i| {
                AssessmentOutcomeMappingV2::new(opt_id(&format!("o-{i}")), GoalOutcome::Achieved)
            })
            .collect();
        assert!(AssessmentContractV2::new(eight).is_ok());

        // one-over the eight-mapping ceiling.
        let nine: Vec<AssessmentOutcomeMappingV2> = (0..9)
            .map(|i| {
                AssessmentOutcomeMappingV2::new(opt_id(&format!("o-{i}")), GoalOutcome::Achieved)
            })
            .collect();
        assert_eq!(
            AssessmentContractV2::new(nine).unwrap_err(),
            invalid("assessment outcome mapping count must be between three and eight")
        );

        assert_eq!(
            AssessmentContractV2::new(outcomes[..2].to_vec()).unwrap_err(),
            invalid("assessment outcome mapping count must be between three and eight")
        );
        let duplicate_option = vec![
            AssessmentOutcomeMappingV2::new(opt_id("dup"), GoalOutcome::Achieved),
            AssessmentOutcomeMappingV2::new(opt_id("dup"), GoalOutcome::NotAchieved),
            AssessmentOutcomeMappingV2::new(opt_id("other"), GoalOutcome::Superseded),
        ];
        assert_eq!(
            AssessmentContractV2::new(duplicate_option).unwrap_err(),
            invalid("assessment outcome option identifiers must be unique")
        );
    }

    #[test]
    fn assessment_target_and_goal_outcome_round_trip_strings() {
        assert_eq!(
            "session_goal".parse::<AssessmentTargetV2>().unwrap(),
            AssessmentTargetV2::new()
        );
        assert_eq!(
            "procedure_goal".parse::<AssessmentTargetV2>(),
            Err(invalid("assessment target must be session_goal"))
        );
        assert_eq!(
            "achieved".parse::<GoalOutcome>().unwrap(),
            GoalOutcome::Achieved
        );
        assert_eq!(
            "not_achieved".parse::<GoalOutcome>().unwrap(),
            GoalOutcome::NotAchieved
        );
        assert_eq!(
            "superseded".parse::<GoalOutcome>().unwrap(),
            GoalOutcome::Superseded
        );
        assert_eq!(
            "partial".parse::<GoalOutcome>(),
            Err(invalid("unknown goal outcome"))
        );
        assert_eq!(GoalOutcome::Achieved.as_str(), "achieved");
        assert_eq!(GoalOutcome::NotAchieved.as_str(), "not_achieved");
        assert_eq!(GoalOutcome::Superseded.as_str(), "superseded");
    }

    #[test]
    fn goal_tracking_opt_in_accepts_only_true() {
        assert!(GoalTrackingOptIn::from_bool(true).unwrap().is_enabled());
        assert_eq!(
            GoalTrackingOptIn::from_bool(false),
            Err(invalid(
                "goal_tracking accepts only true; omit the key to disable"
            ))
        );
        assert!(GoalTrackingOptIn::enabled().is_enabled());
    }

    #[test]
    fn item_specs_enforce_v2_bounds_while_keeping_v1_unchanged() {
        // v2 item prompt is capped at 300 characters (v1 keeps 500).
        assert!(ItemCommonV2::new(ItemId::new("i").unwrap(), "p".repeat(300), None, true).is_ok());
        assert_eq!(
            ItemCommonV2::new(ItemId::new("i").unwrap(), "p".repeat(301), None, true).unwrap_err(),
            invalid("item prompt")
        );
        // v1 item prompt bound stays at 500 (no drift).
        assert!(
            crate::ItemCommonV1::new(ItemId::new("i").unwrap(), "p".repeat(500), None, true)
                .is_ok()
        );
        assert_eq!(
            crate::ItemCommonV1::new(ItemId::new("i").unwrap(), "p".repeat(501), None, true)
                .unwrap_err(),
            crate::DomainError::InvalidState {
                reason: "item prompt"
            }
        );

        // text max length hard cap is 16_384; one over fails.
        assert!(TextItemSpecV2::new(common("t"), 0, 16_384, true).is_ok());
        assert_eq!(
            TextItemSpecV2::new(common("t"), 0, 16_385, true).unwrap_err(),
            invalid("invalid text length constraints")
        );

        // choice count cap is 32; one over fails.
        let choices: Vec<String> = (0..32).map(|i| format!("c-{i}")).collect();
        assert!(ChoiceItemSpecV2::new(common("c"), choices.clone()).is_ok());
        let too_many: Vec<String> = (0..33).map(|i| format!("c-{i}")).collect();
        assert_eq!(
            ChoiceItemSpecV2::new(common("c"), too_many).unwrap_err(),
            invalid("choice count must be between one and 32")
        );

        // list bounds cap at 100 entries of 1_000 characters.
        assert!(ListItemSpecV2::new(common("l"), 0, 100, 1_000, true).is_ok());
        assert_eq!(
            ListItemSpecV2::new(common("l"), 0, 101, 500, true).unwrap_err(),
            invalid("invalid list item count constraints")
        );
        assert_eq!(
            ListItemSpecV2::new(common("l"), 0, 50, 1_001, true).unwrap_err(),
            invalid("invalid list entry length constraint")
        );

        // item kind taxonomy is reused from the v1 item contracts.
        assert_eq!(item("confirm").item_type(), ItemTypeV1::Confirm);
    }

    #[test]
    fn integer_item_spec_enforces_range_order() {
        assert!(IntegerItemSpecV2::new(common("int"), None, None).is_ok());
        let ranged = IntegerItemSpecV2::new(common("int"), Some(-1), Some(1)).unwrap();
        assert_eq!(ranged.minimum(), Some(-1));
        assert_eq!(ranged.maximum(), Some(1));
        // equal bounds are permitted; reversed bounds are rejected.
        assert!(IntegerItemSpecV2::new(common("int"), Some(5), Some(5)).is_ok());
        assert_eq!(
            IntegerItemSpecV2::new(common("int"), Some(2), Some(1)).unwrap_err(),
            invalid("integer minimum must not exceed maximum")
        );
    }

    #[test]
    fn artifact_item_spec_enforces_count_format_and_uniqueness() {
        let artifact =
            ArtifactItemSpecV2::new(common("art"), vec!["text/plain".to_owned()]).unwrap();
        assert_eq!(artifact.allowed_media_types(), &["text/plain".to_owned()]);

        let too_many_media: Vec<String> = (0..65).map(|i| format!("t{i}/plain")).collect();
        assert_eq!(
            ArtifactItemSpecV2::new(common("art"), too_many_media).unwrap_err(),
            invalid("too many allowed media types")
        );
        assert_eq!(
            ArtifactItemSpecV2::new(
                common("art"),
                vec!["text/plain".to_owned(), "text/plain".to_owned()],
            )
            .unwrap_err(),
            invalid("allowed media types must be unique")
        );
        // uppercase kind is rejected by the shared media-type format check.
        assert_eq!(
            ArtifactItemSpecV2::new(common("art"), vec!["Not/Lower".to_owned()]).unwrap_err(),
            invalid("media type must be lowercase ASCII without parameters")
        );
    }

    #[test]
    fn goal_statement_enforces_non_empty_unicode_char_bound() {
        // Each '가' is one Unicode scalar value but three UTF-8 bytes; the bound counts characters.
        let at_limit = "가".repeat(1_000);
        let statement = GoalStatementV2::new(at_limit.clone()).unwrap();
        assert_eq!(statement.as_str(), at_limit.as_str());
        assert_eq!(
            GoalStatementV2::new("가".repeat(1_001)),
            Err(invalid("goal statement"))
        );
        assert_eq!(GoalStatementV2::new(""), Err(invalid("goal statement")));
        assert_eq!(GoalStatementV2::new("   "), Err(invalid("goal statement")));
    }

    #[test]
    fn goal_criterion_enforces_unicode_statement_bound() {
        let criterion =
            GoalCriterionV2::new(criterion_id("deterministic"), "가".repeat(300)).unwrap();
        assert_eq!(criterion.id().as_str(), "deterministic");
        assert_eq!(criterion.statement(), "가".repeat(300).as_str());
        assert_eq!(
            GoalCriterionV2::new(criterion_id("deterministic"), "가".repeat(301)).unwrap_err(),
            invalid("criterion statement")
        );
        assert_eq!(
            GoalCriterionV2::new(criterion_id("deterministic"), "").unwrap_err(),
            invalid("criterion statement")
        );
    }

    #[test]
    fn goal_definition_enforces_criteria_bounds_unique_ids_and_order() {
        fn criterion(id: &str) -> GoalCriterionV2 {
            GoalCriterionV2::new(criterion_id(id), format!("Statement for {id}")).unwrap()
        }

        let one = GoalDefinitionV2::new(vec![criterion("c1")]).unwrap();
        assert_eq!(one.criteria().len(), 1);

        let sixteen: Vec<GoalCriterionV2> = (0..16).map(|i| criterion(&format!("c{i}"))).collect();
        assert!(GoalDefinitionV2::new(sixteen).is_ok());

        assert_eq!(
            GoalDefinitionV2::new(Vec::new()).unwrap_err(),
            invalid("goal criterion count must be between one and 16")
        );
        let seventeen: Vec<GoalCriterionV2> =
            (0..17).map(|i| criterion(&format!("c{i}"))).collect();
        assert_eq!(
            GoalDefinitionV2::new(seventeen).unwrap_err(),
            invalid("goal criterion count must be between one and 16")
        );

        let duplicate = vec![criterion("dup"), criterion("dup")];
        assert_eq!(
            GoalDefinitionV2::new(duplicate).unwrap_err(),
            invalid("goal criterion identifiers must be unique")
        );

        // author order is preserved, independent of identifier lexicographic order.
        let ordered = GoalDefinitionV2::new(vec![
            criterion("gamma"),
            criterion("alpha"),
            criterion("beta"),
        ])
        .unwrap();
        let ids: Vec<&str> = ordered.criteria().iter().map(|c| c.id().as_str()).collect();
        assert_eq!(ids, vec!["gamma", "alpha", "beta"]);
    }

    #[test]
    fn goal_revision_reason_enforces_non_empty_unicode_bound() {
        assert!(GoalRevisionReasonV2::new("가".repeat(1_000)).is_ok());
        assert_eq!(
            GoalRevisionReasonV2::new("가".repeat(1_001)).unwrap_err(),
            invalid("goal revision reason")
        );
        assert_eq!(
            GoalRevisionReasonV2::new("").unwrap_err(),
            invalid("goal revision reason")
        );
        assert_eq!(
            GoalRevisionReasonV2::new("  ").unwrap_err(),
            invalid("goal revision reason")
        );
    }

    #[test]
    fn criterion_assessment_reason_enforces_non_empty_unicode_bound() {
        assert!(CriterionAssessmentReasonV2::new("가".repeat(2_000)).is_ok());
        assert_eq!(
            CriterionAssessmentReasonV2::new("가".repeat(2_001)).unwrap_err(),
            invalid("criterion assessment reason")
        );
        assert_eq!(
            CriterionAssessmentReasonV2::new("").unwrap_err(),
            invalid("criterion assessment reason")
        );
    }
}
