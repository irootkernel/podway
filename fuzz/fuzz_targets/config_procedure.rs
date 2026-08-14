#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_config::{
    CanonicalDigest, CanonicalJson, ParsedProcedure, ProcedureDocumentFormat,
    parse_procedure_document, parse_workspace_config_v1, validate_procedure_v2,
};

fuzz_target!(|input: &[u8]| {
    if let Ok(config) = parse_workspace_config_v1(input) {
        config
            .validate()
            .expect("parsed workspace config must remain valid");
        let canonical = config
            .canonical_json_v1()
            .expect("validated workspace config must canonicalize");
        let digest = config
            .canonical_digest_v1()
            .expect("validated workspace config must have a digest");
        let reparsed = parse_workspace_config_v1(canonical.as_bytes())
            .expect("canonical workspace config must parse");
        assert_eq!(
            reparsed, config,
            "canonical workspace config must round-trip"
        );
        assert_eq!(
            reparsed
                .canonical_digest_v1()
                .expect("reparsed workspace config must have a digest"),
            digest,
            "canonical workspace config must retain its digest"
        );
    }

    for format in [ProcedureDocumentFormat::Json, ProcedureDocumentFormat::Yaml] {
        if let Ok(ParsedProcedure::V2(procedure)) = parse_procedure_document(input, format) {
            let first = validate_procedure_v2(procedure.clone());
            let second = validate_procedure_v2(procedure);
            assert_eq!(
                first, second,
                "Procedure v2 validation must be deterministic"
            );

            if let Ok(validated) = first {
                let ParsedProcedure::V2(reparsed) = parse_procedure_document(
                    validated.canonical_json().as_bytes(),
                    ProcedureDocumentFormat::Json,
                )
                .expect("canonical Procedure v2 must parse");
                let reparsed = validate_procedure_v2(reparsed)
                    .expect("canonical Procedure v2 must remain valid");
                assert_eq!(
                    reparsed.canonical_json(),
                    validated.canonical_json(),
                    "canonical Procedure v2 must round-trip",
                );
                assert_eq!(
                    reparsed.digest(),
                    validated.digest(),
                    "canonical Procedure v2 must retain its digest",
                );
            }
        }
    }
});
