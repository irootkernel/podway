#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use podway_core::{CanonicalJsonErrorV1, Sha256Digest, canonicalize_json_v1};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

mod parser;
mod procedure_v2_authoring;
mod procedure_v2_budget;
mod procedure_v2_canonical;
mod procedure_v2_check;
mod procedure_v2_delegation_package;
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

pub use parser::{
    MAX_PROCEDURE_DOCUMENT_BYTES, MAX_PROCEDURE_DOCUMENT_DEPTH, MAX_PROCEDURE_DOCUMENT_NODES,
    MAX_WORKSPACE_CONFIG_BYTES_V1, MAX_WORKSPACE_CONFIG_DEPTH_V1, MAX_WORKSPACE_CONFIG_NODES_V1,
    ParsedProcedure, ProcedureDocumentFormat, ProcedureDocumentLimits,
    WorkspaceConfigParseLimitsV1, decode_procedure_document, decode_procedure_document_with_limits,
    parse_procedure_document, parse_procedure_yaml, parse_workspace_config_v1,
    parse_workspace_config_v1_with_limits, sniff_procedure_schema,
};
pub use procedure_v2_budget::{
    DECISION_RECORD_BUDGET, EVIDENCE_PREVIEW_BUDGET, NEXT_FRAME_BUDGET, NEXT_SERIALIZATION_RESERVE,
    NEXT_STATIC_BUDGET, PAGE_TOKEN_BUDGET, ProcedurePlacementBudgetV2, READBACK_BUDGET,
    procedure_placement_budget_v2,
};
pub use procedure_v2_check::{ProcedureCheckReport, check_procedure_v2};
pub use procedure_v2_delegation_package::{
    DelegationEvidenceItemV1, DelegationEvidenceResolutionV1, DelegationEvidenceV1,
    DelegationWorkPackageInputV1, DelegationWorkPackageV1, project_delegation_work_package_v1,
};
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

pub const WORKSPACE_SCHEMA_V1: &str = "podway.workspace/v1";
pub const WORKSPACE_SCHEMA_V2: &str = "podway.workspace/v2";
/// The complete, explicit v1 workspace configuration written for a new workspace.
pub const DEFAULT_WORKSPACE_CONFIG_YAML_V1: &[u8] = b"schema: podway.workspace/v1\nprocedure_paths:\n  - .podway/procedures\ndefault_preset: sw-dev-v2\nui:\n  show_stage_in_prompt: false\n";

const DEFAULT_PRESET: &str = "sw-dev-v2";

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
    #[error("invalid {field} value `{value}`: {reason}")]
    InvalidScalar {
        field: &'static str,
        value: String,
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
    #[error("core procedure admission failed: {reason}")]
    CoreAdmission { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigV1 {
    pub schema: String,
    #[serde(default, skip_serializing)]
    pub mode: Option<String>,
    #[serde(default = "default_procedure_paths")]
    pub procedure_paths: Vec<String>,
    #[serde(default = "default_preset")]
    pub default_preset: String,
    /// Accepted legacy configuration; it has no effect and is never written.
    #[serde(default, skip_serializing)]
    pub job_queue: Option<serde_json::Value>,
    #[serde(default)]
    pub ui: UiConfigV1,
}

impl Default for WorkspaceConfigV1 {
    fn default() -> Self {
        Self {
            schema: WORKSPACE_SCHEMA_V1.to_owned(),
            mode: None,
            procedure_paths: default_procedure_paths(),
            default_preset: default_preset(),
            job_queue: None,
            ui: UiConfigV1::default(),
        }
    }
}

impl WorkspaceConfigV1 {
    pub fn new(
        procedure_paths: Vec<String>,
        default_preset: impl Into<String>,
        ui: UiConfigV1,
    ) -> Result<Self, ConfigError> {
        let config = Self {
            schema: WORKSPACE_SCHEMA_V1.to_owned(),
            mode: None,
            procedure_paths,
            default_preset: default_preset.into(),
            job_queue: None,
            ui,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.schema.as_str() {
            WORKSPACE_SCHEMA_V1 if self.mode.is_none() => {}
            WORKSPACE_SCHEMA_V1 => {
                return Err(ConfigError::InvalidValue {
                    field: "mode",
                    reason: "requires workspace schema v2",
                });
            }
            WORKSPACE_SCHEMA_V2 if self.mode.as_deref().is_none_or(|mode| mode == "prod") => {}
            WORKSPACE_SCHEMA_V2 => {
                return Err(ConfigError::InvalidValue {
                    field: "mode",
                    reason: "retired runtime mode; remove mode and reset the workspace",
                });
            }
            _ => {
                return Err(ConfigError::InvalidSchema {
                    expected: "podway.workspace/v1 or podway.workspace/v2",
                    actual: self.schema.clone(),
                });
            }
        }
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
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfigV1 {
    #[serde(default)]
    pub show_stage_in_prompt: bool,
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

fn default_procedure_paths() -> Vec<String> {
    vec![".podway/procedures".to_owned()]
}

fn default_preset() -> String {
    DEFAULT_PRESET.to_owned()
}
