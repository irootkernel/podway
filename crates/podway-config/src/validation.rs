use std::collections::BTreeSet;

use podway_core::{
    CanonicalProcedureJsonV1, CanonicalProcedureSnapshotInputV1, ProcedureSnapshotId,
    ProcedureSnapshotV1, ProcedureSourceLabelV1, ProcedureWarningCodeV1, UnixMillis,
};
use sha2::{Digest as _, Sha256};

use crate::{
    CanonicalJsonV1, ConfigError, ItemDefinitionV1, ProcedureDefinitionV1, ReturnTargetsV1,
    canonical_json_from_serializable,
};

/// A source label that has been validated for storage in a core procedure snapshot.
pub trait IntoProcedureSnapshotSourceV1 {
    fn into_procedure_snapshot_source_v1(self) -> Result<ProcedureSourceLabelV1, ConfigError>;
}

impl IntoProcedureSnapshotSourceV1 for crate::ProcedureSourceLabel {
    fn into_procedure_snapshot_source_v1(self) -> Result<ProcedureSourceLabelV1, ConfigError> {
        ProcedureSourceLabelV1::new(self.display_label()).map_err(core_admission_error)
    }
}

impl IntoProcedureSnapshotSourceV1 for ProcedureSourceLabelV1 {
    fn into_procedure_snapshot_source_v1(self) -> Result<ProcedureSourceLabelV1, ConfigError> {
        Ok(self)
    }
}
/// A config-owned semantic concern found after a procedure has passed admission checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureWarningV1 {
    pub code: ProcedureWarningCodeV1,
    pub stage_id: Option<String>,
    pub item_id: Option<String>,
}

impl ProcedureWarningV1 {
    pub const fn code(&self) -> ProcedureWarningCodeV1 {
        self.code
    }

    pub fn stage_id(&self) -> Option<&str> {
        self.stage_id.as_deref()
    }

    pub fn item_id(&self) -> Option<&str> {
        self.item_id.as_deref()
    }
}
/// The explicit warning-admission decision required before configuration becomes a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcedureWarningPolicyV1 {
    Accept,
    Reject,
}

/// A complete configuration procedure that has been bounded, default-expanded, canonicalized,
/// and semantically validated. It contains no partially accepted document state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedProcedureV1 {
    definition: ProcedureDefinitionV1,
    canonical_json: CanonicalJsonV1,
    digest: podway_core::Sha256Digest,
    warnings: Vec<ProcedureWarningV1>,
}

impl ValidatedProcedureV1 {
    pub(crate) fn new(definition: ProcedureDefinitionV1) -> Result<Self, ConfigError> {
        definition.validate()?;
        let definition = definition.normalized_for_canonical_json();
        let canonical_json = canonical_json_from_serializable(&definition)?;
        let warnings = procedure_semantic_warnings(&definition);
        let digest = podway_core::Sha256Digest::new(format!(
            "sha256:{:x}",
            Sha256::digest(canonical_json.as_bytes())
        ))
        .map_err(|_| ConfigError::InvalidDigest)?;
        Ok(Self {
            definition,
            canonical_json,
            digest,
            warnings,
        })
    }

    pub fn definition(&self) -> &ProcedureDefinitionV1 {
        &self.definition
    }

    pub fn canonical_json(&self) -> &CanonicalJsonV1 {
        &self.canonical_json
    }

    pub fn digest(&self) -> &podway_core::Sha256Digest {
        &self.digest
    }

    pub fn warnings(&self) -> &[ProcedureWarningV1] {
        &self.warnings
    }

    /// Applies a caller's explicit warning policy without modifying validated data or its digest.
    pub fn admit(self, warning_policy: ProcedureWarningPolicyV1) -> Result<Self, ConfigError> {
        if warning_policy == ProcedureWarningPolicyV1::Reject && !self.warnings.is_empty() {
            return Err(ConfigError::WarningsAsErrors {
                warnings: warning_codes(&self.warnings),
            });
        }
        Ok(self)
    }

    /// Constructs the exact immutable core snapshot after applying configuration-owned warning
    /// policy. Core independently verifies the persisted representation and its warning codes.
    pub fn into_snapshot_v1(
        self,
        snapshot_id: ProcedureSnapshotId,
        source: impl IntoProcedureSnapshotSourceV1,
        created_at: UnixMillis,
        warning_policy: ProcedureWarningPolicyV1,
    ) -> Result<ProcedureSnapshotV1, ConfigError> {
        let admitted = self.admit(warning_policy)?;
        let ValidatedProcedureV1 {
            definition,
            canonical_json,
            digest,
            warnings,
        } = admitted;
        let source_label = source.into_procedure_snapshot_source_v1()?;
        let canonical_json = CanonicalProcedureJsonV1::new(canonical_json.into_string())
            .map_err(core_admission_error)?;
        let snapshot =
            ProcedureSnapshotV1::from_canonical_json(CanonicalProcedureSnapshotInputV1 {
                snapshot_id,
                schema_id: definition.schema,
                procedure_id: definition.id,
                procedure_version: definition.version,
                name: definition.name,
                source_label,
                canonical_json,
                digest,
                created_at,
            })
            .map_err(core_admission_error)?;

        if warning_codes(&warnings) != snapshot.accepted_warning_codes() {
            return Err(ConfigError::CoreAdmission {
                reason: "configuration semantic warnings do not match persisted snapshot warnings"
                    .to_owned(),
            });
        }

        Ok(snapshot)
    }
}

fn warning_codes(warnings: &[ProcedureWarningV1]) -> Vec<ProcedureWarningCodeV1> {
    warnings
        .iter()
        .map(|warning| warning.code)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn procedure_semantic_warnings(definition: &ProcedureDefinitionV1) -> Vec<ProcedureWarningV1> {
    let mut warnings = Vec::new();
    if is_near_limit(definition.stages.len(), 64) {
        warnings.push(ProcedureWarningV1 {
            code: ProcedureWarningCodeV1::StageNearHardLimits,
            stage_id: None,
            item_id: None,
        });
    }
    if matches!(
        &definition.rework.allow_return_to,
        ReturnTargetsV1::AnyPrevious
    ) {
        warnings.push(ProcedureWarningV1 {
            code: ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            stage_id: None,
            item_id: None,
        });
    }

    let mut prompts = BTreeSet::new();
    for stage in &definition.stages {
        if !stage.items.iter().any(ItemDefinitionV1::required) {
            warnings.push(ProcedureWarningV1 {
                code: ProcedureWarningCodeV1::StageHasNoRequiredItems,
                stage_id: Some(stage.id.clone()),
                item_id: None,
            });
        }
        if is_near_limit(stage.instructions.len(), 32)
            || is_near_limit(stage.items.len(), 128)
            || stage
                .instructions
                .iter()
                .any(|instruction| is_near_limit(instruction.chars().count(), 2_000))
        {
            warnings.push(ProcedureWarningV1 {
                code: ProcedureWarningCodeV1::StageNearHardLimits,
                stage_id: Some(stage.id.clone()),
                item_id: None,
            });
        }
        for item in &stage.items {
            let prompt = item_prompt(item);
            if !prompts.insert(prompt) {
                warnings.push(ProcedureWarningV1 {
                    code: ProcedureWarningCodeV1::RepeatedPrompt,
                    stage_id: Some(stage.id.clone()),
                    item_id: Some(item.id().to_owned()),
                });
            }
            if !item.required() && looks_required(prompt) {
                warnings.push(ProcedureWarningV1 {
                    code: ProcedureWarningCodeV1::OptionalItemAppearsRequired,
                    stage_id: Some(stage.id.clone()),
                    item_id: Some(item.id().to_owned()),
                });
            }
        }
    }
    if definition
        .stages
        .last()
        .and_then(|stage| stage.skip.as_ref())
        .is_some_and(|skip| skip.allowed)
    {
        warnings.push(ProcedureWarningV1 {
            code: ProcedureWarningCodeV1::FinalStageSkippable,
            stage_id: definition.stages.last().map(|stage| stage.id.clone()),
            item_id: None,
        });
    }
    warnings
}

fn item_prompt(item: &ItemDefinitionV1) -> &str {
    match item {
        ItemDefinitionV1::Confirm { prompt, .. }
        | ItemDefinitionV1::Text { prompt, .. }
        | ItemDefinitionV1::Choice { prompt, .. }
        | ItemDefinitionV1::Integer { prompt, .. }
        | ItemDefinitionV1::List { prompt, .. }
        | ItemDefinitionV1::Artifact { prompt, .. } => prompt,
    }
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

fn core_admission_error(error: podway_core::DomainError) -> ConfigError {
    ConfigError::CoreAdmission {
        reason: error.to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use podway_core::{DomainError, Sha256Digest};

    fn definition_with_skip(skip: Option<crate::SkipPolicyV1>) -> ProcedureDefinitionV1 {
        ProcedureDefinitionV1 {
            schema: crate::PROCEDURE_SCHEMA_V1.to_owned(),
            id: "codec".to_owned(),
            version: "1".to_owned(),
            name: "Codec".to_owned(),
            description: None,
            stages: vec![crate::StageDefinitionV1 {
                id: "stage".to_owned(),
                title: "Stage".to_owned(),
                instructions: Vec::new(),
                items: Vec::new(),
                skip,
            }],
            rework: crate::ReworkPolicyV1::any_previous(),
        }
    }

    fn persisted_input(canonical_json: &str) -> CanonicalProcedureSnapshotInputV1 {
        let canonical_json = CanonicalProcedureJsonV1::new(canonical_json).unwrap();
        let digest = Sha256Digest::new(format!(
            "sha256:{:x}",
            sha2::Sha256::digest(canonical_json.as_str().as_bytes())
        ))
        .unwrap();
        CanonicalProcedureSnapshotInputV1 {
            snapshot_id: ProcedureSnapshotId::new("123e4567-e89b-12d3-a456-426614174000").unwrap(),
            schema_id: crate::PROCEDURE_SCHEMA_V1.to_owned(),
            procedure_id: "codec".to_owned(),
            procedure_version: "1".to_owned(),
            name: "Codec".to_owned(),
            source_label: ProcedureSourceLabelV1::new("preset:codec").unwrap(),
            canonical_json,
            digest,
            created_at: UnixMillis::new(42),
        }
    }

    #[test]
    fn config_production_normalizes_skip_policies_and_matches_core_verification() {
        let expanded = ValidatedProcedureV1::new(definition_with_skip(Some(crate::SkipPolicyV1 {
            allowed: true,
            reason_required: None,
        })))
        .unwrap();
        assert_eq!(
            expanded.canonical_json().as_str(),
            r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"skip":{"allowed":true,"reason_required":true},"title":"Stage"}],"version":"1"}"#
        );

        let snapshot = expanded
            .clone()
            .into_snapshot_v1(
                ProcedureSnapshotId::new("123e4567-e89b-12d3-a456-426614174000").unwrap(),
                ProcedureSourceLabelV1::new("preset:codec").unwrap(),
                UnixMillis::new(42),
                ProcedureWarningPolicyV1::Accept,
            )
            .unwrap();
        let expected_warning_codes = warning_codes(expanded.warnings());
        assert_eq!(
            expected_warning_codes.as_slice(),
            snapshot.accepted_warning_codes()
        );

        let disallowed = ValidatedProcedureV1::new(definition_with_skip(Some(
            crate::SkipPolicyV1::disallowed(),
        )))
        .unwrap();
        assert!(!disallowed.canonical_json().as_str().contains(r#""skip""#));
        assert!(
            disallowed
                .into_snapshot_v1(
                    ProcedureSnapshotId::new("123e4567-e89b-12d3-a456-426614174001").unwrap(),
                    ProcedureSourceLabelV1::new("preset:codec").unwrap(),
                    UnixMillis::new(42),
                    ProcedureWarningPolicyV1::Accept,
                )
                .is_ok()
        );
    }

    #[test]
    fn core_rejects_self_consistent_noncanonical_skip_alternatives() {
        let missing_reason_required = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"skip":{"allowed":true},"title":"Stage"}],"version":"1"}"#;
        assert_eq!(
            ProcedureSnapshotV1::from_canonical_json(persisted_input(missing_reason_required))
                .unwrap_err(),
            DomainError::InvalidState {
                reason: "invalid canonical procedure JSON",
            }
        );

        let redundant_disallowed_skip = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"skip":{"allowed":false,"reason_required":false},"title":"Stage"}],"version":"1"}"#;
        assert_eq!(
            ProcedureSnapshotV1::from_canonical_json(persisted_input(redundant_disallowed_skip))
                .unwrap_err(),
            DomainError::InvalidState {
                reason: "canonical procedure JSON must omit disallowed skip policies",
            }
        );
    }
}
