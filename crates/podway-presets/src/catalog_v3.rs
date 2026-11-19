use crate::{PresetError, PresetMetadata, validate_metadata_field};
use podway_config::{
    ProcedureDocumentFormat,
    execution_groups::{
        ValidatedSuccessorDefinition, parse_successor_document, validate_group_interface,
        validate_successor_definition,
    },
};
use podway_core::Sha256Digest;
use podway_core::execution_groups::PROCEDURE_SCHEMA_V3;

/// Canonical shipped base Procedure and its default group proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddedPresetV3 {
    pub metadata: PresetMetadata,
    pub base_yaml: &'static str,
    pub group_yaml: &'static str,
    pub base_digest: &'static str,
    pub group_digest: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPresetV3 {
    pub base: ValidatedSuccessorDefinition,
    pub default_group: ValidatedSuccessorDefinition,
}

impl EmbeddedPresetV3 {
    pub fn validate(self) -> Result<ValidatedPresetV3, PresetError> {
        let base = self.admit(self.base_yaml, self.base_digest)?;
        let group = self.admit(self.group_yaml, self.group_digest)?;
        let header = base.definition().header();
        for (field, expected, actual) in [
            ("schema", self.metadata.schema, base.definition().schema()),
            ("id", self.metadata.id, header.id().as_str()),
            ("version", self.metadata.version, header.version()),
            ("name", self.metadata.name, header.name()),
            (
                "description",
                self.metadata.description,
                header.description().unwrap_or("<missing>"),
            ),
        ] {
            validate_metadata_field(self.metadata, field, expected, actual)?;
        }
        validate_group_interface(&base, &group).map_err(|error| PresetError::Admission {
            preset_id: self.metadata.id,
            error,
        })?;
        Ok(ValidatedPresetV3 {
            base,
            default_group: group,
        })
    }

    fn admit(self, source: &str, pin: &str) -> Result<ValidatedSuccessorDefinition, PresetError> {
        let preset_id = self.metadata.id;
        let definition = parse_successor_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
            .and_then(validate_successor_definition)
            .map_err(|error| PresetError::Admission { preset_id, error })?;
        let expected = Sha256Digest::new(pin.to_owned())
            .map_err(|_| PresetError::PinnedDigestInvalid { preset_id })?;
        if definition.digest() != &expected {
            return Err(PresetError::PinnedDigestMismatch {
                preset_id,
                expected,
                actual: definition.digest().clone(),
            });
        }
        Ok(definition)
    }
}

pub fn list_v3() -> &'static [EmbeddedPresetV3; 4] {
    &EMBEDDED_PRESETS_V3
}

pub fn lookup_v3(id: &str) -> Option<EmbeddedPresetV3> {
    list_v3()
        .iter()
        .copied()
        .find(|preset| preset.metadata.id == id)
}

const EMBEDDED_PRESETS_V3: [EmbeddedPresetV3; 4] = [
    EmbeddedPresetV3 {
        metadata: PresetMetadata {
            schema: PROCEDURE_SCHEMA_V3,
            id: "analysis-v3",
            version: "1",
            name: "Analysis v3",
            description: "A research and technical-analysis Procedure with guarded evidence-gap rework, phase-owner review, and goal-directed closeout.",
        },
        base_yaml: include_str!("../../../assets/presets/analysis-v3.yaml"),
        group_yaml: include_str!("../../../assets/presets/analysis-default-v3.yaml"),
        base_digest: "sha256:73d8369b03746cbf6f536a8152144ada5d8c4a60dedfaa02ad19e92163c47c8c",
        group_digest: "sha256:8f93b593f90d2570a08695896c9433e95729773161b12c77e62c65e9445cf100",
    },
    EmbeddedPresetV3 {
        metadata: PresetMetadata {
            schema: PROCEDURE_SCHEMA_V3,
            id: "bug-fix-v3",
            version: "1",
            name: "Bug Fix v3",
            description: "The middle reference Procedure for a reproduced defect, typed verification, guarded rework, and goal-directed closeout.",
        },
        base_yaml: include_str!("../../../assets/presets/bug-fix-v3.yaml"),
        group_yaml: include_str!("../../../assets/presets/bug-fix-default-v3.yaml"),
        base_digest: "sha256:b47da579d5b4d40851296b72a64b9290f866610f28c66613b18b3116cfc2e9fa",
        group_digest: "sha256:9284ef379b66623ea6bb15af2c040f99c519cd739a296d034ffd85afc5469d01",
    },
    EmbeddedPresetV3 {
        metadata: PresetMetadata {
            schema: PROCEDURE_SCHEMA_V3,
            id: "small-change-v3",
            version: "1",
            name: "Small Change v3",
            description: "The lightweight assertion-only reference Procedure without goal tracking or typed external results.",
        },
        base_yaml: include_str!("../../../assets/presets/small-change-v3.yaml"),
        group_yaml: include_str!("../../../assets/presets/small-change-default-v3.yaml"),
        base_digest: "sha256:a9cfdd9fda3f9b4005bcc54975ce8c381e987b90d927fc081c811de984150b0e",
        group_digest: "sha256:aeca9eb952b8b3d909da8ffae68f14eb647e4de4be01deafaf4363f0dc2476be",
    },
    EmbeddedPresetV3 {
        metadata: PresetMetadata {
            schema: PROCEDURE_SCHEMA_V3,
            id: "sw-dev-v3",
            version: "1",
            name: "Software Development v3",
            description: "The full reference Procedure for guarded verification, documentation, review, and goal-directed closeout.",
        },
        base_yaml: include_str!("../../../assets/presets/sw-dev-v3.yaml"),
        group_yaml: include_str!("../../../assets/presets/sw-dev-default-v3.yaml"),
        base_digest: "sha256:f2ca01154a6c357ef2f1322e25e00baa222091c1e1e100f798ef5c02e51c7ac5",
        group_digest: "sha256:f40093eb37d6ecdc2dd71c0a7d2904618c0950150a6b48d950c8aa9e4d707f0c",
    },
];
