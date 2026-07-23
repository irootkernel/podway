//! Descriptor-anchored initialization of the worktree-local Podway layout.

use crate::{
    GitInvariantViolationV1, GitResolverContractV1, GitResolverErrorV1, NativeGitResolverV1,
    ValidatedWorktreeV1, WORKTREE_SELECTOR_VERSION_V1, WorkspaceLayoutErrorV1, WorktreeSelectorV1,
    native,
};

const MAX_INITIAL_REVALIDATION_ATTEMPTS: usize = 3;

/// Whether a required worktree-local layout entry was created or was already valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceLayoutElementStatusV1 {
    Created,
    AlreadyValid,
}

/// The result of one idempotent worktree-local layout initialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceLayoutReportV1 {
    podway_directory: WorkspaceLayoutElementStatusV1,
    procedures_directory: WorkspaceLayoutElementStatusV1,
    runtime_directory: WorkspaceLayoutElementStatusV1,
    gitignore_file: WorkspaceLayoutElementStatusV1,
    runtime_ignore_rule: WorkspaceLayoutElementStatusV1,
    config_file: Option<WorkspaceLayoutElementStatusV1>,
}

impl WorkspaceLayoutReportV1 {
    pub fn podway_directory(&self) -> WorkspaceLayoutElementStatusV1 {
        self.podway_directory
    }

    pub fn procedures_directory(&self) -> WorkspaceLayoutElementStatusV1 {
        self.procedures_directory
    }

    pub fn runtime_directory(&self) -> WorkspaceLayoutElementStatusV1 {
        self.runtime_directory
    }

    pub fn gitignore_file(&self) -> WorkspaceLayoutElementStatusV1 {
        self.gitignore_file
    }
    /// The configuration file status is present only for `initialize_with_config`.
    pub fn config_file(&self) -> Option<WorkspaceLayoutElementStatusV1> {
        self.config_file
    }

    pub fn runtime_ignore_rule(&self) -> WorkspaceLayoutElementStatusV1 {
        self.runtime_ignore_rule
    }
}

/// Creates only the worktree-local layout owned by Podway.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceLayoutInitializerV1;

impl WorkspaceLayoutInitializerV1 {
    /// Creates an initializer that never invokes Git or alters Git administrative metadata.
    pub const fn new() -> Self {
        Self
    }

    /// Revalidates `worktree`, then idempotently ensures its local Podway layout.
    pub fn initialize(
        &self,
        worktree: &ValidatedWorktreeV1,
    ) -> Result<WorkspaceLayoutReportV1, WorkspaceLayoutErrorV1> {
        self.initialize_inner(worktree, None)
    }

    /// Revalidates `worktree`, then creates `config.yaml` only when it is absent.
    pub fn initialize_with_config(
        &self,
        worktree: &ValidatedWorktreeV1,
        default_config_bytes: &[u8],
    ) -> Result<WorkspaceLayoutReportV1, WorkspaceLayoutErrorV1> {
        native::validate_workspace_config_bytes(default_config_bytes)
            .map_err(initialization_error)?;
        self.initialize_inner(worktree, Some(default_config_bytes))
    }

    fn initialize_inner(
        &self,
        worktree: &ValidatedWorktreeV1,
        default_config_bytes: Option<&[u8]>,
    ) -> Result<WorkspaceLayoutReportV1, WorkspaceLayoutErrorV1> {
        let revalidated = retry_initial_revalidation(|| self.revalidate(worktree))?;
        if !same_worktree_identity(worktree, &revalidated) {
            return Err(initialization_error(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
            }));
        }
        let root_fingerprint = revalidated
            .identity()
            .root_directory_fingerprint()
            .ok_or_else(|| {
                initialization_error(GitResolverErrorV1::Invariant {
                    problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
                })
            })?;
        let root = native::open_workspace_layout_root(revalidated.roots().worktree_root())
            .map_err(initialization_error)?;
        native::validate_workspace_layout_root(&root, root_fingerprint, revalidated.kind())
            .map_err(initialization_error)?;
        let snapshot = match default_config_bytes {
            Some(default_config_bytes) => {
                native::initialize_workspace_layout_with_config(root, default_config_bytes)
            }
            None => native::initialize_workspace_layout(root),
        }
        .map_err(initialization_error)?;
        snapshot.validate().map_err(initialization_error)?;

        let final_validation = self.revalidate(worktree)?;
        snapshot.validate().map_err(initialization_error)?;
        if !same_worktree_identity(worktree, &final_validation) {
            return Err(initialization_error(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
            }));
        }

        let report = snapshot.report();
        Ok(WorkspaceLayoutReportV1 {
            podway_directory: status(report.podway_created),
            procedures_directory: status(report.procedures_created),
            runtime_directory: status(report.runtime_created),
            gitignore_file: status(report.ignore_created),
            runtime_ignore_rule: status(report.runtime_ignore_rule_created),
            config_file: report.config_created.map(status),
        })
    }

    fn revalidate(
        &self,
        worktree: &ValidatedWorktreeV1,
    ) -> Result<ValidatedWorktreeV1, WorkspaceLayoutErrorV1> {
        let selector = WorktreeSelectorV1::new(
            WORKTREE_SELECTOR_VERSION_V1,
            Some(worktree.identity().clone()),
            worktree.roots().worktree_root().clone(),
        )
        .map_err(|error| WorkspaceLayoutErrorV1::Revalidation {
            source: GitResolverErrorV1::Selector(error),
        })?;
        NativeGitResolverV1::new()
            .resolve(selector)
            .map_err(|source| WorkspaceLayoutErrorV1::Revalidation { source })
    }
}

fn retry_initial_revalidation(
    mut revalidate: impl FnMut() -> Result<ValidatedWorktreeV1, WorkspaceLayoutErrorV1>,
) -> Result<ValidatedWorktreeV1, WorkspaceLayoutErrorV1> {
    for attempt in 1..=MAX_INITIAL_REVALIDATION_ATTEMPTS {
        match revalidate() {
            Err(WorkspaceLayoutErrorV1::Revalidation {
                source:
                    GitResolverErrorV1::Invariant {
                        problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
                    },
            }) if attempt < MAX_INITIAL_REVALIDATION_ATTEMPTS => continue,
            result => return result,
        }
    }
    unreachable!("the bounded initial revalidation loop always returns")
}

fn status(created: bool) -> WorkspaceLayoutElementStatusV1 {
    if created {
        WorkspaceLayoutElementStatusV1::Created
    } else {
        WorkspaceLayoutElementStatusV1::AlreadyValid
    }
}

fn same_worktree_identity(expected: &ValidatedWorktreeV1, actual: &ValidatedWorktreeV1) -> bool {
    expected.identity() == actual.identity()
        && expected.roots() == actual.roots()
        && expected.kind() == actual.kind()
        && actual.containment().workspace_root() == actual.roots().worktree_root()
}

fn initialization_error(source: GitResolverErrorV1) -> WorkspaceLayoutErrorV1 {
    WorkspaceLayoutErrorV1::Initialization { source }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    static CONTAINMENT_SNAPSHOT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn create_main(root: &Path) {
        let administration = root.join(".git");
        fs::create_dir_all(administration.join("objects")).expect("objects directory");
        fs::create_dir_all(administration.join("refs")).expect("refs directory");
        fs::write(administration.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
    }

    fn resolve(root: &Path) -> ValidatedWorktreeV1 {
        let root = fs::canonicalize(root).expect("canonical worktree");
        NativeGitResolverV1::new()
            .resolve(
                WorktreeSelectorV1::new(
                    WORKTREE_SELECTOR_VERSION_V1,
                    None,
                    crate::native::lossless_path(&root).expect("lossless worktree path"),
                )
                .expect("valid worktree selector"),
            )
            .expect("resolved worktree")
    }

    #[test]
    fn initial_revalidation_retries_concurrent_layout_publication() {
        let _hook_guard = CONTAINMENT_SNAPSHOT_TEST_LOCK
            .lock()
            .expect("containment snapshot test lock");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("worktree");
        create_main(&root);
        let root = fs::canonicalize(&root).expect("canonical worktree");
        let worktree = resolve(&root);
        let barrier = Arc::new(Barrier::new(2));
        crate::resolver::install_containment_snapshot_hook_for_test(
            root.clone(),
            Arc::clone(&barrier),
        );

        let racing_worktree = worktree.clone();
        let racing_initializer =
            thread::spawn(move || WorkspaceLayoutInitializerV1::new().initialize(&racing_worktree));

        barrier.wait();
        let winning_result = WorkspaceLayoutInitializerV1::new().initialize(&worktree);
        assert!(
            winning_result.is_ok(),
            "winning initializer failed: {winning_result:#?}"
        );
        barrier.wait();

        let racing_result = racing_initializer
            .join()
            .expect("racing initializer thread");
        assert!(
            racing_result.is_ok(),
            "racing initializer did not retry the monotonic layout publication: {racing_result:#?}"
        );
        assert!(root.join(".podway/procedures").is_dir());
        assert!(root.join(".podway/runtime").is_dir());
        assert_eq!(
            fs::read(root.join(".podway/.gitignore")).expect("runtime ignore file"),
            b"runtime/\n"
        );
    }

    #[test]
    fn initial_revalidation_exhaustion_returns_the_original_error() {
        let expected = WorkspaceLayoutErrorV1::Revalidation {
            source: GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
            },
        };
        let mut attempts = 0;
        let result = retry_initial_revalidation(|| {
            attempts += 1;
            Err(expected.clone())
        });

        assert_eq!(attempts, MAX_INITIAL_REVALIDATION_ATTEMPTS);
        assert_eq!(result, Err(expected));
    }

    #[test]
    fn root_replacement_before_config_layout_mutation_cannot_mutate_replacement_tree() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("worktree");
        create_main(&root);
        let root = fs::canonicalize(&root).expect("canonical worktree");
        let worktree = resolve(&root);
        let barrier = Arc::new(Barrier::new(2));
        crate::native::install_workspace_layout_root_replacement_hook_for_test(
            root.clone(),
            Arc::clone(&barrier),
        );

        let initializer_worktree = worktree.clone();
        let initializer = thread::spawn(move || {
            WorkspaceLayoutInitializerV1::new()
                .initialize_with_config(&initializer_worktree, b"opaque default bytes")
        });

        barrier.wait();
        let retired = temporary.path().join("retired-worktree");
        fs::rename(&root, &retired).expect("retire original worktree");
        create_main(&root);
        barrier.wait();

        let result = initializer.join().expect("initializer thread");
        assert!(matches!(
            result,
            Err(WorkspaceLayoutErrorV1::Initialization {
                source: GitResolverErrorV1::Invariant {
                    problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
                }
            })
        ));
        assert!(
            !root.join(".podway").exists(),
            "replacement worktree must not receive layout mutation"
        );
        assert!(
            retired.join(".podway/runtime").is_dir(),
            "descriptor-anchored mutation must remain in the retired worktree"
        );
        assert_eq!(
            fs::read(retired.join(".podway/config.yaml")).expect("retired config"),
            b"opaque default bytes"
        );
    }
    #[test]
    fn config_name_replacement_after_open_is_rejected() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("worktree");
        create_main(&root);
        let root = fs::canonicalize(&root).expect("canonical worktree");
        let worktree = resolve(&root);
        let config = root.join(".podway/config.yaml");
        let barrier = Arc::new(Barrier::new(2));
        crate::native::install_workspace_layout_config_replacement_hook_for_test(
            config.clone(),
            Arc::clone(&barrier),
        );

        let initializer_worktree = worktree.clone();
        let initializer = thread::spawn(move || {
            WorkspaceLayoutInitializerV1::new()
                .initialize_with_config(&initializer_worktree, b"opaque default bytes")
        });

        barrier.wait();
        let retired = root.join(".podway/config-retired.yaml");
        fs::rename(&config, &retired).expect("retire initialized config");
        fs::write(&config, b"replacement custom bytes").expect("replace config");
        barrier.wait();

        let result = initializer.join().expect("initializer thread");
        assert!(matches!(
            result,
            Err(WorkspaceLayoutErrorV1::Initialization {
                source: GitResolverErrorV1::Invariant {
                    problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
                }
            })
        ));
        assert_eq!(
            fs::read(&retired).expect("retired initialized config"),
            b"opaque default bytes"
        );
        assert_eq!(
            fs::read(&config).expect("replacement config"),
            b"replacement custom bytes"
        );
    }
}
