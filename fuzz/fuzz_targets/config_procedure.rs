#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_config::{
    CanonicalDigest, CanonicalJson, ParsedProcedure, ProcedureDocumentFormat, ProcedureFormatV1,
    parse_procedure_document, parse_procedure_v1, parse_workspace_config_v1, validate_procedure_v2,
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

    for format in [ProcedureFormatV1::Json, ProcedureFormatV1::Yaml] {
        if let Ok(procedure) = parse_procedure_v1(input, format) {
            procedure
                .definition()
                .validate()
                .expect("parsed procedure definition must remain valid");
            let canonical = procedure.canonical_json();
            let reparsed = parse_procedure_v1(canonical.as_bytes(), ProcedureFormatV1::Json)
                .expect("canonical procedure must parse");
            assert_eq!(
                reparsed.canonical_json(),
                canonical,
                "canonical procedure must round-trip"
            );
            assert_eq!(
                reparsed.digest(),
                procedure.digest(),
                "canonical procedure must retain its digest"
            );
        }

        if let Ok(ParsedProcedure::V2(procedure)) = parse_procedure_document(input, format) {
            let first = validate_procedure_v2(procedure.clone());
            let second = validate_procedure_v2(procedure);
            assert_eq!(
                first, second,
                "Procedure v2 validation must be deterministic"
            );

            if let Ok(validated) = first {
                let reparsed = match parse_procedure_document(
                    validated.canonical_json().as_bytes(),
                    ProcedureDocumentFormat::Json,
                )
                .expect("canonical Procedure v2 must parse")
                {
                    ParsedProcedure::V2(reparsed) => reparsed,
                    ParsedProcedure::V1(_) => panic!("canonical Procedure v2 changed schema"),
                };
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
