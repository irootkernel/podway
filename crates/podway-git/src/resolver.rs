//! Resolver orchestration and descriptor-anchored local-artifact hashing.

use podway_core::{Sha256Digest, WorkspaceId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::native;
use crate::{
    ContainmentMetadataV1, DurableWorktreeIdentityV1, GitInvariantViolationV1, GitReadOperationV1,
    GitResolverContractV1, GitResolverErrorV1, HashedLocalArtifactV1, LosslessPathV1,
    ValidatedWorktreeV1, WorkspaceIdentityStateV1, WorkspaceUuidVerificationV1,
    WorktreeMoveMetadataV1, WorktreeRepairMetadataV1, WorktreeRootsV1, WorktreeSelectorV1,
};

/// A zero-configuration, read-only resolver for supported native Unix Git worktrees.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeGitResolverV1;

impl NativeGitResolverV1 {
    /// Creates a resolver that performs no process execution or filesystem mutation.
    pub const fn new() -> Self {
        Self
    }

    /// Hashes a stable regular artifact strictly beneath a freshly revalidated worktree root.
    ///
    /// Every directory component and the final artifact are opened relative to an
    /// already-opened parent descriptor with no-follow semantics. Symlinks are
    /// therefore rejected uniformly, including in-tree aliases.
    pub fn hash_local_artifact(
        &self,
        worktree: &ValidatedWorktreeV1,
        artifact: &LosslessPathV1,
    ) -> Result<HashedLocalArtifactV1, GitResolverErrorV1> {
        let layout = self.reestablish_artifact_layout(worktree)?;
        let supplied = native::decode_lossless_path(artifact)?;
        if !native::is_strictly_contained(layout.worktree_root.path(), &supplied) {
            return Err(GitResolverErrorV1::PathEscape {
                path: artifact.clone(),
            });
        }

        let mut opened = native::open_artifact_beneath(&layout.worktree_root, &supplied)
            .map_err(map_snapshot_change_to_artifact)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut byte_length = 0_u64;
        loop {
            let count = opened.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            byte_length = byte_length
                .checked_add(count as u64)
                .ok_or_else(artifact_metadata_changed)?;
        }

        #[cfg(test)]
        native::synchronize_artifact_hash_replacement_for_test();
        if byte_length != opened.expected_size() {
            return Err(artifact_metadata_changed());
        }
        layout.validate_artifact_snapshot()?;
        self.ensure_artifact_layout_matches(worktree, &layout)?;
        opened.validate()?;

        Ok(HashedLocalArtifactV1::new(
            native::lossless_path(opened.path())?,
            sha256_digest(hasher)?,
            byte_length,
        ))
    }

    fn resolve_native(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<ValidatedWorktreeV1, GitResolverErrorV1> {
        let selected = native::decode_lossless_path(selector.path())?;
        match selector.durable_identity() {
            Some(durable_identity)
                if native::path_is_missing(&selected, GitReadOperationV1::CanonicalizePath)? =>
            {
                let durable_root =
                    native::decode_lossless_path(durable_identity.last_validated_root())?;
                if native::path_is_missing(&durable_root, GitReadOperationV1::CanonicalizePath)? {
                    return Err(GitResolverErrorV1::WorktreeDeleted);
                }
            }
            _ => {}
        }
        let layout = native::discover_worktree(selected)?;
        let root_fingerprint =
            native::fingerprint_directory(&layout.worktree_root, b"worktree-root", &layout.kind)?;
        let common_fingerprint =
            native::fingerprint_directory(&layout.common_directory_root, b"common", &layout.kind)?;
        let administration_fingerprint = native::fingerprint_directory(
            &layout.worktree_administration_root,
            b"worktree-administration",
            &layout.kind,
        )?;
        let containment_snapshot = native::inspect_containment(&layout.worktree_root)?;

        // The result must describe one coherent snapshot, not a mixture of records
        // observed before and after a directory replacement.
        layout.validate_resolution()?;
        containment_snapshot.validate(
            &layout.worktree_root,
            GitInvariantViolationV1::MetadataChangedDuringResolution,
        )?;
        layout.validate_resolution()?;

        let worktree_root = native::lossless_path(layout.worktree_root.path())?;
        let common_root = native::lossless_path(layout.common_directory_root.path())?;
        let administration_root =
            native::lossless_path(layout.worktree_administration_root.path())?;
        let containment = ContainmentMetadataV1::new(
            worktree_root.clone(),
            native::lossless_path(&containment_snapshot.podway_path(layout.worktree_root.path()))?,
            native::lossless_path(&containment_snapshot.runtime_path(layout.worktree_root.path()))?,
        );
        let roots = WorktreeRootsV1::new(worktree_root.clone(), common_root, administration_root);

        let (identity, move_metadata, repair_metadata) = match selector.durable_identity() {
            Some(expected) => {
                let actual = identity_with_state(
                    expected.state(),
                    expected.workspace_id().clone(),
                    common_fingerprint.clone(),
                    administration_fingerprint.clone(),
                    root_fingerprint.clone(),
                    worktree_root.clone(),
                );
                if expected.common_directory_fingerprint() != &common_fingerprint
                    || expected.worktree_administration_fingerprint() != &administration_fingerprint
                    || expected.root_directory_fingerprint() != Some(&root_fingerprint)
                {
                    return Err(GitResolverErrorV1::IdentityMismatch {
                        expected: Box::new(expected.clone()),
                        actual: Box::new(actual),
                    });
                }

                let previous_root = native::decode_lossless_path(expected.last_validated_root())?;
                let moved = previous_root != layout.worktree_root.path();
                let move_metadata = if moved {
                    WorktreeMoveMetadataV1::relocated(
                        expected.last_validated_root().clone(),
                        worktree_root.clone(),
                    )
                    .map_err(|_| GitResolverErrorV1::Invariant {
                        problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
                    })?
                } else {
                    WorktreeMoveMetadataV1::stationary(worktree_root.clone())
                };
                let verification = match expected.state() {
                    WorkspaceIdentityStateV1::Candidate => {
                        WorkspaceUuidVerificationV1::PendingStoreInitialization
                    }
                    WorkspaceIdentityStateV1::Bound => {
                        WorkspaceUuidVerificationV1::RegistryCheckRequired
                    }
                    WorkspaceIdentityStateV1::UnvalidatedLegacy => {
                        return Err(GitResolverErrorV1::IdentityMismatch {
                            expected: Box::new(expected.clone()),
                            actual: Box::new(actual),
                        });
                    }
                };
                let repair_metadata = if moved {
                    WorktreeRepairMetadataV1::update_validated_root_with_uuid_verification(
                        expected.last_validated_root().clone(),
                        verification,
                    )
                } else {
                    WorktreeRepairMetadataV1::not_required_with_uuid_verification(verification)
                };
                (actual, move_metadata, repair_metadata)
            }
            None => {
                let identity = candidate_identity(
                    fresh_workspace_id()?,
                    common_fingerprint,
                    administration_fingerprint,
                    root_fingerprint,
                    worktree_root.clone(),
                );
                (
                    identity,
                    WorktreeMoveMetadataV1::stationary(worktree_root.clone()),
                    WorktreeRepairMetadataV1::not_required_with_uuid_verification(
                        WorkspaceUuidVerificationV1::PendingStoreInitialization,
                    ),
                )
            }
        };

        ValidatedWorktreeV1::new(
            identity,
            roots,
            layout.kind,
            containment,
            move_metadata,
            repair_metadata,
        )
        .map_err(|_| GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
        })
    }

    fn reestablish_artifact_layout(
        &self,
        worktree: &ValidatedWorktreeV1,
    ) -> Result<native::DiscoveredLayout, GitResolverErrorV1> {
        let declared_root = native::decode_lossless_path(worktree.roots().worktree_root())?;
        let layout = native::discover_worktree(declared_root.clone())
            .map_err(map_snapshot_change_to_artifact)?;
        self.ensure_artifact_layout_matches(worktree, &layout)?;
        layout.validate_artifact_snapshot()?;
        Ok(layout)
    }

    fn ensure_artifact_layout_matches(
        &self,
        worktree: &ValidatedWorktreeV1,
        layout: &native::DiscoveredLayout,
    ) -> Result<(), GitResolverErrorV1> {
        let declared_root = native::decode_lossless_path(worktree.roots().worktree_root())?;
        let declared_common =
            native::decode_lossless_path(worktree.roots().common_directory_root())?;
        let declared_administration =
            native::decode_lossless_path(worktree.roots().worktree_administration_root())?;
        let durable_root = native::decode_lossless_path(worktree.identity().last_validated_root())?;
        if layout.worktree_root.path() != declared_root
            || layout.worktree_root.path() != durable_root
            || layout.common_directory_root.path() != declared_common
            || layout.worktree_administration_root.path() != declared_administration
            || &layout.kind != worktree.kind()
        {
            return Err(artifact_metadata_changed());
        }

        let root_fingerprint =
            artifact_fingerprint(&layout.worktree_root, b"worktree-root", &layout.kind)?;
        let common_fingerprint =
            artifact_fingerprint(&layout.common_directory_root, b"common", &layout.kind)?;
        let administration_fingerprint = artifact_fingerprint(
            &layout.worktree_administration_root,
            b"worktree-administration",
            &layout.kind,
        )?;
        if worktree.identity().root_directory_fingerprint() != Some(&root_fingerprint)
            || worktree.identity().common_directory_fingerprint() != &common_fingerprint
            || worktree.identity().worktree_administration_fingerprint()
                != &administration_fingerprint
        {
            return Err(artifact_metadata_changed());
        }
        Ok(())
    }
}

impl GitResolverContractV1 for NativeGitResolverV1 {
    fn resolve(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<ValidatedWorktreeV1, GitResolverErrorV1> {
        self.resolve_native(selector)
    }
}

fn identity_with_state(
    state: &WorkspaceIdentityStateV1,
    workspace_id: WorkspaceId,
    common_directory_fingerprint: Sha256Digest,
    worktree_administration_fingerprint: Sha256Digest,
    root_directory_fingerprint: Sha256Digest,
    last_validated_root: LosslessPathV1,
) -> DurableWorktreeIdentityV1 {
    match state {
        WorkspaceIdentityStateV1::Candidate => candidate_identity(
            workspace_id,
            common_directory_fingerprint,
            worktree_administration_fingerprint,
            root_directory_fingerprint,
            last_validated_root,
        ),
        WorkspaceIdentityStateV1::Bound | WorkspaceIdentityStateV1::UnvalidatedLegacy => {
            durable_identity(
                workspace_id,
                common_directory_fingerprint,
                worktree_administration_fingerprint,
                root_directory_fingerprint,
                last_validated_root,
            )
        }
    }
}

fn candidate_identity(
    workspace_id: WorkspaceId,
    common_directory_fingerprint: Sha256Digest,
    worktree_administration_fingerprint: Sha256Digest,
    root_directory_fingerprint: Sha256Digest,
    last_validated_root: LosslessPathV1,
) -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new_candidate_with_root_directory_fingerprint(
        workspace_id,
        common_directory_fingerprint,
        worktree_administration_fingerprint,
        root_directory_fingerprint,
        last_validated_root,
    )
}

fn durable_identity(
    workspace_id: WorkspaceId,
    common_directory_fingerprint: Sha256Digest,
    worktree_administration_fingerprint: Sha256Digest,
    root_directory_fingerprint: Sha256Digest,
    last_validated_root: LosslessPathV1,
) -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
        workspace_id,
        common_directory_fingerprint,
        worktree_administration_fingerprint,
        root_directory_fingerprint,
        last_validated_root,
    )
}

fn artifact_fingerprint(
    directory: &native::OpenedDirectory,
    role: &'static [u8],
    kind: &crate::WorktreeKindV1,
) -> Result<Sha256Digest, GitResolverErrorV1> {
    native::fingerprint_directory(directory, role, kind).map_err(map_snapshot_change_to_artifact)
}

fn artifact_metadata_changed() -> GitResolverErrorV1 {
    GitResolverErrorV1::Invariant {
        problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash,
    }
}
fn map_snapshot_change_to_artifact(error: GitResolverErrorV1) -> GitResolverErrorV1 {
    match error {
        GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
        } => artifact_metadata_changed(),
        error => error,
    }
}

fn fresh_workspace_id() -> Result<WorkspaceId, GitResolverErrorV1> {
    WorkspaceId::new(Uuid::new_v4().hyphenated().to_string()).map_err(|_| {
        GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
        }
    })
}

fn sha256_digest(hasher: Sha256) -> Result<Sha256Digest, GitResolverErrorV1> {
    let bytes = hasher.finalize();
    let mut rendered = String::with_capacity("sha256:".len() + bytes.len() * 2);
    rendered.push_str("sha256:");
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(rendered, "{byte:02x}");
    }
    Sha256Digest::new(rendered).map_err(|_| GitResolverErrorV1::Invariant {
        problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    static ARTIFACT_HASH_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn create_main(root: &Path) {
        let administration = root.join(".git");
        fs::create_dir_all(administration.join("objects")).expect("objects directory");
        fs::create_dir_all(administration.join("refs")).expect("refs directory");
        fs::write(administration.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
    }

    fn create_common(common: &Path) {
        fs::create_dir_all(common.join("objects")).expect("objects directory");
        fs::create_dir_all(common.join("refs")).expect("refs directory");
        fs::write(common.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
    }

    fn write_linked_administration(common: &Path, worktree: &Path, name: &str) {
        let administration = common.join("worktrees").join(name);
        fs::create_dir_all(&administration).expect("administration directory");
        fs::write(administration.join("HEAD"), b"ref: refs/heads/main\n")
            .expect("administration HEAD record");
        fs::write(administration.join("commondir"), b"../..\n").expect("common directory record");
        fs::write(
            administration.join("gitdir"),
            [worktree.join(".git").as_os_str().as_encoded_bytes(), b"\n"].concat(),
        )
        .expect("backlink record");
    }

    fn create_linked(
        worktree: &Path,
        common: &Path,
        name: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        create_common(common);
        fs::create_dir_all(worktree).expect("worktree directory");
        let worktree = fs::canonicalize(worktree).expect("canonical worktree");
        let common = fs::canonicalize(common).expect("canonical common directory");
        let administration = common.join("worktrees").join(name);
        fs::write(
            worktree.join(".git"),
            [
                b"gitdir: ".as_slice(),
                administration.as_os_str().as_encoded_bytes(),
                b"\n",
            ]
            .concat(),
        )
        .expect("linked marker");
        write_linked_administration(&common, &worktree, name);
        (worktree, common)
    }

    fn assert_linked_replacement_rejected(replace: impl FnOnce(&Path, &Path, &str)) {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let base = fs::canonicalize(temporary.path()).expect("canonical temporary directory");
        let name = "linked";
        let (worktree, common) = create_linked(&base.join("worktree"), &base.join("common"), name);
        let resolver = NativeGitResolverV1::new();
        let validated = resolver
            .resolve_native(selector(&worktree))
            .expect("validated linked worktree");
        let artifact_path = worktree.join("artifact");
        fs::write(&artifact_path, b"original artifact").expect("artifact bytes");
        let artifact = native::lossless_path(&artifact_path).expect("lossless artifact");
        let barrier = Arc::new(Barrier::new(2));
        native::install_artifact_hash_replacement_hook_for_test(Arc::clone(&barrier));

        let hash_worktree = validated.clone();
        let hash_artifact = artifact.clone();
        let worker =
            thread::spawn(move || resolver.hash_local_artifact(&hash_worktree, &hash_artifact));

        barrier.wait();
        replace(&worktree, &common, name);
        barrier.wait();

        let result = worker.join().expect("hash worker");
        assert!(matches!(
            result,
            Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash
            })
        ));
        assert_eq!(
            fs::read(&artifact_path).expect("artifact bytes"),
            b"original artifact"
        );
    }

    fn selector(path: &Path) -> WorktreeSelectorV1 {
        WorktreeSelectorV1::new(
            crate::WORKTREE_SELECTOR_VERSION_V1,
            None,
            native::lossless_path(path).expect("lossless selector"),
        )
        .expect("valid selector")
    }

    #[test]
    fn snapshot_changes_translate_to_artifact_metadata_changes() {
        let io = GitResolverErrorV1::Io {
            operation: GitReadOperationV1::ReadGitDirectory,
        };
        let representation = GitResolverErrorV1::Representation {
            problem: crate::GitRepresentationProblemV1::UnsupportedRepositoryLayout,
        };
        assert_eq!(map_snapshot_change_to_artifact(io.clone()), io);
        assert_eq!(
            map_snapshot_change_to_artifact(representation.clone()),
            representation
        );
        assert!(matches!(
            map_snapshot_change_to_artifact(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
            }),
            GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash
            }
        ));
    }

    #[test]
    fn linked_worktree_graph_replacements_are_rejected_after_hashing() {
        let _hook_guard = ARTIFACT_HASH_TEST_LOCK
            .lock()
            .expect("artifact hash test lock");

        assert_linked_replacement_rejected(|worktree, common, name| {
            let retired = common.with_extension("retired");
            fs::rename(common, &retired).expect("retire common directory");
            create_common(common);
            write_linked_administration(common, worktree, name);
        });
        assert_linked_replacement_rejected(|worktree, common, name| {
            let administration = common.join("worktrees").join(name);
            fs::rename(&administration, administration.with_extension("retired"))
                .expect("retire administration directory");
            write_linked_administration(common, worktree, name);
        });
        assert_linked_replacement_rejected(|worktree, _, _| {
            let marker = worktree.join(".git");
            let bytes = fs::read(&marker).expect("marker bytes");
            fs::rename(&marker, marker.with_extension("retired")).expect("retire marker");
            fs::write(&marker, bytes).expect("replacement marker");
        });
        assert_linked_replacement_rejected(|_, common, name| {
            let commondir = common.join("worktrees").join(name).join("commondir");
            let bytes = fs::read(&commondir).expect("common directory record");
            fs::rename(&commondir, commondir.with_extension("retired"))
                .expect("retire common directory record");
            fs::write(&commondir, bytes).expect("replacement common directory record");
        });
        assert_linked_replacement_rejected(|_, common, name| {
            let backlink = common.join("worktrees").join(name).join("gitdir");
            let bytes = fs::read(&backlink).expect("backlink record");
            fs::rename(&backlink, backlink.with_extension("retired"))
                .expect("retire backlink record");
            fs::write(&backlink, bytes).expect("replacement backlink record");
        });
    }
    #[test]
    fn synchronized_post_hash_root_replacement_is_rejected() {
        let _hook_guard = ARTIFACT_HASH_TEST_LOCK
            .lock()
            .expect("artifact hash test lock");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let worktree = temporary.path().join("worktree");
        create_main(&worktree);
        let worktree = fs::canonicalize(&worktree).expect("canonical worktree");
        let artifact = worktree.join("artifact");
        fs::write(&artifact, b"original artifact").expect("original artifact");

        let resolver = NativeGitResolverV1::new();
        let validated = resolver
            .resolve_native(selector(&worktree))
            .expect("validated worktree");
        let artifact = native::lossless_path(&artifact).expect("lossless artifact");
        let barrier = Arc::new(Barrier::new(2));
        native::install_artifact_hash_replacement_hook_for_test(Arc::clone(&barrier));

        let hash_worktree = validated.clone();
        let hash_artifact = artifact.clone();
        let worker =
            thread::spawn(move || resolver.hash_local_artifact(&hash_worktree, &hash_artifact));

        barrier.wait();
        let retired = temporary.path().join("retired");
        fs::rename(&worktree, &retired).expect("retire original root");
        create_main(&worktree);
        fs::write(worktree.join("artifact"), b"replacement artifact")
            .expect("replacement artifact");
        barrier.wait();

        let result = worker.join().expect("hash worker");
        assert!(matches!(
            result,
            Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash
            })
        ));
        assert_eq!(
            fs::read(worktree.join("artifact")).expect("replacement bytes"),
            b"replacement artifact"
        );
    }
}
