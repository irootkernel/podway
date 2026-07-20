#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{
    decode_request_payload_v1, decode_response_payload_v1, decode_single_frame_v1, encode_frame_v1,
};

fuzz_target!(|input: &[u8]| {
    if let Ok(payload) = decode_single_frame_v1(input) {
        assert_eq!(
            encode_frame_v1(payload).expect("decoded frame payload must re-encode"),
            input
        );

        if let Ok(request) = decode_request_payload_v1(payload) {
            assert_eq!(
                decode_request_payload_v1(
                    &podway_protocol::encode_request_payload_v1(&request)
                        .expect("decoded request must re-encode")
                )
                .expect("re-encoded request must decode"),
                request
            );
        }

        if let Ok(response) = decode_response_payload_v1(payload) {
            assert_eq!(
                decode_response_payload_v1(
                    &podway_protocol::encode_response_payload_v1(&response)
                        .expect("decoded response must re-encode")
                )
                .expect("re-encoded response must decode"),
                response
            );
        }
    }
});
