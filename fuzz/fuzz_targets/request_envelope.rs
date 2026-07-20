#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{decode_request_payload_v1, encode_request_payload_v1};

fuzz_target!(|input: &[u8]| {
    if let Ok(request) = decode_request_payload_v1(input) {
        assert_eq!(
            decode_request_payload_v1(
                &encode_request_payload_v1(&request).expect("decoded request must re-encode")
            )
            .expect("re-encoded request must decode"),
            request
        );
    }
});
