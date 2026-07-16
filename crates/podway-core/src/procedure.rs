use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    DomainError, ProcedureSnapshotId, Sha256Digest, StageId, UnixMillis, canonicalize_json_v1,
    verify_canonical_json_v1,
};

pub const PROCEDURE_SCHEMA_V1: &str = "podway.procedure/v1";
pub const MAX_PROCEDURE_STAGES: usize = 64;
pub const MAX_STAGE_ITEMS: usize = 128;
pub const MAX_STAGE_INSTRUCTIONS: usize = 32;
pub const MAX_TEXT_LENGTH: u32 = 65_536;
pub const MAX_LIST_ITEMS: u16 = 1_000;
pub const MAX_LIST_ITEM_LENGTH: u16 = 4_000;

/// The persisted source category for an admitted procedure snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProcedureSourceKindV1 {
    Preset,
    File,
}

impl ProcedureSourceKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preset => "preset",
            Self::File => "file",
        }
    }

    pub fn from_row_value(value: &str) -> Result<Self, DomainError> {
        match value {
            "preset" => Ok(Self::Preset),
            "file" => Ok(Self::File),
            _ => Err(invalid("unknown procedure source kind")),
        }
    }
}

/// A normalized source label with its storage category and display form.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProcedureSourceLabelV1 {
    kind: ProcedureSourceKindV1,
    label: String,
    display: String,
}

impl ProcedureSourceLabelV1 {
    /// Classifies a legacy display label into the canonical persisted representation.
    ///
    /// Prefixed legacy values are accepted only when their suffix is a valid raw label. All
    /// unprefixed values are canonical file labels.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if let Some(label) = value.strip_prefix("preset:") {
            return Self::preset(label);
        }
        if let Some(label) = value.strip_prefix("procedure:") {
            return Self::file(label);
        }
        Self::file(value)
    }

    pub fn preset(label: impl Into<String>) -> Result<Self, DomainError> {
        Self::from_parts(ProcedureSourceKindV1::Preset, label.into())
    }

    pub fn file(label: impl Into<String>) -> Result<Self, DomainError> {
        Self::from_parts(ProcedureSourceKindV1::File, label.into())
    }

    /// Reconstructs and validates a source from separately persisted kind and raw-label columns.
    ///
    /// The row kind is authoritative. Display prefixes are derived and are therefore rejected in
    /// the raw label to keep the persisted representation unique.
    pub fn from_row(
        kind: ProcedureSourceKindV1,
        raw_label: impl Into<String>,
    ) -> Result<Self, DomainError> {
        Self::from_parts(kind, raw_label.into())
    }

    pub const fn kind(&self) -> ProcedureSourceKindV1 {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn display_label(&self) -> &str {
        &self.display
    }

    /// Compatibility accessor for callers that stored or displayed the legacy label directly.
    pub fn as_str(&self) -> &str {
        self.display_label()
    }

    pub fn into_inner(self) -> String {
        self.display
    }

    fn from_parts(kind: ProcedureSourceKindV1, label: String) -> Result<Self, DomainError> {
        if label.starts_with("preset:") || label.starts_with("procedure:") {
            return Err(invalid(
                "procedure source raw label must not contain a display prefix",
            ));
        }
        validate_text("procedure source label", &label, 1, 4_000, true)?;
        let display = match kind {
            ProcedureSourceKindV1::Preset => format!("preset:{label}"),
            ProcedureSourceKindV1::File => format!("procedure:{label}"),
        };
        validate_text("procedure source display label", &display, 1, 4_000, true)?;
        Ok(Self {
            kind,
            label,
            display,
        })
    }
}

/// Storage-bounded canonical procedure JSON supplied by configuration admission.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CanonicalProcedureJsonV1(String);

impl CanonicalProcedureJsonV1 {
    /// This constructor enforces only storage bounds. Configuration owns document production;
    /// `ProcedureSnapshotV1::from_canonical_json` performs exact persisted-value verification.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(invalid("canonical procedure JSON must not be empty"));
        }
        if value.len() > crate::MAX_PROCEDURE_DOCUMENT_BYTES_V1 {
            return Err(DomainError::ValueTooLong {
                field: "canonical procedure JSON",
                maximum: crate::MAX_PROCEDURE_DOCUMENT_BYTES_V1,
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

/// Validation warnings that may be accepted when admitting a procedure snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProcedureWarningCodeV1 {
    StageHasNoRequiredItems,
    StageNearHardLimits,
    FinalStageSkippable,
    AnyPreviousReturnPolicy,
    RepeatedPrompt,
    OptionalItemAppearsRequired,
}

impl ProcedureWarningCodeV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StageHasNoRequiredItems => "stage_has_no_required_items",
            Self::StageNearHardLimits => "stage_near_hard_limits",
            Self::FinalStageSkippable => "final_stage_skippable",
            Self::AnyPreviousReturnPolicy => "any_previous_return_policy",
            Self::RepeatedPrompt => "repeated_prompt",
            Self::OptionalItemAppearsRequired => "optional_item_appears_required",
        }
    }
}

impl TryFrom<String> for ProcedureWarningCodeV1 {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "stage_has_no_required_items" => Ok(Self::StageHasNoRequiredItems),
            "stage_near_hard_limits" => Ok(Self::StageNearHardLimits),
            "final_stage_skippable" => Ok(Self::FinalStageSkippable),
            "any_previous_return_policy" => Ok(Self::AnyPreviousReturnPolicy),
            "repeated_prompt" => Ok(Self::RepeatedPrompt),
            "optional_item_appears_required" => Ok(Self::OptionalItemAppearsRequired),
            _ => Err(invalid("unknown procedure warning code")),
        }
    }
}

/// Common immutable metadata shared by every procedure item type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemCommonV1 {
    id: crate::ItemId,
    prompt: String,
    help: Option<String>,
    required: bool,
}

impl ItemCommonV1 {
    pub fn new(
        id: crate::ItemId,
        prompt: impl Into<String>,
        help: Option<String>,
        required: bool,
    ) -> Result<Self, DomainError> {
        let prompt = prompt.into();
        validate_text("item prompt", &prompt, 1, 500, true)?;
        if let Some(help) = &help {
            validate_text("item help", help, 0, 4_000, false)?;
        }
        Ok(Self {
            id,
            prompt,
            help,
            required,
        })
    }

    pub fn id(&self) -> &crate::ItemId {
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

/// The six exact item kinds admitted by procedure v1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemTypeV1 {
    Confirm,
    Text,
    Choice,
    Integer,
    List,
    Artifact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmItemSpecV1 {
    common: ItemCommonV1,
}

impl ConfirmItemSpecV1 {
    pub fn new(common: ItemCommonV1) -> Self {
        Self { common }
    }

    pub fn common(&self) -> &ItemCommonV1 {
        &self.common
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextItemSpecV1 {
    common: ItemCommonV1,
    min_length: u32,
    max_length: u32,
    multiline: bool,
}

impl TextItemSpecV1 {
    pub fn new(
        common: ItemCommonV1,
        min_length: u32,
        max_length: u32,
        multiline: bool,
    ) -> Result<Self, DomainError> {
        if min_length > max_length || max_length > MAX_TEXT_LENGTH {
            return Err(invalid("invalid text length constraints"));
        }
        Ok(Self {
            common,
            min_length,
            max_length,
            multiline,
        })
    }

    pub fn common(&self) -> &ItemCommonV1 {
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
pub struct ChoiceItemSpecV1 {
    common: ItemCommonV1,
    choices: Vec<String>,
}

impl ChoiceItemSpecV1 {
    pub fn new(common: ItemCommonV1, choices: Vec<String>) -> Result<Self, DomainError> {
        if choices.is_empty() || choices.len() > 64 {
            return Err(invalid("choice count must be between one and 64"));
        }
        let mut seen = BTreeSet::new();
        for choice in &choices {
            validate_text("choice", choice, 1, 120, true)?;
            if !seen.insert(choice.as_str()) {
                return Err(invalid("choice values must be unique"));
            }
        }
        Ok(Self { common, choices })
    }

    pub fn common(&self) -> &ItemCommonV1 {
        &self.common
    }

    pub fn choices(&self) -> &[String] {
        &self.choices
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegerItemSpecV1 {
    common: ItemCommonV1,
    minimum: Option<i64>,
    maximum: Option<i64>,
}

impl IntegerItemSpecV1 {
    pub fn new(
        common: ItemCommonV1,
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

    pub fn common(&self) -> &ItemCommonV1 {
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
pub struct ListItemSpecV1 {
    common: ItemCommonV1,
    min_items: u16,
    max_items: u16,
    max_item_length: u16,
    unique: bool,
}

impl ListItemSpecV1 {
    pub fn new(
        common: ItemCommonV1,
        min_items: u16,
        max_items: u16,
        max_item_length: u16,
        unique: bool,
    ) -> Result<Self, DomainError> {
        if max_items == 0 {
            return Err(invalid("list must allow at least one item"));
        }
        if min_items > max_items || max_items > MAX_LIST_ITEMS {
            return Err(invalid("invalid list item count constraints"));
        }
        if max_item_length == 0 || max_item_length > MAX_LIST_ITEM_LENGTH {
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

    pub fn common(&self) -> &ItemCommonV1 {
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
pub struct ArtifactItemSpecV1 {
    common: ItemCommonV1,
    allowed_media_types: Vec<String>,
}

impl ArtifactItemSpecV1 {
    pub fn new(
        common: ItemCommonV1,
        allowed_media_types: Vec<String>,
    ) -> Result<Self, DomainError> {
        if allowed_media_types.len() > 64 {
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

    pub fn common(&self) -> &ItemCommonV1 {
        &self.common
    }

    pub fn allowed_media_types(&self) -> &[String] {
        &self.allowed_media_types
    }
}

/// An immutable specification for one of the six supported procedure item types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemSpecV1 {
    Confirm(ConfirmItemSpecV1),
    Text(TextItemSpecV1),
    Choice(ChoiceItemSpecV1),
    Integer(IntegerItemSpecV1),
    List(ListItemSpecV1),
    Artifact(ArtifactItemSpecV1),
}

impl ItemSpecV1 {
    pub fn confirm(common: ItemCommonV1) -> Self {
        Self::Confirm(ConfirmItemSpecV1::new(common))
    }

    pub fn text(
        common: ItemCommonV1,
        min_length: u32,
        max_length: u32,
        multiline: bool,
    ) -> Result<Self, DomainError> {
        Ok(Self::Text(TextItemSpecV1::new(
            common, min_length, max_length, multiline,
        )?))
    }

    pub fn choice(common: ItemCommonV1, choices: Vec<String>) -> Result<Self, DomainError> {
        Ok(Self::Choice(ChoiceItemSpecV1::new(common, choices)?))
    }

    pub fn integer(
        common: ItemCommonV1,
        minimum: Option<i64>,
        maximum: Option<i64>,
    ) -> Result<Self, DomainError> {
        Ok(Self::Integer(IntegerItemSpecV1::new(
            common, minimum, maximum,
        )?))
    }

    pub fn list(
        common: ItemCommonV1,
        min_items: u16,
        max_items: u16,
        max_item_length: u16,
        unique: bool,
    ) -> Result<Self, DomainError> {
        Ok(Self::List(ListItemSpecV1::new(
            common,
            min_items,
            max_items,
            max_item_length,
            unique,
        )?))
    }

    pub fn artifact(
        common: ItemCommonV1,
        allowed_media_types: Vec<String>,
    ) -> Result<Self, DomainError> {
        Ok(Self::Artifact(ArtifactItemSpecV1::new(
            common,
            allowed_media_types,
        )?))
    }

    pub fn common(&self) -> &ItemCommonV1 {
        match self {
            Self::Confirm(specification) => specification.common(),
            Self::Text(specification) => specification.common(),
            Self::Choice(specification) => specification.common(),
            Self::Integer(specification) => specification.common(),
            Self::List(specification) => specification.common(),
            Self::Artifact(specification) => specification.common(),
        }
    }

    pub fn id(&self) -> &crate::ItemId {
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

/// The explicit policy for skipping a stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkipPolicyV1 {
    allowed: bool,
    reason_required: bool,
}

impl SkipPolicyV1 {
    pub const fn not_allowed() -> Self {
        Self {
            allowed: false,
            reason_required: false,
        }
    }

    pub const fn allowed(reason_required: bool) -> Self {
        Self {
            allowed: true,
            reason_required,
        }
    }

    pub fn new(allowed: bool, reason_required: bool) -> Result<Self, DomainError> {
        if !allowed && reason_required {
            return Err(invalid(
                "a non-skippable stage cannot require a skip reason",
            ));
        }
        Ok(Self {
            allowed,
            reason_required,
        })
    }

    pub const fn is_allowed(&self) -> bool {
        self.allowed
    }

    pub const fn reason_required(&self) -> bool {
        self.reason_required
    }
}

/// An ordered stage definition held by an immutable procedure snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageSpecV1 {
    id: StageId,
    title: String,
    instructions: Vec<String>,
    items: Vec<ItemSpecV1>,
    skip_policy: SkipPolicyV1,
}

impl StageSpecV1 {
    pub fn new(
        id: StageId,
        title: impl Into<String>,
        instructions: Vec<String>,
        items: Vec<ItemSpecV1>,
        skip_policy: SkipPolicyV1,
    ) -> Result<Self, DomainError> {
        let title = title.into();
        validate_text("stage title", &title, 1, 120, true)?;
        if instructions.len() > MAX_STAGE_INSTRUCTIONS {
            return Err(invalid("too many stage instructions"));
        }
        for instruction in &instructions {
            validate_text("stage instruction", instruction, 1, 2_000, true)?;
        }
        if items.len() > MAX_STAGE_ITEMS {
            return Err(invalid("too many stage items"));
        }
        let mut ids = BTreeSet::new();
        for item in &items {
            if !ids.insert(item.id()) {
                return Err(invalid("stage item identifiers must be unique"));
            }
        }
        Ok(Self {
            id,
            title,
            instructions,
            items,
            skip_policy,
        })
    }

    pub fn id(&self) -> &StageId {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn instructions(&self) -> &[String] {
        &self.instructions
    }

    pub fn items(&self) -> &[ItemSpecV1] {
        &self.items
    }

    pub const fn skip_policy(&self) -> SkipPolicyV1 {
        self.skip_policy
    }

    pub fn item(&self, item_id: &crate::ItemId) -> Option<&ItemSpecV1> {
        self.items.iter().find(|item| item.id() == item_id)
    }
}

/// A validated non-empty set of explicitly allowed return destinations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReturnDestinationsV1(Vec<StageId>);

impl ReturnDestinationsV1 {
    pub fn new(destinations: Vec<StageId>) -> Result<Self, DomainError> {
        if destinations.is_empty() || destinations.len() > MAX_PROCEDURE_STAGES {
            return Err(invalid(
                "return destinations must contain between one and 64 stages",
            ));
        }
        let mut seen = BTreeSet::new();
        for destination in &destinations {
            if !seen.insert(destination) {
                return Err(invalid("return destinations must be unique"));
            }
        }
        Ok(Self(destinations))
    }

    pub fn as_slice(&self) -> &[StageId] {
        &self.0
    }
}

/// The procedure-wide rework policy. Backward-only checks are made against the active stage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReturnPolicyV1 {
    AnyPrevious,
    Only(ReturnDestinationsV1),
}

impl ReturnPolicyV1 {
    pub const fn any_previous() -> Self {
        Self::AnyPrevious
    }

    pub fn only(destinations: Vec<StageId>) -> Result<Self, DomainError> {
        Ok(Self::Only(ReturnDestinationsV1::new(destinations)?))
    }

    pub fn destinations(&self) -> Option<&[StageId]> {
        match self {
            Self::AnyPrevious => None,
            Self::Only(destinations) => Some(destinations.as_slice()),
        }
    }

    pub fn allows_destination(&self, destination: &StageId) -> bool {
        match self {
            Self::AnyPrevious => true,
            Self::Only(destinations) => destinations.as_slice().contains(destination),
        }
    }
}

/// Immutable row metadata required to rehydrate a persisted procedure snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalProcedureSnapshotInputV1 {
    pub snapshot_id: ProcedureSnapshotId,
    pub schema_id: String,
    pub procedure_id: String,
    pub procedure_version: String,
    pub name: String,
    pub source_label: ProcedureSourceLabelV1,
    pub canonical_json: CanonicalProcedureJsonV1,
    pub digest: Sha256Digest,
    pub created_at: UnixMillis,
}

/// Caller-supplied snapshot parts that `ProcedureSnapshotV1::new` verifies against canonical JSON.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureSnapshotInputV1 {
    pub snapshot_id: ProcedureSnapshotId,
    pub procedure_id: String,
    pub procedure_version: String,
    pub name: String,
    pub description: Option<String>,
    pub stages: Vec<StageSpecV1>,
    pub return_policy: ReturnPolicyV1,
    pub source_label: ProcedureSourceLabelV1,
    pub canonical_json: CanonicalProcedureJsonV1,
    pub digest: Sha256Digest,
    pub accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
    pub created_at: UnixMillis,
}
/// Structured snapshot input assembled through the canonical procedure DTO. Warning acceptance is
/// an explicit admission proof and must exactly match the deterministic warning set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureSnapshotAssemblyInputV1 {
    pub snapshot_id: ProcedureSnapshotId,
    pub procedure_id: String,
    pub procedure_version: String,
    pub name: String,
    pub description: Option<String>,
    pub stages: Vec<StageSpecV1>,
    pub return_policy: ReturnPolicyV1,
    pub source_label: ProcedureSourceLabelV1,
    pub accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
    pub created_at: UnixMillis,
}

/// Immutable admitted procedure data governing one session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureSnapshotV1 {
    snapshot_id: ProcedureSnapshotId,
    procedure_id: String,
    procedure_version: String,
    name: String,
    description: Option<String>,
    stages: Vec<StageSpecV1>,
    return_policy: ReturnPolicyV1,
    source_label: ProcedureSourceLabelV1,
    canonical_json: CanonicalProcedureJsonV1,
    digest: Sha256Digest,
    accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
    created_at: UnixMillis,
}

impl ProcedureSnapshotV1 {
    /// Rehydrates and verifies a persisted snapshot from exact canonical procedure JSON and row
    /// metadata. It is not a configuration ingestion or canonical-document production API.
    pub fn from_canonical_json(
        input: CanonicalProcedureSnapshotInputV1,
    ) -> Result<Self, DomainError> {
        let CanonicalProcedureSnapshotInputV1 {
            snapshot_id,
            schema_id,
            procedure_id,
            procedure_version,
            name,
            source_label,
            canonical_json,
            digest,
            created_at,
        } = input;

        if schema_id != PROCEDURE_SCHEMA_V1 {
            return Err(invalid("snapshot schema is not podway.procedure/v1"));
        }

        let actual_digest = canonical_procedure_digest(&canonical_json)?;
        if actual_digest != digest {
            return Err(invalid(
                "snapshot digest does not match canonical procedure JSON",
            ));
        }

        let procedure = decode_canonical_procedure(&canonical_json)?;
        if procedure.schema != schema_id {
            return Err(invalid(
                "snapshot row schema does not match canonical procedure JSON",
            ));
        }
        if procedure.id != procedure_id {
            return Err(invalid(
                "snapshot procedure identifier does not match canonical procedure JSON",
            ));
        }
        if procedure.version != procedure_version {
            return Err(invalid(
                "snapshot procedure version does not match canonical procedure JSON",
            ));
        }
        if procedure.name != name {
            return Err(invalid(
                "snapshot procedure name does not match canonical procedure JSON",
            ));
        }

        let accepted_warning_codes =
            derived_procedure_warning_codes(&procedure.stages, &procedure.return_policy);

        Self::from_verified(ProcedureSnapshotInputV1 {
            snapshot_id,
            procedure_id,
            procedure_version,
            name,
            description: procedure.description,
            stages: procedure.stages,
            return_policy: procedure.return_policy,
            source_label,
            canonical_json,
            digest,
            accepted_warning_codes,
            created_at,
        })
    }

    /// Assembles an in-memory procedure through the canonical DTO after verifying explicit warning
    /// acceptance. Configuration ingestion must produce snapshots through `podway-config` before
    /// persistence.
    pub fn assemble(input: ProcedureSnapshotAssemblyInputV1) -> Result<Self, DomainError> {
        let ProcedureSnapshotAssemblyInputV1 {
            snapshot_id,
            procedure_id,
            procedure_version,
            name,
            description,
            stages,
            return_policy,
            source_label,
            accepted_warning_codes,
            created_at,
        } = input;
        let expected_warning_codes = derived_procedure_warning_codes(&stages, &return_policy);
        if accepted_warning_codes != expected_warning_codes {
            return Err(invalid(
                "accepted warning codes must exactly match deterministic procedure warnings",
            ));
        }

        let canonical_json = serialize_canonical_procedure(
            &procedure_id,
            &procedure_version,
            &name,
            description.as_deref(),
            &stages,
            &return_policy,
        )?;
        let digest = canonical_procedure_digest(&canonical_json)?;

        Self::new(ProcedureSnapshotInputV1 {
            snapshot_id,
            procedure_id,
            procedure_version,
            name,
            description,
            stages,
            return_policy,
            source_label,
            canonical_json,
            digest,
            accepted_warning_codes,
            created_at,
        })
    }

    /// Verifies caller-supplied parts against their exact canonical procedure representation.
    pub fn new(input: ProcedureSnapshotInputV1) -> Result<Self, DomainError> {
        let ProcedureSnapshotInputV1 {
            snapshot_id,
            procedure_id,
            procedure_version,
            name,
            description,
            stages,
            return_policy,
            source_label,
            canonical_json,
            digest,
            accepted_warning_codes,
            created_at,
        } = input;
        let snapshot = Self::from_canonical_json(CanonicalProcedureSnapshotInputV1 {
            snapshot_id,
            schema_id: PROCEDURE_SCHEMA_V1.to_owned(),
            procedure_id,
            procedure_version,
            name,
            source_label,
            canonical_json,
            digest,
            created_at,
        })?;

        if snapshot.description != description
            || snapshot.stages != stages
            || snapshot.return_policy != return_policy
            || snapshot.accepted_warning_codes != accepted_warning_codes
        {
            return Err(invalid(
                "caller-supplied snapshot fields do not match canonical procedure JSON",
            ));
        }
        Ok(snapshot)
    }

    fn from_verified(input: ProcedureSnapshotInputV1) -> Result<Self, DomainError> {
        let ProcedureSnapshotInputV1 {
            snapshot_id,
            procedure_id,
            procedure_version,
            name,
            description,
            stages,
            return_policy,
            source_label,
            canonical_json,
            digest,
            accepted_warning_codes,
            created_at,
        } = input;
        validate_procedure_identifier(&procedure_id)?;
        validate_text("procedure version", &procedure_version, 1, 64, false)?;
        validate_text("procedure name", &name, 1, 120, true)?;
        if let Some(description) = &description {
            validate_text("procedure description", description, 0, 4_000, false)?;
        }
        if stages.is_empty() || stages.len() > MAX_PROCEDURE_STAGES {
            return Err(invalid("procedure stage count must be between one and 64"));
        }

        let mut stage_ids = BTreeSet::new();
        for stage in &stages {
            if !stage_ids.insert(stage.id()) {
                return Err(invalid("procedure stage identifiers must be unique"));
            }
        }
        if let Some(destinations) = return_policy.destinations() {
            for destination in destinations {
                if !stage_ids.contains(destination) {
                    return Err(invalid("return destination is not a procedure stage"));
                }
            }
        }
        let mut warnings = BTreeSet::new();
        for warning in &accepted_warning_codes {
            if !warnings.insert(*warning) {
                return Err(invalid("accepted warning codes must be unique"));
            }
        }

        Ok(Self {
            snapshot_id,
            procedure_id,
            procedure_version,
            name,
            description,
            stages,
            return_policy,
            source_label,
            canonical_json,
            digest,
            accepted_warning_codes,
            created_at,
        })
    }

    pub fn snapshot_id(&self) -> &ProcedureSnapshotId {
        &self.snapshot_id
    }

    pub fn procedure_id(&self) -> &str {
        &self.procedure_id
    }

    pub fn procedure_version(&self) -> &str {
        &self.procedure_version
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn stages(&self) -> &[StageSpecV1] {
        &self.stages
    }

    pub fn stage(&self, stage_id: &StageId) -> Option<&StageSpecV1> {
        self.stages.iter().find(|stage| stage.id() == stage_id)
    }

    pub fn return_policy(&self) -> &ReturnPolicyV1 {
        &self.return_policy
    }

    pub fn source_label(&self) -> &ProcedureSourceLabelV1 {
        &self.source_label
    }

    pub fn canonical_json(&self) -> &CanonicalProcedureJsonV1 {
        &self.canonical_json
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub fn accepted_warning_codes(&self) -> &[ProcedureWarningCodeV1] {
        &self.accepted_warning_codes
    }

    pub const fn created_at(&self) -> UnixMillis {
        self.created_at
    }
}
fn canonical_procedure_digest(
    canonical_json: &CanonicalProcedureJsonV1,
) -> Result<Sha256Digest, DomainError> {
    Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json.as_str().as_bytes())
    ))
    .map_err(|_| invalid("failed to calculate canonical procedure digest"))
}

fn serialize_canonical_procedure(
    procedure_id: &str,
    procedure_version: &str,
    name: &str,
    description: Option<&str>,
    stages: &[StageSpecV1],
    return_policy: &ReturnPolicyV1,
) -> Result<CanonicalProcedureJsonV1, DomainError> {
    let document = CanonicalProcedureDocumentDtoV1 {
        schema: PROCEDURE_SCHEMA_V1,
        id: procedure_id,
        version: procedure_version,
        name,
        description,
        stages: stages.iter().map(canonical_stage_dto).collect(),
        rework: canonical_rework_dto(return_policy),
    };
    let canonical_json = canonicalize_json_v1(&document)
        .map_err(|_| invalid("failed to serialize canonical procedure JSON"))?;
    CanonicalProcedureJsonV1::new(canonical_json)
}

#[derive(Serialize)]
struct CanonicalProcedureDocumentDtoV1<'a> {
    schema: &'static str,
    id: &'a str,
    version: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    stages: Vec<CanonicalStageDtoV1<'a>>,
    rework: CanonicalReworkDtoV1<'a>,
}

#[derive(Serialize)]
struct CanonicalStageDtoV1<'a> {
    id: &'a str,
    title: &'a str,
    instructions: &'a [String],
    items: Vec<CanonicalItemDtoV1<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip: Option<CanonicalSkipPolicyDtoV1>,
}

#[derive(Serialize)]
struct CanonicalSkipPolicyDtoV1 {
    allowed: bool,
    reason_required: bool,
}

#[derive(Serialize)]
struct CanonicalReworkDtoV1<'a> {
    allow_return_to: CanonicalReturnTargetsDtoV1<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum CanonicalReturnTargetsDtoV1<'a> {
    AnyPrevious(&'static str),
    Only(Vec<&'a str>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum CanonicalItemDtoV1<'a> {
    Confirm {
        id: &'a str,
        prompt: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        help: Option<&'a str>,
        required: bool,
    },
    Text {
        id: &'a str,
        prompt: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        help: Option<&'a str>,
        required: bool,
        min_length: u32,
        max_length: u32,
        multiline: bool,
    },
    Choice {
        id: &'a str,
        prompt: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        help: Option<&'a str>,
        required: bool,
        choices: &'a [String],
    },
    Integer {
        id: &'a str,
        prompt: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        help: Option<&'a str>,
        required: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        minimum: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        maximum: Option<i64>,
    },
    List {
        id: &'a str,
        prompt: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        help: Option<&'a str>,
        required: bool,
        min_items: u16,
        max_items: u16,
        max_item_length: u16,
        unique: bool,
    },
    Artifact {
        id: &'a str,
        prompt: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        help: Option<&'a str>,
        required: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        allowed_media_types: Option<&'a [String]>,
    },
}

fn canonical_stage_dto(stage: &StageSpecV1) -> CanonicalStageDtoV1<'_> {
    let skip_policy = stage.skip_policy();
    CanonicalStageDtoV1 {
        id: stage.id().as_str(),
        title: stage.title(),
        instructions: stage.instructions(),
        items: stage.items().iter().map(canonical_item_dto).collect(),
        skip: skip_policy.is_allowed().then(|| CanonicalSkipPolicyDtoV1 {
            allowed: true,
            reason_required: skip_policy.reason_required(),
        }),
    }
}

fn canonical_rework_dto(return_policy: &ReturnPolicyV1) -> CanonicalReworkDtoV1<'_> {
    let allow_return_to = match return_policy {
        ReturnPolicyV1::AnyPrevious => CanonicalReturnTargetsDtoV1::AnyPrevious("any_previous"),
        ReturnPolicyV1::Only(destinations) => CanonicalReturnTargetsDtoV1::Only(
            destinations
                .as_slice()
                .iter()
                .map(StageId::as_str)
                .collect(),
        ),
    };
    CanonicalReworkDtoV1 { allow_return_to }
}

fn canonical_item_dto(item: &ItemSpecV1) -> CanonicalItemDtoV1<'_> {
    let common = item.common();
    let id = common.id().as_str();
    let prompt = common.prompt();
    let help = common.help();
    let required = common.required();

    match item {
        ItemSpecV1::Confirm(_) => CanonicalItemDtoV1::Confirm {
            id,
            prompt,
            help,
            required,
        },
        ItemSpecV1::Text(specification) => CanonicalItemDtoV1::Text {
            id,
            prompt,
            help,
            required,
            min_length: specification.min_length(),
            max_length: specification.max_length(),
            multiline: specification.multiline(),
        },
        ItemSpecV1::Choice(specification) => CanonicalItemDtoV1::Choice {
            id,
            prompt,
            help,
            required,
            choices: specification.choices(),
        },
        ItemSpecV1::Integer(specification) => CanonicalItemDtoV1::Integer {
            id,
            prompt,
            help,
            required,
            minimum: specification.minimum(),
            maximum: specification.maximum(),
        },
        ItemSpecV1::List(specification) => CanonicalItemDtoV1::List {
            id,
            prompt,
            help,
            required,
            min_items: specification.min_items(),
            max_items: specification.max_items(),
            max_item_length: specification.max_item_length(),
            unique: specification.unique(),
        },
        ItemSpecV1::Artifact(specification) => {
            let allowed_media_types = specification.allowed_media_types();
            CanonicalItemDtoV1::Artifact {
                id,
                prompt,
                help,
                required,
                allowed_media_types: (!allowed_media_types.is_empty())
                    .then_some(allowed_media_types),
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalProcedureDocumentV1 {
    schema: String,
    id: String,
    version: String,
    name: String,
    description: Option<String>,
    stages: Vec<CanonicalStageV1>,
    rework: CanonicalReworkPolicyV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalStageV1 {
    id: String,
    title: String,
    instructions: Vec<String>,
    items: Vec<CanonicalItemV1>,
    skip: Option<CanonicalSkipPolicyV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalSkipPolicyV1 {
    allowed: bool,
    reason_required: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalReworkPolicyV1 {
    allow_return_to: CanonicalReturnTargetsV1,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CanonicalReturnTargetsV1 {
    AnyPrevious(String),
    Only(Vec<String>),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum CanonicalItemV1 {
    Confirm {
        id: String,
        prompt: String,
        help: Option<String>,
        required: bool,
    },
    Text {
        id: String,
        prompt: String,
        help: Option<String>,
        required: bool,
        min_length: u32,
        max_length: u32,
        multiline: bool,
    },
    Choice {
        id: String,
        prompt: String,
        help: Option<String>,
        required: bool,
        choices: Vec<String>,
    },
    Integer {
        id: String,
        prompt: String,
        help: Option<String>,
        required: bool,
        minimum: Option<i64>,
        maximum: Option<i64>,
    },
    List {
        id: String,
        prompt: String,
        help: Option<String>,
        required: bool,
        min_items: u16,
        max_items: u16,
        max_item_length: u16,
        unique: bool,
    },
    Artifact {
        id: String,
        prompt: String,
        help: Option<String>,
        required: bool,
        allowed_media_types: Option<Vec<String>>,
    },
}

struct DecodedCanonicalProcedureV1 {
    schema: String,
    id: String,
    version: String,
    name: String,
    description: Option<String>,
    stages: Vec<StageSpecV1>,
    return_policy: ReturnPolicyV1,
}

fn contains_json_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(values) => values.iter().any(contains_json_null),
        Value::Object(values) => values.values().any(contains_json_null),
        Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}
fn decode_canonical_procedure(
    canonical_json: &CanonicalProcedureJsonV1,
) -> Result<DecodedCanonicalProcedureV1, DomainError> {
    verify_canonical_json_v1(canonical_json.as_str().as_bytes())
        .map_err(|_| invalid("canonical procedure JSON is not exact Podway Canonical JSON v1"))?;
    let value: Value = serde_json::from_str(canonical_json.as_str())
        .map_err(|_| invalid("invalid canonical procedure JSON"))?;
    if contains_json_null(&value) {
        return Err(invalid("canonical procedure JSON must not contain null"));
    }
    let CanonicalProcedureDocumentV1 {
        schema,
        id,
        version,
        name,
        description,
        stages,
        rework,
    } = serde_json::from_value(value).map_err(|_| invalid("invalid canonical procedure JSON"))?;

    if schema != PROCEDURE_SCHEMA_V1 {
        return Err(invalid(
            "canonical procedure JSON has an unsupported schema",
        ));
    }
    validate_procedure_identifier(&id)?;
    validate_text("procedure version", &version, 1, 64, false)?;
    validate_text("procedure name", &name, 1, 120, true)?;
    if let Some(description) = &description {
        validate_text("procedure description", description, 0, 4_000, false)?;
    }
    if stages.is_empty() || stages.len() > MAX_PROCEDURE_STAGES {
        return Err(invalid("procedure stage count must be between one and 64"));
    }

    let stages = stages
        .into_iter()
        .map(canonical_stage_to_core)
        .collect::<Result<Vec<_>, _>>()?;
    let mut stage_ids = BTreeSet::new();
    for stage in &stages {
        if !stage_ids.insert(stage.id()) {
            return Err(invalid("procedure stage identifiers must be unique"));
        }
    }

    let return_policy = canonical_return_policy_to_core(rework.allow_return_to)?;
    if let Some(destinations) = return_policy.destinations() {
        for destination in destinations {
            if !stage_ids.contains(destination) {
                return Err(invalid("return destination is not a procedure stage"));
            }
        }
    }

    Ok(DecodedCanonicalProcedureV1 {
        schema,
        id,
        version,
        name,
        description,
        stages,
        return_policy,
    })
}

fn canonical_stage_to_core(stage: CanonicalStageV1) -> Result<StageSpecV1, DomainError> {
    let items = stage
        .items
        .into_iter()
        .map(canonical_item_to_core)
        .collect::<Result<Vec<_>, _>>()?;
    let skip_policy = match stage.skip {
        Some(skip) if skip.allowed => SkipPolicyV1::new(true, skip.reason_required)?,
        Some(_) => {
            return Err(invalid(
                "canonical procedure JSON must omit disallowed skip policies",
            ));
        }
        None => SkipPolicyV1::not_allowed(),
    };
    StageSpecV1::new(
        StageId::new(stage.id)?,
        stage.title,
        stage.instructions,
        items,
        skip_policy,
    )
}

fn canonical_item_to_core(item: CanonicalItemV1) -> Result<ItemSpecV1, DomainError> {
    match item {
        CanonicalItemV1::Confirm {
            id,
            prompt,
            help,
            required,
        } => Ok(ItemSpecV1::confirm(canonical_item_common(
            id, prompt, help, required,
        )?)),
        CanonicalItemV1::Text {
            id,
            prompt,
            help,
            required,
            min_length,
            max_length,
            multiline,
        } => ItemSpecV1::text(
            canonical_item_common(id, prompt, help, required)?,
            min_length,
            max_length,
            multiline,
        ),
        CanonicalItemV1::Choice {
            id,
            prompt,
            help,
            required,
            choices,
        } => ItemSpecV1::choice(canonical_item_common(id, prompt, help, required)?, choices),
        CanonicalItemV1::Integer {
            id,
            prompt,
            help,
            required,
            minimum,
            maximum,
        } => ItemSpecV1::integer(
            canonical_item_common(id, prompt, help, required)?,
            minimum,
            maximum,
        ),
        CanonicalItemV1::List {
            id,
            prompt,
            help,
            required,
            min_items,
            max_items,
            max_item_length,
            unique,
        } => ItemSpecV1::list(
            canonical_item_common(id, prompt, help, required)?,
            min_items,
            max_items,
            max_item_length,
            unique,
        ),
        CanonicalItemV1::Artifact {
            id,
            prompt,
            help,
            required,
            allowed_media_types,
        } => {
            let allowed_media_types = match allowed_media_types {
                Some(values) if values.is_empty() => {
                    return Err(invalid(
                        "canonical procedure JSON must omit empty allowed media types",
                    ));
                }
                Some(values) => values,
                None => Vec::new(),
            };
            ItemSpecV1::artifact(
                canonical_item_common(id, prompt, help, required)?,
                allowed_media_types,
            )
        }
    }
}

fn canonical_item_common(
    id: String,
    prompt: String,
    help: Option<String>,
    required: bool,
) -> Result<ItemCommonV1, DomainError> {
    ItemCommonV1::new(crate::ItemId::new(id)?, prompt, help, required)
}

fn canonical_return_policy_to_core(
    return_targets: CanonicalReturnTargetsV1,
) -> Result<ReturnPolicyV1, DomainError> {
    match return_targets {
        CanonicalReturnTargetsV1::AnyPrevious(value) if value == "any_previous" => {
            Ok(ReturnPolicyV1::any_previous())
        }
        CanonicalReturnTargetsV1::AnyPrevious(_) => {
            Err(invalid("unknown canonical return target policy"))
        }
        CanonicalReturnTargetsV1::Only(stage_ids) => ReturnPolicyV1::only(
            stage_ids
                .into_iter()
                .map(StageId::new)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    }
}

fn derived_procedure_warning_codes(
    stages: &[StageSpecV1],
    return_policy: &ReturnPolicyV1,
) -> Vec<ProcedureWarningCodeV1> {
    procedure_warning_codes(stages, return_policy)
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn procedure_warning_codes(
    stages: &[StageSpecV1],
    return_policy: &ReturnPolicyV1,
) -> Vec<ProcedureWarningCodeV1> {
    let mut warnings = Vec::new();
    if is_near_limit(stages.len(), MAX_PROCEDURE_STAGES) {
        warnings.push(ProcedureWarningCodeV1::StageNearHardLimits);
    }
    if return_policy.destinations().is_none() {
        warnings.push(ProcedureWarningCodeV1::AnyPreviousReturnPolicy);
    }

    let mut prompts = BTreeSet::new();
    for stage in stages {
        if !stage.items().iter().any(|item| item.common().required()) {
            warnings.push(ProcedureWarningCodeV1::StageHasNoRequiredItems);
        }
        if is_near_limit(stage.instructions().len(), MAX_STAGE_INSTRUCTIONS)
            || is_near_limit(stage.items().len(), MAX_STAGE_ITEMS)
            || stage
                .instructions()
                .iter()
                .any(|instruction| is_near_limit(instruction.chars().count(), 2_000))
        {
            warnings.push(ProcedureWarningCodeV1::StageNearHardLimits);
        }
        for item in stage.items() {
            let common = item.common();
            if !prompts.insert(common.prompt()) {
                warnings.push(ProcedureWarningCodeV1::RepeatedPrompt);
            }
            if !common.required() && looks_required(common.prompt()) {
                warnings.push(ProcedureWarningCodeV1::OptionalItemAppearsRequired);
            }
        }
    }
    if stages
        .last()
        .is_some_and(|stage| stage.skip_policy().is_allowed())
    {
        warnings.push(ProcedureWarningCodeV1::FinalStageSkippable);
    }
    warnings
}

fn is_near_limit(actual: usize, limit: usize) -> bool {
    actual >= limit - (limit / 10).max(1)
}

fn looks_required(prompt: &str) -> bool {
    prompt
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| {
            let word = word.to_ascii_lowercase();
            matches!(word.as_str(), "required" | "must" | "mandatory" | "needed")
        })
}
pub(crate) fn validate_media_type(value: &str) -> Result<(), DomainError> {
    if value.len() > 255 {
        return Err(invalid("media type exceeds 255 bytes"));
    }
    let Some((kind, subtype)) = value.split_once('/') else {
        return Err(invalid("media type must contain exactly one slash"));
    };
    if kind.is_empty() || subtype.is_empty() || subtype.contains('/') {
        return Err(invalid("media type must contain a type and subtype"));
    }
    if !kind
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !subtype
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !kind.bytes().all(is_media_token)
        || !subtype.bytes().all(is_media_token)
    {
        return Err(invalid(
            "media type must be lowercase ASCII without parameters",
        ));
    }
    Ok(())
}

fn is_media_token(byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
        )
}

fn validate_procedure_identifier(value: &str) -> Result<(), DomainError> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return Err(invalid("procedure identifier must not be empty"));
    }
    if bytes.len() > crate::MAX_PROCEDURE_IDENTIFIER_BYTES {
        return Err(invalid("procedure identifier exceeds 64 bytes"));
    }
    if !bytes[0].is_ascii_lowercase() || bytes.last() == Some(&b'-') {
        return Err(invalid("procedure identifier must be lowercase kebab-case"));
    }
    let mut previous_hyphen = false;
    for byte in bytes.iter().copied() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => previous_hyphen = false,
            b'-' if !previous_hyphen => previous_hyphen = true,
            _ => return Err(invalid("procedure identifier must be lowercase kebab-case")),
        }
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
    require_non_whitespace: bool,
) -> Result<(), DomainError> {
    let actual = value.chars().count();
    if actual < minimum || actual > maximum {
        return Err(invalid(field));
    }
    if require_non_whitespace && value.trim().is_empty() {
        return Err(invalid(field));
    }
    Ok(())
}

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}
#[cfg(test)]
mod tests {
    use super::CanonicalProcedureJsonV1;
    use crate::{DomainError, MAX_PROCEDURE_DOCUMENT_BYTES_V1};

    #[test]
    fn canonical_procedure_json_enforces_empty_and_exact_byte_bounds() {
        assert_eq!(
            CanonicalProcedureJsonV1::new(""),
            Err(DomainError::InvalidState {
                reason: "canonical procedure JSON must not be empty",
            })
        );

        let exact = "x".repeat(MAX_PROCEDURE_DOCUMENT_BYTES_V1);
        let exact_document = CanonicalProcedureJsonV1::new(exact.clone()).unwrap();
        assert_eq!(exact_document.as_str(), exact);

        assert_eq!(
            CanonicalProcedureJsonV1::new("x".repeat(MAX_PROCEDURE_DOCUMENT_BYTES_V1 + 1)),
            Err(DomainError::ValueTooLong {
                field: "canonical procedure JSON",
                maximum: MAX_PROCEDURE_DOCUMENT_BYTES_V1,
                actual: MAX_PROCEDURE_DOCUMENT_BYTES_V1 + 1,
            })
        );
    }
}
