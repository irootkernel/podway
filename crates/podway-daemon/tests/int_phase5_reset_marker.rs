//! Phase 5 reset-marker codec and read-only descriptor safety contracts.

#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "support/phase4_workspace.rs"]
mod support_phase4_workspace;

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

use podway_core::{UnixMillis, WorkspaceId};
use podway_daemon::workspace::{
    MAX_RESET_MARKER_BYTES_V1, ResetMarkerErrorV1, ResetMarkerV1, RuntimeDirectoryPathViolationV1,
    SqliteWorkspaceBindingInspectorV1, WorkspaceResolverV1,
};
use podway_git::NativeGitResolverV1;
use podway_protocol::RequestIdV1;
use podway_store::{CanonicalRequestDigestV1, IdempotencyKeyV1, JobIdV1, SqliteStoreOptionsV1};
use support_phase4_workspace::{git_worktrees, selector};

fn marker() -> ResetMarkerV1 {
    ResetMarkerV1::new(
        JobIdV1::new("00000000-0000-4000-8000-000000000101")
            .expect("fixture operation ID must be valid"),
        IdempotencyKeyV1::new("reset-marker-fixture")
            .expect("fixture idempotency key must be valid"),
        CanonicalRequestDigestV1::new(format!("sha256:{}", "a".repeat(64)))
            .expect("fixture request digest must be valid"),
        WorkspaceId::new("00000000-0000-4000-8000-000000000100")
            .expect("fixture predecessor workspace UUID must be valid"),
        WorkspaceId::new("00000000-0000-4000-8000-000000000102")
            .expect("fixture target workspace UUID must be valid"),
        UnixMillis::new(1_700_000_000_123),
    )
}

fn set_mode(path: &std::path::Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("fixture permissions must be set");
}

#[test]
fn marker_codec_binds_predecessor_and_rejects_old_shape() {
    let marker = marker();
    let bytes = marker
        .canonical_bytes()
        .expect("marker must encode canonically");
    assert_eq!(
        ResetMarkerV1::decode_canonical(&bytes).expect("canonical marker must roundtrip"),
        marker
    );
    let tampered = String::from_utf8(bytes.clone())
        .expect("canonical marker bytes must be UTF-8")
        .replace(
            "00000000-0000-4000-8000-000000000100",
            "00000000-0000-4000-8000-000000000199",
        )
        .into_bytes();
    assert_eq!(
        ResetMarkerV1::decode_canonical(&tampered)
            .expect("a canonical marker remains an observable document")
            .previous_workspace_uuid()
            .as_str(),
        "00000000-0000-4000-8000-000000000199"
    );
    let old_shape = br#"{"idempotency_key":"reset-marker-fixture","operation_id":"00000000-0000-4000-8000-000000000101","request_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","schema":"podway.reset-marker/v1","submitted_at_ms":1700000000123,"target_workspace_uuid":"00000000-0000-4000-8000-000000000102"}"#;
    assert!(matches!(
        ResetMarkerV1::decode_canonical(old_shape),
        Err(ResetMarkerErrorV1::InvalidShape)
    ));
    assert!(matches!(
        ResetMarkerV1::decode_canonical(&vec![b'x'; MAX_RESET_MARKER_BYTES_V1 as usize + 1]),
        Err(ResetMarkerErrorV1::TooLarge { .. })
    ));
}

#[test]
fn marker_v2_roundtrips_the_terminal_response_correlation() {
    let legacy = marker();
    let marker = ResetMarkerV1::new_with_response_request_id(
        legacy.operation_id().clone(),
        legacy.idempotency_key().clone(),
        legacy.request_digest().clone(),
        legacy.previous_workspace_uuid().clone(),
        legacy.target_workspace_uuid().clone(),
        legacy.submitted_at_ms(),
        RequestIdV1::new("00000000-0000-4000-8000-000000000103")
            .expect("fixture response request ID must be valid"),
    );
    let bytes = marker
        .canonical_bytes()
        .expect("v2 marker must encode canonically");
    assert!(String::from_utf8_lossy(&bytes).contains("podway.reset-marker/v2"));
    assert_eq!(
        ResetMarkerV1::decode_canonical(&bytes).expect("v2 marker must roundtrip"),
        marker
    );
}

#[test]
fn runtime_marker_read_rejects_symlink_and_nonprivate_files() {
    let fixture = git_worktrees();
    let runtime_path = fixture.main().join(".podway/runtime");
    set_mode(&runtime_path, 0o700);
    let resolver = WorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        SqliteWorkspaceBindingInspectorV1::new(
            SqliteStoreOptionsV1::new(8).expect("fixture Store options must be valid"),
        ),
    );
    let reset = resolver
        .resolve_for_reset(selector(fixture.main()))
        .expect("Git-only reset resolution must not require a database");
    let runtime = reset
        .open_runtime_directory()
        .expect("Git-validated runtime directory must open");

    let target = runtime_path.join("marker-target");
    fs::write(&target, b"target").expect("symlink target must exist");
    set_mode(&target, 0o600);
    symlink(&target, runtime_path.join("reset.marker"))
        .expect("marker symlink fixture must be created");
    assert!(
        runtime.read_reset_marker().is_err(),
        "marker symlinks must fail closed"
    );
    fs::remove_file(runtime_path.join("reset.marker")).expect("symlink fixture must be removed");

    fs::write(
        runtime_path.join("reset.marker"),
        marker()
            .canonical_bytes()
            .expect("fixture marker must encode"),
    )
    .expect("marker fixture must be written");
    set_mode(&runtime_path.join("reset.marker"), 0o644);
    assert!(matches!(
        runtime.read_reset_marker(),
        Err(
            podway_daemon::workspace::ValidatedRuntimeDirectoryErrorV1::UnsafeFile {
                violation: RuntimeDirectoryPathViolationV1::WrongMode { .. },
                ..
            }
        )
    ));
}
