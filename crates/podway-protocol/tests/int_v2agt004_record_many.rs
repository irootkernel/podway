use podway_protocol::{
    ClientInfoV1, CommandNameV1, ITEM_RECORD_MANY_INPUT_SCHEMA_V1, IdempotencyKeyV1,
    ItemRecordManyDispositionV1, ItemRecordValueV1, MAX_FRAME_PAYLOAD_BYTES_V1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    SliceCommandV1, SliceRequestV1, WorkspaceContextV1, canonical_mutation_identity_v1,
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
    assert!(decode_item_record_many_input_v1(&vec![b' '; MAX_FRAME_PAYLOAD_BYTES_V1 + 1]).is_err());
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
    assert!(decode_item_record_many_input_v1(&input(operations(129))).is_err());
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
}

#[test]
fn v2agt004_route_is_durable_and_semantically_item_order_independent() {
    let selector = json!({
        "version": 1,
        "path_bytes_base64url": "L3RtcC93b3JrdHJlZQ",
        "display": "/tmp/worktree",
        "expected_uuid": "00000000-0000-4000-8000-000000000001"
    });
    let request = |operations: Value| {
        let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000010").unwrap(),
            client: ClientInfoV1::new("test", "1", 1).unwrap(),
            operation: OperationV1::Mutate,
            command: CommandNameV1::new("item.record_many").unwrap(),
            workspace: Some(
                WorkspaceContextV1::new(
                    "/tmp/worktree",
                    Some(
                        podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000000001")
                            .unwrap(),
                    ),
                )
                .unwrap(),
            ),
            idempotency_key: Some(IdempotencyKeyV1::new("batch-key").unwrap()),
            preconditions: PreconditionsV1::new(
                Some(
                    "00000000-0000-4000-8000-000000000002"
                        .to_owned()
                        .try_into()
                        .unwrap(),
                ),
                Some(podway_core::Revision::new(7)),
                Some(
                    "00000000-0000-4000-8000-000000000003"
                        .to_owned()
                        .try_into()
                        .unwrap(),
                ),
                None,
                None,
                None,
            )
            .unwrap(),
            options: RequestOptionsV1::new(false, 1000).unwrap(),
            payload: json!({"selector":selector,"operations":operations})
                .as_object()
                .unwrap()
                .clone(),
        })
        .unwrap();
        SliceRequestV1::from_envelope(&envelope).unwrap()
    };
    let left = request(json!([
        {"item_id":"b","expected_item_revision":0,"clear":true},
        {"item_id":"a","expected_item_revision":0,"record":{"type":"integer","value":1}}
    ]));
    let right = request(json!([
        {"item_id":"a","expected_item_revision":0,"record":{"type":"integer","value":1}},
        {"item_id":"b","expected_item_revision":0,"clear":true}
    ]));
    assert_eq!(left.command().operation(), OperationV1::Mutate);
    assert!(left.command().is_durable_job());
    assert!(matches!(left.command(), SliceCommandV1::ItemRecordMany(_)));
    let workspace = podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap();
    assert_eq!(
        canonical_mutation_identity_v1(&left, &workspace).unwrap(),
        canonical_mutation_identity_v1(&right, &workspace).unwrap()
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
        "../../../assets/schemas/item-record-many-result-v1.schema.json"
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
    assert!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "list", "value": over_entries}
        }])))
        .is_err()
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
    assert!(
        decode_item_record_many_input_v1(&input(json!([{
            "item_id": "notes",
            "expected_item_revision": 0,
            "record": {"type": "text", "value": "x".repeat(65_537)}
        }])))
        .is_err()
    );
}
