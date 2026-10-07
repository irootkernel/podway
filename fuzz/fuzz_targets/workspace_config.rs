#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_config::{CanonicalDigest, CanonicalJson, parse_workspace_config_v1};

fuzz_target!(|input: &[u8]| {
    if let Ok(config) = parse_workspace_config_v1(input) {
        let canonical = config
            .canonical_json_v1()
            .expect("validated config must canonicalize");
        let reparsed =
            parse_workspace_config_v1(canonical.as_bytes()).expect("canonical config must parse");
        assert_eq!(
            reparsed.canonical_digest_v1().unwrap(),
            config.canonical_digest_v1().unwrap()
        );
        let value: serde_json::Value = serde_json::from_slice(canonical.as_bytes()).unwrap();
        assert!(value.get("mode").is_none());
        assert!(value.get("job_queue").is_none());
    }
});
