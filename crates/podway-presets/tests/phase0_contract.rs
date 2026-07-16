//! Phase 0 embedded-preset contracts.
//!
//! Requirement IDs: PRD-008, DOM-001, DOM-002.

use podway_config::{PROCEDURE_SCHEMA_V1, ProcedureSourceKind};
use podway_presets::{ANALYSIS_YAML, BUG_FIX_YAML, DOCS_ONLY_YAML, SW_DEV_YAML, list, lookup};

#[test]
fn prd_008_embeds_exactly_four_named_presets_in_lexicographic_id_order() {
    let expected = [
        ("analysis", "Analysis"),
        ("bug-fix", "Bug Fix"),
        ("docs-only", "Documentation Only"),
        ("sw-dev", "Software Development"),
    ];
    let presets = list();

    assert_eq!(presets.len(), expected.len());
    for (preset, (expected_id, expected_name)) in presets.iter().zip(expected) {
        assert_eq!(preset.metadata.id, expected_id);
        assert_eq!(preset.metadata.name, expected_name);
    }
}

#[test]
fn prd_008_embedded_yaml_bytes_match_the_root_preset_sources() {
    let expected: [(&str, &str, &[u8]); 4] = [
        (
            "analysis",
            ANALYSIS_YAML,
            include_bytes!("../../../presets/analysis.yaml"),
        ),
        (
            "bug-fix",
            BUG_FIX_YAML,
            include_bytes!("../../../presets/bug-fix.yaml"),
        ),
        (
            "docs-only",
            DOCS_ONLY_YAML,
            include_bytes!("../../../presets/docs-only.yaml"),
        ),
        (
            "sw-dev",
            SW_DEV_YAML,
            include_bytes!("../../../presets/sw-dev.yaml"),
        ),
    ];

    let presets = list();
    assert_eq!(presets.len(), expected.len());
    for (preset, (expected_id, exported_yaml, root_source_bytes)) in presets.iter().zip(expected) {
        assert_eq!(preset.metadata.id, expected_id);
        assert_eq!(preset.yaml, exported_yaml);
        assert_eq!(preset.yaml.as_bytes(), root_source_bytes);
    }
}

#[test]
fn dom_001_metadata_lookup_and_config_source_labels_are_stable() {
    let expected = [
        (
            "analysis",
            "Analysis",
            "Bounded research or technical analysis with explicit challenge and synthesis.",
        ),
        (
            "bug-fix",
            "Bug Fix",
            "Defect correction with explicit baseline, diagnosis, regression coverage, verification, and review.",
        ),
        (
            "docs-only",
            "Documentation Only",
            "Documentation work grounded in sources, audience, validation, and review.",
        ),
        (
            "sw-dev",
            "Software Development",
            "General software change procedure focused on understanding, implementation, verification, and review.",
        ),
    ];

    for (preset, (expected_id, expected_name, expected_description)) in list().iter().zip(expected)
    {
        assert_eq!(preset.metadata.schema, PROCEDURE_SCHEMA_V1);
        assert_eq!(preset.metadata.id, expected_id);
        assert_eq!(preset.metadata.version, "1");
        assert_eq!(preset.metadata.name, expected_name);
        assert_eq!(preset.metadata.description, expected_description);
        assert_eq!(lookup(expected_id), Some(*preset));

        let source_label = preset
            .metadata
            .source_label()
            .expect("embedded metadata must produce a valid config source label");
        assert_eq!(source_label.kind(), ProcedureSourceKind::Preset);
        assert_eq!(source_label.label(), expected_id);
        assert_eq!(
            source_label.display_label(),
            format!("preset:{expected_id}")
        );
    }

    for unknown_id in ["", "Analysis", "not-a-preset"] {
        assert_eq!(lookup(unknown_id), None);
    }
}
