#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_git::{
    Base64UrlPathBytesV1, DiagnosticPathDisplayV1, LosslessPathV1, MAX_SELECTOR_COMPONENT_BYTES_V1,
    WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};

fuzz_target!(|input: &[u8]| {
    if input.len() > MAX_SELECTOR_COMPONENT_BYTES_V1 {
        return;
    }

    if input.len() < MAX_SELECTOR_COMPONENT_BYTES_V1 {
        let mut path = Vec::with_capacity(input.len().saturating_add(1));
        path.push(b'/');
        path.extend(input.iter().copied().map(|byte| match byte {
            b'\0' => b'_',
            byte => byte,
        }));
        if path.len() == 1 {
            path.push(b'.');
        }
        if path[1] == b'/' {
            path[1] = b'.';
        }

        let display =
            DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid");
        if let Ok(path) = LosslessPathV1::from_raw_bytes(&path, display) {
            let encoded = path.path_bytes_base64url().as_str();
            let parsed = Base64UrlPathBytesV1::new(encoded.to_owned())
                .expect("lossless path encoding must parse");
            assert_eq!(parsed.decode().as_deref(), path.decode_path_bytes().as_deref());

            let selector = WorktreeSelectorV1::new(
                WORKTREE_SELECTOR_VERSION_V1,
                None,
                LosslessPathV1::from_base64url(
                    parsed,
                    DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid"),
                ),
            )
            .expect("lossless selector must accept its canonical path");
            assert_eq!(selector.path().decode_path_bytes(), path.decode_path_bytes());
        }
    }

    if let Ok(encoded) = std::str::from_utf8(input) {
        if let Ok(parsed) = Base64UrlPathBytesV1::new(encoded.to_owned()) {
            let decoded = parsed.decode().expect("validated base64url must decode");
            assert_eq!(Base64UrlPathBytesV1::from_raw_bytes(&decoded), Ok(parsed));
        }
    }
});
