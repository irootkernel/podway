//! Native resolver fixtures construct realistic Git administrative metadata without `git`.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use podway_core::{Sha256Digest, WorkspaceId};
use podway_git::{
    ContainmentMetadataV1, DiagnosticPathDisplayV1, DurableWorktreeIdentityV1,
    GitInvariantViolationV1, GitReadOperationV1, GitRepresentationProblemV1, GitResolverContractV1,
    GitResolverErrorV1, LosslessPathV1, NativeGitResolverV1, RegistryRepairActionV1,
    ValidatedWorktreeV1, ValidatedWorktreeValidationErrorV1, WORKTREE_SELECTOR_VERSION_V1,
    WorkspaceIdentityStateV1, WorkspaceUuidVerificationV1, WorktreeKindV1, WorktreeMoveMetadataV1,
    WorktreeRepairMetadataV1, WorktreeRootsV1, WorktreeSelectorV1,
};
use tempfile::TempDir;

fn lossless(path: &Path) -> LosslessPathV1 {
    let display = DiagnosticPathDisplayV1::new(path.as_os_str().to_string_lossy().into_owned())
        .expect("temporary fixture display is bounded");
    LosslessPathV1::from_raw_bytes(path.as_os_str().as_bytes(), display)
        .expect("temporary fixture path is absolute and canonical")
}

fn create_fifo(path: &Path) {
    // In-process mkfifo(3): the hermetic qualification sandbox permits FIFO
    // node creation under its writable roots but not spawning system tools.
    let raw = std::ffi::CString::new(path.as_os_str().as_bytes())
        .expect("fifo fixture path must not contain interior NUL bytes");
    let created = unsafe { libc::mkfifo(raw.as_ptr(), 0o600) };
    assert!(
        created == 0,
        "mkfifo must create the fixture: {}",
        std::io::Error::last_os_error()
    );
}
fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64)))
        .expect("fixture digest is canonical")
}

fn store_bound(identity: &DurableWorktreeIdentityV1) -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
        identity.workspace_id().clone(),
        identity.common_directory_fingerprint().clone(),
        identity.worktree_administration_fingerprint().clone(),
        identity
            .root_directory_fingerprint()
            .expect("native candidate has root evidence")
            .clone(),
        identity.last_validated_root().clone(),
    )
}

fn manually_validated(
    identity: DurableWorktreeIdentityV1,
    root: LosslessPathV1,
    repair_metadata: WorktreeRepairMetadataV1,
) -> Result<ValidatedWorktreeV1, ValidatedWorktreeValidationErrorV1> {
    let root_path = PathBuf::from(OsString::from_vec(
        root.decode_path_bytes().expect("root bytes"),
    ));
    let podway = lossless(&root_path.join(".podway"));
    let runtime = lossless(&root_path.join(".podway/runtime"));
    ValidatedWorktreeV1::new(
        identity,
        WorktreeRootsV1::new(root.clone(), root.clone(), root.clone()),
        WorktreeKindV1::Main,
        ContainmentMetadataV1::new(root.clone(), podway, runtime),
        WorktreeMoveMetadataV1::stationary(root),
        repair_metadata,
    )
}

fn selector(path: &Path, durable: Option<DurableWorktreeIdentityV1>) -> WorktreeSelectorV1 {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, durable, lossless(&canonical))
        .expect("fixture selector is valid")
}

fn write_head(directory: &Path) {
    fs::write(directory.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
}

fn create_common(common: &Path) {
    fs::create_dir_all(common.join("objects")).expect("common objects directory");
    fs::create_dir_all(common.join("refs")).expect("common refs directory");
    write_head(common);
}

fn create_main(root: &Path) {
    create_common(&root.join(".git"));
}

fn create_linked(worktree: &Path, common: &Path, name: &str) {
    create_common(common);
    let administration = common.join("worktrees").join(name);
    fs::create_dir_all(&administration).expect("linked administration directory");
    write_head(&administration);
    fs::create_dir_all(worktree).expect("linked worktree directory");

    let administration =
        fs::canonicalize(&administration).expect("canonical linked administration directory");
    let marker = fs::canonicalize(worktree)
        .expect("canonical linked worktree directory")
        .join(".git");
    fs::write(
        &marker,
        [
            b"gitdir: ".as_slice(),
            administration.as_os_str().as_bytes(),
            b"\n",
        ]
        .concat(),
    )
    .expect("linked marker");
    fs::write(administration.join("commondir"), b"../..\n").expect("common directory record");
    fs::write(
        administration.join("gitdir"),
        [marker.as_os_str().as_bytes(), b"\n"].concat(),
    )
    .expect("plain reciprocal linked marker");
}

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

// macOS filesystems may reject these native bytes with EILSEQ (92); only that
// platform refusal skips the non-UTF-8 fixture.
fn non_utf8_path_is_unsupported(error: &std::io::Error) -> bool {
    cfg!(target_os = "macos") && error.raw_os_error() == Some(92)
}

fn assert_artifact_snapshot_changed(result: Result<impl std::fmt::Debug, GitResolverErrorV1>) {
    assert!(
        matches!(
            &result,
            Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash
            })
        ),
        "unexpected artifact revalidation result: {result:?}"
    );
}

#[test]
fn realistic_main_linked_nested_and_nearest_layouts_are_discovered() {
    let temporary = temp();
    let outer = temporary.path().join("outer");
    let inner = outer.join("nested");
    create_main(&outer);
    create_main(&inner);
    let inner = fs::canonicalize(&inner).expect("canonical nested root");
    let selected = inner.join("child");
    fs::create_dir_all(&selected).expect("nested child");

    let resolved = NativeGitResolverV1::new()
        .resolve(selector(&selected, None))
        .expect("nearest main worktree");
    assert_eq!(resolved.kind(), &WorktreeKindV1::Main);
    assert_eq!(
        resolved
            .roots()
            .worktree_root()
            .decode_path_bytes()
            .expect("root bytes"),
        inner.as_os_str().as_bytes()
    );
    assert_eq!(
        resolved.repair_metadata().workspace_uuid_verification(),
        &WorkspaceUuidVerificationV1::PendingStoreInitialization
    );

    let common = temporary.path().join("common.git");
    let worktree = temporary.path().join("linked");
    create_linked(&worktree, &common, "linked-name");
    let common = fs::canonicalize(&common).expect("canonical common root");
    let worktree = fs::canonicalize(&worktree).expect("canonical linked root");
    let selected = worktree.join("a").join("b");
    fs::create_dir_all(&selected).expect("linked child");

    let linked = NativeGitResolverV1::new()
        .resolve(selector(&selected, None))
        .expect("linked worktree");
    assert_eq!(linked.kind(), &WorktreeKindV1::Linked);
    assert_eq!(
        linked
            .roots()
            .common_directory_root()
            .decode_path_bytes()
            .expect("common bytes"),
        common.as_os_str().as_bytes()
    );
    assert_eq!(
        linked
            .roots()
            .worktree_administration_root()
            .decode_path_bytes()
            .expect("administration bytes"),
        common.join("worktrees/linked-name").as_os_str().as_bytes()
    );
}

#[test]
fn standalone_main_without_worktrees_directory_is_discovered() {
    let temporary = temp();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    assert!(
        !worktree.join(".git/worktrees").exists(),
        "standalone fixtures must not inherit linked-worktree administration"
    );
    let worktree = fs::canonicalize(&worktree).expect("canonical standalone worktree");

    let resolved = NativeGitResolverV1::new()
        .resolve(selector(&worktree, None))
        .expect("standalone worktree");
    assert_eq!(resolved.kind(), &WorktreeKindV1::Main);
}

#[test]
fn arbitrary_empty_partial_bare_malformed_and_symlinked_layouts_fail_closed() {
    let temporary = temp();
    let resolver = NativeGitResolverV1::new();

    let arbitrary = temporary.path().join("arbitrary");
    fs::create_dir_all(&arbitrary).expect("arbitrary root");
    assert!(matches!(
        resolver.resolve(selector(&arbitrary, None)),
        Err(GitResolverErrorV1::NonGitRepository)
    ));

    let empty_git = temporary.path().join("empty-git");
    fs::create_dir_all(empty_git.join(".git")).expect("empty administration directory");
    assert!(matches!(
        resolver.resolve(selector(&empty_git, None)),
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout
        })
    ));

    let partial = temporary.path().join("partial");
    fs::create_dir_all(partial.join(".git/objects")).expect("partial objects");
    fs::create_dir_all(partial.join(".git/refs")).expect("partial refs");
    fs::write(partial.join(".git/HEAD"), b"\n").expect("empty HEAD");
    assert!(matches!(
        resolver.resolve(selector(&partial, None)),
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout
        })
    ));

    let bare = temporary.path().join("bare");
    create_common(&bare);
    assert!(matches!(
        resolver.resolve(selector(&bare, None)),
        Err(GitResolverErrorV1::BareRepository)
    ));

    let malformed = temporary.path().join("malformed");
    fs::create_dir_all(&malformed).expect("malformed root");
    fs::write(malformed.join(".git"), b"gitdir: \n").expect("malformed marker");
    assert!(matches!(
        resolver.resolve(selector(&malformed, None)),
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::MalformedGitFile
        })
    ));

    let malformed_backlink = temporary.path().join("malformed-backlink");
    let malformed_common = temporary.path().join("malformed-common");
    create_linked(&malformed_backlink, &malformed_common, "malformed-name");
    fs::write(
        malformed_common.join("worktrees/malformed-name/gitdir"),
        b"gitdir: /not-a-plain-backlink\n",
    )
    .expect("malformed backlink");
    assert!(matches!(
        resolver.resolve(selector(&malformed_backlink, None)),
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::MalformedLinkedBacklink
        })
    ));

    let symlinked = temporary.path().join("symlinked");
    fs::create_dir_all(&symlinked).expect("symlinked root");
    let administration = temporary.path().join("administration");
    create_common(&administration);
    symlink(&administration, symlinked.join(".git")).expect("git symlink");
    assert!(matches!(
        resolver.resolve(selector(&symlinked, None)),
        Err(GitResolverErrorV1::SymlinkEscape { .. })
    ));
}

#[test]
fn selector_symlinks_are_rejected_before_repository_discovery() {
    let temporary = temp();
    let target = temporary.path().join("target");
    create_main(&target);
    let target = fs::canonicalize(&target).expect("canonical selector target");
    let selector_link = target
        .parent()
        .expect("temporary parent")
        .join("selector-link");
    symlink(&target, &selector_link).expect("selector symlink");

    let result = NativeGitResolverV1::new().resolve(
        WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, lossless(&selector_link))
            .expect("selector symlink is syntactically canonical"),
    );
    assert!(matches!(
        result,
        Err(GitResolverErrorV1::SymlinkEscape { .. })
    ));
}
#[test]
fn fifo_git_marker_is_rejected_without_blocking() {
    let temporary = temp();
    let worktree = temporary.path().join("fifo-worktree");
    fs::create_dir_all(&worktree).expect("FIFO worktree directory");
    create_fifo(&worktree.join(".git"));

    assert!(matches!(
        NativeGitResolverV1::new().resolve(selector(&worktree, None)),
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::MalformedGitDirectory
        })
    ));
}

#[test]
fn podway_and_runtime_symlinks_are_rejected_even_when_they_point_in_tree() {
    let temporary = temp();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let in_tree = worktree.join("in-tree");
    fs::create_dir_all(&in_tree).expect("in-tree directory");

    symlink("in-tree", worktree.join(".podway")).expect("in-tree podway alias");
    assert!(matches!(
        NativeGitResolverV1::new().resolve(selector(&worktree, None)),
        Err(GitResolverErrorV1::SymlinkEscape { .. })
    ));

    fs::remove_file(worktree.join(".podway")).expect("remove podway symlink");
    fs::create_dir_all(worktree.join(".podway")).expect("podway directory");
    symlink("../in-tree", worktree.join(".podway/runtime")).expect("runtime alias");
    assert!(matches!(
        NativeGitResolverV1::new().resolve(selector(&worktree, None)),
        Err(GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::RuntimeDirectoryIsSymlink
        })
    ));
}

#[test]
fn durable_identity_supports_move_but_rejects_copy_delete_and_stale_root_replacement() {
    let temporary = temp();
    let original = temporary.path().join("original");
    create_main(&original);
    let original = fs::canonicalize(&original).expect("canonical original worktree");
    let resolver = NativeGitResolverV1::new();
    let first = resolver
        .resolve(selector(&original, None))
        .expect("initial validation");
    let durable = store_bound(first.identity());
    let stationary = resolver
        .resolve(selector(&original, Some(durable.clone())))
        .expect("matching durable identity");
    assert!(!stationary.move_metadata().relocated_from_prior_root());
    assert_eq!(
        stationary.repair_metadata().workspace_uuid_verification(),
        &WorkspaceUuidVerificationV1::RegistryCheckRequired
    );
    let deleted_descendant = original.join("deleted-descendant");
    fs::create_dir_all(&deleted_descendant).expect("durable selector descendant");
    fs::remove_dir(&deleted_descendant).expect("remove durable selector descendant");
    assert!(matches!(
        resolver.resolve(selector(&deleted_descendant, Some(durable.clone()))),
        Err(GitResolverErrorV1::Io {
            operation: GitReadOperationV1::CanonicalizePath
        })
    ));

    let moved = temporary.path().join("moved");
    fs::rename(&original, &moved).expect("same-volume rename");
    let repaired = resolver
        .resolve(selector(&moved, Some(durable.clone())))
        .expect("rename preserves descriptor identities");
    assert!(repaired.move_metadata().relocated_from_prior_root());
    let RegistryRepairActionV1::UpdateValidatedRoot { previous_root } =
        repaired.repair_metadata().registry_action()
    else {
        panic!("moved worktree must request registry repair");
    };
    assert_eq!(
        previous_root
            .decode_path_bytes()
            .expect("previous root bytes"),
        original.as_os_str().as_bytes()
    );

    let copied = temporary.path().join("copied");
    create_main(&copied);
    assert!(matches!(
        resolver.resolve(selector(&copied, Some(durable.clone()))),
        Err(GitResolverErrorV1::IdentityMismatch { .. })
    ));

    let linked = temporary.path().join("linked");
    let common = temporary.path().join("linked-common");
    create_linked(&linked, &common, "root-evidence");
    let linked = fs::canonicalize(&linked).expect("canonical linked worktree");
    let common = fs::canonicalize(&common).expect("canonical linked common directory");
    let linked_first = resolver
        .resolve(selector(&linked, None))
        .expect("linked validation");
    let linked_durable = store_bound(linked_first.identity());
    let retired = temporary.path().join("retired");
    fs::rename(&linked, &retired).expect("replace linked root");
    fs::create_dir_all(&linked).expect("replacement root");
    let replacement_marker = linked.join(".git");
    fs::write(
        &replacement_marker,
        [
            b"gitdir: ".as_slice(),
            common
                .join("worktrees/root-evidence")
                .as_os_str()
                .as_bytes(),
            b"\n",
        ]
        .concat(),
    )
    .expect("replacement marker");
    assert!(matches!(
        resolver.resolve(selector(&linked, Some(linked_durable))),
        Err(GitResolverErrorV1::IdentityMismatch { .. })
    ));
    let linked_move = temporary.path().join("linked-move");
    let linked_move_common = temporary.path().join("linked-move-common");
    create_linked(&linked_move, &linked_move_common, "moved-linked");
    let linked_move = fs::canonicalize(&linked_move).expect("canonical linked move worktree");
    let linked_move_common =
        fs::canonicalize(&linked_move_common).expect("canonical linked move common directory");
    let linked_move_durable = store_bound(
        resolver
            .resolve(selector(&linked_move, None))
            .expect("initial linked move validation")
            .identity(),
    );
    let linked_moved = temporary.path().join("linked-moved");
    fs::rename(&linked_move, &linked_moved).expect("move linked worktree");
    let linked_moved = fs::canonicalize(&linked_moved).expect("canonical moved linked worktree");
    fs::write(
        linked_move_common.join("worktrees/moved-linked/gitdir"),
        [linked_moved.join(".git").as_os_str().as_bytes(), b"\n"].concat(),
    )
    .expect("update moved linked backlink");
    let linked_repaired = resolver
        .resolve(selector(&linked_moved, Some(linked_move_durable)))
        .expect("linked move with updated reciprocal backlink");
    assert!(linked_repaired.move_metadata().relocated_from_prior_root());
    assert!(matches!(
        linked_repaired.repair_metadata().registry_action(),
        RegistryRepairActionV1::UpdateValidatedRoot { .. }
    ));

    fs::remove_dir_all(&moved).expect("remove worktree");
    assert!(matches!(
        resolver.resolve(selector(&moved, Some(durable))),
        Err(GitResolverErrorV1::WorktreeDeleted)
    ));
}

#[test]
fn bound_live_workspace_uuid_requires_registry_conflict_verification_on_every_resolution() {
    let temporary = temp();
    let worktree = temporary.path().join("live-worktree");
    create_main(&worktree);
    let worktree = fs::canonicalize(&worktree).expect("canonical live worktree");
    let resolver = NativeGitResolverV1::new();
    let durable = store_bound(
        resolver
            .resolve(selector(&worktree, None))
            .expect("initial live workspace discovery")
            .identity(),
    );

    let first = resolver
        .resolve(selector(&worktree, Some(durable.clone())))
        .expect("first bound resolution");
    let second = resolver
        .resolve(selector(&worktree, Some(durable)))
        .expect("second independently live bound resolution");

    assert_eq!(
        first.identity().workspace_id(),
        second.identity().workspace_id(),
        "the resolver preserves the durable workspace UUID across live resolutions"
    );
    assert_eq!(
        first.repair_metadata().workspace_uuid_verification(),
        &WorkspaceUuidVerificationV1::RegistryCheckRequired
    );
    assert_eq!(
        second.repair_metadata().workspace_uuid_verification(),
        &WorkspaceUuidVerificationV1::RegistryCheckRequired,
        "Git identity validation must delegate duplicate-UUID rejection to the registry boundary"
    );
}
#[test]
fn durable_directory_fingerprints_ignore_permission_bit_changes() {
    let temporary = temp();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let resolver = NativeGitResolverV1::new();
    let durable = store_bound(
        resolver
            .resolve(selector(&worktree, None))
            .expect("initial validation")
            .identity(),
    );
    let administration = worktree.join(".git");
    let original_mode = fs::metadata(&administration)
        .expect("administration metadata")
        .permissions()
        .mode();
    let toggled_mode = original_mode ^ 0o020;
    fs::set_permissions(&administration, fs::Permissions::from_mode(toggled_mode))
        .expect("toggle non-owner administration permission bit");
    assert_eq!(
        fs::metadata(&administration)
            .expect("changed administration metadata")
            .permissions()
            .mode()
            & 0o7777,
        toggled_mode & 0o7777
    );
    assert_ne!(original_mode & 0o7777, toggled_mode & 0o7777);
    let resolved = resolver
        .resolve(selector(&worktree, Some(durable)))
        .expect("chmod does not alter durable fingerprint");
    assert_eq!(resolved.kind(), &WorktreeKindV1::Main);
}

#[test]
fn artifacts_reject_final_and_intermediate_symlinks_and_preserve_non_utf8_bytes() {
    let temporary = temp();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let worktree = fs::canonicalize(&worktree).expect("canonical artifact worktree");
    let resolver = NativeGitResolverV1::new();
    let resolved = resolver
        .resolve(selector(&worktree, None))
        .expect("validated worktree");
    let artifact = worktree.join("artifact");
    fs::write(&artifact, b"hello").expect("artifact bytes");
    let hashed = resolver
        .hash_local_artifact(&resolved, &lossless(&artifact))
        .expect("stable regular artifact");
    assert_eq!(
        hashed.digest().as_str(),
        "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(hashed.byte_length(), 5);

    let non_utf8_artifact = worktree.join(OsString::from_vec(b"artifact-\xff".to_vec()));
    match fs::write(&non_utf8_artifact, b"native bytes") {
        Ok(()) => {
            let hashed = resolver
                .hash_local_artifact(&resolved, &lossless(&non_utf8_artifact))
                .expect("non-UTF-8 artifact");
            assert_eq!(
                hashed.digest().as_str(),
                "sha256:b0ca94ca54cf33f214f3bf9f31dddf9438ae1a42a93cb493454f741a1e6f024e"
            );
            assert_eq!(hashed.byte_length(), 12);
        }
        Err(error) if non_utf8_path_is_unsupported(&error) => {}
        Err(error) => panic!("non-UTF-8 artifact: {error}"),
    }

    let final_link = worktree.join("final-link");
    symlink("artifact", &final_link).expect("final artifact symlink");
    assert!(matches!(
        resolver.hash_local_artifact(&resolved, &lossless(&final_link)),
        Err(GitResolverErrorV1::SymlinkEscape { .. })
    ));

    let real_directory = worktree.join("real-directory");
    fs::create_dir_all(&real_directory).expect("real artifact directory");
    fs::write(real_directory.join("nested"), b"nested").expect("nested artifact");
    let intermediate_link = worktree.join("intermediate-link");
    symlink("real-directory", &intermediate_link).expect("intermediate artifact symlink");
    assert!(matches!(
        resolver.hash_local_artifact(&resolved, &lossless(&intermediate_link.join("nested"))),
        Err(GitResolverErrorV1::SymlinkEscape { .. })
    ));

    let socket = worktree.join("socket");
    let _listener = UnixListener::bind(&socket).expect("Unix socket fixture");
    assert!(matches!(
        resolver.hash_local_artifact(&resolved, &lossless(&socket)),
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::NonRegularArtifact
        })
    ));
    let fifo = worktree.join("artifact-fifo");
    create_fifo(&fifo);
    assert!(matches!(
        resolver.hash_local_artifact(&resolved, &lossless(&fifo)),
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::NonRegularArtifact
        })
    ));
}

#[test]
fn artifact_revalidation_rejects_stale_root_and_replaced_metadata_records() {
    let temporary = temp();
    let common = temporary.path().join("common");
    let worktree = temporary.path().join("worktree");
    create_linked(&worktree, &common, "records");
    let common = fs::canonicalize(&common).expect("canonical linked common directory");
    let worktree = fs::canonicalize(&worktree).expect("canonical linked worktree");
    let resolver = NativeGitResolverV1::new();
    let resolved = resolver
        .resolve(selector(&worktree, None))
        .expect("linked validation");
    let artifact = worktree.join("artifact");
    fs::write(&artifact, b"original").expect("artifact bytes");

    let outside = fs::canonicalize(temporary.path())
        .expect("canonical temporary root")
        .join("outside");
    fs::create_dir(&outside).expect("outside common-directory fixture");
    fs::write(
        common.join("worktrees/records/commondir"),
        outside.as_os_str().as_bytes(),
    )
    .expect("replace commondir record");
    let malformed_common = resolver.hash_local_artifact(&resolved, &lossless(&artifact));
    assert!(
        matches!(
            malformed_common,
            Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::UnsupportedAdministrationRelationship
            })
        ),
        "non-race resolver errors must remain precise"
    );

    create_linked(&worktree, &common, "records");
    let resolved = resolver
        .resolve(selector(&worktree, None))
        .expect("restored linked validation");
    let administration = common.join("worktrees/records");
    let retired_administration = common.join("worktrees/records-retired");
    fs::rename(&administration, &retired_administration).expect("replace administration directory");
    fs::create_dir_all(&administration).expect("replacement administration directory");
    write_head(&administration);
    fs::write(administration.join("commondir"), b"../..\n").expect("replacement commondir");
    fs::write(
        administration.join("gitdir"),
        [worktree.join(".git").as_os_str().as_bytes(), b"\n"].concat(),
    )
    .expect("replacement backlink");
    assert_artifact_snapshot_changed(resolver.hash_local_artifact(&resolved, &lossless(&artifact)));

    let resolved = resolver
        .resolve(selector(&worktree, None))
        .expect("replacement administration validation");
    let retired = temporary.path().join("retired");
    fs::rename(&worktree, &retired).expect("retire original root");
    fs::create_dir_all(&worktree).expect("replacement root");
    fs::write(
        worktree.join(".git"),
        [
            b"gitdir: ".as_slice(),
            common.join("worktrees/records").as_os_str().as_bytes(),
            b"\n",
        ]
        .concat(),
    )
    .expect("replacement marker");
    let replacement_artifact = worktree.join("artifact");
    fs::write(&replacement_artifact, b"replacement").expect("replacement artifact");
    assert_artifact_snapshot_changed(
        resolver.hash_local_artifact(&resolved, &lossless(&replacement_artifact)),
    );
}

#[test]
fn permission_failures_are_propagated_when_the_filesystem_enforces_them() {
    let temporary = temp();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let worktree = fs::canonicalize(&worktree).expect("canonical worktree");
    let original_mode = fs::metadata(&worktree)
        .expect("worktree metadata")
        .permissions()
        .mode();
    fs::set_permissions(&worktree, fs::Permissions::from_mode(0o000))
        .expect("remove worktree traversal permission");
    let permission_is_enforced =
        match fs::read_dir(&worktree).and_then(|mut entries| entries.next().transpose()) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => true,
            Ok(_) => false,
            Err(error) => panic!("independent permission probe: {error}"),
        };
    let result = NativeGitResolverV1::new().resolve(selector(&worktree, None));
    fs::set_permissions(&worktree, fs::Permissions::from_mode(original_mode))
        .expect("restore worktree traversal permission");

    // Privileged users can traverse mode-000 directories only when the independent
    // probe proves this filesystem/user combination does not enforce the boundary.
    if !permission_is_enforced {
        return;
    }
    assert!(matches!(
        result,
        Err(GitResolverErrorV1::PermissionDenied { .. })
    ));
}

#[test]
fn non_utf8_selectors_round_trip_when_the_platform_accepts_native_names() {
    let temporary = temp();
    let name = OsString::from_vec(b"worktree-\xff".to_vec());
    let worktree = temporary.path().join(name);
    let encoded = lossless(&worktree);
    assert_eq!(
        encoded.decode_path_bytes().expect("selector path bytes"),
        worktree.as_os_str().as_bytes()
    );

    match fs::create_dir_all(worktree.join(".git/objects")) {
        Ok(()) => {}
        Err(error) if non_utf8_path_is_unsupported(&error) => return,
        Err(error) => panic!("main administration directory: {error}"),
    }
    fs::create_dir_all(worktree.join(".git/refs")).expect("non-UTF-8 refs directory");
    write_head(&worktree.join(".git"));
    let worktree = fs::canonicalize(&worktree).expect("canonical non-UTF-8 worktree");

    let resolved = NativeGitResolverV1::new()
        .resolve(selector(&worktree, None))
        .expect("non-UTF-8 worktree");
    assert_eq!(
        resolved
            .roots()
            .worktree_root()
            .decode_path_bytes()
            .expect("root bytes"),
        worktree.as_os_str().as_bytes()
    );
}
#[test]
fn validated_worktree_rejects_legacy_rootless_identity_and_quarantines_unique_metadata() {
    let temporary = temp();
    let root_path = temporary.path().join("root");
    fs::create_dir(&root_path).expect("fixture root");
    let root = lossless(&root_path);
    let workspace_id =
        WorkspaceId::new("00000000-0000-4000-8000-000000000099").expect("fixture workspace ID");

    let legacy = DurableWorktreeIdentityV1::new(
        workspace_id.clone(),
        digest('a'),
        digest('b'),
        root.clone(),
    );
    assert!(matches!(
        manually_validated(
            legacy,
            root.clone(),
            WorktreeRepairMetadataV1::not_required()
        ),
        Err(ValidatedWorktreeValidationErrorV1::MissingRootDirectoryFingerprint)
    ));

    let bound = DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
        workspace_id,
        digest('a'),
        digest('b'),
        digest('c'),
        root.clone(),
    );
    let validated = manually_validated(
        bound,
        root,
        WorktreeRepairMetadataV1::not_required_with_uuid_verification(
            WorkspaceUuidVerificationV1::Unique,
        ),
    )
    .expect("legacy unique verification is quarantined");
    assert_eq!(
        validated.identity().state(),
        &WorkspaceIdentityStateV1::Bound
    );
    assert_eq!(
        validated.repair_metadata().workspace_uuid_verification(),
        &WorkspaceUuidVerificationV1::RegistryCheckRequired
    );
}

#[test]
fn validated_worktree_requires_exact_podway_and_runtime_roots() {
    let temporary = temp();
    let root_path = temporary.path().join("root");
    fs::create_dir(&root_path).expect("fixture root");
    let root = lossless(&root_path);
    let identity = DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
        WorkspaceId::new("00000000-0000-4000-8000-000000000100").expect("fixture workspace ID"),
        digest('a'),
        digest('b'),
        digest('c'),
        root.clone(),
    );
    let roots = WorktreeRootsV1::new(root.clone(), root.clone(), root.clone());
    let repair_metadata = WorktreeRepairMetadataV1::not_required_with_uuid_verification(
        WorkspaceUuidVerificationV1::RegistryCheckRequired,
    );
    let validate = |podway_directory: LosslessPathV1, runtime_directory: LosslessPathV1| {
        ValidatedWorktreeV1::new(
            identity.clone(),
            roots.clone(),
            WorktreeKindV1::Main,
            ContainmentMetadataV1::new(root.clone(), podway_directory, runtime_directory),
            WorktreeMoveMetadataV1::stationary(root.clone()),
            repair_metadata.clone(),
        )
    };

    assert!(matches!(
        validate(
            lossless(&root_path.join(".podway/nested")),
            lossless(&root_path.join(".podway/nested/runtime")),
        ),
        Err(ValidatedWorktreeValidationErrorV1::PodwayDirectoryOutsideWorkspace)
    ));
    assert!(matches!(
        validate(
            lossless(&root_path.join(".podway")),
            lossless(&root_path.join(".podway/nested/runtime")),
        ),
        Err(ValidatedWorktreeValidationErrorV1::RuntimeDirectoryOutsidePodway)
    ));
}

#[test]
fn competing_fresh_resolutions_expose_distinct_unbound_candidates() {
    let temporary = temp();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let barrier = Arc::new(Barrier::new(3));

    let first_path = worktree.clone();
    let first_barrier = Arc::clone(&barrier);
    let first = thread::spawn(move || {
        first_barrier.wait();
        NativeGitResolverV1::new()
            .resolve(selector(&first_path, None))
            .expect("first fresh resolution")
    });
    let second_path = worktree.clone();
    let second_barrier = Arc::clone(&barrier);
    let second = thread::spawn(move || {
        second_barrier.wait();
        NativeGitResolverV1::new()
            .resolve(selector(&second_path, None))
            .expect("second fresh resolution")
    });
    barrier.wait();

    let first = first.join().expect("first resolver thread");
    let second = second.join().expect("second resolver thread");
    assert_eq!(
        first.identity().state(),
        &WorkspaceIdentityStateV1::Candidate
    );
    assert_eq!(
        second.identity().state(),
        &WorkspaceIdentityStateV1::Candidate
    );
    assert!(!first.identity().is_store_bound());
    assert!(!second.identity().is_store_bound());
    assert_ne!(
        first.identity().workspace_id(),
        second.identity().workspace_id()
    );
    assert_eq!(
        first.repair_metadata().workspace_uuid_verification(),
        &WorkspaceUuidVerificationV1::PendingStoreInitialization
    );
    assert_eq!(
        second.repair_metadata().workspace_uuid_verification(),
        &WorkspaceUuidVerificationV1::PendingStoreInitialization
    );
}
