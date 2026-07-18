#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if let Ok(request) = serde_json::from_slice::<podway_protocol::RequestEnvelopeV1>(input) {
        let _ = request.validate();
    }
});
