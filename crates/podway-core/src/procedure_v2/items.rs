//! Procedure v2 item specifications and the reusable item-type taxonomy.

use std::collections::BTreeSet;

use crate::procedure::validate_media_type;
use crate::{
    BoundUnitV2, CheckResultOutcomeV2, DomainError, GraphNodeId, ItemId, ItemTypeV1, OperationId,
    RecordedItemValueV2, Sha256Digest, validate_text,
};

use super::invalid;
use super::{
    MAX_LIST_ENTRIES_V2, MAX_LIST_ENTRY_SCALARS_V2, MAX_LIST_TOTAL_SCALARS_V2, MAX_TEXT_SCALARS_V2,
};

const MAX_ITEM_PROMPT_CHARS: usize = 300;
const MAX_ITEM_HELP_CHARS: usize = 1000;
const MIN_V2_CHOICE_COUNT: usize = 1;
const MAX_V2_CHOICE_COUNT: usize = 32;
const MAX_V2_CHOICE_VALUE_CHARS: usize = 120;
const MAX_CONDITION_PREDICATES: usize = 4;
const MAX_V2_ARTIFACT_MEDIA_TYPES: usize = 64;
const MIN_CHECK_RESULT_OUTCOMES: usize = 1;
const MAX_CHECK_RESULT_OUTCOMES: usize = 3;

/// The reusable node contract kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKindV2 {
    Action,
    Decision,
}

/// One selected-evidence predicate declared by a decision option guard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidencePredicateV2 {
    source_node: GraphNodeId,
    item: ItemId,
    field_outcome: bool,
    operator: ItemPredicateOperatorV2,
}

impl EvidencePredicateV2 {
    pub const fn new(
        source_node: GraphNodeId,
        item: ItemId,
        field_outcome: bool,
        operator: ItemPredicateOperatorV2,
    ) -> Self {
        Self {
            source_node,
            item,
            field_outcome,
            operator,
        }
    }

    pub const fn source_node(&self) -> &GraphNodeId {
        &self.source_node
    }
    pub const fn item(&self) -> &ItemId {
        &self.item
    }
    pub const fn field_outcome(&self) -> bool {
        self.field_outcome
    }
    pub const fn operator(&self) -> &ItemPredicateOperatorV2 {
        &self.operator
    }

    pub fn evaluate(&self, value: Option<&RecordedItemValueV2>) -> PredicateStatusV2 {
        ItemPredicateV2::new(self.item.clone(), self.field_outcome, self.operator.clone())
            .evaluate(value)
    }
}

/// One to four AND-combined predicates controlling a decision option.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionGuardV2 {
    predicates: Vec<EvidencePredicateV2>,
}

impl OptionGuardV2 {
    pub fn new(predicates: Vec<EvidencePredicateV2>) -> Result<Self, DomainError> {
        if predicates.is_empty() || predicates.len() > MAX_CONDITION_PREDICATES {
            return Err(invalid(
                "guards must contain between one and four predicates",
            ));
        }
        Ok(Self { predicates })
    }

    pub fn predicates(&self) -> &[EvidencePredicateV2] {
        &self.predicates
    }

    pub fn evaluate<'a, F>(&self, mut value: F) -> ConditionStatusV2
    where
        F: FnMut(&GraphNodeId, &ItemId) -> Option<&'a RecordedItemValueV2>,
    {
        let predicates = self
            .predicates
            .iter()
            .map(|predicate| predicate.evaluate(value(predicate.source_node(), predicate.item())))
            .collect();
        condition_status(predicates)
    }
}

/// One bounded scalar admitted by the typed predicate vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredicateScalarV2 {
    Boolean(bool),
    Integer(i64),
    Text(String),
}

impl PredicateScalarV2 {
    pub fn text(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "predicate scalar",
            &value,
            1,
            MAX_V2_CHOICE_VALUE_CHARS,
            true,
        )?;
        Ok(Self::Text(value))
    }
}

/// The closed operator and expected value for one same-attempt item predicate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemPredicateOperatorV2 {
    Equals(PredicateScalarV2),
    NotEquals(PredicateScalarV2),
    Empty,
    NonEmpty,
    AtLeast(i64),
    AtMost(i64),
}

impl ItemPredicateOperatorV2 {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Equals(_) => "equals",
            Self::NotEquals(_) => "not_equals",
            Self::Empty => "empty",
            Self::NonEmpty => "non_empty",
            Self::AtLeast(_) => "at_least",
            Self::AtMost(_) => "at_most",
        }
    }

    pub const fn expected(&self) -> Option<&PredicateScalarV2> {
        match self {
            Self::Equals(value) | Self::NotEquals(value) => Some(value),
            Self::AtLeast(value) | Self::AtMost(value) => {
                // This branch cannot return a reference to a temporary scalar. Callers that
                // project numeric range expectations use `integer_expected` instead.
                let _ = value;
                None
            }
            Self::Empty | Self::NonEmpty => None,
        }
    }

    pub const fn integer_expected(&self) -> Option<i64> {
        match self {
            Self::AtLeast(value) | Self::AtMost(value) => Some(*value),
            _ => None,
        }
    }
}

/// One immutable same-attempt predicate declared by `required_when`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemPredicateV2 {
    item: ItemId,
    field_outcome: bool,
    operator: ItemPredicateOperatorV2,
}

impl ItemPredicateV2 {
    pub const fn new(item: ItemId, field_outcome: bool, operator: ItemPredicateOperatorV2) -> Self {
        Self {
            item,
            field_outcome,
            operator,
        }
    }

    pub fn item(&self) -> &ItemId {
        &self.item
    }

    pub const fn field_outcome(&self) -> bool {
        self.field_outcome
    }

    pub const fn operator(&self) -> &ItemPredicateOperatorV2 {
        &self.operator
    }

    pub fn evaluate(&self, value: Option<&RecordedItemValueV2>) -> PredicateStatusV2 {
        let Some(value) = value else {
            return PredicateStatusV2::unevaluable(self.clone());
        };
        let (state, actual) =
            match &self.operator {
                ItemPredicateOperatorV2::Equals(expected) => scalar_value(value)
                    .map(|actual| (actual == *expected, PredicateActualV2::Scalar(actual))),
                ItemPredicateOperatorV2::NotEquals(expected) => scalar_value(value)
                    .map(|actual| (actual != *expected, PredicateActualV2::Scalar(actual))),
                ItemPredicateOperatorV2::Empty => collection_count(value)
                    .map(|count| (count == 0, PredicateActualV2::Count(count))),
                ItemPredicateOperatorV2::NonEmpty => collection_count(value)
                    .map(|count| (count != 0, PredicateActualV2::Count(count))),
                ItemPredicateOperatorV2::AtLeast(expected) => value.as_integer().map(|actual| {
                    (
                        actual >= *expected,
                        PredicateActualV2::Scalar(PredicateScalarV2::Integer(actual)),
                    )
                }),
                ItemPredicateOperatorV2::AtMost(expected) => value.as_integer().map(|actual| {
                    (
                        actual <= *expected,
                        PredicateActualV2::Scalar(PredicateScalarV2::Integer(actual)),
                    )
                }),
            }
            .unwrap_or((false, PredicateActualV2::Unavailable));
        match actual {
            PredicateActualV2::Unavailable => PredicateStatusV2::unevaluable(self.clone()),
            actual => PredicateStatusV2 {
                predicate: self.clone(),
                state: if state {
                    ConditionStateV2::Met
                } else {
                    ConditionStateV2::Unmet
                },
                actual,
            },
        }
    }
}

fn scalar_value(value: &RecordedItemValueV2) -> Option<PredicateScalarV2> {
    match value.item_type() {
        ItemTypeV1::Confirm => Some(PredicateScalarV2::Boolean(true)),
        ItemTypeV1::Choice => value
            .as_choice()
            .map(|value| PredicateScalarV2::Text(value.to_owned())),
        ItemTypeV1::Integer => value.as_integer().map(PredicateScalarV2::Integer),
        ItemTypeV1::CheckResult => value
            .as_check_result()
            .map(|value| PredicateScalarV2::Text(value.outcome().as_str().to_owned())),
        ItemTypeV1::Text | ItemTypeV1::List | ItemTypeV1::Artifact => None,
    }
}

fn collection_count(value: &RecordedItemValueV2) -> Option<u32> {
    value
        .as_text()
        .map(|value| u32::try_from(value.trim().chars().count()).unwrap_or(u32::MAX))
        .or_else(|| {
            value
                .as_list()
                .map(|value| u32::try_from(value.len()).unwrap_or(u32::MAX))
        })
}

/// One to four AND-combined predicates controlling an otherwise optional item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemConditionV2 {
    predicates: Vec<ItemPredicateV2>,
}

impl ItemConditionV2 {
    pub fn new(predicates: Vec<ItemPredicateV2>) -> Result<Self, DomainError> {
        if predicates.is_empty() || predicates.len() > MAX_CONDITION_PREDICATES {
            return Err(invalid(
                "required_when must contain between one and four predicates",
            ));
        }
        Ok(Self { predicates })
    }

    pub fn predicates(&self) -> &[ItemPredicateV2] {
        &self.predicates
    }

    pub fn evaluate<'a, F>(&self, mut value: F) -> ConditionStatusV2
    where
        F: FnMut(&ItemId) -> Option<&'a RecordedItemValueV2>,
    {
        let predicates = self
            .predicates
            .iter()
            .map(|predicate| predicate.evaluate(value(predicate.item())))
            .collect::<Vec<_>>();
        condition_status(predicates)
    }
}

fn condition_status(predicates: Vec<PredicateStatusV2>) -> ConditionStatusV2 {
    let state = if predicates
        .iter()
        .any(|status| status.state() == ConditionStateV2::Unmet)
    {
        ConditionStateV2::Unmet
    } else if predicates
        .iter()
        .any(|status| status.state() == ConditionStateV2::Unevaluable)
    {
        ConditionStateV2::Unevaluable
    } else {
        ConditionStateV2::Met
    };
    ConditionStatusV2 { state, predicates }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConditionStateV2 {
    Met,
    Unmet,
    Unevaluable,
}

impl ConditionStateV2 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Met => "met",
            Self::Unmet => "unmet",
            Self::Unevaluable => "unevaluable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredicateActualV2 {
    Scalar(PredicateScalarV2),
    Count(u32),
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PredicateStatusV2 {
    predicate: ItemPredicateV2,
    state: ConditionStateV2,
    actual: PredicateActualV2,
}

impl PredicateStatusV2 {
    fn unevaluable(predicate: ItemPredicateV2) -> Self {
        Self {
            predicate,
            state: ConditionStateV2::Unevaluable,
            actual: PredicateActualV2::Unavailable,
        }
    }

    pub const fn predicate(&self) -> &ItemPredicateV2 {
        &self.predicate
    }
    pub const fn state(&self) -> ConditionStateV2 {
        self.state
    }
    pub const fn actual(&self) -> &PredicateActualV2 {
        &self.actual
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConditionStatusV2 {
    state: ConditionStateV2,
    predicates: Vec<PredicateStatusV2>,
}

impl ConditionStatusV2 {
    pub const fn state(&self) -> ConditionStateV2 {
        self.state
    }
    pub fn predicates(&self) -> &[PredicateStatusV2] {
        &self.predicates
    }
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
    required_when: Option<ItemConditionV2>,
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
            required_when: None,
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

    pub fn with_required_when(
        mut self,
        required_when: Option<ItemConditionV2>,
    ) -> Result<Self, DomainError> {
        if self.required && required_when.is_some() {
            return Err(invalid(
                "an item with required_when must declare required: false",
            ));
        }
        self.required_when = required_when;
        Ok(self)
    }

    pub const fn required_when(&self) -> Option<&ItemConditionV2> {
        self.required_when.as_ref()
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
        if max_length > MAX_TEXT_SCALARS_V2 {
            return Err(DomainError::BoundExceeded {
                field: "text max_length",
                actual: u64::from(max_length),
                maximum: u64::from(MAX_TEXT_SCALARS_V2),
                unit: BoundUnitV2::Scalars,
            });
        }
        if min_length > max_length {
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
    max_total_length: u32,
    unique: bool,
}

impl ListItemSpecV2 {
    pub fn new(
        common: ItemCommonV2,
        min_items: u16,
        max_items: u16,
        max_item_length: u16,
        max_total_length: u32,
        unique: bool,
    ) -> Result<Self, DomainError> {
        if max_items > MAX_LIST_ENTRIES_V2 {
            return Err(DomainError::BoundExceeded {
                field: "list max_items",
                actual: u64::from(max_items),
                maximum: u64::from(MAX_LIST_ENTRIES_V2),
                unit: BoundUnitV2::Entries,
            });
        }
        if max_items == 0 || min_items > max_items {
            return Err(invalid("invalid list item count constraints"));
        }
        if max_item_length > MAX_LIST_ENTRY_SCALARS_V2 {
            return Err(DomainError::BoundExceeded {
                field: "list max_item_length",
                actual: u64::from(max_item_length),
                maximum: u64::from(MAX_LIST_ENTRY_SCALARS_V2),
                unit: BoundUnitV2::Scalars,
            });
        }
        if max_item_length == 0 {
            return Err(invalid("invalid list entry length constraint"));
        }
        if max_total_length > MAX_LIST_TOTAL_SCALARS_V2 {
            return Err(DomainError::BoundExceeded {
                field: "list max_total_length",
                actual: u64::from(max_total_length),
                maximum: u64::from(MAX_LIST_TOTAL_SCALARS_V2),
                unit: BoundUnitV2::Scalars,
            });
        }
        // List entries are non-empty, so a declaration is satisfiable only when the total content
        // ceiling admits at least one scalar per required entry.
        if max_total_length == 0 || u32::from(min_items) > max_total_length {
            return Err(invalid("invalid list total content constraint"));
        }
        Ok(Self {
            common,
            min_items,
            max_items,
            max_item_length,
            max_total_length,
            unique,
        })
    }

    /// The effective total-content ceiling, which is the tighter of the declared total and the
    /// product of the entry-count and per-entry ceilings.
    pub const fn effective_total_length(&self) -> u32 {
        let product = (self.max_items as u32).saturating_mul(self.max_item_length as u32);
        if product < self.max_total_length {
            product
        } else {
            self.max_total_length
        }
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

    pub const fn max_total_length(&self) -> u32 {
        self.max_total_length
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

/// Immutable declaration that binds a result slot to one external operation identity and a closed
/// set of accepted outcomes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckResultItemSpecV2 {
    common: ItemCommonV2,
    operation_id: OperationId,
    operation_digest: Sha256Digest,
    accepted_outcomes: Vec<CheckResultOutcomeV2>,
}

impl CheckResultItemSpecV2 {
    pub fn new(
        common: ItemCommonV2,
        operation_id: OperationId,
        operation_digest: Sha256Digest,
        accepted_outcomes: Vec<CheckResultOutcomeV2>,
    ) -> Result<Self, DomainError> {
        if accepted_outcomes.len() < MIN_CHECK_RESULT_OUTCOMES
            || accepted_outcomes.len() > MAX_CHECK_RESULT_OUTCOMES
        {
            return Err(invalid(
                "check result accepted outcomes must contain between one and three values",
            ));
        }
        if accepted_outcomes.iter().collect::<BTreeSet<_>>().len() != accepted_outcomes.len() {
            return Err(invalid("check result accepted outcomes must be unique"));
        }
        Ok(Self {
            common,
            operation_id,
            operation_digest,
            accepted_outcomes,
        })
    }

    pub fn common(&self) -> &ItemCommonV2 {
        &self.common
    }

    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub fn operation_digest(&self) -> &Sha256Digest {
        &self.operation_digest
    }

    pub fn accepted_outcomes(&self) -> &[CheckResultOutcomeV2] {
        &self.accepted_outcomes
    }
}

/// An immutable specification for one of the seven supported Procedure v2 item types, reusing the
/// shared item-type taxonomy under the tightened v2 bounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemSpecV2 {
    Confirm(ConfirmItemSpecV2),
    Text(TextItemSpecV2),
    Choice(ChoiceItemSpecV2),
    Integer(IntegerItemSpecV2),
    List(ListItemSpecV2),
    Artifact(ArtifactItemSpecV2),
    CheckResult(CheckResultItemSpecV2),
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
        max_total_length: u32,
        unique: bool,
    ) -> Result<Self, DomainError> {
        Ok(Self::List(ListItemSpecV2::new(
            common,
            min_items,
            max_items,
            max_item_length,
            max_total_length,
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

    pub fn check_result(
        common: ItemCommonV2,
        operation_id: OperationId,
        operation_digest: Sha256Digest,
        accepted_outcomes: Vec<CheckResultOutcomeV2>,
    ) -> Result<Self, DomainError> {
        Ok(Self::CheckResult(CheckResultItemSpecV2::new(
            common,
            operation_id,
            operation_digest,
            accepted_outcomes,
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
            Self::CheckResult(specification) => specification.common(),
        }
    }

    pub fn id(&self) -> &ItemId {
        self.common().id()
    }

    /// Evaluates this item's authored condition against one complete same-definition item
    /// snapshot. Static authoring validation guarantees every source is an earlier item; this
    /// lookup remains total and fail-closed for reconstructed or otherwise inconsistent models.
    pub fn condition_status(
        &self,
        definition_items: &[ItemSpecV2],
        values: &[Option<&RecordedItemValueV2>],
    ) -> Option<ConditionStatusV2> {
        let condition = self.common().required_when()?;
        Some(condition.evaluate(|source| {
            definition_items
                .iter()
                .position(|item| item.id() == source)
                .and_then(|index| values.get(index).copied().flatten())
        }))
    }

    /// Returns the derived requirement from the same authoritative item snapshot used for
    /// satisfaction. Unconditional requirements remain true; a conditional item is required only
    /// when every predicate is met.
    pub fn required_now(
        &self,
        definition_items: &[ItemSpecV2],
        values: &[Option<&RecordedItemValueV2>],
    ) -> bool {
        self.common().required()
            || self
                .condition_status(definition_items, values)
                .is_some_and(|status| status.state() == ConditionStateV2::Met)
    }

    pub const fn item_type(&self) -> ItemTypeV1 {
        match self {
            Self::Confirm(_) => ItemTypeV1::Confirm,
            Self::Text(_) => ItemTypeV1::Text,
            Self::Choice(_) => ItemTypeV1::Choice,
            Self::Integer(_) => ItemTypeV1::Integer,
            Self::List(_) => ItemTypeV1::List,
            Self::Artifact(_) => ItemTypeV1::Artifact,
            Self::CheckResult(_) => ItemTypeV1::CheckResult,
        }
    }

    /// Returns whether a bounded recorded value satisfies this item's immutable declaration.
    pub fn admits_recorded_value(&self, value: &RecordedItemValueV2) -> bool {
        match self {
            Self::Confirm(_) => value.item_type() == ItemTypeV1::Confirm,
            Self::Text(specification) => value.as_text().is_some_and(|value| {
                let length = value.trim().chars().count();
                length >= specification.min_length() as usize
                    && length <= specification.max_length() as usize
            }),
            Self::Choice(specification) => value
                .as_choice()
                .is_some_and(|value| specification.choices().iter().any(|choice| choice == value)),
            Self::Integer(specification) => value.as_integer().is_some_and(|value| {
                specification
                    .minimum()
                    .is_none_or(|minimum| value >= minimum)
                    && specification
                        .maximum()
                        .is_none_or(|maximum| value <= maximum)
            }),
            Self::List(specification) => value.as_list().is_some_and(|values| {
                let length = values.len();
                let total: u64 = values
                    .iter()
                    .map(|value| value.chars().count() as u64)
                    .sum();
                length >= specification.min_items() as usize
                    && length <= specification.max_items() as usize
                    && values.iter().all(|value| {
                        value.chars().count() <= specification.max_item_length() as usize
                    })
                    && total <= u64::from(specification.max_total_length())
                    && (!specification.unique()
                        || values.iter().collect::<BTreeSet<_>>().len() == values.len())
            }),
            Self::Artifact(specification) => value.as_artifact().is_some_and(|value| {
                specification.allowed_media_types().is_empty()
                    || specification
                        .allowed_media_types()
                        .iter()
                        .any(|media_type| media_type == value.media_type())
            }),
            Self::CheckResult(_) => value.item_type() == ItemTypeV1::CheckResult,
        }
    }

    /// Reports whether one admitted value satisfies this declaration's progression constraints.
    pub fn is_satisfied_by(&self, value: &RecordedItemValueV2) -> bool {
        match self {
            Self::CheckResult(specification) => value.as_check_result().is_some_and(|value| {
                value.operation_id() == specification.operation_id()
                    && value.operation_digest() == specification.operation_digest()
                    && specification.accepted_outcomes().contains(&value.outcome())
            }),
            _ => self.admits_recorded_value(value),
        }
    }
}
