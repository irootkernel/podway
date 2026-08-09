#![cfg(unix)]

use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    fs,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::symlink,
    },
    path::{Path, PathBuf},
    process::Command,
};

use nix::{sys::stat::Mode, unistd::mkfifo};
use podway_core::{
    DomainError, ItemId, MAX_PROCEDURE_DOCUMENT_BYTES_V1, ProcedureSnapshotId, UnixMillis,
};
use podway_daemon::{
    execution::{
        ArtifactVerifierV1, ExecutionBoundaryErrorV1, ExecutionIdSourceV1, ProcedureProviderV1,
        WorkspaceRevalidatorV1,
    },
    native_execution::{
        NativeArtifactVerifierV1, NativeExecutionIdSourceV1, NativeProcedureProviderV1,
        NativeWorkspaceRevalidatorV1, embedded_media_type_v1,
    },
    workspace::{SqliteWorkspaceBindingInspectorV1, WorkspaceResolverV1},
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::WorktreeSelectorWireV1;
use podway_store::{SqliteStoreOptionsV1, SqliteStoreV1, WorkspaceBindingV1};
use uuid::{Uuid, Version};

const FIXTURE_DISPLAY_V1: &str = "native execution fixture";
const PROCEDURE_YAML: &[u8] = include_bytes!("../../../assets/presets/sw-dev.yaml");
const PROCEDURE_V2_YAML: &[u8] = br#"schema: podway.procedure/v2
id: unsupported-native-v2
version: "2"
name: Unsupported native v2
entry: work
nodes:
  - id: work
    action:
      instructions: ["Do work"]
"#;

struct WorktreeFixtureV1 {
    temporary_root: PathBuf,
    worktree_root: PathBuf,
    binding: WorkspaceBindingV1,
    options: SqliteStoreOptionsV1,
}

impl Drop for WorktreeFixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temporary_root);
    }
}

fn fixture_with_root_name_v1(name: OsString) -> Option<WorktreeFixtureV1> {
    let requested_temporary_root =
        env::temp_dir().join(format!("podway-native-execution-{}", Uuid::new_v4()));
    fs::create_dir_all(&requested_temporary_root).expect("temporary fixture directory");
    let temporary_root = fs::canonicalize(&requested_temporary_root)
        .expect("temporary fixture root must canonicalize");
    let initialized_root = temporary_root.join("initialized-worktree");
    let worktree_root = temporary_root.join(name);
    let status = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&initialized_root)
        .status()
        .expect("git must be available for native worktree fixtures");
    assert!(status.success(), "git init must create a non-bare worktree");
    if let Err(error) = fs::rename(&initialized_root, &worktree_root) {
        if error.raw_os_error() == Some(92) {
            let _ = fs::remove_dir_all(&temporary_root);
            return None;
        }
        panic!("fixture worktree must move to its exact native path: {error}");
    }
    assert!(worktree_root.join(".git").is_dir());
    fs::create_dir_all(worktree_root.join(".podway/runtime")).expect("runtime directory");

    let options = SqliteStoreOptionsV1::new(32).expect("valid Store options");
    let inspector = SqliteWorkspaceBindingInspectorV1::new(options.clone());
    let bootstrap = WorkspaceResolverV1::new(NativeGitResolverV1::new(), inspector.clone())
        .resolve_bootstrap(git_selector_v1(&worktree_root))
        .expect("bootstrap resolution");
    let store = SqliteStoreV1::open(
        bootstrap.database_path(),
        bootstrap.workspace_root(),
        bootstrap.store_identity().clone(),
        options.clone(),
        UnixMillis::new(1),
    )
    .expect("SQLite binding initialization");
    drop(store);

    let revalidator = NativeWorkspaceRevalidatorV1::new(inspector);
    let selector = wire_selector_v1(
        &worktree_root,
        Some(bootstrap.store_identity().workspace_uuid().clone()),
    );
    let binding = revalidator
        .revalidate(&selector)
        .expect("two-pass Store/Git revalidation");
    Some(WorktreeFixtureV1 {
        temporary_root,
        worktree_root,
        binding,
        options,
    })
}

fn fixture_v1() -> WorktreeFixtureV1 {
    fixture_with_root_name_v1(OsString::from("worktree"))
        .expect("the fixture filesystem must accept an ASCII worktree name")
}

fn git_selector_v1(path: &Path) -> WorktreeSelectorV1 {
    let path = LosslessPathV1::from_raw_bytes(
        path.as_os_str().as_bytes(),
        DiagnosticPathDisplayV1::new(FIXTURE_DISPLAY_V1).expect("fixed display"),
    )
    .expect("fixture path is a lossless local path");
    WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, path)
        .expect("supported Git selector")
}

fn wire_selector_v1(
    path: &Path,
    expected_uuid: Option<podway_core::WorkspaceId>,
) -> WorktreeSelectorWireV1 {
    WorktreeSelectorWireV1::new(
        path.as_os_str().as_bytes(),
        FIXTURE_DISPLAY_V1,
        expected_uuid,
    )
    .expect("supported wire selector")
}

fn revalidator_v1(
    options: SqliteStoreOptionsV1,
) -> NativeWorkspaceRevalidatorV1<SqliteWorkspaceBindingInspectorV1> {
    NativeWorkspaceRevalidatorV1::new(SqliteWorkspaceBindingInspectorV1::new(options))
}

fn artifact_verifier_v1(
    options: SqliteStoreOptionsV1,
) -> NativeArtifactVerifierV1<SqliteWorkspaceBindingInspectorV1> {
    NativeArtifactVerifierV1::new(SqliteWorkspaceBindingInspectorV1::new(options))
}

fn procedure_provider_v1(
    options: SqliteStoreOptionsV1,
) -> NativeProcedureProviderV1<SqliteWorkspaceBindingInspectorV1> {
    NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(options))
}

fn assert_domain_rejection_v1<T: std::fmt::Debug>(result: Result<T, ExecutionBoundaryErrorV1>) {
    match result {
        Err(ExecutionBoundaryErrorV1::Domain(DomainError::InvalidState { .. })) => {}
        outcome => panic!("expected a durable domain rejection, received {outcome:?}"),
    }
}
fn assert_artifact_changed_v1<T: std::fmt::Debug>(result: Result<T, ExecutionBoundaryErrorV1>) {
    match result {
        Err(ExecutionBoundaryErrorV1::Domain(DomainError::ArtifactChanged)) => {}
        outcome => panic!("expected an artifact-changed rejection, received {outcome:?}"),
    }
}

fn copy_directory_v1(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("copy destination directory");
    for entry in fs::read_dir(source).expect("source directory") {
        let entry = entry.expect("directory entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = entry.file_type().expect("entry file type");
        if kind.is_dir() {
            copy_directory_v1(&source_path, &destination_path);
        } else {
            assert!(kind.is_file(), "fixture repositories contain no symlinks");
            fs::copy(&source_path, &destination_path).expect("copy fixture file");
        }
    }
}

#[test]
fn store_bound_worktree_move_retains_uuid_and_fingerprint_continuity() {
    let mut fixture = fixture_v1();
    let moved_root = fixture.temporary_root.join("moved-worktree");
    fs::rename(&fixture.worktree_root, &moved_root).expect("move worktree");
    fixture.worktree_root = moved_root.clone();

    let revalidated = revalidator_v1(fixture.options.clone())
        .revalidate(&wire_selector_v1(
            &moved_root,
            Some(fixture.binding.identity().workspace_uuid().clone()),
        ))
        .expect("moved worktree must revalidate through the prior Store binding");

    assert_eq!(
        revalidated.identity(),
        fixture.binding.identity(),
        "a move must not create a path-keyed workspace identity"
    );
    assert_eq!(
        revalidated.last_validated_root().unix_bytes(),
        moved_root.as_os_str().as_bytes(),
        "the returned binding must carry the resolver's current root"
    );
}

#[test]
fn copied_sqlite_binding_is_rejected_as_a_different_worktree() {
    let fixture = fixture_v1();
    let copied_root = fixture.temporary_root.join("copied-worktree");
    copy_directory_v1(&fixture.worktree_root, &copied_root);

    assert_domain_rejection_v1(revalidator_v1(fixture.options.clone()).revalidate(
        &wire_selector_v1(
            &copied_root,
            Some(fixture.binding.identity().workspace_uuid().clone()),
        ),
    ));
}

#[test]
fn g006_workspace_procedure_start_uses_a_bounded_regular_file_without_symlink_traversal() {
    let fixture = fixture_v1();
    fs::write(fixture.worktree_root.join("procedure.yaml"), PROCEDURE_YAML)
        .expect("original Procedure");
    let provider = procedure_provider_v1(fixture.options.clone());
    provider
        .load_workspace_procedure_snapshot(
            &fixture.binding,
            "procedure.yaml",
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009906").expect("snapshot ID"),
            UnixMillis::new(100),
        )
        .expect("stable Procedure source");

    symlink(
        fixture.worktree_root.join("procedure.yaml"),
        fixture.worktree_root.join("linked.yaml"),
    )
    .expect("Procedure symlink");
    assert!(
        provider
            .load_workspace_procedure_snapshot(
                &fixture.binding,
                "linked.yaml",
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009908")
                    .expect("snapshot ID"),
                UnixMillis::new(100),
            )
            .is_err()
    );

    let nested = fixture.worktree_root.join("nested");
    fs::create_dir(&nested).expect("nested Procedure directory");
    fs::write(nested.join("procedure.yaml"), PROCEDURE_YAML).expect("nested Procedure");
    provider
        .load_workspace_procedure_snapshot(
            &fixture.binding,
            "nested/procedure.yaml",
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009910").expect("snapshot ID"),
            UnixMillis::new(100),
        )
        .expect("stable nested Procedure source");
    symlink(&nested, fixture.worktree_root.join("linked-directory"))
        .expect("Procedure directory symlink");
    assert!(
        provider
            .load_workspace_procedure_snapshot(
                &fixture.binding,
                "linked-directory/procedure.yaml",
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009911")
                    .expect("snapshot ID"),
                UnixMillis::new(100),
            )
            .is_err()
    );

    mkfifo(
        &fixture.worktree_root.join("procedure.fifo"),
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .expect("Procedure FIFO");
    assert!(
        provider
            .load_workspace_procedure_snapshot(
                &fixture.binding,
                "procedure.fifo",
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009912")
                    .expect("snapshot ID"),
                UnixMillis::new(100),
            )
            .is_err()
    );

    let oversized = fs::File::create(fixture.worktree_root.join("oversized.yaml"))
        .expect("oversized Procedure");
    oversized
        .set_len((MAX_PROCEDURE_DOCUMENT_BYTES_V1 + 1) as u64)
        .expect("oversized Procedure length");
    assert!(
        provider
            .load_workspace_procedure_snapshot(
                &fixture.binding,
                "oversized.yaml",
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009909")
                    .expect("snapshot ID"),
                UnixMillis::new(100),
            )
            .is_err()
    );

    let retired = fixture.temporary_root.join("retired-worktree");
    fs::rename(&fixture.worktree_root, &retired).expect("retire validated worktree");
    let status = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&fixture.worktree_root)
        .status()
        .expect("replacement git init");
    assert!(status.success());
    fs::write(
        fixture.worktree_root.join("procedure.yaml"),
        b"replacement workspace bytes",
    )
    .expect("replacement Procedure");

    assert!(
        provider
            .load_workspace_procedure_snapshot(
                &fixture.binding,
                "procedure.yaml",
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009907")
                    .expect("snapshot ID"),
                UnixMillis::new(101),
            )
            .is_err()
    );
}

#[test]
fn v2plt007_native_provider_classifies_source_declared_v2_before_v1_parsing() {
    let fixture = fixture_v1();
    fs::write(
        fixture.worktree_root.join("procedure-v2.yaml"),
        PROCEDURE_V2_YAML,
    )
    .expect("v2 Procedure source");

    let error = procedure_provider_v1(fixture.options.clone())
        .load_workspace_procedure_snapshot(
            &fixture.binding,
            "procedure-v2.yaml",
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009913").expect("snapshot ID"),
            UnixMillis::new(100),
        )
        .expect_err("v2 admission must remain locked");

    assert!(matches!(
        error,
        ExecutionBoundaryErrorV1::ProcedureV2Unsupported
    ));
}

#[test]
fn non_utf8_worktree_roots_round_trip_without_lossy_path_identity() {
    let mut leaf = b"non-utf8-".to_vec();
    leaf.push(0xff);
    let Some(fixture) = fixture_with_root_name_v1(OsString::from_vec(leaf)) else {
        return;
    };
    assert!(fixture.worktree_root.as_os_str().as_bytes().contains(&0xff));
    assert_eq!(
        fixture.binding.last_validated_root().unix_bytes(),
        fixture.worktree_root.as_os_str().as_bytes()
    );

    fs::write(fixture.worktree_root.join("artifact.txt"), b"native bytes").expect("artifact bytes");
    let artifact = artifact_verifier_v1(fixture.options.clone())
        .hash_local_artifact(&fixture.binding, "artifact.txt", None)
        .expect("artifact under a non-UTF-8 root");
    assert_eq!(artifact.location(), "artifact.txt");
    assert_eq!(artifact.media_type(), "text/plain");
    assert_eq!(artifact.size_bytes(), b"native bytes".len() as u64);
}

#[test]
fn artifact_paths_reject_symlinks_and_worktree_escapes() {
    let fixture = fixture_v1();
    fs::write(fixture.worktree_root.join("inside.txt"), b"inside").expect("artifact bytes");
    symlink(
        fixture.worktree_root.join("inside.txt"),
        fixture.worktree_root.join("alias.txt"),
    )
    .expect("fixture symlink");
    let verifier = artifact_verifier_v1(fixture.options.clone());

    assert_domain_rejection_v1(verifier.hash_local_artifact(&fixture.binding, "alias.txt", None));
    assert_domain_rejection_v1(verifier.hash_local_artifact(
        &fixture.binding,
        "../outside.txt",
        None,
    ));
}

#[test]
fn artifact_completion_revalidation_detects_content_replacement() {
    let fixture = fixture_v1();
    fs::write(
        fixture.worktree_root.join("report.json"),
        b"{\"state\":\"before\"}\n",
    )
    .expect("original artifact bytes");
    let verifier = artifact_verifier_v1(fixture.options.clone());
    let artifact = verifier
        .hash_local_artifact(&fixture.binding, "report.json", None)
        .expect("initial artifact hash");
    fs::write(
        fixture.worktree_root.join("report.json"),
        b"{\"state\":\"after_\"}\n",
    )
    .expect("replacement artifact bytes");
    assert_eq!(
        fs::metadata(fixture.worktree_root.join("report.json"))
            .expect("replacement metadata")
            .len(),
        artifact.size_bytes(),
        "the replacement keeps the original byte length so digest revalidation is required"
    );

    assert_artifact_changed_v1(verifier.revalidate_local_artifact(
        &fixture.binding,
        &ItemId::new("proof").expect("valid item identifier"),
        &artifact,
    ));
}

#[test]
fn embedded_media_mapping_is_deterministic_and_has_a_fixed_unknown_fallback() {
    assert_eq!(
        embedded_media_type_v1("reports/summary.json"),
        "application/json"
    );
    assert_eq!(embedded_media_type_v1("reports/notes.md"), "text/markdown");
    assert_eq!(
        embedded_media_type_v1("reports/unknown.extension"),
        "application/octet-stream"
    );

    let fixture = fixture_v1();
    fs::write(fixture.worktree_root.join("result.json"), b"{}\n").expect("JSON artifact");
    fs::write(fixture.worktree_root.join("result.unknown"), b"bytes").expect("unknown artifact");
    let verifier = artifact_verifier_v1(fixture.options.clone());
    assert_eq!(
        verifier
            .hash_local_artifact(&fixture.binding, "result.json", None)
            .expect("JSON hash")
            .media_type(),
        "application/json"
    );
    assert_eq!(
        verifier
            .hash_local_artifact(&fixture.binding, "result.unknown", None)
            .expect("unknown hash")
            .media_type(),
        "application/octet-stream"
    );
}

#[test]
fn native_id_source_emits_unique_canonical_uuid_v4_values() {
    let ids = NativeExecutionIdSourceV1;
    let mut values = HashSet::new();
    for _ in 0..128 {
        let value = ids.next_job_id();
        let parsed = Uuid::parse_str(value.as_str()).expect("generated UUID syntax");
        assert_eq!(parsed.get_version(), Some(Version::Random));
        assert!(
            values.insert(value.into_inner()),
            "UUID v4 collision in fixture sample"
        );
    }

    for value in [
        ids.next_session_id().into_inner(),
        ids.next_attempt_id().into_inner(),
        ids.next_blocker_id().into_inner(),
        ids.next_procedure_snapshot_id().into_inner(),
    ] {
        let parsed = Uuid::parse_str(&value).expect("generated UUID syntax");
        assert_eq!(parsed.get_version(), Some(Version::Random));
        assert!(
            values.insert(value),
            "IDs across execution types must not collide"
        );
    }
}
