//! Phase 0B Git-selector construction contracts; no Git repository is opened or mutated.
//!
//! Requirements: ARC-005, SEC-002.

use podway_core::{Sha256Digest, WorkspaceId};
use podway_git::{
    Base64UrlPathBytesV1, ContainmentMetadataV1, DiagnosticPathDisplayV1,
    DurableWorktreeIdentityV1, GitResolveErrorV1, GitResolverContractV1, LosslessPathV1,
    MAX_SELECTOR_COMPONENT_BYTES_V1, SelectorValidationErrorV1, ValidatedWorktreeV1,
    ValidatedWorktreeValidationErrorV1, WORKTREE_SELECTOR_VERSION_V1, WorkspaceUuidVerificationV1,
    WorktreeKindV1, WorktreeMoveMetadataV1, WorktreeRepairMetadataV1, WorktreeRootsV1,
    WorktreeSelectorV1,
};

fn digest(hex_digit: char) -> Sha256Digest {
    let value = format!("sha256:{}", hex_digit.to_string().repeat(64));
    match Sha256Digest::new(value) {
        Ok(value) => value,
        Err(_) => panic!("fixture digest must be valid"),
    }
}

fn path(raw_bytes: &[u8]) -> LosslessPathV1 {
    path_with_display(raw_bytes, "fixture path")
}

fn path_with_display(raw_bytes: &[u8], display: &str) -> LosslessPathV1 {
    let display = match DiagnosticPathDisplayV1::new(display) {
        Ok(value) => value,
        Err(_) => panic!("fixture display must be valid"),
    };
    match LosslessPathV1::from_raw_bytes(raw_bytes, display) {
        Ok(value) => value,
        Err(_) => panic!("fixture path bytes must be valid"),
    }
}

fn validated_worktree(
    podway_directory: &[u8],
    runtime_directory: &[u8],
) -> Result<ValidatedWorktreeV1, ValidatedWorktreeValidationErrorV1> {
    ValidatedWorktreeV1::new(
        DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
            WorkspaceId::new("00000000-0000-4000-8000-000000000010").expect("valid workspace ID"),
            digest('a'),
            digest('b'),
            digest('c'),
            path(b"/workspace"),
        ),
        WorktreeRootsV1::new(
            path(b"/workspace"),
            path(b"/common/.git"),
            path(b"/common/.git/worktrees/main"),
        ),
        WorktreeKindV1::Main,
        ContainmentMetadataV1::new(
            path(b"/workspace"),
            path(podway_directory),
            path(runtime_directory),
        ),
        WorktreeMoveMetadataV1::stationary(path(b"/workspace")),
        WorktreeRepairMetadataV1::not_required_with_uuid_verification(
            WorkspaceUuidVerificationV1::RegistryCheckRequired,
        ),
    )
}
struct SignatureResolver;

impl GitResolverContractV1 for SignatureResolver {
    fn resolve(
        &self,
        _selector: WorktreeSelectorV1,
    ) -> Result<ValidatedWorktreeV1, GitResolveErrorV1> {
        Err(GitResolveErrorV1::Selector(
            SelectorValidationErrorV1::RelativePath,
        ))
    }
}

#[test]
fn git_resolver_v1_frozen_resolve_signature_is_executable() {
    let selector = WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, path(b"/workspace"))
        .expect("valid selector");
    let result: Result<ValidatedWorktreeV1, GitResolveErrorV1> =
        SignatureResolver.resolve(selector);

    assert!(matches!(
        result,
        Err(GitResolveErrorV1::Selector(
            SelectorValidationErrorV1::RelativePath
        ))
    ));
}

#[test]
fn sec_002_git_v1_lossless_base64url_round_trips_non_utf8_path_bytes() {
    let raw_path = b"/workspace/non-utf8-\xff\x80";
    let encoded = match Base64UrlPathBytesV1::from_raw_bytes(raw_path) {
        Ok(value) => value,
        Err(_) => panic!("non-UTF-8 path bytes without NUL must be accepted"),
    };
    assert!(
        encoded
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
    assert!(!encoded.as_str().contains('='));
    let decoded = match encoded.decode() {
        Ok(value) => value,
        Err(_) => panic!("encoded path bytes must decode"),
    };
    assert_eq!(decoded, raw_path);

    let lossless_path = match LosslessPathV1::from_raw_bytes(
        raw_path,
        match DiagnosticPathDisplayV1::new("/workspace/non-utf8") {
            Ok(value) => value,
            Err(_) => panic!("fixture display must be valid"),
        },
    ) {
        Ok(value) => value,
        Err(_) => panic!("lossless path must preserve valid native bytes"),
    };
    let round_trip = match lossless_path.decode_path_bytes() {
        Ok(value) => value,
        Err(_) => panic!("lossless path must decode"),
    };
    assert_eq!(round_trip, raw_path);
    let selector = match WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, lossless_path)
    {
        Ok(value) => value,
        Err(_) => panic!("canonical selector must accept a valid lossless path"),
    };
    assert_eq!(selector.version(), WORKTREE_SELECTOR_VERSION_V1);
    let selected_bytes = match selector.path().decode_path_bytes() {
        Ok(value) => value,
        Err(_) => panic!("selector path must remain lossless"),
    };
    assert_eq!(selected_bytes, raw_path);
}

#[test]
fn sec_002_git_v1_selector_rejects_nul_oversized_and_unknown_components() {
    assert!(matches!(
        Base64UrlPathBytesV1::from_raw_bytes(b"/workspace\0state"),
        Err(SelectorValidationErrorV1::EmbeddedNulPathByte)
    ));
    assert!(matches!(
        Base64UrlPathBytesV1::new("AA"),
        Err(SelectorValidationErrorV1::EmbeddedNulPathByte)
    ));
    assert!(matches!(
        Base64UrlPathBytesV1::from_raw_bytes(vec![b'x'; MAX_SELECTOR_COMPONENT_BYTES_V1 + 1]),
        Err(SelectorValidationErrorV1::FieldTooLong {
            field: "path_bytes",
            maximum_bytes: MAX_SELECTOR_COMPONENT_BYTES_V1,
        })
    ));
    assert!(matches!(
        DiagnosticPathDisplayV1::new("d".repeat(MAX_SELECTOR_COMPONENT_BYTES_V1 + 1)),
        Err(SelectorValidationErrorV1::FieldTooLong {
            field: "display",
            maximum_bytes: MAX_SELECTOR_COMPONENT_BYTES_V1,
        })
    ));

    assert!(matches!(
        WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1 + 1, None, path(b"/workspace"),),
        Err(SelectorValidationErrorV1::UnsupportedVersion { found })
            if found == WORKTREE_SELECTOR_VERSION_V1 + 1
    ));
}
#[test]
fn sec_002_git_v1_selector_rejects_relative_traversal_and_non_local_paths() {
    assert!(matches!(
        Base64UrlPathBytesV1::from_raw_bytes(b"workspace"),
        Err(SelectorValidationErrorV1::RelativePath)
    ));
    assert!(matches!(
        Base64UrlPathBytesV1::new("d29ya3NwYWNl"),
        Err(SelectorValidationErrorV1::RelativePath)
    ));
    assert!(matches!(
        Base64UrlPathBytesV1::from_raw_bytes(b"/workspace/./.podway"),
        Err(SelectorValidationErrorV1::NonCanonicalPath)
    ));
    assert!(matches!(
        Base64UrlPathBytesV1::from_raw_bytes(b"/workspace/../outside"),
        Err(SelectorValidationErrorV1::NonCanonicalPath)
    ));
    assert!(matches!(
        Base64UrlPathBytesV1::from_raw_bytes(b"//host/workspace"),
        Err(SelectorValidationErrorV1::NonLocalPath)
    ));
    assert!(matches!(
        Base64UrlPathBytesV1::from_raw_bytes(b"file:///workspace"),
        Err(SelectorValidationErrorV1::NonLocalPath)
    ));
}

#[test]
fn sec_002_git_v1_path_identity_ignores_diagnostic_display_text() {
    let worktree = ValidatedWorktreeV1::new(
        DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
            WorkspaceId::new("00000000-0000-4000-8000-000000000011").expect("valid workspace ID"),
            digest('a'),
            digest('b'),
            digest('c'),
            path_with_display(b"/workspace", "identity root"),
        ),
        WorktreeRootsV1::new(
            path_with_display(b"/workspace", "discovered root"),
            path_with_display(b"/common/.git", "common directory"),
            path_with_display(b"/common/.git/worktrees/main", "worktree administration"),
        ),
        WorktreeKindV1::Main,
        ContainmentMetadataV1::new(
            path_with_display(b"/workspace", "containment root"),
            path_with_display(b"/workspace/.podway", "Podway directory"),
            path_with_display(b"/workspace/.podway/runtime", "runtime directory"),
        ),
        WorktreeMoveMetadataV1::stationary(path_with_display(b"/workspace", "current root")),
        WorktreeRepairMetadataV1::not_required_with_uuid_verification(
            WorkspaceUuidVerificationV1::RegistryCheckRequired,
        ),
    );
    assert!(worktree.is_ok());

    assert!(matches!(
        WorktreeMoveMetadataV1::relocated(
            path_with_display(b"/workspace", "previous diagnostic"),
            path_with_display(b"/workspace", "current diagnostic"),
        ),
        Err(ValidatedWorktreeValidationErrorV1::RelocationWithoutRootChange)
    ));
}

#[test]
fn sec_002_git_v1_validated_worktree_enforces_containment_without_git_mutation() {
    let worktree = match validated_worktree(b"/workspace/.podway", b"/workspace/.podway/runtime") {
        Ok(value) => value,
        Err(_) => panic!("contained runtime metadata must be accepted"),
    };
    assert!(matches!(worktree.kind(), WorktreeKindV1::Main));
    let runtime_bytes = match worktree
        .containment()
        .runtime_directory()
        .decode_path_bytes()
    {
        Ok(value) => value,
        Err(_) => panic!("validated runtime directory must decode"),
    };
    assert_eq!(runtime_bytes, b"/workspace/.podway/runtime");

    assert!(matches!(
        validated_worktree(b"/outside/.podway", b"/outside/.podway/runtime"),
        Err(ValidatedWorktreeValidationErrorV1::PodwayDirectoryOutsideWorkspace)
    ));
    assert!(matches!(
        validated_worktree(b"/workspace/.podway", b"/workspace/runtime"),
        Err(ValidatedWorktreeValidationErrorV1::RuntimeDirectoryOutsidePodway)
    ));
    assert!(matches!(
        validated_worktree(b"/workspace2/.podway", b"/workspace2/.podway/runtime"),
        Err(ValidatedWorktreeValidationErrorV1::PodwayDirectoryOutsideWorkspace)
    ));
}
