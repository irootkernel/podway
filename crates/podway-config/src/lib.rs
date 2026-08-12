#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use podway_core::{
    CanonicalJsonErrorV1, ProcedureWarningCodeV1, Sha256Digest, canonicalize_json_v1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

mod parser;
mod procedure_v2_authoring;
mod procedure_v2_budget;
mod procedure_v2_canonical;
mod procedure_v2_check;
mod procedure_v2_convert;
mod procedure_v2_diagnostics;
mod procedure_v2_document;
mod procedure_v2_dot_projection;
mod procedure_v2_format;
mod procedure_v2_graph;
mod procedure_v2_graph_projection;
mod procedure_v2_lint;
mod procedure_v2_mermaid_projection;
mod procedure_v2_parse;
mod procedure_v2_plantuml_projection;
mod procedure_v2_preview;
mod procedure_v2_scaffold;
mod procedure_v2_source;
mod procedure_v2_validate;
mod procedure_v2_vet;
mod procedure_v2_wire;
mod validation;

pub use parser::{
    MAX_PROCEDURE_DOCUMENT_BYTES, MAX_PROCEDURE_DOCUMENT_BYTES_V1, MAX_PROCEDURE_DOCUMENT_DEPTH,
    MAX_PROCEDURE_DOCUMENT_DEPTH_V1, MAX_PROCEDURE_DOCUMENT_NODES, MAX_PROCEDURE_DOCUMENT_NODES_V1,
    MAX_WORKSPACE_CONFIG_BYTES_V1, MAX_WORKSPACE_CONFIG_DEPTH_V1, MAX_WORKSPACE_CONFIG_NODES_V1,
    ParsedProcedure, ProcedureDocumentFormat, ProcedureDocumentLimits, ProcedureFormatV1,
    ProcedureParseLimitsV1, WorkspaceConfigParseLimitsV1, decode_procedure_document,
    decode_procedure_document_with_limits, parse_procedure_document, parse_procedure_v1,
    parse_procedure_v1_with_limits, parse_procedure_yaml, parse_workspace_config_v1,
    parse_workspace_config_v1_with_limits, sniff_procedure_schema,
};
pub use procedure_v2_budget::{
    NEXT_STATIC_BUDGET, ProcedurePlacementBudgetV2, READBACK_BUDGET, procedure_placement_budget_v2,
};
pub use procedure_v2_check::{ProcedureCheckReport, check_procedure_v2};
pub use procedure_v2_convert::{ConvertedProcedureV2, convert_procedure_v1_to_v2};
pub use procedure_v2_diagnostics::{
    AuthoringContext, AuthoringStage, FinalizedDiagnostics, config_error_diagnostic,
    finalize_diagnostics,
};
pub use procedure_v2_dot_projection::{ProcedureDotProjectionV2, project_procedure_v2_dot};
pub use procedure_v2_format::{
    FormatFailure, FormatRequest, FormattedProcedureV2, format_procedure_v2,
};
pub use procedure_v2_graph::goal_revision_safe_targets_v2;
pub use procedure_v2_graph_projection::{
    GraphProjectionNodeTypeV2, ProcedureGraphEdgeV2, ProcedureGraphModelV2, ProcedureGraphNodeV2,
    ProcedureGraphProjectionV2, normalize_procedure_v2_graph, project_procedure_v2_graph,
};
pub use procedure_v2_lint::lint_procedure_v2;
pub use procedure_v2_mermaid_projection::{
    ProcedureMermaidProjectionV2, project_procedure_v2_mermaid,
};
pub use procedure_v2_parse::{ParsedNodeDefinition, ParsedProcedureV2};
pub use procedure_v2_plantuml_projection::{
    ProcedurePlantUmlProjectionV2, project_procedure_v2_plantuml,
};
pub use procedure_v2_preview::{
    ProcedurePreviewChecksV2, ProcedurePreviewDetailsV2, ProcedurePreviewGraphEdgeV2,
    ProcedurePreviewGraphNodeV2, ProcedurePreviewGraphV2, ProcedurePreviewReportV2,
    ProcedurePreviewStartSuggestionV2, ProcedurePreviewSummaryV2, preview_procedure_v2,
};
pub use procedure_v2_scaffold::{
    SCAFFOLD_TEMPLATE_MINIMAL, ScaffoldTemplate, scaffold_procedure_v2,
};
pub use procedure_v2_validate::{ValidatedProcedureV2, validate_procedure_v2};
pub use procedure_v2_vet::vet_procedure_v2;
pub use validation::{
    IntoProcedureSnapshotSourceV1, ProcedureWarningPolicyV1, ProcedureWarningV1,
    ValidatedProcedureV1,
};

pub const WORKSPACE_SCHEMA_V1: &str = "podway.workspace/v1";
pub const PROCEDURE_SCHEMA_V1: &str = "podway.procedure/v1";
/// The complete, explicit v1 workspace configuration written for a new workspace.
pub const DEFAULT_WORKSPACE_CONFIG_YAML_V1: &[u8] = b"schema: podway.workspace/v1\nprocedure_paths:\n  - .podway/procedures\ndefault_preset: sw-dev\njob_queue:\n  max_pending: 256\nui:\n  show_stage_in_prompt: false\n";

const DEFAULT_PRESET: &str = "sw-dev";
const DEFAULT_MAX_PENDING: u16 = 256;
const DEFAULT_TEXT_MAX_LENGTH: u32 = 8_000;
const DEFAULT_LIST_MAX_ITEMS: u16 = 100;
const DEFAULT_LIST_MAX_ITEM_LENGTH: u16 = 500;

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ConfigError {
    #[error("unsupported schema `{actual}`; expected `{expected}`")]
    InvalidSchema {
        expected: &'static str,
        actual: String,
    },
    #[error("invalid {field} identifier `{value}`")]
    InvalidIdentifier { field: &'static str, value: String },
    #[error("invalid {field}: {reason}")]
    InvalidValue {
        field: &'static str,
        reason: &'static str,
    },
    #[error("{field} length or count {actual} is outside {min}..={max}")]
    OutOfBounds {
        field: &'static str,
        min: usize,
        max: usize,
        actual: usize,
    },
    #[error("duplicate {field} value `{value}`")]
    DuplicateValue { field: &'static str, value: String },
    #[error("return target `{stage_id}` is not a procedure stage")]
    UnknownReturnTarget { stage_id: String },
    /// A Procedure v2 closed reference names a declaration the same document does not make
    /// (dossier section 11.2). `field` is a static authored path; `value` is the offending
    /// identifier.
    #[error("unknown procedure v2 {field} reference `{value}`")]
    UnknownV2Reference { field: &'static str, value: String },
    /// A Procedure v2 declaration is internally inconsistent with the declaration set it resolves
    /// against — a placement/definition kind disagreement, a route table that does not match its
    /// definition's option set, or an assessment contract that does not fit its definition or its
    /// procedure (dossier section 11.2).
    #[error("invalid procedure v2 {field}: {reason}")]
    V2ShapeMismatch {
        field: &'static str,
        reason: &'static str,
    },
    #[error("canonical JSON serialization failed: {0}")]
    Serialization(String),
    #[error("canonical JSON cannot contain a non-integer number")]
    NonCanonicalNumber,
    #[error("generated digest is not a valid SHA-256 digest")]
    InvalidDigest,
    #[error("procedure input exceeds the maximum of {maximum} bytes (received {actual})")]
    InputTooLarge { maximum: usize, actual: usize },
    #[error("procedure input exceeds the maximum depth of {maximum} (received {actual})")]
    InputTooDeep { maximum: usize, actual: usize },
    #[error("procedure input exceeds the maximum of {maximum} nodes (received {actual})")]
    InputTooComplex { maximum: usize, actual: usize },
    #[error("duplicate mapping key `{key}`")]
    DuplicateKey { key: String },
    #[error("unsupported YAML feature `{feature}`")]
    UnsupportedYamlFeature { feature: &'static str },
    #[error("invalid procedure document: {reason}")]
    InvalidDocument { reason: String },
    #[error("procedure warnings are errors: {warnings:?}")]
    WarningsAsErrors {
        warnings: Vec<ProcedureWarningCodeV1>,
    },
    #[error("core procedure admission failed: {reason}")]
    CoreAdmission { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigV1 {
    pub schema: String,
    #[serde(default = "default_procedure_paths")]
    pub procedure_paths: Vec<String>,
    #[serde(default = "default_preset")]
    pub default_preset: String,
    #[serde(default)]
    pub job_queue: JobQueueConfigV1,
    #[serde(default)]
    pub ui: UiConfigV1,
}

impl Default for WorkspaceConfigV1 {
    fn default() -> Self {
        Self {
            schema: WORKSPACE_SCHEMA_V1.to_owned(),
            procedure_paths: default_procedure_paths(),
            default_preset: default_preset(),
            job_queue: JobQueueConfigV1::default(),
            ui: UiConfigV1::default(),
        }
    }
}

impl WorkspaceConfigV1 {
    pub fn new(
        procedure_paths: Vec<String>,
        default_preset: impl Into<String>,
        job_queue: JobQueueConfigV1,
        ui: UiConfigV1,
    ) -> Result<Self, ConfigError> {
        let config = Self {
            schema: WORKSPACE_SCHEMA_V1.to_owned(),
            procedure_paths,
            default_preset: default_preset.into(),
            job_queue,
            ui,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_schema(&self.schema, WORKSPACE_SCHEMA_V1)?;
        validate_count("procedure_paths", self.procedure_paths.len(), 1, 16)?;

        let mut paths = BTreeSet::new();
        for path in &self.procedure_paths {
            validate_safe_relative_path("procedure_paths", path)?;
            if !paths.insert(path) {
                return Err(ConfigError::DuplicateValue {
                    field: "procedure_paths",
                    value: path.clone(),
                });
            }
        }

        validate_identifier("default_preset", &self.default_preset)?;
        self.job_queue.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobQueueConfigV1 {
    #[serde(default = "default_max_pending")]
    pub max_pending: u16,
}

impl Default for JobQueueConfigV1 {
    fn default() -> Self {
        Self {
            max_pending: DEFAULT_MAX_PENDING,
        }
    }
}

impl JobQueueConfigV1 {
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_count("job_queue.max_pending", self.max_pending as usize, 1, 4_096)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfigV1 {
    #[serde(default)]
    pub show_stage_in_prompt: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureDefinitionV1 {
    pub schema: String,
    pub id: String,
    pub version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub stages: Vec<StageDefinitionV1>,
    pub rework: ReworkPolicyV1,
}

impl ProcedureDefinitionV1 {
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        name: impl Into<String>,
        stages: Vec<StageDefinitionV1>,
        rework: ReworkPolicyV1,
    ) -> Result<Self, ConfigError> {
        let definition = Self {
            schema: PROCEDURE_SCHEMA_V1.to_owned(),
            id: id.into(),
            version: version.into(),
            name: name.into(),
            description: None,
            stages,
            rework,
        };
        definition.validate()?;
        Ok(definition)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_schema(&self.schema, PROCEDURE_SCHEMA_V1)?;
        validate_identifier("procedure.id", &self.id)?;
        validate_text("procedure.version", &self.version, 1, 64, false)?;
        validate_text("procedure.name", &self.name, 1, 120, true)?;
        if let Some(description) = &self.description {
            validate_text("procedure.description", description, 0, 4_000, false)?;
        }
        validate_count("procedure.stages", self.stages.len(), 1, 64)?;

        let mut stage_ids = BTreeSet::new();
        for stage in &self.stages {
            stage.validate()?;
            if !stage_ids.insert(stage.id.as_str()) {
                return Err(ConfigError::DuplicateValue {
                    field: "stage.id",
                    value: stage.id.clone(),
                });
            }
        }
        self.rework.validate(&stage_ids)
    }

    pub(crate) fn apply_documented_defaults(&mut self) {
        for stage in &mut self.stages {
            match &mut stage.skip {
                Some(skip) if skip.allowed && skip.reason_required.is_none() => {
                    skip.reason_required = Some(true);
                }
                _ => {}
            }
        }
    }

    fn normalized_for_canonical_json(&self) -> Self {
        let mut normalized = self.clone();
        normalized.apply_documented_defaults();
        for stage in &mut normalized.stages {
            if stage.skip.as_ref().is_some_and(|skip| !skip.allowed) {
                stage.skip = None;
            }
            for item in &mut stage.items {
                match item {
                    ItemDefinitionV1::Artifact {
                        allowed_media_types,
                        ..
                    } if allowed_media_types.as_ref().is_some_and(Vec::is_empty) => {
                        *allowed_media_types = None;
                    }
                    _ => {}
                }
            }
        }
        normalized
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageDefinitionV1 {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub items: Vec<ItemDefinitionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<SkipPolicyV1>,
}

impl StageDefinitionV1 {
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_identifier("stage.id", &self.id)?;
        validate_text("stage.title", &self.title, 1, 120, true)?;
        validate_count("stage.instructions", self.instructions.len(), 0, 32)?;
        for instruction in &self.instructions {
            validate_text("stage.instructions", instruction, 1, 2_000, true)?;
        }
        validate_count("stage.items", self.items.len(), 0, 128)?;

        let mut item_ids = BTreeSet::new();
        for item in &self.items {
            item.validate()?;
            if !item_ids.insert(item.id()) {
                return Err(ConfigError::DuplicateValue {
                    field: "item.id",
                    value: item.id().to_owned(),
                });
            }
        }

        if let Some(skip) = &self.skip {
            skip.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkipPolicyV1 {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_required: Option<bool>,
}

impl SkipPolicyV1 {
    pub fn allowed(reason_required: bool) -> Self {
        Self {
            allowed: true,
            reason_required: Some(reason_required),
        }
    }

    pub fn disallowed() -> Self {
        Self {
            allowed: false,
            reason_required: None,
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.allowed && self.reason_required.is_some() {
            return Err(ConfigError::InvalidValue {
                field: "stage.skip.reason_required",
                reason: "must be absent when skipping is disabled",
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReworkPolicyV1 {
    pub allow_return_to: ReturnTargetsV1,
}

impl ReworkPolicyV1 {
    pub fn any_previous() -> Self {
        Self {
            allow_return_to: ReturnTargetsV1::AnyPrevious,
        }
    }

    pub fn only(stage_ids: Vec<String>) -> Result<Self, ConfigError> {
        let policy = Self {
            allow_return_to: ReturnTargetsV1::Only(stage_ids),
        };
        policy.validate(&BTreeSet::new())?;
        Ok(policy)
    }

    fn validate(&self, known_stages: &BTreeSet<&str>) -> Result<(), ConfigError> {
        match &self.allow_return_to {
            ReturnTargetsV1::AnyPrevious => Ok(()),
            ReturnTargetsV1::Only(stage_ids) => {
                validate_count("rework.allow_return_to", stage_ids.len(), 1, 64)?;
                let mut seen = BTreeSet::new();
                for stage_id in stage_ids {
                    validate_identifier("rework.allow_return_to", stage_id)?;
                    if !seen.insert(stage_id.as_str()) {
                        return Err(ConfigError::DuplicateValue {
                            field: "rework.allow_return_to",
                            value: stage_id.clone(),
                        });
                    }
                    if !known_stages.is_empty() && !known_stages.contains(stage_id.as_str()) {
                        return Err(ConfigError::UnknownReturnTarget {
                            stage_id: stage_id.clone(),
                        });
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReturnTargetsV1 {
    AnyPrevious,
    Only(Vec<String>),
}

impl Serialize for ReturnTargetsV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::AnyPrevious => serializer.serialize_str("any_previous"),
            Self::Only(stage_ids) => stage_ids.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ReturnTargetsV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawReturnTargets {
            Keyword(String),
            Only(Vec<String>),
        }

        match RawReturnTargets::deserialize(deserializer)? {
            RawReturnTargets::Keyword(keyword) if keyword == "any_previous" => {
                Ok(Self::AnyPrevious)
            }
            RawReturnTargets::Keyword(keyword) => Err(serde::de::Error::custom(format!(
                "unsupported return target policy `{keyword}`"
            ))),
            RawReturnTargets::Only(stage_ids) => Ok(Self::Only(stage_ids)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ItemDefinitionV1 {
    Confirm {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
        required: bool,
    },
    Text {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
        required: bool,
        #[serde(default)]
        min_length: u32,
        #[serde(default = "default_text_max_length")]
        max_length: u32,
        #[serde(default = "default_multiline")]
        multiline: bool,
    },
    Choice {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
        required: bool,
        choices: Vec<String>,
    },
    Integer {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimum: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        maximum: Option<i64>,
    },
    List {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
        required: bool,
        #[serde(default)]
        min_items: u16,
        #[serde(default = "default_list_max_items")]
        max_items: u16,
        #[serde(default = "default_list_max_item_length")]
        max_item_length: u16,
        #[serde(default = "default_unique")]
        unique: bool,
    },
    Artifact {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allowed_media_types: Option<Vec<String>>,
    },
}

impl ItemDefinitionV1 {
    pub fn id(&self) -> &str {
        match self {
            Self::Confirm { id, .. }
            | Self::Text { id, .. }
            | Self::Choice { id, .. }
            | Self::Integer { id, .. }
            | Self::List { id, .. }
            | Self::Artifact { id, .. } => id,
        }
    }

    pub fn required(&self) -> bool {
        match self {
            Self::Confirm { required, .. }
            | Self::Text { required, .. }
            | Self::Choice { required, .. }
            | Self::Integer { required, .. }
            | Self::List { required, .. }
            | Self::Artifact { required, .. } => *required,
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let (id, prompt, help) = match self {
            Self::Confirm {
                id, prompt, help, ..
            }
            | Self::Text {
                id, prompt, help, ..
            }
            | Self::Choice {
                id, prompt, help, ..
            }
            | Self::Integer {
                id, prompt, help, ..
            }
            | Self::List {
                id, prompt, help, ..
            }
            | Self::Artifact {
                id, prompt, help, ..
            } => (id, prompt, help),
        };
        validate_identifier("item.id", id)?;
        validate_text("item.prompt", prompt, 1, 500, true)?;
        if let Some(help) = help {
            validate_text("item.help", help, 0, 4_000, false)?;
        }

        match self {
            Self::Confirm { .. } => Ok(()),
            Self::Text {
                min_length,
                max_length,
                ..
            } => {
                if min_length > max_length {
                    return Err(ConfigError::InvalidValue {
                        field: "item.text.length",
                        reason: "minimum cannot exceed maximum",
                    });
                }
                validate_count("item.text.max_length", *max_length as usize, 0, 65_536)
            }
            Self::Choice { choices, .. } => {
                validate_count("item.choice.choices", choices.len(), 1, 64)?;
                let mut seen = BTreeSet::new();
                for choice in choices {
                    validate_text("item.choice.choices", choice, 1, 120, true)?;
                    if !seen.insert(choice) {
                        return Err(ConfigError::DuplicateValue {
                            field: "item.choice.choices",
                            value: choice.clone(),
                        });
                    }
                }
                Ok(())
            }
            Self::Integer {
                minimum, maximum, ..
            } => {
                match (minimum, maximum) {
                    (Some(minimum), Some(maximum)) if minimum > maximum => {
                        return Err(ConfigError::InvalidValue {
                            field: "item.integer.bounds",
                            reason: "minimum cannot exceed maximum",
                        });
                    }
                    _ => {}
                }
                Ok(())
            }
            Self::List {
                min_items,
                max_items,
                max_item_length,
                ..
            } => {
                if min_items > max_items {
                    return Err(ConfigError::InvalidValue {
                        field: "item.list.item_count",
                        reason: "minimum cannot exceed maximum",
                    });
                }
                validate_count("item.list.max_items", *max_items as usize, 1, 1_000)?;
                validate_count(
                    "item.list.max_item_length",
                    *max_item_length as usize,
                    1,
                    4_000,
                )
            }
            Self::Artifact {
                allowed_media_types,
                ..
            } => {
                if let Some(media_types) = allowed_media_types {
                    validate_count(
                        "item.artifact.allowed_media_types",
                        media_types.len(),
                        0,
                        64,
                    )?;
                    let mut seen = BTreeSet::new();
                    for media_type in media_types {
                        validate_media_type(media_type)?;
                        if !seen.insert(media_type) {
                            return Err(ConfigError::DuplicateValue {
                                field: "item.artifact.allowed_media_types",
                                value: media_type.clone(),
                            });
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureSourceKind {
    Preset,
    WorkspacePath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProcedureSourceLabel {
    kind: ProcedureSourceKind,
    label: String,
}

impl ProcedureSourceLabel {
    pub fn preset(id: impl Into<String>) -> Result<Self, ConfigError> {
        let label = id.into();
        validate_identifier("preset source", &label)?;
        Ok(Self {
            kind: ProcedureSourceKind::Preset,
            label,
        })
    }

    pub fn workspace_path(path: impl Into<String>) -> Result<Self, ConfigError> {
        let label = path.into();
        validate_safe_relative_path("procedure source path", &label)?;
        Ok(Self {
            kind: ProcedureSourceKind::WorkspacePath,
            label,
        })
    }

    pub const fn kind(&self) -> ProcedureSourceKind {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn display_label(&self) -> String {
        match self.kind {
            ProcedureSourceKind::Preset => format!("preset:{}", self.label),
            ProcedureSourceKind::WorkspacePath => format!("procedure:{}", self.label),
        }
    }
}

impl<'de> Deserialize<'de> for ProcedureSourceLabel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawProcedureSourceLabel {
            kind: ProcedureSourceKind,
            label: String,
        }

        let raw = RawProcedureSourceLabel::deserialize(deserializer)?;
        match raw.kind {
            ProcedureSourceKind::Preset => Self::preset(raw.label),
            ProcedureSourceKind::WorkspacePath => Self::workspace_path(raw.label),
        }
        .map_err(serde::de::Error::custom)
    }
}

/// A config-produced canonical JSON document.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CanonicalJsonV1(String);

impl CanonicalJsonV1 {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

/// Produces the config-owned canonical representation for validated values.
pub trait CanonicalJson {
    fn canonical_json_v1(&self) -> Result<CanonicalJsonV1, ConfigError>;
}

/// Produces a SHA-256 digest from the config-owned canonical representation.
pub trait CanonicalDigest: CanonicalJson {
    fn canonical_digest_v1(&self) -> Result<Sha256Digest, ConfigError> {
        let canonical_json = self.canonical_json_v1()?;
        let digest = Sha256::digest(canonical_json.as_bytes());
        Sha256Digest::new(format!("sha256:{digest:x}")).map_err(|_| ConfigError::InvalidDigest)
    }
}

impl<T: CanonicalJson + ?Sized> CanonicalDigest for T {}

impl CanonicalJson for WorkspaceConfigV1 {
    fn canonical_json_v1(&self) -> Result<CanonicalJsonV1, ConfigError> {
        self.validate()?;
        canonical_json_from_serializable(self)
    }
}

impl CanonicalJson for ProcedureDefinitionV1 {
    fn canonical_json_v1(&self) -> Result<CanonicalJsonV1, ConfigError> {
        self.validate()?;
        canonical_json_from_serializable(&self.normalized_for_canonical_json())
    }
}

/// Uses core's dependency-free deterministic primitive without transferring configuration
/// validation, normalization, or digest-production ownership out of this crate.
fn canonical_json_from_serializable<T: Serialize>(
    value: &T,
) -> Result<CanonicalJsonV1, ConfigError> {
    let canonical = canonicalize_json_v1(value).map_err(|error| match error {
        CanonicalJsonErrorV1::UnsupportedNumber => ConfigError::NonCanonicalNumber,
        _ => ConfigError::Serialization(error.to_string()),
    })?;
    Ok(CanonicalJsonV1(canonical))
}

fn validate_schema(actual: &str, expected: &'static str) -> Result<(), ConfigError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ConfigError::InvalidSchema {
            expected,
            actual: actual.to_owned(),
        })
    }
}

pub(crate) fn validate_identifier(field: &'static str, value: &str) -> Result<(), ConfigError> {
    validate_count(field, value.len(), 1, 64)?;
    let bytes = value.as_bytes();
    let valid = bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes.last().is_some_and(|byte| *byte != b'-')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && !bytes.windows(2).any(|window| window == b"--");
    if valid {
        Ok(())
    } else {
        Err(ConfigError::InvalidIdentifier {
            field,
            value: value.to_owned(),
        })
    }
}

pub(crate) fn validate_text(
    field: &'static str,
    value: &str,
    min: usize,
    max: usize,
    require_non_whitespace: bool,
) -> Result<(), ConfigError> {
    validate_count(field, value.chars().count(), min, max)?;
    if require_non_whitespace && value.trim().is_empty() {
        return validate_count(field, 0, min, max);
    }
    Ok(())
}

pub(crate) fn validate_count(
    field: &'static str,
    actual: usize,
    min: usize,
    max: usize,
) -> Result<(), ConfigError> {
    if (min..=max).contains(&actual) {
        Ok(())
    } else {
        Err(ConfigError::OutOfBounds {
            field,
            min,
            max,
            actual,
        })
    }
}

fn validate_safe_relative_path(field: &'static str, path: &str) -> Result<(), ConfigError> {
    validate_count(field, path.chars().count(), 1, 1_024)?;
    if path.trim().is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
        || path.contains(':')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ConfigError::InvalidValue {
            field,
            reason: "must be a normalized relative path",
        });
    }
    Ok(())
}

fn validate_media_type(media_type: &str) -> Result<(), ConfigError> {
    validate_count(
        "item.artifact.allowed_media_types",
        media_type.len(),
        1,
        255,
    )?;
    let Some((kind, subtype)) = media_type.split_once('/') else {
        return Err(ConfigError::InvalidValue {
            field: "item.artifact.allowed_media_types",
            reason: "must be a lowercase type/subtype value",
        });
    };
    if subtype.contains('/') || !is_media_token(kind) || !is_media_token(subtype) {
        return Err(ConfigError::InvalidValue {
            field: "item.artifact.allowed_media_types",
            reason: "must be a lowercase type/subtype value",
        });
    }
    Ok(())
}

fn is_media_token(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(
                    *byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
}

fn default_procedure_paths() -> Vec<String> {
    vec![".podway/procedures".to_owned()]
}

fn default_preset() -> String {
    DEFAULT_PRESET.to_owned()
}

const fn default_max_pending() -> u16 {
    DEFAULT_MAX_PENDING
}

const fn default_text_max_length() -> u32 {
    DEFAULT_TEXT_MAX_LENGTH
}

const fn default_multiline() -> bool {
    true
}

const fn default_list_max_items() -> u16 {
    DEFAULT_LIST_MAX_ITEMS
}

const fn default_list_max_item_length() -> u16 {
    DEFAULT_LIST_MAX_ITEM_LENGTH
}

const fn default_unique() -> bool {
    true
}
