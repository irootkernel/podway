#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{
    MAX_FRAME_PAYLOAD_BYTES_V1, decode_response_payload_v2, decode_single_frame_v1,
    encode_frame_v1, encode_response_payload_v2,
};

fuzz_target!(|input: &[u8]| {
    if let Ok(response) = decode_response_payload_v2(input) {
        let encoded = encode_response_payload_v2(&response)
            .expect("a decoded v2-aware response must re-encode");
        assert!(encoded.len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
        assert_eq!(
            decode_response_payload_v2(&encoded)
                .expect("a re-encoded v2-aware response must decode"),
            response,
        );

        let frame = encode_frame_v1(&encoded).expect("a bounded response must frame");
        assert_eq!(
            decode_single_frame_v1(&frame).expect("an encoded response frame must decode"),
            encoded,
        );
    }
});
