//! Phase 1 conformance for built-in preset admission.

use podway_config::{ConfigError, ProcedureFormatV1, ProcedureWarningPolicyV1, parse_procedure_v1};
use podway_core::{ProcedureSnapshotId, ProcedureWarningCodeV1, UnixMillis};
use podway_presets::{PresetError, catalog_v1, list};

#[test]
fn built_in_yaml_matches_catalog_sources_and_public_config_admission() {
    let expected_sources: [(&str, &[u8]); 4] = [
        ("analysis", include_bytes!("../../../presets/analysis.yaml")),
        ("bug-fix", include_bytes!("../../../presets/bug-fix.yaml")),
        (
            "docs-only",
            include_bytes!("../../../presets/docs-only.yaml"),
        ),
        ("sw-dev", include_bytes!("../../../presets/sw-dev.yaml")),
    ];
    let catalog = catalog_v1();

    assert_eq!(catalog.list(), list());
    assert_eq!(catalog.lookup("missing"), None);

    for (preset, (expected_id, source)) in catalog.list().iter().zip(expected_sources) {
        assert_eq!(preset.metadata.id, expected_id);
        assert_eq!(preset.source_bytes(), source);
        assert_eq!(preset.yaml.as_bytes(), source);
        assert_eq!(catalog.lookup(expected_id), Some(*preset));

        let parsed = parse_procedure_v1(source, ProcedureFormatV1::Yaml)
            .expect("root preset source must pass public config admission");
        let validated = preset
            .validate()
            .expect("catalog preset must pass public config admission");

        assert_eq!(validated.definition(), parsed.definition());
        assert_eq!(validated.canonical_json(), parsed.canonical_json());
        assert_eq!(validated.digest(), parsed.digest());
        assert_eq!(
            validated.metadata().schema,
            validated.definition().schema.as_str()
        );
        assert_eq!(validated.metadata().id, validated.definition().id.as_str());
        assert_eq!(
            validated.metadata().version,
            validated.definition().version.as_str()
        );
        assert_eq!(
            validated.metadata().name,
            validated.definition().name.as_str()
        );
        assert_eq!(
            validated.metadata().description,
            validated
                .definition()
                .description
                .as_deref()
                .expect("all built-in presets have descriptions"),
        );

        assert!(
            validated
                .clone()
                .admit(ProcedureWarningPolicyV1::Accept)
                .is_ok()
        );
    }
}

#[test]
fn built_in_presets_have_deterministic_canonical_digests_and_core_snapshots() {
    let snapshot_ids = [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        "00000000-0000-4000-8000-000000000003",
        "00000000-0000-4000-8000-000000000004",
    ];

    for ((index, preset), snapshot_id) in catalog_v1().list().iter().enumerate().zip(snapshot_ids) {
        let first = preset
            .validate()
            .expect("catalog preset must validate deterministically");
        let second = preset
            .validate()
            .expect("catalog preset must validate deterministically");

        assert_eq!(
            first.canonical_json().as_bytes(),
            second.canonical_json().as_bytes()
        );
        assert_eq!(first.digest(), second.digest());

        assert_eq!(
            first
                .warnings()
                .iter()
                .map(|warning| warning.code())
                .collect::<Vec<_>>(),
            vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
        );

        let snapshot = first
            .into_snapshot_v1(
                ProcedureSnapshotId::new(snapshot_id).expect("fixed snapshot ID must be valid"),
                UnixMillis::new(index as u64),
                ProcedureWarningPolicyV1::Accept,
            )
            .expect("validated built-in preset must convert to a core snapshot");

        assert_eq!(snapshot.procedure_id(), preset.metadata.id);
        assert_eq!(snapshot.procedure_version(), preset.metadata.version);
        assert_eq!(snapshot.name(), preset.metadata.name);
        assert_eq!(
            snapshot.canonical_json().as_str().as_bytes(),
            second.canonical_json().as_bytes()
        );
        assert_eq!(snapshot.digest(), second.digest());
        assert_eq!(
            snapshot.accepted_warning_codes(),
            &[ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
        );
        assert_eq!(
            snapshot.source_label().as_str(),
            format!("preset:{}", preset.metadata.id),
        );
    }
}
#[test]
fn reject_warning_policy_blocks_all_built_in_preset_snapshot_conversions() {
    for (index, preset) in catalog_v1().list().iter().enumerate() {
        let error = preset
            .validate()
            .expect("catalog preset must validate before policy admission")
            .into_snapshot_v1(
                ProcedureSnapshotId::new(format!(
                    "00000000-0000-4000-8000-0000000001{:02}",
                    index + 1
                ))
                .expect("fixed snapshot ID must be valid"),
                UnixMillis::new(index as u64),
                ProcedureWarningPolicyV1::Reject,
            )
            .expect_err("any_previous preset must be rejected when warnings are rejected");

        assert_eq!(
            error,
            PresetError::Admission {
                preset_id: preset.metadata.id,
                error: ConfigError::WarningsAsErrors {
                    warnings: vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
                },
            },
        );
    }
}
