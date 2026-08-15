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

#[test]
fn v2agt004_stdin_accepts_all_typed_values_and_canonicalizes_item_order() {
    let decoded = decode_item_record_many_input_v1(&input(json!([
        {"item_id":"text","expected_item_revision":0,"record":{"type":"text","value":"done"}},
        {"item_id":"artifact","expected_item_revision":0,"record":{"type":"artifact","reference":"issue:42","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":12,"media_type":"text/plain"}},
        {"item_id":"list","expected_item_revision":0,"record":{"type":"list","value":["one","two"]}},
        {"item_id":"integer","expected_item_revision":0,"record":{"type":"integer","value":42}},
        {"item_id":"confirm","expected_item_revision":0,"record":{"type":"confirm","value":true}},
        {"item_id":"choice","expected_item_revision":0,"record":{"type":"choice","value":"ship"}},
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
            "artifact", "choice", "clear", "confirm", "integer", "list", "text"
        ]
    );
    assert!(matches!(
        decoded.operations[0].disposition,
        ItemRecordManyDispositionV1::Record {
            record: ItemRecordValueV1::Artifact { .. }
        }
    ));
    assert!(matches!(
        decoded.operations[2].disposition,
        ItemRecordManyDispositionV1::Clear { clear: true }
    ));
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
fn v2agt004_stdin_accepts_exactly_64_operations_and_rejects_65() {
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
        decode_item_record_many_input_v1(&input(operations(64)))
            .unwrap()
            .operations
            .len(),
        64
    );
    assert!(decode_item_record_many_input_v1(&input(operations(65))).is_err());
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
