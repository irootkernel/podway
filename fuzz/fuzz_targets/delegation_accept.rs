#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_core::{AttemptId, Revision, SessionId};
use podway_protocol::{
    DelegationAcceptV1, SessionMutationPreconditionsWireV1, validate_delegation_document_v1,
};

fuzz_target!(|bytes: &[u8]| {
    // Match the real CLI's bounded stdin admission before JSON parsing.
    if bytes.len() > 32 * 1024 {
        return;
    }
    if let Ok(input) = serde_json::from_slice(bytes) {
        if validate_delegation_document_v1("podway.delegation-accept-input/v1", &input) {
            let preconditions = SessionMutationPreconditionsWireV1 {
                expected_session_id: SessionId::new(input["session_id"].as_str().unwrap())
                    .expect("validated session"),
                expected_session_revision: Revision::new(
                    input["session_revision"].as_u64().unwrap(),
                ),
                expected_attempt_id: AttemptId::new(input["attempt_id"].as_str().unwrap())
                    .expect("validated attempt"),
            };
            let command = DelegationAcceptV1 {
                input,
                preconditions,
            };
            let canonical = serde_json::to_vec(&command.input).expect("validated input serializes");
            assert!(canonical.len() <= 32 * 1024);
            let decoded = serde_json::from_slice(&canonical).expect("canonical JSON parses");
            assert!(validate_delegation_document_v1(
                "podway.delegation-accept-input/v1",
                &decoded
            ));
            assert_eq!(command.input, decoded);
        }
    }
});
