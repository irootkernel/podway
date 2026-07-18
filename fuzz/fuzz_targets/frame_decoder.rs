#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if let Ok(payload) = podway_protocol::decode_single_frame_v1(input) {
        let _ = serde_json::from_slice::<serde_json::Value>(payload);
    }
});
