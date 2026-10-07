#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_protocol::{MAX_DIRECT_INPUT_BYTES_V1, decode_item_record_many_input_v1};

fuzz_target!(|input: &[u8]| {
    if let Ok(batch) = decode_item_record_many_input_v1(input) {
        assert!(input.len() <= MAX_DIRECT_INPUT_BYTES_V1);
        let canonical = serde_json::to_vec(&batch).expect("validated batch must serialize");
        assert_eq!(
            decode_item_record_many_input_v1(&canonical).expect("serialized batch must parse"),
            batch
        );
    }
});
