//! V2DOG-003 source, embedding, and shipped-digest identity coverage.

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{AuthoringSeverity, PROCEDURE_SCHEMA_V2, Sha256Digest};
use podway_presets::{
    BUG_FIX_V2_SHIPPED_DIGEST, EmbeddedPresetV2, PresetError, SMALL_CHANGE_V2_SHIPPED_DIGEST,
    SW_DEV_V2_SHIPPED_DIGEST, catalog_v2,
};

#[test]
fn v2_presets_embed_the_exact_canonical_sources_and_pinned_digests() {
    let expected = [
        (
            "bug-fix-v2",
            include_bytes!("../../../assets/presets/bug-fix-v2.yaml").as_slice(),
            BUG_FIX_V2_SHIPPED_DIGEST,
        ),
        (
            "small-change-v2",
            include_bytes!("../../../assets/presets/small-change-v2.yaml").as_slice(),
            SMALL_CHANGE_V2_SHIPPED_DIGEST,
        ),
        (
            "sw-dev-v2",
            include_bytes!("../../../assets/presets/sw-dev-v2.yaml").as_slice(),
            SW_DEV_V2_SHIPPED_DIGEST,
        ),
    ];

    assert_eq!(catalog_v2().list().len(), expected.len());
    assert_eq!(catalog_v2().lookup("missing"), None);
    for (preset, (id, source, digest)) in catalog_v2().list().iter().zip(expected) {
        assert_eq!(preset.metadata.schema, PROCEDURE_SCHEMA_V2);
        assert_eq!(preset.metadata.id, id);
        assert_eq!(preset.source_bytes(), source);
        assert_eq!(preset.yaml.as_bytes(), source);
        assert_eq!(preset.shipped_digest, digest);
        assert_eq!(catalog_v2().lookup(id), Some(*preset));

        let admitted = preset.validate().expect("shipped v2 preset must admit");
        assert_eq!(admitted.metadata(), preset.metadata);
        assert_eq!(admitted.parsed().id(), id);
        assert_eq!(admitted.digest(), admitted.pinned_digest());
        assert_eq!(admitted.digest().as_str(), digest);

        let ParsedProcedure::V2(parsed) =
            parse_procedure_document(source, ProcedureDocumentFormat::Yaml)
                .expect("canonical source must parse");
        let validated = validate_procedure_v2(parsed).expect("canonical source must validate");
        assert_eq!(admitted.canonical_json(), validated.canonical_json());
        assert_eq!(admitted.digest(), validated.digest());
        assert!(
            vet_procedure_v2(
                &validated,
                &AuthoringContext::new(id, preset.yaml, ProcedureDocumentFormat::Yaml),
            )
            .iter()
            .all(|diagnostic| diagnostic.severity() != AuthoringSeverity::Error)
        );
    }
}

#[test]
fn v2_preset_digest_pin_is_independent() {
    let preset = catalog_v2().lookup("bug-fix-v2").unwrap();
    let mismatched = EmbeddedPresetV2 {
        shipped_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ..preset
    };
    let source_admitted = mismatched
        .validate_source()
        .expect("source admission must preserve a distinct valid pin for the runtime fence");
    assert_ne!(source_admitted.digest(), source_admitted.pinned_digest());
    assert!(matches!(
        mismatched.validate(),
        Err(PresetError::PinnedDigestMismatch {
            expected,
            actual,
            ..
        }) if expected == Sha256Digest::new(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ).unwrap() && actual.as_str() == BUG_FIX_V2_SHIPPED_DIGEST
    ));
}
