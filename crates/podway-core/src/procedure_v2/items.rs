//! Procedure v2 item specifications and the reusable item-type taxonomy.

use std::collections::BTreeSet;

use crate::procedure::validate_media_type;
use crate::{DomainError, ItemId, ItemTypeV1, validate_text};

use super::invalid;

const MAX_ITEM_PROMPT_CHARS: usize = 300;
const MAX_ITEM_HELP_CHARS: usize = 1000;
const MAX_V2_TEXT_LENGTH: u32 = 16_384;
const MIN_V2_CHOICE_COUNT: usize = 1;
const MAX_V2_CHOICE_COUNT: usize = 32;
const MAX_V2_CHOICE_VALUE_CHARS: usize = 120;
const MAX_V2_LIST_ENTRIES: u16 = 200;
const MAX_V2_LIST_ENTRY_CHARS: u16 = 1_000;
const MAX_V2_ARTIFACT_MEDIA_TYPES: usize = 64;

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
