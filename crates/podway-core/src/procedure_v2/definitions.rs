//! Procedure v2 reusable action/decision node definitions and their reason, option, and
//! session-goal assessment contracts.

use std::collections::BTreeSet;

use crate::{DomainError, NodeDefinitionId, OptionId, validate_text};

use super::invalid;
use super::{ItemSpecV2, NodeKindV2};

const MAX_DEFINITION_TITLE_CHARS: usize = 120;
const MAX_ACTION_INTENT_CHARS: usize = 300;
const MAX_DEFINITION_DESCRIPTION_CHARS: usize = 1000;
const MAX_DECISION_OBJECTIVE_CHARS: usize = 300;
const MAX_DECISION_PROMPT_CHARS: usize = 500;
const MAX_REASON_POLICY_PROMPT_CHARS: usize = 300;
const MAX_INSTRUCTION_CHARS: usize = 1000;
const MAX_OPTION_LABEL_CHARS: usize = 120;
const MAX_OPTION_CRITERIA_CHARS: usize = 500;
const MAX_EVIDENCE_GUIDANCE_ENTRY_CHARS: usize = 200;
const MAX_INSTRUCTIONS_PER_DEFINITION: usize = 16;
const MAX_ITEMS_PER_DEFINITION: usize = 64;
const MIN_OPTIONS_PER_DECISION: usize = 1;
const MAX_OPTIONS_PER_DECISION: usize = 8;
const MAX_EVIDENCE_GUIDANCE_ENTRIES: usize = 8;
const MIN_ASSESSMENT_OUTCOME_MAPPINGS: usize = 3;
const MAX_ASSESSMENT_OUTCOME_MAPPINGS: usize = 8;

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
