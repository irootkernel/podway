#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{decode_response_payload_v1, encode_response_payload_v1};

fuzz_target!(|input: &[u8]| {
    if let Ok(response) = decode_response_payload_v1(input) {
        response
            .validate()
            .expect("decoded response must remain valid");

        let encoded = encode_response_payload_v1(&response)
            .expect("validated response must encode");
        let round_trip = decode_response_payload_v1(&encoded)
            .expect("encoded response must decode");
        assert_eq!(round_trip, response, "encoded response must round-trip");

        let mut additive = serde_json::to_value(&response)
            .expect("validated response must serialize");
        additive
            .as_object_mut()
            .expect("response must serialize to an object")
            .insert("_fuzz_additive".to_owned(), serde_json::Value::Null);
        let additive = serde_json::to_vec(&additive)
            .expect("additive response document must serialize");
        assert!(
            decode_response_payload_v1(&additive).is_err(),
            "response decoder must reject additive fields"
        );
    }
});
