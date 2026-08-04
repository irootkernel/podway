use podway_config::{
    ConfigError, MAX_PROCEDURE_DOCUMENT_BYTES, MAX_PROCEDURE_DOCUMENT_DEPTH,
    MAX_PROCEDURE_DOCUMENT_NODES, ProcedureDocumentFormat, ProcedureDocumentLimits,
    decode_procedure_document, decode_procedure_document_with_limits,
};
use serde_json::json;

fn limits(max_bytes: usize, max_depth: usize, max_nodes: usize) -> ProcedureDocumentLimits {
    ProcedureDocumentLimits {
        max_bytes,
        max_depth,
        max_nodes,
    }
}

#[test]
fn raw_decoder_accepts_equivalent_yaml_and_json_without_version_dispatch() {
    let json_document = br#"{
        "schema":"podway.procedure/v2",
        "future_field":{"enabled":true,"values":[1,2]}
    }"#;
    let yaml_document =
        b"schema: podway.procedure/v2\nfuture_field:\n  enabled: true\n  values: [1, 2]\n";

    let json_value = decode_procedure_document(json_document, ProcedureDocumentFormat::Json)
        .expect("bounded raw JSON");
    let yaml_value = decode_procedure_document(yaml_document, ProcedureDocumentFormat::Yaml)
        .expect("bounded raw YAML");

    assert_eq!(json_value, yaml_value);
    assert_eq!(json_value["schema"], json!("podway.procedure/v2"));
    assert!(json_value.get("future_field").is_some());
}

#[test]
fn raw_decoder_rejects_shared_document_hazards_before_typed_parsing() {
    for (format, input, expected) in [
        (
            ProcedureDocumentFormat::Json,
            br#"{"schema":"one","schema":"two"}"#.as_slice(),
            ConfigError::DuplicateKey {
                key: "schema".to_owned(),
            },
        ),
        (
            ProcedureDocumentFormat::Yaml,
            b"schema: one\nschema: two\n".as_slice(),
            ConfigError::DuplicateKey {
                key: "schema".to_owned(),
            },
        ),
        (
            ProcedureDocumentFormat::Yaml,
            b"value: &shared one\ncopy: *shared\n".as_slice(),
            ConfigError::UnsupportedYamlFeature { feature: "anchor" },
        ),
        (
            ProcedureDocumentFormat::Yaml,
            b"value: !include other.yaml\n".as_slice(),
            ConfigError::UnsupportedYamlFeature { feature: "tag" },
        ),
    ] {
        assert_eq!(decode_procedure_document(input, format), Err(expected));
    }

    for (format, input) in [
        (
            ProcedureDocumentFormat::Json,
            br#"{"value":null}"#.as_slice(),
        ),
        (ProcedureDocumentFormat::Yaml, b"value: null\n".as_slice()),
    ] {
        assert_eq!(
            decode_procedure_document(input, format),
            Err(ConfigError::InvalidDocument {
                reason: "explicit null is not allowed by Procedure document".to_owned(),
            })
        );
    }

    assert_eq!(
        decode_procedure_document(b"---\na: 1\n---\nb: 2\n", ProcedureDocumentFormat::Yaml),
        Err(ConfigError::InvalidDocument {
            reason: "procedure document must contain exactly one YAML document".to_owned(),
        })
    );
    assert_eq!(
        decode_procedure_document([0xff], ProcedureDocumentFormat::Json),
        Err(ConfigError::InvalidDocument {
            reason: "input must be valid UTF-8".to_owned(),
        })
    );
}

#[test]
fn raw_decoder_enforces_exact_byte_depth_and_node_boundaries_for_both_formats() {
    assert_eq!(MAX_PROCEDURE_DOCUMENT_BYTES, 1_048_576);
    assert_eq!(MAX_PROCEDURE_DOCUMENT_DEPTH, 64);
    assert_eq!(MAX_PROCEDURE_DOCUMENT_NODES, 100_000);
    for format in [ProcedureDocumentFormat::Json, ProcedureDocumentFormat::Yaml] {
        let input = match format {
            ProcedureDocumentFormat::Json => "[0]",
            ProcedureDocumentFormat::Yaml => "[0]",
        };
        decode_procedure_document_with_limits(input, format, limits(input.len(), 2, 2))
            .expect("document exactly at all bounds");
        assert_eq!(
            decode_procedure_document_with_limits(
                format!("{input} "),
                format,
                limits(input.len(), 2, 2),
            ),
            Err(ConfigError::InputTooLarge {
                maximum: input.len(),
                actual: input.len() + 1,
            })
        );
        assert_eq!(
            decode_procedure_document_with_limits("[[0]]", format, limits(32, 2, 8)),
            Err(ConfigError::InputTooDeep {
                maximum: 2,
                actual: 3,
            })
        );
        assert_eq!(
            decode_procedure_document_with_limits("[0,1]", format, limits(32, 4, 2)),
            Err(ConfigError::InputTooComplex {
                maximum: 2,
                actual: 3,
            })
        );
    }
}
