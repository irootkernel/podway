#![forbid(unsafe_code)]

use std::{error::Error, fmt};

use podway_config::{
    CanonicalJsonV1, ConfigError, PROCEDURE_SCHEMA_V1, ProcedureDefinitionV1, ProcedureFormatV1,
    ProcedureSourceLabel, ProcedureWarningPolicyV1, ValidatedProcedureV1, parse_procedure_v1,
};
use podway_core::{ProcedureSnapshotId, ProcedureSnapshotV1, UnixMillis};
use serde::Serialize;

pub use podway_core::Sha256Digest;

pub const ANALYSIS_YAML: &str = include_str!("../../../presets/analysis.yaml");
pub const BUG_FIX_YAML: &str = include_str!("../../../presets/bug-fix.yaml");
pub const DOCS_ONLY_YAML: &str = include_str!("../../../presets/docs-only.yaml");
pub const SW_DEV_YAML: &str = include_str!("../../../presets/sw-dev.yaml");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PresetMetadata {
    pub schema: &'static str,
    pub id: &'static str,
    pub version: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

impl PresetMetadata {
    pub fn source_label(self) -> Result<ProcedureSourceLabel, ConfigError> {
        ProcedureSourceLabel::preset(self.id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddedPreset {
    pub metadata: PresetMetadata,
    pub yaml: &'static str,
}

impl EmbeddedPreset {
    /// Returns the exact UTF-8 bytes embedded from the root preset source file.
    pub fn source_bytes(self) -> &'static [u8] {
        self.yaml.as_bytes()
    }

    /// Uses the public procedure parser and validation path to admit this preset's YAML.
    pub fn validate(self) -> Result<ValidatedPresetV1, PresetError> {
        let procedure = parse_procedure_v1(self.source_bytes(), ProcedureFormatV1::Yaml)
            .map_err(|error| PresetError::admission(self.metadata.id, error))?;
        validate_metadata(self.metadata, procedure.definition())?;
        Ok(ValidatedPresetV1 {
            preset: self,
            procedure,
        })
    }
}

/// A preset admitted by the public configuration parser without preset-specific normalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPresetV1 {
    preset: EmbeddedPreset,
    procedure: ValidatedProcedureV1,
}

impl ValidatedPresetV1 {
    pub const fn preset(&self) -> EmbeddedPreset {
        self.preset
    }

    pub const fn metadata(&self) -> PresetMetadata {
        self.preset.metadata
    }

    pub fn definition(&self) -> &ProcedureDefinitionV1 {
        self.procedure.definition()
    }

    pub fn canonical_json(&self) -> &CanonicalJsonV1 {
        self.procedure.canonical_json()
    }

    pub fn digest(&self) -> &Sha256Digest {
        self.procedure.digest()
    }

    pub fn warnings(&self) -> &[podway_config::ProcedureWarningV1] {
        self.procedure.warnings()
    }

    /// Applies the caller's explicit warning policy using the same config admission behavior as user input.
    pub fn admit(self, warning_policy: ProcedureWarningPolicyV1) -> Result<Self, PresetError> {
        let Self { preset, procedure } = self;
        let procedure = procedure
            .admit(warning_policy)
            .map_err(|error| PresetError::admission(preset.metadata.id, error))?;
        Ok(Self { preset, procedure })
    }

    /// Converts the preset through the public config-to-core snapshot path after applying the
    /// caller's explicit warning policy.
    pub fn into_snapshot_v1(
        self,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
        warning_policy: ProcedureWarningPolicyV1,
    ) -> Result<ProcedureSnapshotV1, PresetError> {
        let source = self
            .preset
            .metadata
            .source_label()
            .map_err(|error| PresetError::admission(self.preset.metadata.id, error))?;
        self.procedure
            .into_snapshot_v1(snapshot_id, source, created_at, warning_policy)
            .map_err(|error| PresetError::admission(self.preset.metadata.id, error))
    }
}

/// An explicit failure while admitting an embedded preset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresetError {
    Admission {
        preset_id: &'static str,
        error: ConfigError,
    },
    MetadataMismatch {
        preset_id: &'static str,
        field: &'static str,
        expected: &'static str,
        actual: String,
    },
}

impl PresetError {
    fn admission(preset_id: &'static str, error: ConfigError) -> Self {
        Self::Admission { preset_id, error }
    }
}

impl fmt::Display for PresetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission { preset_id, error } => {
                write!(
                    formatter,
                    "built-in preset `{preset_id}` admission failed: {error}"
                )
            }
            Self::MetadataMismatch {
                preset_id,
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "built-in preset `{preset_id}` metadata `{field}` expected `{expected}`, received `{actual}`"
            ),
        }
    }
}

impl Error for PresetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Admission { error, .. } => Some(error),
            Self::MetadataMismatch { .. } => None,
        }
    }
}

fn validate_metadata(
    metadata: PresetMetadata,
    definition: &ProcedureDefinitionV1,
) -> Result<(), PresetError> {
    validate_metadata_field(metadata, "schema", metadata.schema, &definition.schema)?;
    validate_metadata_field(metadata, "id", metadata.id, &definition.id)?;
    validate_metadata_field(metadata, "version", metadata.version, &definition.version)?;
    validate_metadata_field(metadata, "name", metadata.name, &definition.name)?;
    validate_metadata_field(
        metadata,
        "description",
        metadata.description,
        definition.description.as_deref().unwrap_or("<missing>"),
    )
}

fn validate_metadata_field(
    metadata: PresetMetadata,
    field: &'static str,
    expected: &'static str,
    actual: &str,
) -> Result<(), PresetError> {
    if expected == actual {
        return Ok(());
    }
    Err(PresetError::MetadataMismatch {
        preset_id: metadata.id,
        field,
        expected,
        actual: actual.to_owned(),
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PresetCatalogV1;

impl PresetCatalogV1 {
    /// Returns the built-in v1 presets in stable lexicographic ID order.
    pub fn list(self) -> &'static [EmbeddedPreset; 4] {
        &EMBEDDED_PRESETS
    }

    pub fn lookup(self, id: &str) -> Option<EmbeddedPreset> {
        match id {
            "analysis" => Some(EMBEDDED_PRESETS[0]),
            "bug-fix" => Some(EMBEDDED_PRESETS[1]),
            "docs-only" => Some(EMBEDDED_PRESETS[2]),
            "sw-dev" => Some(EMBEDDED_PRESETS[3]),
            _ => None,
        }
    }
}

pub const PRESET_CATALOG_V1: PresetCatalogV1 = PresetCatalogV1;

pub const fn catalog_v1() -> PresetCatalogV1 {
    PRESET_CATALOG_V1
}

const ANALYSIS_METADATA: PresetMetadata = PresetMetadata {
    schema: PROCEDURE_SCHEMA_V1,
    id: "analysis",
    version: "1",
    name: "Analysis",
    description: "Bounded research or technical analysis with explicit challenge and synthesis.",
};

const BUG_FIX_METADATA: PresetMetadata = PresetMetadata {
    schema: PROCEDURE_SCHEMA_V1,
    id: "bug-fix",
    version: "1",
    name: "Bug Fix",
    description: "Defect correction with explicit baseline, diagnosis, regression coverage, verification, and review.",
};

const DOCS_ONLY_METADATA: PresetMetadata = PresetMetadata {
    schema: PROCEDURE_SCHEMA_V1,
    id: "docs-only",
    version: "1",
    name: "Documentation Only",
    description: "Documentation work grounded in sources, audience, validation, and review.",
};

const SW_DEV_METADATA: PresetMetadata = PresetMetadata {
    schema: PROCEDURE_SCHEMA_V1,
    id: "sw-dev",
    version: "1",
    name: "Software Development",
    description: "General software change procedure focused on understanding, implementation, verification, and review.",
};

const EMBEDDED_PRESETS: [EmbeddedPreset; 4] = [
    EmbeddedPreset {
        metadata: ANALYSIS_METADATA,
        yaml: ANALYSIS_YAML,
    },
    EmbeddedPreset {
        metadata: BUG_FIX_METADATA,
        yaml: BUG_FIX_YAML,
    },
    EmbeddedPreset {
        metadata: DOCS_ONLY_METADATA,
        yaml: DOCS_ONLY_YAML,
    },
    EmbeddedPreset {
        metadata: SW_DEV_METADATA,
        yaml: SW_DEV_YAML,
    },
];

/// Returns the built-in presets in stable lexicographic ID order.
pub fn list() -> &'static [EmbeddedPreset; 4] {
    catalog_v1().list()
}

pub fn lookup(id: &str) -> Option<EmbeddedPreset> {
    catalog_v1().lookup(id)
}
