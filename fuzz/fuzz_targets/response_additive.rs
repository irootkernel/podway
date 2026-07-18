#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{
    decode_response_payload_v1, encode_response_payload_v1, MAX_FRAME_PAYLOAD_BYTES_V1,
};
use serde_json::{Map, Value};

fn mutation(input: &[u8]) -> (String, Value) {
    let name = format!(
        "fuzz_{:02x}_{:02x}",
        input.first().copied().unwrap_or(0),
        input.len() % 251
    );
    let value = match input.first().copied().unwrap_or(0) % 4 {
        0 => Value::Null,
        1 => Value::Bool(input.len() % 2 == 1),
        2 => Value::from(input.len() as u64),
        _ => Value::String(
            input[..input.len().min(MAX_FRAME_PAYLOAD_BYTES_V1 / 16)]
                .iter()
                .map(|byte| char::from(b'a' + byte % 26))
                .collect(),
        ),
    };
    (name, value)
}

fuzz_target!(|input: &[u8]| {
    let seed = br#"{"schema":"podway.output/v1","request_id":"00000000-0000-4000-8000-000000000001","command":"session.status","generated_at":"2026-07-15T12:34:56.789Z","result":{},"warnings":[]}"#;
    let response = decode_response_payload_v1(seed).expect("seed response must be valid");
    let encoded = encode_response_payload_v1(&response).expect("seed response must encode");
    assert!(encoded.len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    assert_eq!(
        decode_response_payload_v1(&encoded).expect("seed response round-trip"),
        response
    );

    let (name, value) = mutation(input);
    let mut top_level: Map<String, Value> =
        serde_json::from_slice(&encoded).expect("encoded response is an object");
    top_level.insert(name.clone(), value.clone());
    let top_level = serde_json::to_vec(&top_level).expect("top-level mutation serializes");
    assert!(top_level.len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    assert!(
        decode_response_payload_v1(&top_level).is_err(),
        "response envelope must reject additive top-level fields"
    );

    let mut nested: Map<String, Value> =
        serde_json::from_slice(&encoded).expect("encoded response is an object");
    nested
        .get_mut("result")
        .and_then(Value::as_object_mut)
        .expect("seed result map")
        .insert(name.clone(), value.clone());
    nested
        .get_mut("warnings")
        .and_then(Value::as_array_mut)
        .expect("seed warnings")
        .push(Value::Object(Map::from_iter([(name, value)])));
    let nested = serde_json::to_vec(&nested).expect("nested mutation serializes");
    assert!(nested.len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
    let decoded = decode_response_payload_v1(&nested).expect("valid additive nested maps decode");
    assert_eq!(
        decode_response_payload_v1(
            &encode_response_payload_v1(&decoded).expect("nested response encodes")
        )
        .expect("nested response round-trips"),
        decoded
    );
});
