#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_config::{
    CanonicalDigest, CanonicalJson, MAX_WORKSPACE_CONFIG_BYTES_V1, parse_workspace_config_v1,
};
use podway_core::{canonicalize_json_v1, verify_canonical_json_v1};

fuzz_target!(|input: &[u8]| {
    if input.len() > MAX_WORKSPACE_CONFIG_BYTES_V1 {
        return;
    }

    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(input) {
        if let Ok(canonical) = canonicalize_json_v1(&value) {
            assert!(verify_canonical_json_v1(canonical.as_bytes()).is_ok());
            let reparsed = serde_json::from_str::<serde_json::Value>(&canonical)
                .expect("canonical JSON must always parse");
            assert_eq!(canonicalize_json_v1(&reparsed).as_deref(), Ok(canonical.as_str()));
        }
    }

    if let Ok(config) = parse_workspace_config_v1(input) {
        let canonical = config
            .canonical_json_v1()
            .expect("validated workspace config must canonicalize");
        let digest = config
            .canonical_digest_v1()
            .expect("validated workspace config must have a digest");
        assert!(verify_canonical_json_v1(canonical.as_bytes()).is_ok());
        assert_eq!(
            config
                .canonical_json_v1()
                .expect("validated workspace config must remain canonical"),
            canonical
        );
        assert_eq!(
            config
                .canonical_digest_v1()
                .expect("validated workspace config must retain its digest"),
            digest
        );
    }
});
