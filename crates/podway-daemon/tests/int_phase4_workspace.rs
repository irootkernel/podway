//! Phase 4 daemon-authoritative Git/Store workspace resolution contracts.

#[path = "support/phase4_workspace.rs"]
mod support_phase4_workspace;

use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

#[cfg(unix)]
use std::{os::unix::ffi::OsStrExt, process::Command};

use podway_core::{UnixMillis, WorkspaceId};
use podway_daemon::workspace;
use podway_daemon::workspace::{
    SqliteWorkspaceBindingInspectorV1, WorkspaceBindingInspectorV1, WorkspaceGitObservationV1,
    WorkspaceResolutionErrorV1, WorkspaceResolverV1,
};
use podway_git::{
    GitInvariantViolationV1, GitResolveErrorV1, GitResolverContractV1, NativeGitResolverV1,
    ValidatedWorktreeV1, WorkspaceIdentityStateV1, WorktreeKindV1, WorktreeSelectorV1,
};
use podway_store::{SqliteStoreOptionsV1, SqliteStoreV1};
use support_phase4_workspace::{
    GitWorktreeFixtureV1, TemporaryDirectoryV1, copy_tree, git_worktrees, prepare_runtime,
    read_file, selector,
};

fn options() -> SqliteStoreOptionsV1 {
    SqliteStoreOptionsV1::new(8).expect("fixture Store options must be valid")
}

fn resolver() -> WorkspaceResolverV1<NativeGitResolverV1, SqliteWorkspaceBindingInspectorV1> {
    WorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        SqliteWorkspaceBindingInspectorV1::new(options()),
    )
}
struct RevalidationRaceHookV1 {
    calls: AtomicUsize,
}

impl RevalidationRaceHookV1 {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl GitResolverContractV1 for RevalidationRaceHookV1 {
    fn resolve(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<ValidatedWorktreeV1, GitResolveErrorV1> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            return Err(GitResolveErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
            });
        }
        NativeGitResolverV1::new().resolve(selector)
    }
}
struct MissingBindingInspectorV1 {
    calls: AtomicUsize,
}

impl MissingBindingInspectorV1 {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl WorkspaceBindingInspectorV1 for MissingBindingInspectorV1 {
    fn inspect_workspace_binding(
        &self,
        _database_path: &Path,
    ) -> Result<
        Option<podway_store::WorkspaceBindingV1>,
        workspace::WorkspaceBindingInspectionErrorV1,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

fn bind_bootstrap_workspace(
    resolved: &workspace::ResolvedWorkspaceV1,
) -> podway_store::SqliteStoreV1 {
    SqliteStoreV1::open(
        resolved.database_path(),
        resolved.workspace_root(),
        resolved.store_identity().clone(),
        options(),
        UnixMillis::UNIX_EPOCH,
    )
    .expect("bootstrap candidate must be bindable atomically by SQLite Store")
}

fn bootstrap_main(fixture: &GitWorktreeFixtureV1) -> workspace::ResolvedWorkspaceV1 {
    resolver()
        .resolve_bootstrap(selector(fixture.main()))
        .expect("new main worktree must resolve for bootstrap")
}

#[test]
fn bootstrap_then_existing_uses_the_store_uuid_and_real_main_and_linked_worktrees() {
    let fixture = git_worktrees();
    let bootstrap = bootstrap_main(&fixture);
    assert_eq!(bootstrap.worktree().kind(), &WorktreeKindV1::Main);
    assert_eq!(
        bootstrap.worktree().identity().state(),
        &WorkspaceIdentityStateV1::Candidate
    );
    let bootstrap_key = bootstrap.scheduler_key().clone();
    let durable_uuid = bootstrap.store_identity().workspace_uuid().clone();
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);

    let existing = resolver()
        .resolve_existing(selector(fixture.main()), Some(&durable_uuid))
        .expect("existing binding must replace the fresh Git candidate UUID");
    assert_eq!(
        existing.worktree().identity().state(),
        &WorkspaceIdentityStateV1::Bound
    );
    assert_eq!(existing.store_identity().workspace_uuid(), &durable_uuid);
    assert_eq!(existing.scheduler_key(), &bootstrap_key);
    assert_eq!(
        existing.scheduler_key().workspace_uuid(),
        existing.store_identity().workspace_uuid()
    );

    let linked = resolver()
        .resolve_bootstrap(selector(fixture.linked()))
        .expect("real linked worktree with no database must resolve for bootstrap");
    assert_eq!(linked.worktree().kind(), &WorktreeKindV1::Linked);
    assert_eq!(
        linked.worktree().identity().state(),
        &WorkspaceIdentityStateV1::Candidate
    );
}
#[test]
fn injected_binding_inspector_is_observed_without_granting_store_mutation_authority() {
    let fixture = git_worktrees();
    let resolver =
        WorkspaceResolverV1::new(NativeGitResolverV1::new(), MissingBindingInspectorV1::new());

    let resolved = resolver
        .resolve_bootstrap(selector(fixture.main()))
        .expect("injected read-only missing-binding observation must support bootstrap");
    assert_eq!(resolver.binding_inspector().calls(), 1);
    assert_eq!(
        resolved.worktree().identity().state(),
        &WorkspaceIdentityStateV1::Candidate
    );
    assert!(!resolved.database_path().exists());
}

#[test]
fn existing_resolution_rejects_an_expected_uuid_mismatch_before_bound_git_revalidation() {
    let fixture = git_worktrees();
    let bootstrap = bootstrap_main(&fixture);
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);
    let wrong_uuid = WorkspaceId::new("11111111-1111-4111-8111-111111111111")
        .expect("fixture UUID must be valid");

    let error = resolver()
        .resolve_existing(selector(fixture.main()), Some(&wrong_uuid))
        .expect_err("a caller-supplied UUID must match the Store binding");
    assert!(matches!(
        error,
        WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {
            expected,
            actual: _
        } if expected == wrong_uuid
    ));
}

#[test]
fn copied_live_workspace_fingerprint_conflict_never_mutates_or_adopts_the_store_binding() {
    let fixture = git_worktrees();
    let bootstrap = bootstrap_main(&fixture);
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);

    let copied = fixture.temporary_path().join("copied-live-worktree");
    copy_tree(fixture.main(), &copied);
    let copied_database = copied.join(".podway/runtime/state.sqlite3");
    let before = read_file(&copied_database);

    let error = resolver()
        .resolve_existing(selector(&copied), None)
        .expect_err("a live filesystem copy must not auto-adopt the original Store UUID");
    match error {
        WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch {
            stored,
            observed_common_directory_fingerprint,
            observed_worktree_administration_fingerprint,
        } => {
            assert_eq!(
                stored,
                *bootstrap.store_identity(),
                "the rejection must classify the copied Store as the original durable identity"
            );
            assert_ne!(
                stored.common_dir_identity(),
                &observed_common_directory_fingerprint,
                "the copied common Git directory must not satisfy the original fingerprint"
            );
            assert_ne!(
                stored.worktree_admin_identity(),
                &observed_worktree_administration_fingerprint,
                "the copied worktree administration must not satisfy the original fingerprint"
            );
        }
        _ => panic!("a live filesystem copy must fail exact Git/Store fingerprint classification"),
    }
    assert_eq!(read_file(&copied_database), before);
}
#[test]
fn deterministic_revalidation_hook_rejects_a_git_race_before_store_mutation() {
    let fixture = git_worktrees();
    let bootstrap = bootstrap_main(&fixture);
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);
    let durable_uuid = bootstrap.store_identity().workspace_uuid().clone();
    let database = bootstrap.database_path().to_path_buf();
    let before = read_file(&database);
    let resolver = WorkspaceResolverV1::new(
        RevalidationRaceHookV1::new(),
        SqliteWorkspaceBindingInspectorV1::new(options()),
    );

    let error = resolver
        .resolve_existing(selector(fixture.main()), Some(&durable_uuid))
        .expect_err("the deterministic second Git observation must reject the race");
    assert!(matches!(
        error,
        WorkspaceResolutionErrorV1::Git {
            observation: WorkspaceGitObservationV1::BoundRevalidation,
            source: GitResolveErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringResolution
            }
        }
    ));
    assert_eq!(resolver.git_resolver().calls(), 2);
    assert_eq!(read_file(&database), before);
}

#[test]
fn root_and_nested_selectors_converge_to_the_same_path_free_scheduler_key() {
    let fixture = git_worktrees();
    let bootstrap = bootstrap_main(&fixture);
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);
    let durable_uuid = bootstrap.store_identity().workspace_uuid().clone();
    let nested = fixture.main().join("nested/child");
    fs::create_dir_all(&nested).expect("nested selector fixture directory must be created");

    let root = resolver()
        .resolve_existing(selector(fixture.main()), Some(&durable_uuid))
        .expect("root alias must resolve");
    let nested = resolver()
        .resolve_existing(selector(&nested), Some(&durable_uuid))
        .expect("nested alias must resolve to the same worktree");

    assert_eq!(root.scheduler_key(), nested.scheduler_key());
    assert_eq!(root.store_identity(), nested.store_identity());
    assert_eq!(root.database_path(), nested.database_path());
}

#[test]
fn moved_worktree_preserves_its_key_and_exposes_a_root_repair() {
    let fixture = git_worktrees();
    let bootstrap = bootstrap_main(&fixture);
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);
    let durable_uuid = bootstrap.store_identity().workspace_uuid().clone();
    let original_root = fixture.main().to_path_buf();
    let canonical_original_root =
        fs::canonicalize(&original_root).expect("original root must canonicalize");
    let original_key = bootstrap.scheduler_key().clone();
    let relocated = fixture.temporary_path().join("relocated-main");
    fs::rename(&original_root, &relocated).expect("main worktree must be movable without copying");
    let canonical_relocated_root =
        fs::canonicalize(&relocated).expect("relocated root must canonicalize");

    let moved = resolver()
        .resolve_existing(selector(&relocated), Some(&durable_uuid))
        .expect("same Git directories at a new root must retain the Store identity");
    assert_eq!(moved.scheduler_key(), &original_key);
    assert!(moved.move_metadata().relocated_from_prior_root());
    #[cfg(unix)]
    assert_eq!(
        moved
            .move_metadata()
            .previous_root()
            .expect("move repair retains the prior root")
            .decode_path_bytes()
            .expect("prior root bytes must decode"),
        canonical_original_root.as_os_str().as_bytes()
    );
    #[cfg(unix)]
    assert_eq!(
        moved
            .move_metadata()
            .current_root()
            .decode_path_bytes()
            .expect("current root bytes must decode"),
        canonical_relocated_root.as_os_str().as_bytes()
    );
}

#[test]
fn missing_database_and_deleted_root_are_typed_resolution_failures() {
    let fixture = git_worktrees();
    let no_database = resolver()
        .resolve_existing(selector(fixture.main()), None)
        .expect_err(
            "existing resolution must never adopt a fresh Git UUID when the database is absent",
        );
    assert!(matches!(
        no_database,
        WorkspaceResolutionErrorV1::ExistingBindingMissing
    ));

    let bootstrap = bootstrap_main(&fixture);
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);
    let selected_before_deletion = selector(fixture.main());
    fs::remove_dir_all(fixture.main()).expect("fixture root must be removable");
    let deleted = resolver()
        .resolve_existing(selected_before_deletion, None)
        .expect_err("a deleted selected root must not resolve from stale Store state");
    assert!(matches!(
        deleted,
        WorkspaceResolutionErrorV1::Git {
            observation: WorkspaceGitObservationV1::Preliminary,
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn non_utf8_worktree_path_bytes_remain_exact_through_bootstrap_resolution() {
    use std::os::unix::ffi::OsStrExt;

    let temporary = TemporaryDirectoryV1::new("podway-daemon-phase4-nonutf8");
    let root = support_phase4_workspace::non_utf8_child_path(temporary.path());
    match fs::create_dir(&root) {
        Ok(()) => {}
        Err(error) if cfg!(target_os = "macos") && error.raw_os_error() == Some(92) => return,
        Err(error) => panic!("non-UTF-8 fixture directory must be created: {error}"),
    }
    run_non_utf8_git_fixture(&root);
    prepare_runtime(&root);

    let resolved = resolver()
        .resolve_bootstrap(selector(&root))
        .expect("non-UTF-8 root bytes must resolve without lossy conversion");
    assert!(
        resolved.workspace_root().unix_bytes().contains(&0xff),
        "Store root evidence must retain the non-UTF-8 byte"
    );
    assert!(
        resolved
            .database_path()
            .as_os_str()
            .as_bytes()
            .contains(&0xff),
        "SQLite database path must retain the non-UTF-8 byte"
    );
}

#[test]
fn sqlite_binding_inspection_is_read_only_for_missing_and_existing_databases() {
    let missing_parent = TemporaryDirectoryV1::new("podway-binding-missing-parent");
    let missing_parent_database = missing_parent
        .path()
        .join("fresh/.podway/runtime/state.sqlite3");
    let inspector = SqliteWorkspaceBindingInspectorV1::new(options());
    assert_eq!(
        inspector
            .inspect_workspace_binding(&missing_parent_database)
            .expect("a missing runtime parent must be an uninitialized binding"),
        None
    );
    assert!(
        !missing_parent_database
            .parent()
            .expect("database path must have a parent")
            .exists(),
        "read-only inspection must not create the missing runtime hierarchy"
    );
    let fixture = git_worktrees();
    let inspector = SqliteWorkspaceBindingInspectorV1::new(options());
    let database = fixture.main().join(".podway/runtime/state.sqlite3");
    assert!(!database.exists());
    assert_eq!(
        inspector
            .inspect_workspace_binding(&database)
            .expect("missing database inspection must succeed"),
        None
    );
    assert!(!database.exists());

    let bootstrap = bootstrap_main(&fixture);
    let store = bind_bootstrap_workspace(&bootstrap);
    drop(store);
    let before = read_file(&database);
    assert!(
        inspector
            .inspect_workspace_binding(&database)
            .expect("existing database inspection must succeed")
            .is_some()
    );
    assert_eq!(read_file(&database), before);
}

#[cfg(unix)]
fn run_non_utf8_git_fixture(root: &Path) {
    run_git(&["init", "--quiet"], Some(root));
    run_git(
        &["config", "user.email", "podway-tests@example.invalid"],
        Some(root),
    );
    run_git(&["config", "user.name", "Podway Tests"], Some(root));
    run_git(
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "non-UTF-8 fixture commit",
        ],
        Some(root),
    );
}

#[cfg(unix)]
fn run_git(arguments: &[&str], directory: Option<&Path>) {
    let mut command = Command::new("git");
    if let Some(directory) = directory {
        command.arg("-C").arg(directory);
    }
    command.args(arguments);
    let status = command
        .status()
        .expect("Git must be available for non-UTF-8 fixture setup");
    assert!(status.success(), "Git fixture setup command must succeed");
}
