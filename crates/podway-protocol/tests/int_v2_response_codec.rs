use podway_protocol::{
    MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1, PayloadCodecErrorV1, ProtocolError, ResponseEnvelopeV1,
    ResponseEnvelopeV2, decode_response_payload_v1, decode_response_payload_v2,
    encode_response_payload_v1, encode_response_payload_v2,
};
use serde_json::{Value, json};

fn diagnostics_output_v2() -> Value {
    json!({
        "schema": "podway.output/v2",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "procedure.validate",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "result": {
            "schema": "podway.procedure-diagnostics-result/v1",
            "operation": "validate",
            "procedure_schema": "podway.procedure/v2",
            "file": "procedure.yaml",
            "valid": true,
            "diagnostics": [],
            "diagnostics_truncated": false,
            "diagnostics_total": 0
        },
        "warnings": []
    })
}

fn legacy_output_v1() -> Value {
    json!({
        "schema": "podway.output/v1",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "status",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "result": {},
        "warnings": []
    })
}

#[test]
fn v2plt006_v2_aware_response_codec_round_trips_output_v2() {
    let expected = diagnostics_output_v2();
    let input = serde_json::to_vec(&expected).unwrap();

    let decoded = decode_response_payload_v2(&input).expect("v2 output must decode");
    assert!(matches!(decoded, ResponseEnvelopeV2::OutputV2(_)));

    let encoded = encode_response_payload_v2(&decoded).expect("v2 output must encode");
    assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), expected);
}

#[test]
fn v2plt006_released_v1_decoder_still_rejects_output_v2() {
    let input = serde_json::to_vec(&diagnostics_output_v2()).unwrap();

    assert!(matches!(
        decode_response_payload_v1(&input),
        Err(PayloadCodecErrorV1::UnsupportedResponseSchema { received, supported })
            if received == "podway.output/v2"
                && supported == ["podway.output/v1", "podway.error/v1"]
    ));
}

#[test]
fn v2plt006_v2_aware_codec_preserves_released_v1_output_bytes() {
    let input = serde_json::to_vec(&legacy_output_v1()).unwrap();
    let decoded = decode_response_payload_v2(&input).expect("v1 output must remain compatible");
    let ResponseEnvelopeV2::OutputV1(output) = decoded else {
        panic!("v1 schema must select the v1 output variant");
    };

    let legacy = encode_response_payload_v1(&ResponseEnvelopeV1::Output(output.clone())).unwrap();
    let aware = encode_response_payload_v2(&ResponseEnvelopeV2::OutputV1(output)).unwrap();
    assert_eq!(aware, legacy);
}

#[test]
fn v2plt006_v2_aware_codec_round_trips_released_error_v1() {
    let expected = json!({
        "schema": "podway.error/v1",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "session.status",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "code": "GRAPH_NODE_NOT_FOUND",
        "message": "Graph node not found.",
        "retryable": false,
        "exit_code": 1,
        "details": {
            "schema": "podway.v2-runtime-error-details/v1",
            "kind": "GRAPH_NODE_NOT_FOUND",
            "graph_node_id": "work"
        }
    });
    let decoded = decode_response_payload_v2(&serde_json::to_vec(&expected).unwrap())
        .expect("error/v1 must remain compatible");
    assert!(matches!(decoded, ResponseEnvelopeV2::Error(_)));
    let encoded = encode_response_payload_v2(&decoded).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), expected);
}

#[test]
fn v2plt006_v2_aware_decoder_keeps_schema_dispatch_closed() {
    let mut value = diagnostics_output_v2();
    value["schema"] = json!("podway.output/v3");
    let input = serde_json::to_vec(&value).unwrap();

    assert!(matches!(
        decode_response_payload_v2(&input),
        Err(PayloadCodecErrorV1::UnsupportedResponseSchema { received, supported })
            if received == "podway.output/v3"
                && supported == [
                    "podway.output/v1",
                    "podway.output/v2",
                    "podway.error/v1"
                ]
    ));
}

#[test]
fn v2plt006_compact_v2_decode_counts_the_json_newline() {
    let mut value = diagnostics_output_v2();
    value["command"] = json!("session.status");
    value["result"] = json!({"schema": "podway.compact-status-result/v2"});
    value["future_padding"] = json!("");
    let base_length = serde_json::to_vec(&value).unwrap().len() + 1;
    value["future_padding"] =
        json!("x".repeat(MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1 - base_length + 1));
    let input = serde_json::to_vec(&value).unwrap();

    assert!(matches!(
        decode_response_payload_v2(&input),
        Err(PayloadCodecErrorV1::JsonContract(
            ProtocolError::CompactStatusEnvelopeTooLarge { length, maximum }
        )) if length == MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1 + 1
            && maximum == MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1
    ));
}
