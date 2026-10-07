#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{MAX_DIRECT_OUTPUT_BYTES_V1, OutputEnvelopeV4};

fuzz_target!(|input: &[u8]| {
    if input.len() > MAX_DIRECT_OUTPUT_BYTES_V1 {
        return;
    }
    if let Ok(output) = serde_json::from_slice::<OutputEnvelopeV4>(input) {
        let encoded = serde_json::to_vec(&output).expect("validated output must serialize");
        assert!(encoded.len() <= MAX_DIRECT_OUTPUT_BYTES_V1);
        assert_eq!(
            serde_json::from_slice::<OutputEnvelopeV4>(&encoded)
                .expect("serialized output must validate"),
            output
        );
    }
});
