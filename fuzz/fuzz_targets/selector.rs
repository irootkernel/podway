#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_git::{
    Base64UrlPathBytesV1, DiagnosticPathDisplayV1, LosslessPathV1, MAX_SELECTOR_COMPONENT_BYTES_V1,
    WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};

fn canonical_path(component_bytes: usize) -> Vec<u8> {
    let mut path = Vec::with_capacity(component_bytes);
    path.push(b'/');
    path.extend(std::iter::repeat_n(b'a', component_bytes.saturating_sub(1)));
    path
}

fn assert_canonical(path: &[u8]) {
    let display = DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid");
    let path = LosslessPathV1::from_raw_bytes(path, display).expect("canonical local path");
    let parsed = Base64UrlPathBytesV1::new(path.path_bytes_base64url().as_str().to_owned())
        .expect("canonical encoding parses");
    assert_eq!(
        parsed.decode().expect("canonical encoding decodes"),
        path.decode_path_bytes().expect("canonical path decodes")
    );
    let selector = WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_base64url(
            parsed,
            DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid"),
        ),
    )
    .expect("canonical selector");
    assert_eq!(
        selector.path().decode_path_bytes().expect("selector path decodes"),
        path.decode_path_bytes().expect("canonical path decodes")
    );
}

fuzz_target!(|input: &[u8]| {
    let max_raw_bytes = (MAX_SELECTOR_COMPONENT_BYTES_V1 / 4) * 3;
    let max_minus_one = canonical_path(max_raw_bytes - 1);
    let max = canonical_path(max_raw_bytes);
    assert_canonical(&max_minus_one);
    assert_canonical(&max);
    assert!(
        LosslessPathV1::from_raw_bytes(
            canonical_path(max_raw_bytes + 1),
            DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid"),
        )
        .is_err(),
        "base64url selector component max+1 must reject"
    );

    assert!(
        LosslessPathV1::from_raw_bytes(
            b"/nul\0byte",
            DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid"),
        )
        .is_err(),
        "NUL path bytes must reject"
    );
    assert!(
        LosslessPathV1::from_raw_bytes(
            b"/repeated//separator",
            DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid"),
        )
        .is_err(),
        "repeated separators must reject"
    );
    for malformed in ["%", "A=", "AAAA", "YWJj="] {
        assert!(
            Base64UrlPathBytesV1::new(malformed.to_owned()).is_err(),
            "malformed or noncanonical base64url must reject: {malformed}"
        );
    }

    let mut adversarial_path = Vec::with_capacity(input.len().min(MAX_SELECTOR_COMPONENT_BYTES_V1));
    adversarial_path.push(b'/');
    adversarial_path.extend_from_slice(
        &input[..input.len().min(MAX_SELECTOR_COMPONENT_BYTES_V1.saturating_sub(1))],
    );
    match LosslessPathV1::from_raw_bytes(
        &adversarial_path,
        DiagnosticPathDisplayV1::new("/fuzz-selector").expect("static display is valid"),
    ) {
        Ok(path) => assert_canonical(&path.decode_path_bytes().expect("raw path bytes")),
        Err(_) => {}
    }

    if let Ok(encoded) = std::str::from_utf8(input) {
        match Base64UrlPathBytesV1::new(encoded.to_owned()) {
            Ok(parsed) => assert_eq!(
                Base64UrlPathBytesV1::from_raw_bytes(parsed.decode().expect("validated base64url")),
                Ok(parsed)
            ),
            Err(_) => {}
        }
    }
});
