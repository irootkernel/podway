#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{
    ErrorEnvelopeV1, MAX_FRAME_PAYLOAD_BYTES_V1, MAX_JSON_DEPTH_V1, OutputEnvelopeV1,
    ResponseEnvelopeV1, decode_response_payload_v1, encode_response_payload_v1,
};
use serde_json::{Map, Value, json};

const OUTPUT_SEED: &[u8] = br#"{"schema":"podway.output/v1","request_id":"00000000-0000-4000-8000-000000000001","command":"session.status","generated_at":"2026-07-15T12:34:56.789Z","workspace":{"uuid":"00000000-0000-4000-8000-000000000002","root":"/tmp/podway","latest_workspace_sequence":7},"job":{"id":"00000000-0000-4000-8000-000000000003","sequence":1,"state":"succeeded","submitted_at":"2026-07-15T12:34:56.789Z","claimed_at":null,"finished_at":"2026-07-15T12:34:57.789Z"},"session":{"id":"00000000-0000-4000-8000-000000000004","title":"Phase 0","lifecycle":"completed","revision_before":1,"revision_after":2},"result":{"status":"complete"},"warnings":[{"code":"NOTICE"}]}"#;
const ERROR_SEED: &[u8] = br#"{"schema":"podway.error/v1","request_id":"00000000-0000-4000-8000-000000000001","command":"session.status","generated_at":"2026-07-15T12:34:56.789Z","code":"PRECONDITION_FAILED","message":"precondition failed","retryable":false,"exit_code":3,"workspace":{"uuid":"00000000-0000-4000-8000-000000000002"},"details":{"reason":"revision"}}"#;

fn mutation(input: &[u8]) -> (String, Value) {
    let name = format!("fuzz_{:02x}_{:02x}", input.first().copied().unwrap_or(0), input.len() % 251);
    let mut nested = Value::String(input[..input.len().min(64)].iter().map(|byte| char::from(b'a' + byte % 26)).collect());
    for _ in 0..input.len() % (MAX_JSON_DEPTH_V1 - 4) {
        nested = Value::Array(vec![nested]);
    }
    (name, json!({"input_length": input.len(), "nested": nested}))
}

fn object(seed: &[u8]) -> Map<String, Value> {
    serde_json::from_slice(seed).expect("response seed must be an object")
}

fn decode_payload(payload: Map<String, Value>) -> ResponseEnvelopeV1 {
    let bytes = serde_json::to_vec(&payload).expect("response must serialize");
    assert!(bytes.len() <= MAX_FRAME_PAYLOAD_BYTES_V1);

    let value = Value::Object(payload);
    let direct = serde_json::from_value::<ResponseEnvelopeV1>(value.clone())
        .expect("response must decode through public serde");
    match value["schema"].as_str() {
        Some("podway.output/v1") => {
            assert_eq!(
                serde_json::from_value::<OutputEnvelopeV1>(value.clone())
                    .expect("output must decode through public serde"),
                match &direct {
                    ResponseEnvelopeV1::Output(output) => output.clone(),
                    ResponseEnvelopeV1::Error(_) => panic!("output seed must decode as output"),
                },
            );
        }
        Some("podway.error/v1") => {
            assert_eq!(
                serde_json::from_value::<ErrorEnvelopeV1>(value.clone())
                    .expect("error must decode through public serde"),
                match &direct {
                    ResponseEnvelopeV1::Error(error) => error.clone(),
                    ResponseEnvelopeV1::Output(_) => panic!("error seed must decode as error"),
                },
            );
        }
        _ => panic!("seed must have a supported response schema"),
    }
    let framed = decode_response_payload_v1(&bytes).expect("response must decode through codec");
    assert_eq!(direct, framed);
    direct
}

fn encoded_document(response: &ResponseEnvelopeV1) -> Value {
    let encoded = encode_response_payload_v1(response).expect("response must encode");
    assert_eq!(
        decode_response_payload_v1(&encoded).expect("encoded response must decode"),
        *response,
    );
    serde_json::from_slice(&encoded).expect("encoded response must be JSON")
}

fn decode_with_addition(
    mut payload: Map<String, Value>,
    path: &[&str],
    name: &str,
    value: Value,
) -> ResponseEnvelopeV1 {
    let mut target = &mut payload;
    for field in path {
        target = target
            .get_mut(*field)
            .and_then(Value::as_object_mut)
            .expect("seed DTO must be an object");
    }
    target.insert(name.to_owned(), value);
    decode_payload(payload)
}

fn response_with_additive_depth(
    mut payload: Map<String, Value>,
    path: &[&str],
    array_count: usize,
) -> Value {
    let mut target = &mut payload;
    for field in path {
        target = target
            .get_mut(*field)
            .and_then(Value::as_object_mut)
            .expect("seed DTO must be an object");
    }
    target.insert("future_extension".to_owned(), nested_arrays(array_count));
    Value::Object(payload)
}

fn nested_arrays(count: usize) -> Value {
    let mut value = Value::Null;
    for _ in 0..count { value = Value::Array(vec![value]); }
    value
}

fuzz_target!(|input: &[u8]| {
    if let Ok(response) = decode_response_payload_v1(input) {
        let encoded = encode_response_payload_v1(&response).expect("decoded response must re-encode");
        assert_eq!(decode_response_payload_v1(&encoded).expect("re-encoded response must decode"), response);
    }

    let (name, value) = mutation(input);
    let output_seed = decode_payload(object(OUTPUT_SEED));
    let error_seed = decode_payload(object(ERROR_SEED));

    for path in [Vec::new(), vec!["workspace"], vec!["job"], vec!["session"]] {
        assert_eq!(
            decode_with_addition(object(OUTPUT_SEED), &path, &name, value.clone()),
            output_seed,
        );
    }
    assert_eq!(
        decode_with_addition(object(ERROR_SEED), &[], &name, value.clone()),
        error_seed,
    );

    let mut output = object(OUTPUT_SEED);
    output
        .get_mut("result")
        .and_then(Value::as_object_mut)
        .expect("result map")
        .insert(name.clone(), value.clone());
    output
        .get_mut("warnings")
        .and_then(Value::as_array_mut)
        .expect("warnings array")
        .push(Value::Object(Map::from_iter([(name.clone(), value.clone())])));
    let mut expected_output = Value::Object(object(OUTPUT_SEED));
    expected_output["job"]
        .as_object_mut()
        .expect("expected job map")
        .remove("claimed_at");
    expected_output
        .get_mut("result")
        .and_then(Value::as_object_mut)
        .expect("serialized result map")
        .insert(name.clone(), value.clone());
    expected_output
        .get_mut("warnings")
        .and_then(Value::as_array_mut)
        .expect("serialized warnings array")
        .push(Value::Object(Map::from_iter([(name.clone(), value.clone())])));
    let response = decode_payload(output);
    assert_eq!(encoded_document(&response), expected_output);
    let ResponseEnvelopeV1::Output(output) = response else {
        panic!("output seed must decode as output");
    };
    assert_eq!(output.result().get(&name), Some(&value));
    assert_eq!(
        output.warnings().last().and_then(|warning| warning.get(&name)),
        Some(&value),
    );

    let mut error = object(ERROR_SEED);
    error
        .get_mut("workspace")
        .and_then(Value::as_object_mut)
        .expect("workspace map")
        .insert(name.clone(), value.clone());
    error
        .get_mut("details")
        .and_then(Value::as_object_mut)
        .expect("details map")
        .insert(name.clone(), value.clone());
    let mut expected_error = Value::Object(object(ERROR_SEED));
    expected_error
        .get_mut("workspace")
        .and_then(Value::as_object_mut)
        .expect("serialized workspace map")
        .insert(name.clone(), value.clone());
    expected_error
        .get_mut("details")
        .and_then(Value::as_object_mut)
        .expect("serialized details map")
        .insert(name.clone(), value.clone());
    let response = decode_payload(error);
    assert_eq!(encoded_document(&response), expected_error);
    let ResponseEnvelopeV1::Error(error) = response else {
        panic!("error seed must decode as error");
    };
    assert_eq!(error.details().get(&name), Some(&value));
    let encoded_error = serde_json::to_value(error).expect("error response must serialize");
    assert_eq!(encoded_error["workspace"][&name], value);

    for (payload, path) in [
        (object(OUTPUT_SEED), Vec::new()),
        (object(ERROR_SEED), Vec::new()),
        (object(OUTPUT_SEED), vec!["workspace"]),
        (object(OUTPUT_SEED), vec!["job"]),
        (object(OUTPUT_SEED), vec!["session"]),
        (object(ERROR_SEED), vec!["workspace"]),
    ] {
        let at_limit =
            response_with_additive_depth(payload.clone(), &path, MAX_JSON_DEPTH_V1 - 1 - path.len());
        match at_limit["schema"].as_str() {
            Some("podway.output/v1") => {
                assert!(serde_json::from_value::<OutputEnvelopeV1>(at_limit.clone()).is_ok());
            }
            Some("podway.error/v1") => {
                assert!(serde_json::from_value::<ErrorEnvelopeV1>(at_limit.clone()).is_ok());
            }
            _ => panic!("seed must have a supported response schema"),
        }
        assert!(serde_json::from_value::<ResponseEnvelopeV1>(at_limit.clone()).is_ok());
        assert!(
            decode_response_payload_v1(
                &serde_json::to_vec(&at_limit).expect("at-limit response must serialize")
            )
            .is_ok()
        );

        let over_limit =
            response_with_additive_depth(payload, &path, MAX_JSON_DEPTH_V1 - path.len());
        match over_limit["schema"].as_str() {
            Some("podway.output/v1") => {
                assert!(serde_json::from_value::<OutputEnvelopeV1>(over_limit.clone()).is_err());
            }
            Some("podway.error/v1") => {
                assert!(serde_json::from_value::<ErrorEnvelopeV1>(over_limit.clone()).is_err());
            }
            _ => panic!("seed must have a supported response schema"),
        }
        assert!(serde_json::from_value::<ResponseEnvelopeV1>(over_limit.clone()).is_err());
        assert!(
            decode_response_payload_v1(
                &serde_json::to_vec(&over_limit).expect("over-limit response must serialize")
            )
            .is_err()
        );
    }

    let mut invalid = object(OUTPUT_SEED);
    invalid["job"]["sequence"] = json!(0);
    assert!(decode_response_payload_v1(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut invalid_error = object(ERROR_SEED);
    invalid_error["exit_code"] = json!(7);
    assert!(decode_response_payload_v1(&serde_json::to_vec(&invalid_error).unwrap()).is_err());
});
