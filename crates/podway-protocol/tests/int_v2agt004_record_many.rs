use podway_protocol::{
    ITEM_RECORD_MANY_INPUT_SCHEMA_V1, ItemRecordManyDispositionV1, ItemRecordManyV1,
    ItemRecordValueV1, MAX_DIRECT_INPUT_BYTES_V1, SessionMutationPreconditionsWireV1,
    SliceCommandV1, SliceErrorV1, canonical_direct_slice_mutation_identity_v1,
    decode_item_record_many_input_v1,
};
use serde_json::{Value, json};

fn input(operations: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": ITEM_RECORD_MANY_INPUT_SCHEMA_V1,
        "workspace_uuid": "00000000-0000-4000-8000-000000000001",
        "session_id": "00000000-0000-4000-8000-000000000002",
        "session_revision": 7,
        "attempt_id": "00000000-0000-4000-8000-000000000003",
        "idempotency_key": "record-many-1",
        "operations": operations,
    }))
    .unwrap()
}

fn check_result_record() -> Value {
    json!({
        "type": "check_result",
        "operation_id": "make-test",
        "operation_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "input_basis": {
            "descriptor": "HEAD and dirty-tree snapshot",
            "digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        },
        "executor": {"name": "gaori", "version": "1.0.0"},
        "outcome": "pass",
        "summary": "The complete development gate passed.",
        "output_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
    })
}

fn maximal_check_result_record() -> Value {
    json!({
        "type": "check_result",
        "operation_id": "o".repeat(64),
        "operation_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "input_basis": {
            "descriptor": "\u{10ffff}".repeat(512),
            "digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        },
        "executor": {
            "name": "\u{10ffff}".repeat(128),
            "version": "\u{10ffff}".repeat(64)
        },
        "outcome": "inconclusive",
        "summary": "\u{10ffff}".repeat(2_000),
        "output_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
    })
}

#[test]
fn v2agt004_stdin_accepts_all_typed_values_and_canonicalizes_item_order() {
    let decoded = decode_item_record_many_input_v1(&input(json!([
        {"item_id":"text","expected_item_revision":0,"record":{"type":"text","value":"done"}},
        {"item_id":"artifact","expected_item_revision":0,"record":{"type":"artifact","reference":"issue:42","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":12,"media_type":"text/plain"}},
        {"item_id":"list","expected_item_revision":0,"record":{"type":"list","value":["one","two"]}},
        {"item_id":"integer","expected_item_revision":0,"record":{"type":"integer","value":42}},
        {"item_id":"confirm","expected_item_revision":0,"record":{"type":"confirm","value":true}},
        {"item_id":"choice","expected_item_revision":0,"record":{"type":"choice","value":"ship"}},
        {"item_id":"check-result","expected_item_revision":0,"record":check_result_record()},
        {"item_id":"clear","expected_item_revision":3,"clear":true}
    ])))
    .unwrap();

    assert_eq!(
        decoded
            .operations
            .iter()
            .map(|operation| operation.item_id.as_str())
            .collect::<Vec<_>>(),
        [
            "artifact",
            "check-result",
            "choice",
            "clear",
            "confirm",
            "integer",
            "list",
            "text"
        ]
    );
    assert!(matches!(
        decoded.operations[0].disposition,
        ItemRecordManyDispositionV1::Record {
            record: ItemRecordValueV1::Artifact { .. }
        }
    ));
    assert!(matches!(
        decoded.operations[1].disposition,
        ItemRecordManyDispositionV1::Record {
            record: ItemRecordValueV1::CheckResult { .. }
        }
    ));
    assert!(matches!(
        decoded.operations[3].disposition,
        ItemRecordManyDispositionV1::Clear { clear: true }
    ));
}

#[test]
fn v2ast004_check_result_decoder_enforces_closed_shape_and_scalar_bounds() {
    let decode = |record: Value| {
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "verification",
            "expected_item_revision": 0,
            "record": record
        }])))
    };
    assert!(decode(check_result_record()).is_ok());

    for path in ["record", "input_basis", "executor"] {
        let mut record = check_result_record();
        let target = match path {
            "record" => &mut record,
            "input_basis" => &mut record["input_basis"],
            "executor" => &mut record["executor"],
            _ => unreachable!(),
        };
        target["unexpected"] = json!(true);
        assert!(decode(record).is_err(), "{path} must be closed");
    }

    for (pointer, valid, invalid) in [
        ("/input_basis/descriptor", "d".repeat(512), "d".repeat(513)),
        ("/executor/name", "n".repeat(128), "n".repeat(129)),
        ("/executor/version", "v".repeat(64), "v".repeat(65)),
        ("/summary", "s".repeat(2_000), "s".repeat(2_001)),
    ] {
        let mut record = check_result_record();
        *record.pointer_mut(pointer).unwrap() = json!(valid);
        assert!(decode(record).is_ok(), "{pointer} must accept its maximum");

        let mut record = check_result_record();
        *record.pointer_mut(pointer).unwrap() = json!(invalid);
        assert!(
            decode(record).is_err(),
            "{pointer} must reject over its maximum"
        );

        let mut record = check_result_record();
        *record.pointer_mut(pointer).unwrap() = json!("   ");
        assert!(decode(record).is_err(), "{pointer} must reject blank text");
    }

    for (pointer, invalid) in [
        ("/operation_id", json!("bad operation")),
        ("/operation_digest", json!("sha256:AAAA")),
        ("/input_basis/digest", json!("sha256:short")),
        ("/outcome", json!("unknown")),
        ("/output_digest", json!("sha256:short")),
    ] {
        let mut record = check_result_record();
        *record.pointer_mut(pointer).unwrap() = invalid;
        assert!(
            decode(record).is_err(),
            "{pointer} must reject malformed input"
        );
    }
}

#[test]
fn v2agt004_stdin_rejects_duplicates_invalid_dispositions_unknown_fields_and_oversize() {
    for operations in [
        json!([
            {"item_id":"same","expected_item_revision":0,"clear":true},
            {"item_id":"same","expected_item_revision":0,"record":{"type":"confirm","value":true}}
        ]),
        json!([{"item_id":"x","expected_item_revision":0,"clear":false}]),
        json!([{"item_id":"x","expected_item_revision":0,"clear":true,"record":{"type":"confirm","value":true}}]),
        json!([{"item_id":"x","expected_item_revision":0,"clear":true,"extra":1}]),
        Value::Array(Vec::new()),
    ] {
        assert!(decode_item_record_many_input_v1(&input(operations)).is_err());
    }
    assert!(decode_item_record_many_input_v1(&vec![b' '; MAX_DIRECT_INPUT_BYTES_V1 + 1]).is_err());
}

#[test]
fn v2agt004_stdin_accepts_exactly_128_operations_and_rejects_129() {
    let operations = |count: usize| {
        Value::Array(
            (0..count)
                .map(|index| {
                    json!({
                        "item_id": format!("item-{index:02}"),
                        "expected_item_revision": 0,
                        "clear": true
                    })
                })
                .collect(),
        )
    };

    assert_eq!(
        decode_item_record_many_input_v1(&input(operations(128)))
            .unwrap()
            .operations
            .len(),
        128
    );
    assert_eq!(
        decode_item_record_many_input_v1(&input(operations(129))),
        Err(SliceErrorV1::BoundExceeded {
            field: "item record operations",
            maximum: 128,
            actual: 129,
            unit: "operations",
        })
    );
}

#[test]
fn v2ast006_maximal_check_results_require_bounded_split_batches() {
    let operations = |start: usize, count: usize| {
        Value::Array(
            (start..start + count)
                .map(|index| {
                    json!({
                        "item_id": format!("check-{index:03}"),
                        "expected_item_revision": 0,
                        "record": maximal_check_result_record()
                    })
                })
                .collect(),
        )
    };

    // The split count follows the current scalar maxima encoded as UTF-8, not the independent
    // 32 KiB per-value ceiling. Keep that coupling explicit if any scalar bound changes.
    let maximal_record_bytes = serde_json::to_vec(&maximal_check_result_record())
        .unwrap()
        .len();
    assert!(maximal_record_bytes <= podway_core::MAX_CHECK_RESULT_VALUE_BYTES_V2);
    assert!(maximal_record_bytes * 64 < MAX_DIRECT_INPUT_BYTES_V1);
    assert!(maximal_record_bytes * 128 > MAX_DIRECT_INPUT_BYTES_V1);

    let complete_attempt = input(operations(0, 128));
    assert!(complete_attempt.len() > MAX_DIRECT_INPUT_BYTES_V1);
    assert!(decode_item_record_many_input_v1(&complete_attempt).is_err());

    for half in [operations(0, 64), operations(64, 64)] {
        let batch = input(half);
        assert!(batch.len() <= MAX_DIRECT_INPUT_BYTES_V1);
        assert_eq!(
            decode_item_record_many_input_v1(&batch)
                .unwrap()
                .operations
                .len(),
            64
        );
    }

    assert_eq!(
        podway_core::MAX_CHECK_RESULT_VALUE_BYTES_V2 * podway_core::MAX_ITEMS_PER_DEFINITION_V2,
        4 * 1_024 * 1_024,
        "the per-value ceiling derives the documented 4 MiB attempt product"
    );
}

#[test]
fn v2agt007_stdin_schema_and_decoder_share_public_string_bounds() {
    let schema: Value = serde_json::from_slice(include_bytes!(
        "../../../assets/schemas/item-record-many-input-v1.schema.json"
    ))
    .unwrap();
    assert_eq!(schema["properties"]["idempotency_key"]["maxLength"], 256);
    let record_variants = schema["$defs"]["recordValue"]["oneOf"].as_array().unwrap();
    for field in ["path", "reference"] {
        let variant = record_variants
            .iter()
            .find(|variant| variant["properties"].get(field).is_some())
            .unwrap();
        assert_eq!(variant["properties"][field]["maxLength"], 4_000);
    }

    let document = |idempotency_key: String, artifact_field: &str, artifact_value: String| {
        let mut document: Value = serde_json::from_slice(&input(json!([{
            "item_id": "artifact",
            "expected_item_revision": 0,
            "record": {
                "type": "artifact",
                "reference": "placeholder",
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "size_bytes": 0,
                "media_type": "text/plain"
            }
        }])))
        .unwrap();
        document["idempotency_key"] = json!(idempotency_key);
        let record = &mut document["operations"][0]["record"];
        if artifact_field == "path" {
            record.as_object_mut().unwrap().remove("reference");
            record.as_object_mut().unwrap().remove("digest");
            record.as_object_mut().unwrap().remove("size_bytes");
        }
        record[artifact_field] = json!(artifact_value);
        serde_json::to_vec(&document).unwrap()
    };

    assert!(
        decode_item_record_many_input_v1(&document(
            "k".repeat(256),
            "reference",
            "r".repeat(4_000),
        ))
        .is_ok()
    );
    assert!(
        decode_item_record_many_input_v1(&document(
            "k".repeat(257),
            "reference",
            "r".repeat(4_000),
        ))
        .is_err()
    );
    assert!(
        decode_item_record_many_input_v1(&document(
            "key".to_owned(),
            "reference",
            "r".repeat(4_001),
        ))
        .is_err()
    );
    assert!(
        decode_item_record_many_input_v1(&document("key".to_owned(), "path", "p".repeat(4_000),))
            .is_ok()
    );
    assert!(
        decode_item_record_many_input_v1(&document("key".to_owned(), "path", "p".repeat(4_001),))
            .is_err()
    );

    let i64_max_plus_one = (i64::MAX as u64) + 1;
    assert_eq!(schema["properties"]["session_revision"]["minimum"], 0);
    assert_eq!(
        schema["properties"]["session_revision"]["maximum"],
        u64::MAX
    );
    assert_eq!(
        schema["properties"]["operations"]["items"]["properties"]["expected_item_revision"]["minimum"],
        0
    );
    assert_eq!(
        schema["properties"]["operations"]["items"]["properties"]["expected_item_revision"]["maximum"],
        u64::MAX
    );
    let integer_variant = record_variants
        .iter()
        .find(|variant| variant["properties"]["type"]["const"] == "integer")
        .unwrap();
    assert_eq!(
        integer_variant["properties"]["value"]["minimum"],
        json!(i64::MIN)
    );
    assert_eq!(
        integer_variant["properties"]["value"]["maximum"],
        json!(i64::MAX)
    );
    let reference_artifact_variant = record_variants
        .iter()
        .find(|variant| variant["properties"].get("size_bytes").is_some())
        .unwrap();
    assert_eq!(
        reference_artifact_variant["properties"]["size_bytes"]["minimum"],
        0
    );
    assert_eq!(
        reference_artifact_variant["properties"]["size_bytes"]["maximum"],
        u64::MAX
    );

    for valid_signed in [json!(i64::MIN), json!(0), json!(i64::MAX)] {
        let payload = input(json!([{
            "item_id": "count",
            "expected_item_revision": 0,
            "record": {"type": "integer", "value": valid_signed}
        }]));
        assert!(
            decode_item_record_many_input_v1(&payload).is_ok(),
            "signed integer item record must accept {valid_signed}"
        );
    }
    for invalid_signed in [json!(i64_max_plus_one), json!(u64::MAX)] {
        let payload = input(json!([{
            "item_id": "count",
            "expected_item_revision": 0,
            "record": {"type": "integer", "value": invalid_signed}
        }]));
        assert!(
            podway_core::canonicalize_json_v1(&serde_json::from_slice::<Value>(&payload).unwrap())
                .is_ok(),
            "canonical primitive admits unsigned 64-bit integer {invalid_signed}"
        );
        assert!(
            decode_item_record_many_input_v1(&payload).is_err(),
            "signed integer item record must reject above-i64::MAX value {invalid_signed}"
        );
    }

    let mut boundary_request: Value = serde_json::from_slice(&input(json!([
        {
            "item_id": "min-int",
            "expected_item_revision": i64_max_plus_one,
            "record": {"type": "integer", "value": i64::MIN}
        },
        {
            "item_id": "max-int",
            "expected_item_revision": u64::MAX,
            "record": {"type": "integer", "value": i64::MAX}
        },
        {
            "item_id": "artifact",
            "expected_item_revision": u64::MAX,
            "record": {
                "type": "artifact",
                "reference": "issue:64",
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "size_bytes": u64::MAX,
                "media_type": "text/plain"
            }
        }
    ])))
    .unwrap();
    boundary_request["session_revision"] = json!(u64::MAX);
    let decoded =
        decode_item_record_many_input_v1(&serde_json::to_vec(&boundary_request).unwrap()).unwrap();
    let workspace = podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let preimage = canonical_direct_slice_mutation_identity_v1(
        &SliceCommandV1::ItemRecordMany(ItemRecordManyV1 {
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: decoded.session_id,
                expected_session_revision: decoded.session_revision,
                expected_attempt_id: decoded.attempt_id,
            },
            operations: decoded.operations,
        }),
        &workspace,
    )
    .unwrap();
    assert_eq!(
        podway_core::verify_canonical_json_v1(preimage.as_bytes()),
        Ok(())
    );
}

#[test]
fn v2agt004_direct_identity_is_semantically_item_order_independent() {
    let request = |operations: Value| {
        let input = decode_item_record_many_input_v1(&input(operations)).unwrap();
        SliceCommandV1::ItemRecordMany(ItemRecordManyV1 {
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: input.session_id,
                expected_session_revision: input.session_revision,
                expected_attempt_id: input.attempt_id,
            },
            operations: input.operations,
        })
    };
    let left = request(json!([
        {"item_id":"b","expected_item_revision":0,"clear":true},
        {"item_id":"a","expected_item_revision":0,"record":{"type":"integer","value":1}}
    ]));
    let right = request(json!([
        {"item_id":"a","expected_item_revision":0,"record":{"type":"integer","value":1}},
        {"item_id":"b","expected_item_revision":0,"clear":true}
    ]));
    assert!(left.is_mutation());
    let workspace = podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    assert_eq!(
        canonical_direct_slice_mutation_identity_v1(&left, &workspace).unwrap(),
        canonical_direct_slice_mutation_identity_v1(&right, &workspace).unwrap()
    );
    let changed = request(
        json!([{ "item_id":"a", "expected_item_revision":1, "record":{"type":"integer","value":1}}]),
    );
    assert_ne!(
        canonical_direct_slice_mutation_identity_v1(&left, &workspace).unwrap(),
        canonical_direct_slice_mutation_identity_v1(&changed, &workspace).unwrap()
    );
}

#[test]
fn v2scl003_record_many_schema_and_decoder_share_the_scale_envelope() {
    let schema: Value = serde_json::from_slice(include_bytes!(
        "../../../assets/schemas/item-record-many-input-v1.schema.json"
    ))
    .unwrap();
    assert_eq!(schema["properties"]["operations"]["maxItems"], 128);
    assert_eq!(
        schema["properties"]["operations"]["maxItems"],
        podway_core::MAX_ITEMS_PER_DEFINITION_V2,
        "the input ceiling is the items one definition declares"
    );

    // The result family has to answer every operation the input admits. It emits one outcome per
    // operation and cannot be cut — the replay-integrity check requires the counts to match — so a
    // narrower result ceiling would let an admitted batch commit and then answer contract-invalid.
    let result: Value = serde_json::from_slice(include_bytes!(
        "../../../assets/schemas/item-record-many-result-v2.schema.json"
    ))
    .unwrap();
    assert_eq!(
        result["properties"]["items"]["maxItems"], schema["properties"]["operations"]["maxItems"],
        "the result must hold one outcome for every operation the input admits"
    );
    let record_variants = schema["$defs"]["recordValue"]["oneOf"].as_array().unwrap();
    let text = record_variants
        .iter()
        .find(|variant| variant["properties"]["type"]["const"] == "text")
        .unwrap();
    assert_eq!(
        text["properties"]["value"]["maxLength"],
        podway_core::MAX_TEXT_SCALARS_V2
    );
    let list = record_variants
        .iter()
        .find(|variant| variant["properties"]["type"]["const"] == "list")
        .unwrap();
    assert_eq!(
        list["properties"]["value"]["maxItems"],
        podway_core::MAX_LIST_ENTRIES_V2
    );
    assert_eq!(
        list["properties"]["value"]["items"]["maxLength"],
        podway_core::MAX_LIST_ENTRY_SCALARS_V2
    );

    // V2SCL-002 reserved the wider input contract and V2SCL-003 raised the decoder onto the same
    // domain constants, so the schema and the decoder now accept and reject exactly together.
    let entries = Value::Array(
        (0..1_000)
            .map(|index| json!(format!("entry-{index}")))
            .collect(),
    );
    assert!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "list", "value": entries}
        }])))
        .is_ok()
    );
    let over_entries = Value::Array(
        (0..1_001)
            .map(|index| json!(format!("entry-{index}")))
            .collect(),
    );
    assert_eq!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "list", "value": over_entries}
        }]))),
        Err(SliceErrorV1::BoundExceeded {
            field: "item list entries",
            maximum: 1_000,
            actual: 1_001,
            unit: "entries",
        })
    );
    assert!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "list", "value": ["x".repeat(8_192)]}
        }])))
        .is_ok()
    );
    assert!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "list", "value": ["x".repeat(8_193)]}
        }])))
        .is_err()
    );
    assert!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "text", "value": "x".repeat(65_536)}
        }])))
        .is_ok()
    );
    assert_eq!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "text", "value": "x".repeat(65_537)}
        }]))),
        Err(SliceErrorV1::BoundExceeded {
            field: "item text value",
            maximum: 65_536,
            actual: 65_537,
            unit: "scalars",
        })
    );
}

#[test]
fn direct_start_identity_preserves_the_exact_preimage_and_both_digest_fences() {
    use podway_core::{Sha256Digest, WorkspaceId};
    use podway_protocol::{
        SessionStartCommandV1, SessionStartSourceV1, SessionStartV1,
        canonical_direct_start_identity_v1,
    };
    let workspace = WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let digest = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
    let changed_digest = Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap();
    let start = SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "workflow.yaml".to_owned(),
        },
        expected_procedure_digest: None,
        task_title: "Implement protocol".to_owned(),
        dry_run: false,
    };
    let command = SliceCommandV1::SessionStart(SessionStartCommandV1 {
        start: start.clone(),
        preconditions: None,
    });
    let base = canonical_direct_slice_mutation_identity_v1(&command, &workspace).unwrap();
    assert_eq!(
        base,
        "{\"command\":\"session.start\",\"payload\":{\"source\":{\"procedure\":\"workflow.yaml\"},\"task_title\":\"Implement protocol\"},\"preconditions\":{},\"protocol_major\":1,\"workspace_id\":\"00000000-0000-4000-8000-000000000001\"}"
    );
    let identity = canonical_direct_start_identity_v1(&command, &workspace, &digest).unwrap();
    let value: Value = serde_json::from_str(&identity).unwrap();
    assert_eq!(value["preconditions"]["procedure_digest"], digest.as_str());
    assert_eq!(
        value["preconditions"]["expected_procedure_digest"],
        Value::Null
    );
    assert_ne!(
        identity,
        canonical_direct_start_identity_v1(&command, &workspace, &changed_digest).unwrap()
    );
    let mut guarded = start;
    guarded.expected_procedure_digest = Some(digest.clone());
    let guarded = SliceCommandV1::SessionStart(SessionStartCommandV1 {
        start: guarded,
        preconditions: None,
    });
    let guard_identity = canonical_direct_start_identity_v1(&guarded, &workspace, &digest).unwrap();
    assert_ne!(identity, guard_identity);
    assert_eq!(
        serde_json::from_str::<Value>(&guard_identity).unwrap()["preconditions"]["expected_procedure_digest"],
        digest.as_str()
    );
}

#[test]
fn direct_start_identity_joins_paired_session_fences_and_keeps_unfenced_bytes() {
    use podway_core::{Revision, SessionId, Sha256Digest, WorkspaceId};
    use podway_protocol::{
        SessionIdentityPreconditionsWireV1, SessionStartCommandV1, SessionStartSourceV1,
        SessionStartV1, SliceCommandV1, canonical_direct_start_identity_v1,
    };
    let workspace = WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let session = SessionId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let digest = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
    let plain = SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "workflow.yaml".to_owned(),
        },
        expected_procedure_digest: None,
        task_title: "Implement protocol".to_owned(),
        dry_run: false,
    };
    let unfenced = SliceCommandV1::SessionStart(SessionStartCommandV1 {
        start: plain.clone(),
        preconditions: None,
    });
    let fenced = SliceCommandV1::SessionStart(SessionStartCommandV1 {
        start: plain,
        preconditions: Some(SessionIdentityPreconditionsWireV1 {
            expected_session_id: session,
            expected_session_revision: Revision::new(3),
        }),
    });
    let unfenced_identity =
        canonical_direct_start_identity_v1(&unfenced, &workspace, &digest).unwrap();
    assert_eq!(
        unfenced_identity,
        format!(
            "{{\"command\":\"session.start\",\"payload\":{{\"source\":{{\"procedure\":\"workflow.yaml\"}},\"task_title\":\"Implement protocol\"}},\"preconditions\":{{\"expected_procedure_digest\":null,\"procedure_digest\":\"{}\"}},\"protocol_major\":1,\"workspace_id\":\"00000000-0000-4000-8000-000000000001\"}}",
            digest.as_str()
        )
    );
    let fenced_identity = canonical_direct_start_identity_v1(&fenced, &workspace, &digest).unwrap();
    assert_eq!(
        fenced_identity,
        format!(
            "{{\"command\":\"session.start\",\"payload\":{{\"source\":{{\"procedure\":\"workflow.yaml\"}},\"task_title\":\"Implement protocol\"}},\"preconditions\":{{\"expected_procedure_digest\":null,\"procedure_digest\":\"{}\",\"session_id\":\"00000000-0000-4000-8000-000000000002\",\"session_revision\":3}},\"protocol_major\":1,\"workspace_id\":\"00000000-0000-4000-8000-000000000001\"}}",
            digest.as_str()
        )
    );
    assert_ne!(fenced_identity, unfenced_identity);
    let other_session = SessionId::new("00000000-0000-4000-8000-000000000003").unwrap();
    let other = SliceCommandV1::SessionStart(SessionStartCommandV1 {
        start: SessionStartV1 {
            source: SessionStartSourceV1::Procedure {
                procedure: "workflow.yaml".to_owned(),
            },
            expected_procedure_digest: None,
            task_title: "Implement protocol".to_owned(),
            dry_run: false,
        },
        preconditions: Some(SessionIdentityPreconditionsWireV1 {
            expected_session_id: other_session,
            expected_session_revision: Revision::new(3),
        }),
    });
    assert_ne!(
        fenced_identity,
        canonical_direct_start_identity_v1(&other, &workspace, &digest).unwrap()
    );
}

#[test]
fn direct_action_identities_bind_session_fences_and_item_semantics() {
    use podway_core::{AttemptId, ItemId, Revision, SessionId, WorkspaceId};
    use podway_protocol::{ItemMutationPreconditionsWireV1, ItemSetV1, SessionCompleteV1};
    let workspace = WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let session = SessionId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let attempt = AttemptId::new("00000000-0000-4000-8000-000000000003").unwrap();
    let complete = SessionCompleteV1 {
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: session.clone(),
            expected_session_revision: Revision::new(3),
            expected_attempt_id: attempt.clone(),
        },
    };
    let identity = canonical_direct_slice_mutation_identity_v1(
        &SliceCommandV1::SessionComplete(complete.clone()),
        &workspace,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&identity).unwrap(),
        json!({"command":"session.complete","workspace_id":workspace.as_str(),"protocol_major":1,"preconditions":{"session_id":session.as_str(),"session_revision":3,"attempt_id":attempt.as_str()},"payload":{}})
    );
    let mut changed = complete;
    changed.preconditions.expected_session_id =
        SessionId::new("00000000-0000-4000-8000-000000000004").unwrap();
    assert_ne!(
        identity,
        canonical_direct_slice_mutation_identity_v1(
            &SliceCommandV1::SessionComplete(changed),
            &workspace
        )
        .unwrap()
    );
    let item = ItemSetV1 {
        item_id: ItemId::new("note").unwrap(),
        value: "verified".to_owned(),
        preconditions: ItemMutationPreconditionsWireV1 {
            expected_session_id: session,
            expected_attempt_id: attempt,
            expected_item_revision: Revision::new(2),
        },
    };
    let identity = canonical_direct_slice_mutation_identity_v1(
        &SliceCommandV1::ItemSet(item.clone()),
        &workspace,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&identity).unwrap()["payload"],
        json!({"item_id":"note","value":"verified"})
    );
    let mut changed_revision = item.clone();
    changed_revision.preconditions.expected_item_revision = Revision::new(3);
    assert_ne!(
        identity,
        canonical_direct_slice_mutation_identity_v1(
            &SliceCommandV1::ItemSet(changed_revision),
            &workspace
        )
        .unwrap()
    );
    let mut changed = item;
    changed.value = "changed".to_owned();
    assert_ne!(
        identity,
        canonical_direct_slice_mutation_identity_v1(&SliceCommandV1::ItemSet(changed), &workspace)
            .unwrap()
    );
}

#[test]
fn direct_goal_and_decision_identities_bind_optional_and_required_goal_revisions() {
    use podway_core::{
        AttemptId, CriterionId, GraphNodeId, OptionId, Revision, SessionId, WorkspaceId,
    };
    use podway_protocol::{
        GoalCriterionWireV2, GoalReviseV2, GoalRevisionPreconditionsWireV2,
        ProcedureV2MutationCommandV1, SessionDecideV2, canonical_procedure_v2_command_identity_v1,
    };
    let workspace = WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let session = SessionId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let attempt = AttemptId::new("00000000-0000-4000-8000-000000000003").unwrap();
    let decide = SessionDecideV2 {
        option_id: OptionId::new("accept").unwrap(),
        reason: "Choose the route.".to_owned(),
        actor: None,
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: session.clone(),
            expected_session_revision: Revision::new(7),
            expected_attempt_id: attempt,
        },
        expected_goal_revision: None,
    };
    let general = canonical_procedure_v2_command_identity_v1(
        &ProcedureV2MutationCommandV1::SessionDecide(decide.clone()),
        &workspace,
    )
    .unwrap();
    assert!(
        serde_json::from_str::<Value>(&general).unwrap()["preconditions"]
            .get("goal_revision")
            .is_none()
    );
    let mut assessed = decide;
    assessed.expected_goal_revision = Some(1);
    let assessed = canonical_procedure_v2_command_identity_v1(
        &ProcedureV2MutationCommandV1::SessionDecide(assessed),
        &workspace,
    )
    .unwrap();
    assert_ne!(general, assessed);
    assert_eq!(
        serde_json::from_str::<Value>(&assessed).unwrap()["preconditions"]["goal_revision"],
        1
    );
    let revise = GoalReviseV2 {
        goal: "Ship safely.".to_owned(),
        criteria: vec![GoalCriterionWireV2 {
            criterion_id: CriterionId::new("verified").unwrap(),
            statement: "The gate passes.".to_owned(),
        }],
        target_graph_node_id: GraphNodeId::new("implement").unwrap(),
        reason: "Clarified.".to_owned(),
        actor: None,
        reactivate: false,
        preconditions: GoalRevisionPreconditionsWireV2 {
            expected_session_id: session,
            expected_session_revision: Revision::new(7),
            expected_attempt_id: None,
            expected_goal_revision: 1,
        },
    };
    let first = canonical_procedure_v2_command_identity_v1(
        &ProcedureV2MutationCommandV1::GoalRevise(revise.clone()),
        &workspace,
    )
    .unwrap();
    let mut changed = revise;
    changed.preconditions.expected_goal_revision = 2;
    assert_ne!(
        first,
        canonical_procedure_v2_command_identity_v1(
            &ProcedureV2MutationCommandV1::GoalRevise(changed),
            &workspace
        )
        .unwrap()
    );
}

#[test]
fn agent001_criterion_assessment_many_input_decoder_enforces_pre_fence_bounds_and_order() {
    use podway_core::{
        AttemptId, CriterionId, GraphNodeId, ItemId, OptionId, Revision, SessionId, Sha256Digest,
        WorkspaceId,
    };
    use podway_protocol::{
        CRITERION_ASSESSMENT_MANY_INPUT_SCHEMA_V1, CriterionAssessmentManyEntryWireV1,
        GoalAssessCriteriaV2, IdempotencyKeyV1, MAX_CRITERION_ASSESSMENT_MANY_ENTRIES_V1,
        MAX_CRITERION_ASSESSMENT_REASON_SCALARS_V1, MAX_JSON_DEPTH_V1, MAX_SLICE_ACTOR_SCALARS_V1,
        OBSERVATION_BEARING_MUTATION_COMMANDS_V1, ProcedureV2MutationCommandV1, SessionBeginV1,
        SessionCompleteV1, SessionDecideV2, SessionIdentityPreconditionsWireV1,
        SessionMutationPreconditionsWireV1, SessionStartCommandV1, SessionStartSourceV1,
        SessionStartV1, SliceCommandV1, canonical_direct_slice_mutation_digest_with_observation_v1,
        canonical_direct_slice_mutation_identity_v1,
        canonical_direct_slice_mutation_identity_with_observation_v1,
        canonical_direct_start_digest_with_observation_v1, canonical_direct_start_identity_v1,
        canonical_direct_start_identity_with_observation_v1,
        canonical_goal_assess_criteria_digest_v1, canonical_goal_assess_criteria_identity_v1,
        canonical_procedure_v2_command_digest_with_observation_v1,
        canonical_procedure_v2_command_identity_v1,
        canonical_procedure_v2_command_identity_with_observation_v1,
        decode_criterion_assessment_many_input_v1,
    };

    assert_eq!(
        OBSERVATION_BEARING_MUTATION_COMMANDS_V1,
        &[
            "session.start",
            "session.begin",
            "item.record_many",
            "session.complete",
            "session.decide",
            "goal.assess_criteria",
        ]
    );

    let valid_doc = |assessments: Value| {
        json!({
            "schema": CRITERION_ASSESSMENT_MANY_INPUT_SCHEMA_V1,
            "workspace_uuid": "00000000-0000-4000-8000-000000000001",
            "session_id": "00000000-0000-4000-8000-000000000002",
            "session_revision": 2,
            "attempt_id": "00000000-0000-4000-8000-000000000003",
            "goal_revision": 1,
            "idempotency_key": "assess-key-1",
            "assessments": assessments,
        })
    };

    // Valid 1..=16 entries with omitted evidence/items/actor defaults and preserved order.
    for count in [1usize, MAX_CRITERION_ASSESSMENT_MANY_ENTRIES_V1] {
        let entries: Vec<Value> = (0..count)
            .rev()
            .map(|idx| {
                json!({
                    "criterion_id": format!("crit-{idx:02}"),
                    "status": "satisfied",
                    "reason": format!("Reason {idx}"),
                })
            })
            .collect();
        let bytes = serde_json::to_vec(&valid_doc(Value::Array(entries))).unwrap();
        let decoded = decode_criterion_assessment_many_input_v1(&bytes).unwrap();
        assert_eq!(decoded.actor, None);
        assert_eq!(decoded.assessments.len(), count);
        assert_eq!(
            decoded.assessments[0].criterion_id.as_str(),
            format!("crit-{:02}", count - 1)
        );
        assert!(decoded.assessments[0].evidence.is_empty());
        assert!(decoded.assessments[0].items.is_empty());
    }

    // 0 and 17 entries fail closed.
    assert_eq!(
        decode_criterion_assessment_many_input_v1(
            &serde_json::to_vec(&valid_doc(json!([]))).unwrap()
        ),
        Err(SliceErrorV1::InvalidValue {
            field: "criterion assessments"
        })
    );
    let entries_17: Vec<Value> = (0..17)
        .map(|idx| {
            json!({
                "criterion_id": format!("crit-{idx:02}"),
                "status": "satisfied",
                "reason": "ok",
            })
        })
        .collect();
    assert_eq!(
        decode_criterion_assessment_many_input_v1(
            &serde_json::to_vec(&valid_doc(Value::Array(entries_17))).unwrap()
        ),
        Err(SliceErrorV1::BoundExceeded {
            field: "criterion assessments",
            maximum: 16,
            actual: 17,
            unit: "entries",
        })
    );

    // Duplicate criterion_id rejected before fences.
    let dup_criteria = valid_doc(json!([
        {"criterion_id": "c1", "status": "satisfied", "reason": "r1"},
        {"criterion_id": "c1", "status": "unsatisfied", "reason": "r2"},
    ]));
    assert_eq!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&dup_criteria).unwrap()),
        Err(SliceErrorV1::InvalidValue {
            field: "criterion assessment criterion_id"
        })
    );

    // Explicit null and unknown fields fail closed; duplicate JSON keys at top-level or entry fail closed.
    for raw_json in [
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":1,"idempotency_key":"k1","actor":null,"assessments":[{"criterion_id":"c1","status":"satisfied","reason":"ok"}]}"#,
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":1,"idempotency_key":"k1","assessments":[{"criterion_id":"c1","status":"satisfied","reason":"ok","evidence":null}]}"#,
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":1,"idempotency_key":"k1","assessments":[{"criterion_id":"c1","status":"satisfied","reason":"ok","items":null}]}"#,
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":1,"idempotency_key":"k1","extra":1,"assessments":[{"criterion_id":"c1","status":"satisfied","reason":"ok"}]}"#,
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":1,"idempotency_key":"k1","idempotency_key":"k2","assessments":[{"criterion_id":"c1","status":"satisfied","reason":"ok"}]}"#,
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":1,"idempotency_key":"k1","assessments":[{"criterion_id":"c1","status":"satisfied","status":"unsatisfied","reason":"ok"}]}"#,
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2.0,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":1,"idempotency_key":"k1","assessments":[{"criterion_id":"c1","status":"satisfied","reason":"ok"}]}"#,
        r#"{"schema":"podway.criterion-assessment-many-input/v1","workspace_uuid":"00000000-0000-4000-8000-000000000001","session_id":"00000000-0000-4000-8000-000000000002","session_revision":2,"attempt_id":"00000000-0000-4000-8000-000000000003","goal_revision":0,"idempotency_key":"k1","assessments":[{"criterion_id":"c1","status":"satisfied","reason":"ok"}]}"#,
    ] {
        assert!(
            decode_criterion_assessment_many_input_v1(raw_json.as_bytes()).is_err(),
            "must reject {raw_json}"
        );
    }

    // Actor and reason resource bounds and raw semantic preservation for AGENT-003.
    let mut actor_ok = valid_doc(json!([{
        "criterion_id": "c1",
        "status": "unknown_status_for_post_fence",
        "reason": "x".repeat(MAX_CRITERION_ASSESSMENT_REASON_SCALARS_V1),
        "evidence": ["n1", "n2", "n3", "n4"],
        "items": ["i1", "i2", "i3", "i4"],
    }]));
    actor_ok["actor"] = json!("a".repeat(MAX_SLICE_ACTOR_SCALARS_V1));
    let decoded_raw =
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&actor_ok).unwrap()).unwrap();
    assert_eq!(
        decoded_raw.assessments[0].status,
        "unknown_status_for_post_fence"
    );
    assert_eq!(decoded_raw.assessments[0].evidence.len(), 4);
    assert_eq!(decoded_raw.assessments[0].items.len(), 4);

    let mut actor_too_long = actor_ok.clone();
    actor_too_long["actor"] = json!("a".repeat(MAX_SLICE_ACTOR_SCALARS_V1 + 1));
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&actor_too_long).unwrap())
            .is_err()
    );
    let mut actor_blank = actor_ok.clone();
    actor_blank["actor"] = json!("   ");
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&actor_blank).unwrap())
            .is_err()
    );
    let mut reason_too_long = actor_ok.clone();
    reason_too_long["assessments"][0]["reason"] =
        json!("x".repeat(MAX_CRITERION_ASSESSMENT_REASON_SCALARS_V1 + 1));
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&reason_too_long).unwrap())
            .is_err()
    );
    let mut evidence_too_many = actor_ok.clone();
    evidence_too_many["assessments"][0]["evidence"] = json!(["n1", "n2", "n3", "n4", "n5"]);
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&evidence_too_many).unwrap())
            .is_err()
    );
    let mut items_too_many = actor_ok;
    items_too_many["assessments"][0]["items"] = json!(["i1", "i2", "i3", "i4", "i5"]);
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&items_too_many).unwrap())
            .is_err()
    );

    // Multibyte UTF-8 idempotency key boundary: 256 bytes accepted, 257 bytes rejected.
    let key_256 = "가".repeat(85) + "a";
    assert_eq!(key_256.len(), 256);
    assert!(IdempotencyKeyV1::new(key_256.clone()).is_ok());
    let mut doc_key_256 =
        valid_doc(json!([{"criterion_id": "c1", "status": "satisfied", "reason": "ok"}]));
    doc_key_256["idempotency_key"] = json!(key_256);
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&doc_key_256).unwrap())
            .is_ok()
    );
    let key_257 = "가".repeat(85) + "ab";
    assert_eq!(key_257.len(), 257);
    assert!(IdempotencyKeyV1::new(key_257.clone()).is_err());
    doc_key_256["idempotency_key"] = json!(key_257);
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&doc_key_256).unwrap())
            .is_err()
    );

    // 1 MiB +/- 1 byte and depth 64 vs 65 boundaries.
    let base_bytes = serde_json::to_vec(&valid_doc(
        json!([{"criterion_id": "c1", "status": "satisfied", "reason": "ok"}]),
    ))
    .unwrap();
    let mut exact_1mib = base_bytes.clone();
    exact_1mib.resize(MAX_DIRECT_INPUT_BYTES_V1, b' ');
    assert!(decode_criterion_assessment_many_input_v1(&exact_1mib).is_ok());
    exact_1mib.push(b' ');
    assert_eq!(
        decode_criterion_assessment_many_input_v1(&exact_1mib),
        Err(SliceErrorV1::ValueTooLong {
            field: "criterion assessment input",
            maximum: MAX_DIRECT_INPUT_BYTES_V1,
            actual: MAX_DIRECT_INPUT_BYTES_V1 + 1,
        })
    );

    let mut deep = Value::Null;
    for _ in 0..MAX_JSON_DEPTH_V1 + 1 {
        deep = json!([deep]);
    }
    assert!(
        decode_criterion_assessment_many_input_v1(&serde_json::to_vec(&deep).unwrap()).is_err()
    );

    // Canonical request identity & digest known-answer vectors across all 6 routes.
    let workspace_id = WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let session_id = SessionId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let attempt_id = AttemptId::new("00000000-0000-4000-8000-000000000003").unwrap();
    let proc_digest = Sha256Digest::new(
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .unwrap();

    let batch = GoalAssessCriteriaV2 {
        assessments: vec![
            CriterionAssessmentManyEntryWireV1 {
                criterion_id: CriterionId::new("c1").unwrap(),
                status: "satisfied".to_owned(),
                reason: "ok 1".to_owned(),
                evidence: vec![GraphNodeId::new("deliver").unwrap()],
                items: vec![ItemId::new("notes").unwrap()],
            },
            CriterionAssessmentManyEntryWireV1 {
                criterion_id: CriterionId::new("c2").unwrap(),
                status: "satisfied".to_owned(),
                reason: "ok 2".to_owned(),
                evidence: vec![],
                items: vec![],
            },
        ],
        actor: Some("agent-to".to_owned()),
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: session_id.clone(),
            expected_session_revision: Revision::new(2),
            expected_attempt_id: attempt_id.clone(),
        },
        expected_goal_revision: 1,
    };
    assert_eq!(
        canonical_goal_assess_criteria_identity_v1(&batch, &workspace_id, false).unwrap(),
        "{\"command\":\"goal.assess_criteria\",\"payload\":{\"actor\":\"agent-to\",\"assessments\":[{\"criterion_id\":\"c1\",\"evidence\":[\"deliver\"],\"items\":[\"notes\"],\"reason\":\"ok 1\",\"status\":\"satisfied\"},{\"criterion_id\":\"c2\",\"evidence\":[],\"items\":[],\"reason\":\"ok 2\",\"status\":\"satisfied\"}]},\"preconditions\":{\"attempt_id\":\"00000000-0000-4000-8000-000000000003\",\"goal_revision\":1,\"session_id\":\"00000000-0000-4000-8000-000000000002\",\"session_revision\":2},\"protocol_major\":1,\"workspace_id\":\"00000000-0000-4000-8000-000000000001\"}"
    );
    assert_eq!(
        canonical_goal_assess_criteria_identity_v1(&batch, &workspace_id, true).unwrap(),
        "{\"command\":\"goal.assess_criteria\",\"payload\":{\"actor\":\"agent-to\",\"assessments\":[{\"criterion_id\":\"c1\",\"evidence\":[\"deliver\"],\"items\":[\"notes\"],\"reason\":\"ok 1\",\"status\":\"satisfied\"},{\"criterion_id\":\"c2\",\"evidence\":[],\"items\":[],\"reason\":\"ok 2\",\"status\":\"satisfied\"}]},\"preconditions\":{\"attempt_id\":\"00000000-0000-4000-8000-000000000003\",\"goal_revision\":1,\"session_id\":\"00000000-0000-4000-8000-000000000002\",\"session_revision\":2},\"protocol_major\":1,\"return_observation\":true,\"workspace_id\":\"00000000-0000-4000-8000-000000000001\"}"
    );
    assert_eq!(
        canonical_goal_assess_criteria_digest_v1(&batch, &workspace_id, false)
            .unwrap()
            .as_str(),
        "sha256:6b9ac5c1bd76f93ae46747a277244c10a604357945c791c101a136e98c3a4ed5"
    );
    assert_eq!(
        canonical_goal_assess_criteria_digest_v1(&batch, &workspace_id, true)
            .unwrap()
            .as_str(),
        "sha256:54beb045545d1b12e05408e91bf44c1e2eb04607e3919f87d4a1c90b74e26392"
    );
    let batch_cmd = ProcedureV2MutationCommandV1::GoalAssessCriteria(batch.clone());
    assert_eq!(batch_cmd.command_name(), "goal.assess_criteria");
    assert_eq!(
        canonical_procedure_v2_command_identity_v1(&batch_cmd, &workspace_id).unwrap(),
        canonical_goal_assess_criteria_identity_v1(&batch, &workspace_id, false).unwrap()
    );
    let mut reversed_batch = batch.clone();
    reversed_batch.assessments.reverse();
    let reversed_cmd = ProcedureV2MutationCommandV1::GoalAssessCriteria(reversed_batch);
    assert_ne!(
        canonical_procedure_v2_command_identity_v1(&batch_cmd, &workspace_id).unwrap(),
        canonical_procedure_v2_command_identity_v1(&reversed_cmd, &workspace_id).unwrap()
    );

    let start_cmd = SliceCommandV1::SessionStart(SessionStartCommandV1 {
        start: SessionStartV1 {
            source: SessionStartSourceV1::Preset {
                preset: "small-change-v2".to_owned(),
            },
            expected_procedure_digest: None,
            task_title: "Ship".to_owned(),
            dry_run: false,
        },
        preconditions: None,
    });
    assert_eq!(
        canonical_direct_start_identity_with_observation_v1(
            &start_cmd,
            &workspace_id,
            &proc_digest,
            false
        )
        .unwrap(),
        canonical_direct_start_identity_v1(&start_cmd, &workspace_id, &proc_digest).unwrap()
    );
    assert_ne!(
        canonical_direct_start_digest_with_observation_v1(
            &start_cmd,
            &workspace_id,
            &proc_digest,
            false
        )
        .unwrap(),
        canonical_direct_start_digest_with_observation_v1(
            &start_cmd,
            &workspace_id,
            &proc_digest,
            true
        )
        .unwrap()
    );

    let begin_cmd = ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
        initial_goal: None,
        preconditions: SessionIdentityPreconditionsWireV1 {
            expected_session_id: session_id.clone(),
            expected_session_revision: Revision::new(0),
        },
    });
    assert_eq!(
        canonical_procedure_v2_command_identity_with_observation_v1(
            &begin_cmd,
            &workspace_id,
            false
        )
        .unwrap(),
        canonical_procedure_v2_command_identity_v1(&begin_cmd, &workspace_id).unwrap()
    );
    assert_ne!(
        canonical_procedure_v2_command_digest_with_observation_v1(&begin_cmd, &workspace_id, false)
            .unwrap(),
        canonical_procedure_v2_command_digest_with_observation_v1(&begin_cmd, &workspace_id, true)
            .unwrap()
    );

    let decide_cmd = ProcedureV2MutationCommandV1::SessionDecide(SessionDecideV2 {
        option_id: OptionId::new("proceed").unwrap(),
        reason: "Ready".to_owned(),
        actor: None,
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: session_id.clone(),
            expected_session_revision: Revision::new(2),
            expected_attempt_id: attempt_id.clone(),
        },
        expected_goal_revision: Some(1),
    });
    assert_ne!(
        canonical_procedure_v2_command_digest_with_observation_v1(
            &decide_cmd,
            &workspace_id,
            false
        )
        .unwrap(),
        canonical_procedure_v2_command_digest_with_observation_v1(&decide_cmd, &workspace_id, true)
            .unwrap()
    );

    let complete_cmd = SliceCommandV1::SessionComplete(SessionCompleteV1 {
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: session_id,
            expected_session_revision: Revision::new(2),
            expected_attempt_id: attempt_id,
        },
    });
    assert_eq!(
        canonical_direct_slice_mutation_identity_with_observation_v1(
            &complete_cmd,
            &workspace_id,
            false
        )
        .unwrap(),
        canonical_direct_slice_mutation_identity_v1(&complete_cmd, &workspace_id).unwrap()
    );
    assert_ne!(
        canonical_direct_slice_mutation_digest_with_observation_v1(
            &complete_cmd,
            &workspace_id,
            false
        )
        .unwrap(),
        canonical_direct_slice_mutation_digest_with_observation_v1(
            &complete_cmd,
            &workspace_id,
            true
        )
        .unwrap()
    );
}
