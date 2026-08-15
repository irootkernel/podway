#![forbid(unsafe_code)]

use std::{error::Error, fmt};

use podway_config::{
    AuthoringContext, CanonicalJsonV1, ConfigError, ParsedProcedure, ParsedProcedureV2,
    ProcedureDocumentFormat, ProcedureSourceLabel, ValidatedProcedureV2, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{AuthoringSeverity, PROCEDURE_SCHEMA_V2};
use serde::Serialize;

pub use podway_core::Sha256Digest;

pub const BUG_FIX_V2_YAML: &str = include_str!("../../../assets/presets/bug-fix-v2.yaml");
pub const SMALL_CHANGE_V2_YAML: &str = include_str!("../../../assets/presets/small-change-v2.yaml");
pub const SW_DEV_V2_YAML: &str = include_str!("../../../assets/presets/sw-dev-v2.yaml");

pub const BUG_FIX_V2_SHIPPED_DIGEST: &str =
    "sha256:53e249a158bdbec6e8437595378509a35cb05288b48db9505cad25d04ef8f768";
pub const SMALL_CHANGE_V2_SHIPPED_DIGEST: &str =
    "sha256:7b9855f12f85d7fb895dad592e8f0ed6ce46bf2bab34c4fcf7113dd91b111e96";
pub const SW_DEV_V2_SHIPPED_DIGEST: &str =
    "sha256:810d438bde83d3055d5d8ab49eec59d60f0c3de61610f74e73c5815fd0087854";

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
pub struct EmbeddedPresetV2 {
    pub metadata: PresetMetadata,
    pub yaml: &'static str,
    pub shipped_digest: &'static str,
}

impl EmbeddedPresetV2 {
    /// Returns the exact UTF-8 bytes embedded from the root preset source file.
    pub fn source_bytes(self) -> &'static [u8] {
        self.yaml.as_bytes()
    }

    /// Admits the shipped bytes through the complete Procedure v2 authoring path and verifies the
    /// independently pinned canonical digest before exposing them to a runtime consumer.
    pub fn validate(self) -> Result<ValidatedPresetV2, PresetError> {
        let admitted = self.validate_source()?;
        if admitted.digest() != admitted.pinned_digest() {
            return Err(PresetError::PinnedDigestMismatch {
                preset_id: self.metadata.id,
                expected: admitted.pinned_digest().clone(),
                actual: admitted.digest().clone(),
            });
        }
        Ok(admitted)
    }

    /// Admits and vets the shipped source while retaining its independent digest pin.
    ///
    /// Runtime callers use this only when their next step verifies the returned canonical snapshot
    /// against `pinned_digest`; callers that do not own that final fence must use [`Self::validate`].
    pub fn validate_source(self) -> Result<ValidatedPresetV2, PresetError> {
        let ParsedProcedure::V2(parsed) =
            parse_procedure_document(self.source_bytes(), ProcedureDocumentFormat::Yaml)
                .map_err(|error| PresetError::admission(self.metadata.id, error))?;
        validate_metadata_v2(self.metadata, &parsed)?;
        let procedure = validate_procedure_v2(parsed)
            .map_err(|error| PresetError::admission(self.metadata.id, error))?;
        let context =
            AuthoringContext::new(self.metadata.id, self.yaml, ProcedureDocumentFormat::Yaml);
        let diagnostic_codes: Vec<String> = vet_procedure_v2(&procedure, &context)
            .into_iter()
            .filter(|diagnostic| diagnostic.severity() == AuthoringSeverity::Error)
            .map(|diagnostic| diagnostic.code().as_str().to_owned())
            .collect();
        if !diagnostic_codes.is_empty() {
            return Err(PresetError::Vetting {
                preset_id: self.metadata.id,
                diagnostic_codes,
            });
        }
        let pinned_digest = Sha256Digest::new(self.shipped_digest.to_owned()).map_err(|_| {
            PresetError::PinnedDigestInvalid {
                preset_id: self.metadata.id,
            }
        })?;
        Ok(ValidatedPresetV2 {
            preset: self,
            procedure,
            pinned_digest,
        })
    }
}

/// A shipped Procedure v2 preset admitted without preset-specific normalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPresetV2 {
    preset: EmbeddedPresetV2,
    procedure: ValidatedProcedureV2,
    pinned_digest: Sha256Digest,
}

impl ValidatedPresetV2 {
    pub const fn preset(&self) -> EmbeddedPresetV2 {
        self.preset
    }

    pub const fn metadata(&self) -> PresetMetadata {
        self.preset.metadata
    }

    pub fn parsed(&self) -> &ParsedProcedureV2 {
        self.procedure.parsed()
    }

    pub fn canonical_json(&self) -> &CanonicalJsonV1 {
        self.procedure.canonical_json()
    }

    pub fn digest(&self) -> &Sha256Digest {
        self.procedure.digest()
    }

    pub fn pinned_digest(&self) -> &Sha256Digest {
        &self.pinned_digest
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
    Vetting {
        preset_id: &'static str,
        diagnostic_codes: Vec<String>,
    },
    PinnedDigestInvalid {
        preset_id: &'static str,
    },
    PinnedDigestMismatch {
        preset_id: &'static str,
        expected: Sha256Digest,
        actual: Sha256Digest,
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
            Self::Vetting {
                preset_id,
                diagnostic_codes,
            } => write!(
                formatter,
                "built-in preset `{preset_id}` failed graph vetting: {}",
                diagnostic_codes.join(", ")
            ),
            Self::PinnedDigestInvalid { preset_id } => write!(
                formatter,
                "built-in preset `{preset_id}` has an invalid shipped digest"
            ),
            Self::PinnedDigestMismatch {
                preset_id,
                expected,
                actual,
            } => write!(
                formatter,
                "built-in preset `{preset_id}` shipped digest expected `{expected}`, received `{actual}`"
            ),
        }
    }
}

impl Error for PresetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Admission { error, .. } => Some(error),
            Self::MetadataMismatch { .. }
            | Self::Vetting { .. }
            | Self::PinnedDigestInvalid { .. }
            | Self::PinnedDigestMismatch { .. } => None,
        }
    }
}

fn validate_metadata_v2(
    metadata: PresetMetadata,
    procedure: &ParsedProcedureV2,
) -> Result<(), PresetError> {
    validate_metadata_field(metadata, "schema", metadata.schema, PROCEDURE_SCHEMA_V2)?;
    validate_metadata_field(metadata, "id", metadata.id, procedure.id())?;
    validate_metadata_field(metadata, "version", metadata.version, procedure.version())?;
    validate_metadata_field(metadata, "name", metadata.name, procedure.name())?;
    validate_metadata_field(
        metadata,
        "description",
        metadata.description,
        procedure.description().unwrap_or("<missing>"),
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
pub struct PresetCatalogV2;

impl PresetCatalogV2 {
    /// Returns the built-in v2 presets in stable lexicographic ID order.
    pub fn list(self) -> &'static [EmbeddedPresetV2; 3] {
        &EMBEDDED_PRESETS_V2
    }

    pub fn lookup(self, id: &str) -> Option<EmbeddedPresetV2> {
        match id {
            "bug-fix-v2" => Some(EMBEDDED_PRESETS_V2[0]),
            "small-change-v2" => Some(EMBEDDED_PRESETS_V2[1]),
            "sw-dev-v2" => Some(EMBEDDED_PRESETS_V2[2]),
            _ => None,
        }
    }
}

pub const PRESET_CATALOG_V2: PresetCatalogV2 = PresetCatalogV2;

pub const fn catalog_v2() -> PresetCatalogV2 {
    PRESET_CATALOG_V2
}

const BUG_FIX_V2_METADATA: PresetMetadata = PresetMetadata {
    schema: PROCEDURE_SCHEMA_V2,
    id: "bug-fix-v2",
    version: "2",
    name: "Bug Fix v2",
    description: "A bounded full-feature bug-fix procedure with explicit rework and goal-directed closeout.",
};

const SMALL_CHANGE_V2_METADATA: PresetMetadata = PresetMetadata {
    schema: PROCEDURE_SCHEMA_V2,
    id: "small-change-v2",
    version: "2",
    name: "Small Change v2",
    description: "A short verified change procedure without goal tracking or artifact bookkeeping.",
};

const SW_DEV_V2_METADATA: PresetMetadata = PresetMetadata {
    schema: PROCEDURE_SCHEMA_V2,
    id: "sw-dev-v2",
    version: "2",
    name: "Software Development v2",
    description: "A bounded full-feature software development procedure with goal-directed closeout.",
};

const EMBEDDED_PRESETS_V2: [EmbeddedPresetV2; 3] = [
    EmbeddedPresetV2 {
        metadata: BUG_FIX_V2_METADATA,
        yaml: BUG_FIX_V2_YAML,
        shipped_digest: BUG_FIX_V2_SHIPPED_DIGEST,
    },
    EmbeddedPresetV2 {
        metadata: SMALL_CHANGE_V2_METADATA,
        yaml: SMALL_CHANGE_V2_YAML,
        shipped_digest: SMALL_CHANGE_V2_SHIPPED_DIGEST,
    },
    EmbeddedPresetV2 {
        metadata: SW_DEV_V2_METADATA,
        yaml: SW_DEV_V2_YAML,
        shipped_digest: SW_DEV_V2_SHIPPED_DIGEST,
    },
];

/// Returns the built-in Procedure v2 presets in stable lexicographic ID order.
pub fn list() -> &'static [EmbeddedPresetV2; 3] {
    catalog_v2().list()
}

pub fn lookup(id: &str) -> Option<EmbeddedPresetV2> {
    catalog_v2().lookup(id)
}
