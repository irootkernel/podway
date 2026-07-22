use podway_config::{
    ConfigError, MAX_PROCEDURE_DOCUMENT_BYTES_V1, MAX_PROCEDURE_DOCUMENT_DEPTH_V1,
    MAX_PROCEDURE_DOCUMENT_NODES_V1, ProcedureFormatV1, ProcedureParseLimitsV1,
    ProcedureSourceLabel, ProcedureWarningPolicyV1, ProcedureWarningV1, parse_procedure_v1,
    parse_procedure_v1_with_limits,
};
use podway_core::{
    ArtifactValueV1, AttemptId, ItemTypeV1, ItemValueV1, ProcedureSnapshotId,
    ProcedureSourceLabelV1, ProcedureWarningCodeV1, SessionAggregateV1, SessionId, Sha256Digest,
    UnixMillis, item_satisfied,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

fn parse_json(value: serde_json::Value) -> podway_config::ValidatedProcedureV1 {
    parse_procedure_v1(value.to_string(), ProcedureFormatV1::Json).unwrap()
}

fn base_document() -> serde_json::Value {
    json!({
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
}

fn base_yaml_document() -> String {
    concat!(
        "schema: podway.procedure/v1\n",
        "id: release\n",
        "version: \"1\"\n",
        "name: Release\n",
        "stages:\n",
        "  - id: prepare\n",
        "    title: Prepare\n",
        "    items:\n",
        "      - id: approval\n",
        "        type: confirm\n",
        "        prompt: Approved\n",
        "        required: true\n",
        "rework:\n",
        "  allow_return_to: [prepare]\n",
    )
    .to_owned()
}

fn yaml_document_with_name(name: &str) -> String {
    format!(
        concat!(
            "schema: podway.procedure/v1\n",
            "id: release\n",
            "version: \"1\"\n",
            "name: {name}\n",
            "stages:\n",
            "  - id: prepare\n",
            "    title: Prepare\n",
            "    items:\n",
            "      - id: approval\n",
            "        type: confirm\n",
            "        prompt: Approved\n",
            "        required: true\n",
            "rework:\n",
            "  allow_return_to: [prepare]\n",
        ),
        name = name,
    )
}

fn required_confirm_item(id: impl Into<String>, prompt: impl Into<String>) -> serde_json::Value {
    json!({
        "id": id.into(),
        "type": "confirm",
        "prompt": prompt.into(),
        "required": true,
    })
}

fn required_stage(index: usize) -> serde_json::Value {
    let id = if index == 0 {
        "prepare".to_owned()
    } else {
        format!("stage-{index}")
    };
    json!({
        "id": id,
        "title": format!("Stage {index}"),
        "items": [required_confirm_item(
            format!("approval-{index}"),
            format!("Prompt {index}"),
        )],
    })
}

fn document_with_stages(stages: Vec<serde_json::Value>) -> serde_json::Value {
    let mut document = base_document();
    document["stages"] = json!(stages);
    document
}

fn document_with_stage_contents(
    instructions: Vec<String>,
    items: Option<Vec<serde_json::Value>>,
) -> serde_json::Value {
    let mut document = base_document();
    document["stages"][0]["instructions"] = json!(instructions);
    if let Some(items) = items {
        document["stages"][0]["items"] = json!(items);
    }
    document
}

fn resource_node_document(
    instruction_count: usize,
    include_empty_items: bool,
) -> serde_json::Value {
    let mut document = base_document();
    document["description"] = json!("Boundary");
    document["stages"][0]["instructions"] = json!(vec!["Read"; instruction_count]);
    if include_empty_items {
        document["stages"][0]["items"] = json!([]);
    } else {
        document["stages"][0]
            .as_object_mut()
            .unwrap()
            .remove("items");
    }
    document["stages"][0]["skip"] = json!({ "allowed": true });
    document
}
fn yaml_document_without_items() -> String {
    concat!(
        "schema: podway.procedure/v1\n",
        "id: release\n",
        "version: \"1\"\n",
        "name: Release\n",
        "stages:\n",
        "  - id: prepare\n",
        "    title: Prepare\n",
        "rework:\n",
        "  allow_return_to: [prepare]\n",
    )
    .to_owned()
}

fn resource_node_yaml(instruction_count: usize, include_empty_items: bool) -> String {
    let instructions = "      - Read\n".repeat(instruction_count);
    let items = if include_empty_items {
        "    items: []\n"
    } else {
        ""
    };
    format!(
        concat!(
            "schema: podway.procedure/v1\n",
            "id: release\n",
            "version: \"1\"\n",
            "name: Release\n",
            "description: Boundary\n",
            "stages:\n",
            "  - id: prepare\n",
            "    title: Prepare\n",
            "    instructions:\n",
            "{instructions}",
            "{items}",
            "    skip:\n",
            "      allowed: true\n",
            "rework:\n",
            "  allow_return_to: [prepare]\n",
        ),
        instructions = instructions,
        items = items,
    )
}

fn limits(max_bytes: usize, max_depth: usize, max_nodes: usize) -> ProcedureParseLimitsV1 {
    ProcedureParseLimitsV1 {
        max_bytes,
        max_depth,
        max_nodes,
    }
}

fn procedure_warning(code: ProcedureWarningCodeV1) -> ProcedureWarningV1 {
    ProcedureWarningV1 {
        code,
        stage_id: None,
        item_id: None,
    }
}

fn stage_warning(code: ProcedureWarningCodeV1, stage_id: &str) -> ProcedureWarningV1 {
    ProcedureWarningV1 {
        code,
        stage_id: Some(stage_id.to_owned()),
        item_id: None,
    }
}

fn item_warning(
    code: ProcedureWarningCodeV1,
    stage_id: &str,
    item_id: impl Into<String>,
) -> ProcedureWarningV1 {
    ProcedureWarningV1 {
        code,
        stage_id: Some(stage_id.to_owned()),
        item_id: Some(item_id.into()),
    }
}

#[test]
fn yaml_json_and_explicit_defaults_have_one_canonical_digest() {
    let yaml = r#"
schema: podway.procedure/v1
id: release
version: "1"
name: Release
stages:
  - id: prepare
    title: Prepare
    skip:
      allowed: true
    items:
      - id: summary
        type: text
        prompt: Summary
        required: true
rework:
  allow_return_to:
    - prepare
"#;
    let explicit_json = json!({
        "schema": "podway.procedure/v1",
        "id": "release",
        "version": "1",
        "name": "Release",
        "stages": [{
            "id": "prepare",
            "title": "Prepare",
            "instructions": [],
            "skip": { "allowed": true, "reason_required": true },
            "items": [{
                "id": "summary",
                "type": "text",
                "prompt": "Summary",
                "required": true,
                "min_length": 0,
                "max_length": 8000,
                "multiline": true
            }]
        }],
        "rework": { "allow_return_to": ["prepare"] }
    });

    let from_yaml = parse_procedure_v1(yaml, ProcedureFormatV1::Yaml).unwrap();
    let from_json = parse_json(explicit_json);
    let expected_digest = format!(
        "sha256:{:x}",
        Sha256::digest(from_yaml.canonical_json().as_bytes())
    );

    assert_eq!(from_yaml.canonical_json(), from_json.canonical_json());
    assert_eq!(from_yaml.digest(), from_json.digest());
    assert_eq!(from_yaml.digest().as_str(), expected_digest);
}
#[test]
fn canonical_json_and_digest_match_golden_fixture() {
    let procedure = parse_json(base_document());

    assert_eq!(
        procedure.canonical_json().as_bytes(),
        br#"{"id":"release","name":"Release","rework":{"allow_return_to":["prepare"]},"schema":"podway.procedure/v1","stages":[{"id":"prepare","instructions":[],"items":[{"id":"approval","prompt":"Approved","required":true,"type":"confirm"}],"title":"Prepare"}],"version":"1"}"#
    );
    assert_eq!(
        procedure.digest().as_str(),
        "sha256:2d2e9f9453b36610949989fadbae5b3665778701aae59727fe4fa1330b5e0c5a"
    );
}
#[test]
fn yaml_and_json_default_forms_share_canonical_bytes_and_digest() {
    let yaml = r#"
schema: podway.procedure/v1
id: release
version: "1"
name: Release
stages:
  - id: prepare
    title: Prepare
    items:
      - id: summary
        type: text
        prompt: Summary
        required: true
      - id: reviewers
        type: list
        prompt: Reviewers
        required: true
      - id: artifact
        type: artifact
        prompt: Artifact
        required: true
rework:
  allow_return_to:
    - prepare
"#;
    let explicit_json = json!({
        "schema": "podway.procedure/v1",
        "id": "release",
        "version": "1",
        "name": "Release",
        "stages": [{
            "id": "prepare",
            "title": "Prepare",
            "instructions": [],
            "skip": { "allowed": false },
            "items": [
                {
                    "id": "summary",
                    "type": "text",
                    "prompt": "Summary",
                    "required": true,
                    "min_length": 0,
                    "max_length": 8000,
                    "multiline": true
                },
                {
                    "id": "reviewers",
                    "type": "list",
                    "prompt": "Reviewers",
                    "required": true,
                    "min_items": 0,
                    "max_items": 100,
                    "max_item_length": 500,
                    "unique": true
                },
                {
                    "id": "artifact",
                    "type": "artifact",
                    "prompt": "Artifact",
                    "required": true,
                    "allowed_media_types": []
                }
            ]
        }],
        "rework": { "allow_return_to": ["prepare"] }
    });

    let from_yaml = parse_procedure_v1(yaml, ProcedureFormatV1::Yaml).unwrap();
    let from_json = parse_json(explicit_json);

    assert_eq!(
        from_yaml.canonical_json().as_bytes(),
        from_json.canonical_json().as_bytes()
    );
    assert_eq!(from_yaml.digest(), from_json.digest());
}

#[test]
fn yaml_block_scalars_do_not_trigger_unsupported_feature_scanning() {
    let yaml = r#"
schema: podway.procedure/v1
id: release
version: "1"
name: |
  Release &not-an-anchor
  *not-an-alias
  !not-a-tag
  <<: not-a-merge
stages:
  - id: prepare
    title: >
      Prepare ?not-an-explicit-key
      &not-an-anchor
    instructions:
      - |
        Nested block sequence:
        <<: not-a-merge
        !not-a-tag
        *not-an-alias
        ?not-an-explicit-key
    items:
      - id: approval
        type: confirm
        prompt: Approved
        required: true
rework:
  allow_return_to:
    - prepare
"#;

    assert!(parse_procedure_v1(yaml, ProcedureFormatV1::Yaml).is_ok());
}
#[test]
fn compact_sequence_mapping_block_scalars_do_not_trigger_unsupported_feature_scanning() {
    let yaml = r#"
schema: podway.procedure/v1
id: release
version: "1"
name: Release
description: >+2
  Release &not-an-anchor
  *not-an-alias
  !not-a-tag
  ? not-an-explicit-key
  <<: not-a-merge
stages:
  - title: |
      Prepare &not-an-anchor
      *not-an-alias
      !not-a-tag
      ? not-an-explicit-key
      <<: not-a-merge
    id: prepare
  - title: |2-
      Review &not-an-anchor
      *not-an-alias
      !not-a-tag
      ? not-an-explicit-key
      <<: not-a-merge
    id: review
    items:
      - prompt: >-
          Approved &not-an-anchor
          *not-an-alias
          !not-a-tag
          ? not-an-explicit-key
          <<: not-a-merge
        id: approval
        type: confirm
        required: true
rework:
  allow_return_to:
    - prepare
"#;

    assert!(parse_procedure_v1(yaml, ProcedureFormatV1::Yaml).is_ok());
}
#[test]
fn yaml_quoted_multiline_and_flow_scalars_mask_feature_punctuation() {
    let yaml = r#"
schema: podway.procedure/v1
id: release
version: "1"
name: "Release &not-an-anchor
  *not-an-alias !not-a-tag ?not-an-explicit-key <<: not-a-merge"
stages: [{id: prepare, title: "Prepare ?not-an-explicit-key &not-an-anchor *not-an-alias !not-a-tag <<: not-a-merge", instructions: ["Read \"?not-an-explicit-key &not-an-anchor *not-an-alias !not-a-tag <<: not-a-merge"], items: [{id: approval, type: confirm, prompt: "Approved ?not-an-explicit-key &not-an-anchor *not-an-alias !not-a-tag <<: not-a-merge", required: true}]}]
rework: {allow_return_to: [prepare]}
"#;

    assert!(parse_procedure_v1(yaml, ProcedureFormatV1::Yaml).is_ok());
}

#[test]
fn object_key_order_and_whitespace_do_not_change_admission() {
    let ordered = r#"{"schema":"podway.procedure/v1","id":"release","version":"1","name":"Release","stages":[{"id":"prepare","title":"Prepare","items":[{"id":"approval","type":"confirm","prompt":"Approved","required":true}]}],"rework":{"allow_return_to":["prepare"]}}"#;
    let reordered = r#"
    {
      "rework": { "allow_return_to": [ "prepare" ] },
      "stages": [ { "items": [ { "required": true, "prompt": "Approved", "type": "confirm", "id": "approval" } ], "title": "Prepare", "id": "prepare" } ],
      "name": "Release", "version": "1", "id": "release", "schema": "podway.procedure/v1"
    }
    "#;

    let ordered = parse_procedure_v1(ordered, ProcedureFormatV1::Json).unwrap();
    let reordered = parse_procedure_v1(reordered, ProcedureFormatV1::Json).unwrap();
    assert_eq!(ordered.canonical_json(), reordered.canonical_json());
    assert_eq!(ordered.digest(), reordered.digest());
}

#[test]
fn pac_011_016_custom_procedure_validates_snapshots_and_starts_with_all_item_constraints() {
    let procedure = parse_json(json!({
        "schema": "podway.procedure/v1",
        "id": "release",
        "version": "1",
        "name": "Release",
        "description": "All kinds",
        "stages": [{
            "id": "prepare",
            "title": "Prepare",
            "instructions": ["Inspect."],
            "items": [
                {"id":"confirm","type":"confirm","prompt":"Confirm","help":"h","required":true},
                {"id":"text","type":"text","prompt":"Text","required":true,"min_length":1,"max_length":2,"multiline":false},
                {"id":"choice","type":"choice","prompt":"Choice","required":true,"choices":["one","two"]},
                {"id":"integer","type":"integer","prompt":"Integer","required":true,"minimum":-1,"maximum":1},
                {"id":"list","type":"list","prompt":"List","required":true,"min_items":1,"max_items":2,"max_item_length":3,"unique":false},
                {"id":"artifact","type":"artifact","prompt":"Artifact","required":true,"allowed_media_types":["text/plain"]}
            ]
        }],
        "rework": { "allow_return_to": ["prepare"] }
    }));
    let canonical = procedure.canonical_json().as_str().to_owned();
    let digest = procedure.digest().clone();
    let source = ProcedureSourceLabel::workspace_path("release.json").unwrap();
    let snapshot = procedure
        .into_snapshot_v1(
            ProcedureSnapshotId::new("123e4567-e89b-12d3-a456-426614174000").unwrap(),
            source.clone(),
            UnixMillis::new(42),
            ProcedureWarningPolicyV1::Accept,
        )
        .unwrap();

    assert_eq!(snapshot.canonical_json().as_str(), canonical);
    assert_eq!(snapshot.digest(), &digest);
    assert_eq!(
        snapshot.source_label(),
        &ProcedureSourceLabelV1::new("procedure:release.json").unwrap()
    );
    assert_eq!(snapshot.created_at(), UnixMillis::new(42));
    assert_eq!(
        snapshot.stages()[0]
            .items()
            .iter()
            .map(|item| item.item_type())
            .collect::<Vec<_>>(),
        vec![
            ItemTypeV1::Confirm,
            ItemTypeV1::Text,
            ItemTypeV1::Choice,
            ItemTypeV1::Integer,
            ItemTypeV1::List,
            ItemTypeV1::Artifact,
        ]
    );
    let session = SessionAggregateV1::start(
        SessionId::new("123e4567-e89b-42d3-a456-426614174001").unwrap(),
        "custom procedure boundary",
        snapshot,
        AttemptId::new("123e4567-e89b-42d3-a456-426614174002").unwrap(),
        UnixMillis::new(43),
    )
    .expect("a validated custom-procedure snapshot must start a session");
    let attempt = session
        .active_attempt_id()
        .and_then(|id| {
            session
                .attempts()
                .iter()
                .find(|attempt| attempt.attempt_id() == id)
        })
        .expect("started custom procedure must have one active attempt");
    assert_eq!(
        attempt.item_slots().len(),
        6,
        "start boundary must materialize every validated custom item type"
    );
    assert_eq!(
        attempt
            .item_slots()
            .iter()
            .map(|slot| (slot.item_id().as_str(), slot.item_type()))
            .collect::<Vec<_>>(),
        vec![
            ("confirm", ItemTypeV1::Confirm),
            ("text", ItemTypeV1::Text),
            ("choice", ItemTypeV1::Choice),
            ("integer", ItemTypeV1::Integer),
            ("list", ItemTypeV1::List),
            ("artifact", ItemTypeV1::Artifact),
        ],
        "start boundary must materialize every custom definition with its declared type"
    );
    let started_items = &session.snapshot().stages()[0].items();
    assert_eq!(
        started_items
            .iter()
            .map(|item| item.id().as_str())
            .collect::<Vec<_>>(),
        ["confirm", "text", "choice", "integer", "list", "artifact"],
        "session creation must retain all custom item identities in their declared order"
    );
    assert!(matches!(
        started_items[0],
        podway_core::ItemSpecV1::Confirm(_)
    ));
    let podway_core::ItemSpecV1::Text(text) = &started_items[1] else {
        panic!("started text item must retain its type");
    };
    assert_eq!(
        (text.min_length(), text.max_length(), text.multiline()),
        (1, 2, false)
    );
    let podway_core::ItemSpecV1::Choice(choice) = &started_items[2] else {
        panic!("started choice item must retain its type");
    };
    assert_eq!(choice.choices(), &["one".to_owned(), "two".to_owned()]);
    let podway_core::ItemSpecV1::Integer(integer) = &started_items[3] else {
        panic!("started integer item must retain its type");
    };
    assert_eq!((integer.minimum(), integer.maximum()), (Some(-1), Some(1)));
    let podway_core::ItemSpecV1::List(list) = &started_items[4] else {
        panic!("started list item must retain its type");
    };
    assert_eq!(
        (
            list.min_items(),
            list.max_items(),
            list.max_item_length(),
            list.unique(),
        ),
        (1, 2, 3, false)
    );
    let podway_core::ItemSpecV1::Artifact(artifact) = &started_items[5] else {
        panic!("started artifact item must retain its type");
    };
    assert_eq!(artifact.allowed_media_types(), &["text/plain".to_owned()]);
    assert!(
        item_satisfied(&started_items[0], Some(&ItemValueV1::confirm())),
        "the started confirm item must retain check behavior"
    );
    assert!(
        item_satisfied(&started_items[1], Some(&ItemValueV1::text("o")))
            && item_satisfied(&started_items[1], Some(&ItemValueV1::text("ok")))
            && item_satisfied(&started_items[1], Some(&ItemValueV1::text("o\n")))
            && !item_satisfied(&started_items[1], Some(&ItemValueV1::text("")))
            && !item_satisfied(&started_items[1], Some(&ItemValueV1::text("too"))),
        "the started text item must retain its length constraints while multiline remains presentation metadata"
    );
    assert!(
        item_satisfied(
            &started_items[2],
            Some(&ItemValueV1::choice("two").expect("choice value must be well formed"))
        ) && !item_satisfied(
            &started_items[2],
            Some(&ItemValueV1::choice("other").expect("choice value must be well formed"))
        ),
        "the started choice item must retain its declared choices"
    );
    assert!(
        item_satisfied(&started_items[3], Some(&ItemValueV1::integer(-1)))
            && item_satisfied(&started_items[3], Some(&ItemValueV1::integer(1)))
            && !item_satisfied(&started_items[3], Some(&ItemValueV1::integer(-2)))
            && !item_satisfied(&started_items[3], Some(&ItemValueV1::integer(2))),
        "the started integer item must retain both inclusive bounds"
    );
    assert!(
        item_satisfied(
            &started_items[4],
            Some(&ItemValueV1::list(vec!["a".to_owned()]).unwrap())
        ) && item_satisfied(
            &started_items[4],
            Some(&ItemValueV1::list(vec!["a".to_owned(), "a".to_owned()]).unwrap())
        ) && !item_satisfied(
            &started_items[4],
            Some(&ItemValueV1::list(Vec::new()).unwrap())
        ) && !item_satisfied(
            &started_items[4],
            Some(&ItemValueV1::list(vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]).unwrap())
        ) && !item_satisfied(
            &started_items[4],
            Some(&ItemValueV1::list(vec!["long".to_owned()]).unwrap())
        ),
        "the started list item must retain minimum, maximum, item-length, and uniqueness constraints"
    );
    let digest =
        Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).expect("test digest must be valid");
    assert!(
        item_satisfied(
            &started_items[5],
            Some(&ItemValueV1::artifact(
                ArtifactValueV1::local_path("reports/result.txt", digest.clone(), 1, "text/plain")
                    .expect("test artifact must be valid")
            ))
        ) && !item_satisfied(
            &started_items[5],
            Some(&ItemValueV1::artifact(
                ArtifactValueV1::local_path("reports/result.json", digest, 1, "application/json")
                    .expect("test artifact must be valid")
            ))
        ),
        "the started artifact item must retain its allowed media types"
    );
}

#[test]
fn parser_rejects_duplicate_keys_noncanonical_numbers_and_unknown_or_null_fields() {
    for (format, input) in [
        (
            ProcedureFormatV1::Json,
            r#"{"schema":"podway.procedure/v1","schema":"podway.procedure/v1"}"#,
        ),
        (
            ProcedureFormatV1::Yaml,
            "schema: podway.procedure/v1\nschema: podway.procedure/v1\n",
        ),
    ] {
        assert_eq!(
            parse_procedure_v1(input, format),
            Err(ConfigError::DuplicateKey {
                key: "schema".to_owned(),
            })
        );
    }

    for (format, input) in [
        (ProcedureFormatV1::Json, r#"{"schema":1.0}"#),
        (ProcedureFormatV1::Yaml, "schema: 1.0\n"),
    ] {
        assert_eq!(
            parse_procedure_v1(input, format),
            Err(ConfigError::NonCanonicalNumber)
        );
    }

    assert_eq!(
        parse_procedure_v1("schema: null\n", ProcedureFormatV1::Yaml),
        Err(ConfigError::InvalidDocument {
            reason: "explicit null is not allowed by procedure v1".to_owned(),
        })
    );
    assert_eq!(
        parse_procedure_v1("1: podway.procedure/v1\n", ProcedureFormatV1::Yaml),
        Err(ConfigError::InvalidDocument {
            reason: "mapping keys must be strings".to_owned(),
        })
    );

    for (field, value) in [
        ("include", json!("https://example.invalid/procedure.yaml")),
        ("extends", json!("base.yaml")),
        ("command", json!("sh -c 'rm -rf /'")),
    ] {
        let mut unknown = base_document();
        unknown[field] = value;
        assert!(parse_procedure_v1(unknown.to_string(), ProcedureFormatV1::Json).is_err());
    }
    assert!(
        serde_json::from_str::<ProcedureSourceLabel>(
            r#"{"kind":"preset","label":"sw-dev","unexpected":true}"#,
        )
        .is_err()
    );
    let mut unsupported_schema = base_document();
    unsupported_schema["schema"] = json!("podway.procedure/v2");
    assert!(matches!(
        parse_procedure_v1(unsupported_schema.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::InvalidSchema { .. })
    ));

    let mut unsupported_type = base_document();
    unsupported_type["stages"][0]["items"][0]["type"] = json!("remote");
    assert!(parse_procedure_v1(unsupported_type.to_string(), ProcedureFormatV1::Json).is_err());

    let mut null = base_document();
    null["description"] = serde_json::Value::Null;
    assert!(matches!(
        parse_procedure_v1(null.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::InvalidDocument { .. })
    ));
}
#[test]
fn yaml_scanner_rejects_noncanonical_numbers_and_allows_prefix_strings() {
    for scalar in ["+1", "01", "-0", "1.0", "1e2", ".inf", ".nan"] {
        assert_eq!(
            parse_procedure_v1(format!("schema: {scalar}\n"), ProcedureFormatV1::Yaml,),
            Err(ConfigError::NonCanonicalNumber),
            "{scalar}"
        );
    }

    for prefix in ["0x", "0X", "0o", "0O", "0b", "0B"] {
        for sign in ["", "+", "-"] {
            let scalar = format!("{sign}{prefix}10");
            assert_eq!(
                parse_procedure_v1(format!("schema: {scalar}\n"), ProcedureFormatV1::Yaml,),
                Err(ConfigError::NonCanonicalNumber),
                "{scalar}"
            );
        }
    }

    for prefix in ["0x", "0X", "0o", "0O", "0b", "0B"] {
        for sign in ["", "+", "-"] {
            let scalar = format!("{sign}{prefix}1_0");
            assert_eq!(
                parse_procedure_v1(format!("schema: {scalar}\n"), ProcedureFormatV1::Yaml,),
                Err(ConfigError::NonCanonicalNumber),
                "{scalar}"
            );
        }
    }

    for scalar in [
        "1_000", "+1_000", "-1_000", "1_0.0", "1.0_0", "1_0e1", "1e1_0",
    ] {
        assert_eq!(
            parse_procedure_v1(format!("schema: {scalar}\n"), ProcedureFormatV1::Yaml,),
            Err(ConfigError::NonCanonicalNumber),
            "{scalar}"
        );
    }

    for name in [
        "+Release",
        "0xrelease",
        "0Xrelease",
        "0orelease",
        "0Orelease",
        "0brelease",
        "0Brelease",
        "+0xrelease",
        "-0xrelease",
        "+0Xrelease",
        "-0Xrelease",
        "+0orelease",
        "-0Orelease",
        "+0brelease",
        "-0Brelease",
        ".release",
        "1_e",
        "1_-2",
        "1e_",
        "1._2",
    ] {
        parse_procedure_v1(yaml_document_with_name(name), ProcedureFormatV1::Yaml)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn parser_rejects_yaml_aliases_tags_directives_merges_and_explicit_keys() {
    for (case, input, feature) in [
        ("anchor", "schema: &schema podway.procedure/v1\n", "anchor"),
        ("alias", "schema: *schema\n", "alias"),
        ("tag", "schema: !include remote.yaml\n", "tag"),
        (
            "directive",
            "%YAML 1.2\n---\nschema: podway.procedure/v1\n",
            "tag",
        ),
        (
            "top-level merge key",
            "<<: { schema: podway.procedure/v1 }\n",
            "merge key",
        ),
        (
            "nested block-map merge key",
            "schema: podway.procedure/v1\nmetadata:\n  nested:\n    <<: {}\n",
            "merge key",
        ),
        (
            "flow-map merge key",
            "schema: podway.procedure/v1\nmetadata: { nested: { <<: {} } }\n",
            "merge key",
        ),
        (
            "block-sequence-map merge key",
            "schema: podway.procedure/v1\nmetadata:\n  - <<: {}\n",
            "merge key",
        ),
        (
            "flow-sequence-map merge key",
            "schema: podway.procedure/v1\nmetadata: [{ <<: {} }]\n",
            "merge key",
        ),
        (
            "block explicit mapping key",
            "? schema\n: podway.procedure/v1\n",
            "explicit mapping key",
        ),
        (
            "flow explicit mapping key",
            "{? schema: podway.procedure/v1}\n",
            "explicit mapping key",
        ),
        (
            "multibyte-prefix explicit mapping key",
            "name: 東京\n? schema\n: podway.procedure/v1\n",
            "explicit mapping key",
        ),
    ] {
        assert_eq!(
            parse_procedure_v1(input, ProcedureFormatV1::Yaml),
            Err(ConfigError::UnsupportedYamlFeature { feature }),
            "{case}"
        );
    }

    for (case, name) in [
        ("quoted scalar", "\"<<: not-a-merge\""),
        ("plain scalar", "<<"),
        ("block scalar", "|\n  <<: not-a-merge"),
    ] {
        parse_procedure_v1(yaml_document_with_name(name), ProcedureFormatV1::Yaml)
            .unwrap_or_else(|error| panic!("{case}: {error}"));
    }
}

#[test]
fn yaml_preflight_order_precedes_deserialization_but_not_byte_limits() {
    let preflight_limits = limits(1_024, 0, 0);
    for (case, input) in [
        (
            "duplicate key",
            "schema: podway.procedure/v1\nschema: !tag duplicate\n",
        ),
        ("explicit null", "schema: null\nname: !tag Release\n"),
        ("syntax", "schema: !tag value\n["),
        (
            "depth and node limits",
            "stages: [[[[[[0]]]]]]\nschema: !tag podway.procedure/v1\n",
        ),
    ] {
        assert_eq!(
            parse_procedure_v1_with_limits(input, ProcedureFormatV1::Yaml, preflight_limits),
            Err(ConfigError::UnsupportedYamlFeature { feature: "tag" }),
            "{case}"
        );
    }

    let input = "schema: !tag podway.procedure/v1\n";
    assert_eq!(
        parse_procedure_v1_with_limits(
            input,
            ProcedureFormatV1::Yaml,
            limits(input.len() - 1, 64, 64),
        ),
        Err(ConfigError::InputTooLarge {
            maximum: input.len() - 1,
            actual: input.len(),
        })
    );
}

#[test]
fn parser_enforces_size_depth_and_node_limits() {
    let oversized = vec![b' '; MAX_PROCEDURE_DOCUMENT_BYTES_V1 + 1];
    assert_eq!(
        parse_procedure_v1(oversized, ProcedureFormatV1::Json),
        Err(ConfigError::InputTooLarge {
            maximum: MAX_PROCEDURE_DOCUMENT_BYTES_V1,
            actual: MAX_PROCEDURE_DOCUMENT_BYTES_V1 + 1,
        })
    );

    let mut deep = "0".to_owned();
    for _ in 0..MAX_PROCEDURE_DOCUMENT_DEPTH_V1 {
        deep = format!("[{deep}]");
    }
    assert_eq!(
        parse_procedure_v1(deep, ProcedureFormatV1::Json),
        Err(ConfigError::InputTooDeep {
            maximum: MAX_PROCEDURE_DOCUMENT_DEPTH_V1,
            actual: MAX_PROCEDURE_DOCUMENT_DEPTH_V1 + 1,
        })
    );

    let nodes = format!(
        "[{}]",
        std::iter::repeat_n("0", MAX_PROCEDURE_DOCUMENT_NODES_V1)
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(
        parse_procedure_v1(nodes, ProcedureFormatV1::Json),
        Err(ConfigError::InputTooComplex {
            maximum: MAX_PROCEDURE_DOCUMENT_NODES_V1,
            actual: MAX_PROCEDURE_DOCUMENT_NODES_V1 + 1,
        })
    );

    let limits = ProcedureParseLimitsV1 {
        max_bytes: 1_024,
        max_depth: 2,
        max_nodes: 8,
    };
    for format in [ProcedureFormatV1::Json, ProcedureFormatV1::Yaml] {
        assert_eq!(
            parse_procedure_v1_with_limits("[[0]]", format, limits),
            Err(ConfigError::InputTooDeep {
                maximum: 2,
                actual: 3,
            })
        );
    }

    let yaml_node_limits = ProcedureParseLimitsV1 {
        max_bytes: 1_024,
        max_depth: 8,
        max_nodes: 1,
    };
    assert_eq!(
        parse_procedure_v1_with_limits(
            "schema: podway.procedure/v1\n",
            ProcedureFormatV1::Yaml,
            yaml_node_limits,
        ),
        Err(ConfigError::InputTooComplex {
            maximum: 1,
            actual: 2,
        })
    );
}
#[test]
fn custom_limits_admit_limit_minus_one_and_equality_before_first_overflow() {
    for format in [ProcedureFormatV1::Json, ProcedureFormatV1::Yaml] {
        let input = match format {
            ProcedureFormatV1::Json => base_document().to_string(),
            ProcedureFormatV1::Yaml => base_yaml_document(),
        };
        let at_limit = format!("{input} ");
        let first_overflow = format!("{at_limit} ");
        let byte_limit = at_limit.len();
        parse_procedure_v1_with_limits(&input, format, limits(byte_limit, 8, 64))
            .unwrap_or_else(|error| panic!("{format:?} bytes below limit: {error}"));
        parse_procedure_v1_with_limits(&at_limit, format, limits(byte_limit, 8, 64))
            .unwrap_or_else(|error| panic!("{format:?} bytes at limit: {error}"));
        assert_eq!(
            parse_procedure_v1_with_limits(&first_overflow, format, limits(byte_limit, 8, 64),),
            Err(ConfigError::InputTooLarge {
                maximum: byte_limit,
                actual: byte_limit + 1,
            }),
            "{format:?} bytes first overflow"
        );

        let (below_depth, at_depth, first_depth_overflow) = match format {
            ProcedureFormatV1::Json => {
                let mut below_depth = base_document();
                below_depth["stages"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("items");
                (
                    below_depth.to_string(),
                    resource_node_document(1, false).to_string(),
                    base_document().to_string(),
                )
            }
            ProcedureFormatV1::Yaml => (
                yaml_document_without_items(),
                resource_node_yaml(1, false),
                base_yaml_document(),
            ),
        };
        parse_procedure_v1_with_limits(&below_depth, format, limits(1_024, 5, 64))
            .unwrap_or_else(|error| panic!("{format:?} depth below limit: {error}"));
        parse_procedure_v1_with_limits(&at_depth, format, limits(1_024, 5, 64))
            .unwrap_or_else(|error| panic!("{format:?} depth at limit: {error}"));
        assert_eq!(
            parse_procedure_v1_with_limits(&first_depth_overflow, format, limits(1_024, 5, 64),),
            Err(ConfigError::InputTooDeep {
                maximum: 5,
                actual: 6,
            }),
            "{format:?} depth first overflow"
        );

        let (below_nodes, at_nodes, first_node_overflow) = match format {
            ProcedureFormatV1::Json => (
                resource_node_document(1, false).to_string(),
                resource_node_document(1, true).to_string(),
                resource_node_document(2, true).to_string(),
            ),
            ProcedureFormatV1::Yaml => (
                resource_node_yaml(1, false),
                resource_node_yaml(1, true),
                resource_node_yaml(2, true),
            ),
        };
        parse_procedure_v1_with_limits(&below_nodes, format, limits(1_024, 6, 18))
            .unwrap_or_else(|error| panic!("{format:?} nodes below limit: {error}"));
        parse_procedure_v1_with_limits(&at_nodes, format, limits(1_024, 6, 18))
            .unwrap_or_else(|error| panic!("{format:?} nodes at limit: {error}"));
        assert_eq!(
            parse_procedure_v1_with_limits(&first_node_overflow, format, limits(1_024, 6, 18),),
            Err(ConfigError::InputTooComplex {
                maximum: 18,
                actual: 19,
            }),
            "{format:?} nodes first overflow"
        );
    }
}

#[test]
fn yaml_requires_exactly_one_document() {
    assert_eq!(
        parse_procedure_v1("", ProcedureFormatV1::Yaml),
        Err(ConfigError::InvalidDocument {
            reason: "procedure document must not be empty".to_owned(),
        })
    );

    parse_procedure_v1(
        format!("---\n{}", base_yaml_document()),
        ProcedureFormatV1::Yaml,
    )
    .unwrap_or_else(|error| panic!("one YAML document: {error}"));

    assert_eq!(
        parse_procedure_v1(
            format!("---\n{}---\n{}", base_yaml_document(), base_yaml_document(),),
            ProcedureFormatV1::Yaml,
        ),
        Err(ConfigError::InvalidDocument {
            reason: "procedure document must contain exactly one YAML document".to_owned(),
        })
    );
}
#[test]
fn near_hard_limit_warnings_are_inclusive_and_scoped() {
    assert_eq!(
        parse_json(document_with_stages((0..57).map(required_stage).collect())).warnings(),
        &[]
    );
    assert_eq!(
        parse_json(document_with_stages((0..58).map(required_stage).collect())).warnings(),
        &[procedure_warning(
            ProcedureWarningCodeV1::StageNearHardLimits,
        )]
    );

    assert_eq!(
        parse_json(document_with_stage_contents(
            vec!["Read".to_owned(); 28],
            None,
        ))
        .warnings(),
        &[]
    );
    assert_eq!(
        parse_json(document_with_stage_contents(
            vec!["Read".to_owned(); 29],
            None,
        ))
        .warnings(),
        &[stage_warning(
            ProcedureWarningCodeV1::StageNearHardLimits,
            "prepare",
        )]
    );

    let below_item_threshold = (0..115)
        .map(|index| required_confirm_item(format!("item-{index}"), format!("Prompt {index}")))
        .collect();
    assert_eq!(
        parse_json(document_with_stage_contents(
            Vec::new(),
            Some(below_item_threshold),
        ))
        .warnings(),
        &[]
    );
    let at_item_threshold = (0..116)
        .map(|index| required_confirm_item(format!("item-{index}"), format!("Prompt {index}")))
        .collect();
    assert_eq!(
        parse_json(document_with_stage_contents(
            Vec::new(),
            Some(at_item_threshold),
        ))
        .warnings(),
        &[stage_warning(
            ProcedureWarningCodeV1::StageNearHardLimits,
            "prepare",
        )]
    );

    let instruction_at_1_799_characters = format!("{}{}", "é".repeat(899), "x".repeat(900));
    assert_eq!(
        parse_json(document_with_stage_contents(
            vec![instruction_at_1_799_characters],
            None,
        ))
        .warnings(),
        &[]
    );
    let instruction_at_1_800_characters = format!("{}{}", "é".repeat(900), "x".repeat(900));
    assert_eq!(
        parse_json(document_with_stage_contents(
            vec![instruction_at_1_800_characters],
            None,
        ))
        .warnings(),
        &[stage_warning(
            ProcedureWarningCodeV1::StageNearHardLimits,
            "prepare",
        )]
    );
}

#[test]
fn semantic_validation_rejects_boundaries_and_emits_every_warning_category() {
    let duplicate_stage = json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{"id":"prepare","title":"One"},{"id":"prepare","title":"Two"}],
        "rework":{"allow_return_to":"any_previous"}
    });
    assert!(matches!(
        parse_procedure_v1(duplicate_stage.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::DuplicateValue {
            field: "stage.id",
            ..
        })
    ));

    let invalid_return = json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{"id":"prepare","title":"Prepare"}],
        "rework":{"allow_return_to":["missing"]}
    });
    assert!(matches!(
        parse_procedure_v1(invalid_return.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::UnknownReturnTarget { .. })
    ));

    let invalid_constraints = json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{"id":"prepare","title":"Prepare","items":[{"id":"text","type":"text","prompt":"Text","required":true,"min_length":2,"max_length":1}]}],
        "rework":{"allow_return_to":["prepare"]}
    });
    assert!(matches!(
        parse_procedure_v1(invalid_constraints.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::InvalidValue {
            field: "item.text.length",
            ..
        })
    ));

    let duplicate_item = json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{"id":"prepare","title":"Prepare","items":[
            {"id":"approval","type":"confirm","prompt":"One","required":true},
            {"id":"approval","type":"confirm","prompt":"Two","required":true}
        ]}],
        "rework":{"allow_return_to":["prepare"]}
    });
    assert!(matches!(
        parse_procedure_v1(duplicate_item.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::DuplicateValue {
            field: "item.id",
            ..
        })
    ));

    let duplicate_choices = json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{"id":"prepare","title":"Prepare","items":[{
            "id":"choice","type":"choice","prompt":"Choice","required":true,"choices":["one","one"]
        }]}],
        "rework":{"allow_return_to":["prepare"]}
    });
    assert!(matches!(
        parse_procedure_v1(duplicate_choices.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::DuplicateValue {
            field: "item.choice.choices",
            ..
        })
    ));

    let duplicate_media = json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{"id":"prepare","title":"Prepare","items":[{
            "id":"artifact","type":"artifact","prompt":"Artifact","required":true,
            "allowed_media_types":["text/plain","text/plain"]
        }]}],
        "rework":{"allow_return_to":["prepare"]}
    });
    assert!(matches!(
        parse_procedure_v1(duplicate_media.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::DuplicateValue {
            field: "item.artifact.allowed_media_types",
            ..
        })
    ));

    let duplicate_destinations = json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{"id":"prepare","title":"Prepare"}],
        "rework":{"allow_return_to":["prepare","prepare"]}
    });
    assert!(matches!(
        parse_procedure_v1(duplicate_destinations.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::DuplicateValue {
            field: "rework.allow_return_to",
            ..
        })
    ));
    let near_limit_items = (0..116)
        .map(|index| {
            json!({
                "id": format!("item-{index}"),
                "type": "confirm",
                "prompt": if index < 2 { "Repeated prompt" } else { "This must be supplied" },
                "required": false
            })
        })
        .collect::<Vec<_>>();
    let warnings = parse_json(json!({
        "schema":"podway.procedure/v1", "id":"release", "version":"1", "name":"Release",
        "stages":[{
            "id":"prepare", "title":"Prepare", "instructions": vec!["Read"; 29],
            "items": near_limit_items
        }, {
            "id":"finish", "title":"Finish", "skip":{"allowed":true}
        }],
        "rework":{"allow_return_to":"any_previous"}
    }));
    let mut expected_warnings = vec![
        procedure_warning(ProcedureWarningCodeV1::AnyPreviousReturnPolicy),
        stage_warning(ProcedureWarningCodeV1::StageHasNoRequiredItems, "prepare"),
        stage_warning(ProcedureWarningCodeV1::StageNearHardLimits, "prepare"),
        item_warning(ProcedureWarningCodeV1::RepeatedPrompt, "prepare", "item-1"),
        item_warning(
            ProcedureWarningCodeV1::OptionalItemAppearsRequired,
            "prepare",
            "item-2",
        ),
    ];
    for index in 3..116 {
        expected_warnings.push(item_warning(
            ProcedureWarningCodeV1::RepeatedPrompt,
            "prepare",
            format!("item-{index}"),
        ));
        expected_warnings.push(item_warning(
            ProcedureWarningCodeV1::OptionalItemAppearsRequired,
            "prepare",
            format!("item-{index}"),
        ));
    }
    expected_warnings.push(stage_warning(
        ProcedureWarningCodeV1::StageHasNoRequiredItems,
        "finish",
    ));
    expected_warnings.push(stage_warning(
        ProcedureWarningCodeV1::FinalStageSkippable,
        "finish",
    ));
    assert_eq!(warnings.warnings(), expected_warnings.as_slice());

    let expected_warning_codes = vec![
        ProcedureWarningCodeV1::StageHasNoRequiredItems,
        ProcedureWarningCodeV1::StageNearHardLimits,
        ProcedureWarningCodeV1::FinalStageSkippable,
        ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ProcedureWarningCodeV1::RepeatedPrompt,
        ProcedureWarningCodeV1::OptionalItemAppearsRequired,
    ];
    assert_eq!(
        warnings.clone().admit(ProcedureWarningPolicyV1::Reject),
        Err(ConfigError::WarningsAsErrors {
            warnings: expected_warning_codes.clone(),
        })
    );
    let accepted = warnings
        .clone()
        .admit(ProcedureWarningPolicyV1::Accept)
        .expect("accepting warnings must preserve the validated procedure");
    assert_eq!(accepted.warnings(), expected_warnings.as_slice());

    let accepted_snapshot = warnings
        .clone()
        .into_snapshot_v1(
            ProcedureSnapshotId::new("123e4567-e89b-12d3-a456-426614174001").unwrap(),
            ProcedureSourceLabel::workspace_path("warning.json").unwrap(),
            UnixMillis::new(43),
            ProcedureWarningPolicyV1::Accept,
        )
        .expect("accepting warnings must persist the complete warning-code set");
    assert_eq!(
        accepted_snapshot.accepted_warning_codes(),
        expected_warning_codes.as_slice()
    );

    assert_eq!(
        warnings.into_snapshot_v1(
            ProcedureSnapshotId::new("123e4567-e89b-12d3-a456-426614174001").unwrap(),
            ProcedureSourceLabel::workspace_path("warning.json").unwrap(),
            UnixMillis::new(43),
            ProcedureWarningPolicyV1::Reject,
        ),
        Err(ConfigError::WarningsAsErrors {
            warnings: expected_warning_codes,
        })
    );
}
