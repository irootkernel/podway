//! V2MOD-005: equivalent YAML and JSON Procedure v2 (and v1) documents must produce equal
//! `ParsedProcedure` values on success and equal `ConfigError` diagnostics on failure. Canonical
//! bytes/digest equivalence is out of scope here (V2MOD-007); this file only proves the
//! `ParsedProcedureV2`/`ParsedProcedure`/`ConfigError` values `parse_procedure_document` returns
//! for the two source encodings are `==`.

use std::fs;
use std::path::{Path, PathBuf};

use podway_config::{
    ConfigError, MAX_PROCEDURE_DOCUMENT_BYTES, ParsedNodeDefinition, ParsedProcedure,
    ParsedProcedureV2, ProcedureDocumentFormat, ProcedureFormatV1, parse_procedure_document,
    parse_procedure_v1, parse_procedure_yaml,
};
use serde::Deserialize;
use serde_json::json;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(relative: &str) -> Vec<u8> {
    let path = repo_root().join(relative);
    fs::read(&path).unwrap_or_else(|error| {
        panic!("failed to read fixture {}: {error}", path.display());
    })
}

fn v2(bytes: &[u8], format: ProcedureDocumentFormat) -> Result<ParsedProcedureV2, ConfigError> {
    match parse_procedure_document(bytes, format) {
        Ok(ParsedProcedure::V2(parsed)) => Ok(parsed),
        Ok(ParsedProcedure::V1(_)) => panic!("expected v2 dispatch, got v1"),
        Err(error) => Err(error),
    }
}

fn err_yaml(text: &str) -> ConfigError {
    v2(text.as_bytes(), ProcedureDocumentFormat::Yaml).expect_err("expected a closed yaml failure")
}

fn err_json(text: &str) -> ConfigError {
    v2(text.as_bytes(), ProcedureDocumentFormat::Json).expect_err("expected a closed json failure")
}

// A minimal valid v2 document, held as byte-identical YAML/JSON text so each failure case below
// can apply the same one-field edit to both and compare the resulting diagnostics.
const BASE_YAML: &str = concat!(
    "schema: podway.procedure/v2\n",
    "id: p\n",
    "version: \"1\"\n",
    "name: P\n",
    "purpose: P.\n",
    "node_definitions:\n",
    "  a:\n",
    "    type: action\n",
    "    title: A\n",
    "    intent: I\n",
    "graph:\n",
    "  entry: n\n",
    "  nodes:\n",
    "    - id: n\n",
    "      use: a\n",
    "      terminal: true\n",
);
const BASE_JSON: &str = concat!(
    r#"{"schema":"podway.procedure/v2","id":"p","version":"1","name":"P","#,
    r#""purpose":"P.","node_definitions":{"a":{"type":"action","title":"A","intent":"I"}},"#,
    r#""graph":{"entry":"n","nodes":[{"id":"n","use":"a","terminal":true}]}}"#,
);

// ---------------------------------------------------------------------------------------------
// Success cases
// ---------------------------------------------------------------------------------------------

#[test]
fn yaml_and_json_fixture_pair_parse_to_equal_procedures() {
    let yaml = fixture("tests/fixtures/v2/procedures/equivalent-procedure.yaml");
    let json = fixture("tests/fixtures/v2/procedures/equivalent-procedure.json");

    let from_yaml = v2(&yaml, ProcedureDocumentFormat::Yaml).expect("yaml fixture parses");
    let from_json = v2(&json, ProcedureDocumentFormat::Json).expect("json fixture parses");

    assert_eq!(from_yaml, from_json);
}

#[test]
fn json_dispatch_preserves_author_order_for_maps_and_arrays() {
    // Deliberately non-alphabetical node_definitions/options/routes keys, mirroring
    // int_v2_procedure.rs's `v2_preserves_author_order_for_maps_and_arrays` YAML case exactly,
    // but as literal JSON source text: `serde_json::Value` would alphabetize these keys (the
    // workspace builds serde_json without `preserve_order`), so this must go through the wire
    // DTOs' `OrderedMap`, not a `Value` round-trip, to prove JSON author order truly survives.
    let json = r#"{
        "schema": "podway.procedure/v2",
        "id": "ordered",
        "version": "1",
        "name": "Ordered",
        "purpose": "Preserve author order.",
        "node_definitions": {
            "zeta": {"type": "action", "title": "Zeta", "intent": "First authored."},
            "alpha": {
                "type": "decision",
                "title": "Alpha",
                "objective": "Decide.",
                "prompt": "Which?",
                "options": [
                    {"id": "zoo", "label": "Zoo"},
                    {"id": "apple", "label": "Apple"}
                ],
                "reason": {"required": true}
            }
        },
        "graph": {
            "entry": "z",
            "nodes": [
                {"id": "z", "use": "zeta", "terminal": true},
                {
                    "id": "a",
                    "use": "alpha",
                    "routes": {
                        "zoo": {"to": "z", "effect": "advance"},
                        "apple": {"to": "z", "effect": "advance"}
                    }
                }
            ]
        }
    }"#;
    let parsed = v2(json.as_bytes(), ProcedureDocumentFormat::Json).expect("ordered json maps");

    let ids: Vec<_> = parsed
        .node_definitions()
        .iter()
        .map(|d| d.id().as_str())
        .collect();
    assert_eq!(ids, vec!["zeta", "alpha"]);

    let alpha = match &parsed.node_definitions()[1] {
        ParsedNodeDefinition::Decision(d) => d,
        _ => panic!("alpha is a decision"),
    };
    let option_order: Vec<_> = alpha.options().iter().map(|o| o.id().as_str()).collect();
    assert_eq!(option_order, vec!["zoo", "apple"]);

    let alpha_placement = match parsed
        .graph()
        .placement(&podway_core::GraphNodeId::new("a").unwrap())
    {
        Some(podway_core::GraphPlacementV2::Decision(d)) => d,
        _ => panic!("a is a decision placement"),
    };
    let route_order: Vec<_> = alpha_placement
        .routes()
        .entries()
        .iter()
        .map(|e| e.option_id().as_str())
        .collect();
    assert_eq!(route_order, vec!["zoo", "apple"]);

    let node_order: Vec<_> = parsed
        .graph()
        .placements()
        .iter()
        .map(|p| p.id().as_str())
        .collect();
    assert_eq!(node_order, vec!["z", "a"]);
}

#[test]
fn omitted_defaults_materialize_identically_for_both_formats() {
    let yaml = concat!(
        "schema: podway.procedure/v2\n",
        "id: defaults\n",
        "version: \"1\"\n",
        "name: Defaults\n",
        "purpose: Exercise omitted defaults.\n",
        "node_definitions:\n",
        "  act:\n",
        "    type: action\n",
        "    title: Act\n",
        "    intent: Do the work.\n",
        "    items:\n",
        "      - id: note\n",
        "        type: text\n",
        "        prompt: Note?\n",
        "        required: false\n",
        "      - id: findings\n",
        "        type: list\n",
        "        prompt: Findings?\n",
        "        required: false\n",
        "graph:\n",
        "  entry: do\n",
        "  nodes:\n",
        "    - id: do\n",
        "      use: act\n",
        "      terminal: true\n",
        "    - id: ref\n",
        "      use: act\n",
        "      evidence_from:\n",
        "        - node: do\n",
        "      next: do\n",
    );
    let json = json!({
        "schema": "podway.procedure/v2",
        "id": "defaults",
        "version": "1",
        "name": "Defaults",
        "purpose": "Exercise omitted defaults.",
        "node_definitions": {
            "act": {
                "type": "action",
                "title": "Act",
                "intent": "Do the work.",
                "items": [
                    {"id": "note", "type": "text", "prompt": "Note?", "required": false},
                    {"id": "findings", "type": "list", "prompt": "Findings?", "required": false}
                ]
            }
        },
        "graph": {
            "entry": "do",
            "nodes": [
                {"id": "do", "use": "act", "terminal": true},
                {"id": "ref", "use": "act", "evidence_from": [{"node": "do"}], "next": "do"}
            ]
        }
    })
    .to_string();

    let from_yaml = v2(yaml.as_bytes(), ProcedureDocumentFormat::Yaml).expect("yaml defaults");
    let from_json = v2(json.as_bytes(), ProcedureDocumentFormat::Json).expect("json defaults");
    assert_eq!(from_yaml, from_json);

    // Spot-check the concrete default values both formats agreed on.
    let action = match &from_yaml.node_definitions()[0] {
        ParsedNodeDefinition::Action(a) => a,
        _ => panic!("act is an action"),
    };
    let text = match &action.items()[0] {
        podway_core::ItemSpecV2::Text(t) => t,
        _ => panic!("note is a text item"),
    };
    assert_eq!(text.max_length(), 4_000);
    assert!(text.multiline());
    let list = match &action.items()[1] {
        podway_core::ItemSpecV2::List(l) => l,
        _ => panic!("findings is a list item"),
    };
    assert_eq!(list.max_items(), 50);
    assert_eq!(list.max_item_length(), 500);
    assert!(list.unique());

    let evidence_required = from_yaml
        .graph()
        .placement(&podway_core::GraphNodeId::new("ref").unwrap())
        .and_then(|p| match p {
            podway_core::GraphPlacementV2::Action(a) => a.evidence_from(),
            _ => None,
        })
        .expect("evidence present")
        .entries()
        .first()
        .expect("first reference")
        .required();
    assert!(evidence_required);
}

#[test]
fn v1_json_document_dispatches_identically_through_the_new_entry_point() {
    let document = json!({
        "schema": "podway.procedure/v1",
        "id": "release",
        "version": "1",
        "name": "Release",
        "stages": [{
            "id": "prepare",
            "title": "Prepare",
            "items": [{
                "id": "approval",
                "type": "confirm",
                "prompt": "Approved",
                "required": true
            }]
        }],
        "rework": { "allow_return_to": ["prepare"] }
    })
    .to_string();
    let bytes = document.as_bytes();

    let via_document = parse_procedure_document(bytes, ProcedureDocumentFormat::Json)
        .expect("v1 json parses via parse_procedure_document");
    let via_v1 = parse_procedure_v1(bytes, ProcedureFormatV1::Json).expect("v1 json parses");

    match via_document {
        ParsedProcedure::V1(validated) => assert_eq!(validated, via_v1),
        ParsedProcedure::V2(_) => panic!("expected v1 dispatch, got v2"),
    }
}

#[test]
fn yaml_dispatch_behavior_is_unchanged_by_the_shared_entry_point() {
    let yaml = fixture("tests/fixtures/v2/procedures/equivalent-procedure.yaml");
    assert_eq!(
        parse_procedure_document(&yaml, ProcedureDocumentFormat::Yaml),
        parse_procedure_yaml(&yaml),
    );
}

// ---------------------------------------------------------------------------------------------
// Failure cases: identical diagnostics for equivalent invalid pairs
// ---------------------------------------------------------------------------------------------

#[test]
fn identical_diagnostics_for_unknown_top_level_field() {
    let yaml = BASE_YAML.replacen("id: p\n", "id: p\nunknown: true\n", 1);
    let json = BASE_JSON.replacen("\"id\":\"p\",", "\"id\":\"p\",\"unknown\":true,", 1);
    assert_eq!(err_yaml(&yaml), err_json(&json));
}

#[test]
fn identical_diagnostics_for_unknown_node_definition_field() {
    let yaml = BASE_YAML.replacen("    intent: I\n", "    intent: I\n    bogus: true\n", 1);
    let json = BASE_JSON.replacen("\"intent\":\"I\"}", "\"intent\":\"I\",\"bogus\":true}", 1);
    assert_eq!(err_yaml(&yaml), err_json(&json));
}

#[test]
fn identical_diagnostics_for_goal_tracking_string() {
    let yaml = BASE_YAML.replacen("id: p\n", "id: p\ngoal_tracking: \"true\"\n", 1);
    let json = BASE_JSON.replacen(
        "\"id\":\"p\",",
        "\"id\":\"p\",\"goal_tracking\":\"true\",",
        1,
    );
    assert_eq!(err_yaml(&yaml), err_json(&json));
}

#[test]
fn identical_diagnostics_for_goal_tracking_false() {
    let yaml = BASE_YAML.replacen("id: p\n", "id: p\ngoal_tracking: false\n", 1);
    let json = BASE_JSON.replacen("\"id\":\"p\",", "\"id\":\"p\",\"goal_tracking\":false,", 1);
    let (yaml_error, json_error) = (err_yaml(&yaml), err_json(&json));
    assert_eq!(yaml_error, json_error);
    assert!(matches!(yaml_error, ConfigError::InvalidValue { .. }));
}

#[test]
fn goal_tracking_non_true_forms_are_rejected_identically_by_yaml_and_json_dispatch() {
    for (case, yaml_value, json_value) in [
        ("false", "false", "false"),
        ("string", "\"true\"", "\"true\""),
        ("list", "[]", "[]"),
        ("object", "{}", "{}"),
    ] {
        let yaml = BASE_YAML.replacen(
            "id: p\n",
            &format!("id: p\ngoal_tracking: {yaml_value}\n"),
            1,
        );
        let json = BASE_JSON.replacen(
            "\"id\":\"p\",",
            &format!("\"id\":\"p\",\"goal_tracking\":{json_value},"),
            1,
        );

        let (yaml_error, json_error) = (err_yaml(&yaml), err_json(&json));
        assert_eq!(yaml_error, json_error, "case {case}");
    }
}

#[test]
fn identical_diagnostics_for_missing_required_field() {
    let yaml = BASE_YAML.replacen("name: P\n", "", 1);
    let json = BASE_JSON.replacen("\"name\":\"P\",", "", 1);
    assert_eq!(err_yaml(&yaml), err_json(&json));
}

#[test]
fn identical_diagnostics_for_nine_decision_options() {
    let yaml = concat!(
        "schema: podway.procedure/v2\n",
        "id: p\n",
        "version: \"1\"\n",
        "name: P\n",
        "purpose: P.\n",
        "node_definitions:\n",
        "  d:\n",
        "    type: decision\n",
        "    title: D\n",
        "    objective: O\n",
        "    prompt: P\n",
        "    options:\n",
        "      - { id: a, label: A }\n",
        "      - { id: b, label: B }\n",
        "      - { id: c, label: C }\n",
        "      - { id: e, label: E }\n",
        "      - { id: f, label: F }\n",
        "      - { id: g, label: G }\n",
        "      - { id: h, label: H }\n",
        "      - { id: i, label: I }\n",
        "      - { id: j, label: J }\n",
        "    reason: { required: true }\n",
        "graph:\n",
        "  entry: n\n",
        "  nodes:\n",
        "    - id: n\n",
        "      use: d\n",
        "      routes:\n",
        "        a: { to: n, effect: advance }\n",
    );
    let json = json!({
        "schema": "podway.procedure/v2",
        "id": "p",
        "version": "1",
        "name": "P",
        "purpose": "P.",
        "node_definitions": {
            "d": {
                "type": "decision",
                "title": "D",
                "objective": "O",
                "prompt": "P",
                "options": [
                    {"id": "a", "label": "A"},
                    {"id": "b", "label": "B"},
                    {"id": "c", "label": "C"},
                    {"id": "e", "label": "E"},
                    {"id": "f", "label": "F"},
                    {"id": "g", "label": "G"},
                    {"id": "h", "label": "H"},
                    {"id": "i", "label": "I"},
                    {"id": "j", "label": "J"}
                ],
                "reason": {"required": true}
            }
        },
        "graph": {
            "entry": "n",
            "nodes": [
                {"id": "n", "use": "d", "routes": {"a": {"to": "n", "effect": "advance"}}}
            ]
        }
    })
    .to_string();
    let (yaml_error, json_error) = (err_yaml(yaml), err_json(&json));
    assert_eq!(yaml_error, json_error);
    assert!(matches!(yaml_error, ConfigError::InvalidValue { .. }));
}

#[test]
fn identical_diagnostics_for_over_long_identifier() {
    let long_id = "p".repeat(65);
    let yaml = BASE_YAML.replacen("id: p\n", &format!("id: {long_id}\n"), 1);
    let json = BASE_JSON.replacen("\"id\":\"p\"", &format!("\"id\":\"{long_id}\""), 1);
    let (yaml_error, json_error) = (err_yaml(&yaml), err_json(&json));
    assert_eq!(yaml_error, json_error);
    assert!(matches!(
        yaml_error,
        ConfigError::OutOfBounds { field, actual: 65, .. } if field == "procedure.id"
    ));
}

// ---------------------------------------------------------------------------------------------
// Failure cases: JSON hazards rejected by the shared decoder
// ---------------------------------------------------------------------------------------------

#[test]
fn json_hazards_are_rejected_by_the_shared_decoder_before_v2_dispatch() {
    let duplicate = br#"{"schema":"podway.procedure/v2","schema":"podway.procedure/v2"}"#;
    assert_eq!(
        parse_procedure_document(duplicate, ProcedureDocumentFormat::Json),
        Err(ConfigError::DuplicateKey {
            key: "schema".to_owned(),
        })
    );

    let float_value = br#"{"schema":"podway.procedure/v2","stray":1.5}"#;
    assert_eq!(
        parse_procedure_document(float_value, ProcedureDocumentFormat::Json),
        Err(ConfigError::NonCanonicalNumber)
    );

    let trailing = br#"{"schema":"podway.procedure/v2"} trailing"#;
    assert!(matches!(
        parse_procedure_document(trailing, ProcedureDocumentFormat::Json),
        Err(ConfigError::InvalidDocument { .. })
    ));

    let malformed = br#"{"schema":"#;
    assert!(matches!(
        parse_procedure_document(malformed, ProcedureDocumentFormat::Json),
        Err(ConfigError::InvalidDocument { .. })
    ));

    let oversized = vec![b'a'; MAX_PROCEDURE_DOCUMENT_BYTES + 1];
    assert_eq!(
        parse_procedure_document(&oversized, ProcedureDocumentFormat::Json),
        Err(ConfigError::InputTooLarge {
            maximum: MAX_PROCEDURE_DOCUMENT_BYTES,
            actual: MAX_PROCEDURE_DOCUMENT_BYTES + 1,
        })
    );
}

#[derive(Deserialize)]
struct MalformedCase {
    id: String,
    format: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    bytes_hex: Option<String>,
}

#[derive(Deserialize)]
struct MalformedFixture {
    cases: Vec<MalformedCase>,
}

fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0, "hex fixture value must have even length");
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("valid hex byte"))
        .collect()
}

#[test]
fn malformed_inputs_fixture_json_and_binary_cases_are_rejected_through_json_dispatch() {
    let raw = fixture("tests/fixtures/v2/procedures/malformed-inputs.json");
    let parsed: MalformedFixture =
        serde_json::from_slice(&raw).expect("malformed-inputs.json fixture parses as JSON");

    let mut exercised = 0usize;
    for case in &parsed.cases {
        // JSON-feedable cases only: JSON-format sources, plus format-agnostic raw-byte recipes
        // (currently just the invalid-UTF-8 case) that are equally rejected regardless of which
        // format they are fed through.
        if case.format != "json" && case.format != "binary-recipe" {
            continue;
        }
        exercised += 1;

        let bytes = match (&case.source, &case.bytes_hex) {
            (Some(source), None) => source.clone().into_bytes(),
            (None, Some(hex)) => decode_hex(hex),
            _ => panic!(
                "case {}: expected exactly one of `source`/`bytes_hex`",
                case.id
            ),
        };

        let error = match parse_procedure_document(&bytes, ProcedureDocumentFormat::Json) {
            Err(error) => error,
            Ok(_) => panic!("case {}: expected rejection, parsed successfully", case.id),
        };

        // Variant asserted only where the fixture's structural intent maps unambiguously onto a
        // single decoder/wire error variant under the current (pre-canonicalization) pipeline.
        match case.id.as_str() {
            "duplicate-json-key" => assert!(
                matches!(error, ConfigError::DuplicateKey { .. }),
                "case {}: {error:?}",
                case.id
            ),
            "trailing-json" | "malformed-json" => assert!(
                matches!(error, ConfigError::InvalidDocument { .. }),
                "case {}: {error:?}",
                case.id
            ),
            "invalid-utf8" => assert_eq!(
                error,
                ConfigError::InvalidDocument {
                    reason: "input must be valid UTF-8".to_owned(),
                },
                "case {}",
                case.id
            ),
            // These fixture documents are truncated to just `schema` + `goal_tracking`. `false`
            // is a well-typed `bool`, so that case's wire deserialization reaches the end of the
            // map and reports the first missing required field (`id`) with a stable reason. The
            // other three (string/list/object) are themselves the wrong wire type for
            // `goal_tracking: Option<bool>`, so they fail immediately on that field with an
            // `invalid type: <kind>, expected a boolean` reason that varies by JSON kind — same
            // `InvalidDocument` variant, not the same text, so only the variant is asserted.
            "goal-tracking-false" => assert_eq!(
                error,
                ConfigError::InvalidDocument {
                    reason: "missing field `id`".to_owned(),
                },
                "case {}",
                case.id
            ),
            "goal-tracking-string" | "goal-tracking-list" | "goal-tracking-object" => assert!(
                matches!(error, ConfigError::InvalidDocument { .. }),
                "case {}: {error:?}",
                case.id
            ),
            _ => {}
        }
    }
    assert_eq!(
        exercised, 8,
        "expected exactly 8 json/binary-recipe cases in the malformed-inputs fixture"
    );
}

// ---------------------------------------------------------------------------------------------
// Failure case: cross-format rejection
// ---------------------------------------------------------------------------------------------

#[test]
fn a_v2_yaml_block_document_fed_as_json_is_rejected() {
    let yaml = fixture("tests/fixtures/v2/procedures/equivalent-procedure.yaml");
    assert!(matches!(
        parse_procedure_document(&yaml, ProcedureDocumentFormat::Json),
        Err(ConfigError::InvalidDocument { .. })
    ));
}
