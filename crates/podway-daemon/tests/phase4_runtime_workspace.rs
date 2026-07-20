//! Phase 4 daemon-owned workspace runtime composition contracts.

mod support_phase4_workspace;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

#[cfg(unix)]
use std::{
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    process::Command,
};

use podway_core::UnixMillis;
use podway_daemon::{
    runtime_workspace::{
        WorkspaceRuntimeErrorV1, WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1,
        WorkspaceSchedulerRevalidationV1,
    },
    workspace::WorkspaceResolutionErrorV1,
};
use podway_protocol::Rfc3339MillisV1;
use podway_service::ServiceRuntimePathsV1;
use podway_store::{
    SqliteStoreOptionsV1, StoreContractV1, StoreErrorV1, StoreReadContractV1,
    StoreUnavailableReasonV1, WorkerIdV1,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use support_phase4_workspace::{
    copy_tree, git_worktrees, non_utf8_child_path, read_file, selector,
};

fn fixture_runtime_directory(root: &Path) -> PathBuf {
    let root = fs::canonicalize(root).expect("fixture root must canonicalize");
    #[cfg(unix)]
    let digest = Sha256::digest(root.as_os_str().as_bytes());
    #[cfg(not(unix))]
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let digest = format!("{digest:x}");
    std::env::temp_dir().join(format!("pdr-{}", &digest[..16]))
}
#[derive(Debug, Eq, PartialEq)]
struct SnapshotMetadataV1 {
    is_directory: bool,
    is_regular_file: bool,
    is_symlink: bool,
    byte_length: u64,
    mode: u32,
    modified_at: SystemTime,
}

#[derive(Debug, Eq, PartialEq)]
enum RuntimeTreePayloadV1 {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

#[derive(Debug, Eq, PartialEq)]
struct RuntimeTreeEntryV1 {
    relative_path: PathBuf,
    metadata: SnapshotMetadataV1,
    payload: RuntimeTreePayloadV1,
}

#[derive(Debug, Eq, PartialEq)]
struct FileSnapshotV1 {
    metadata: SnapshotMetadataV1,
    bytes: Vec<u8>,
}

#[cfg(unix)]
fn snapshot_mode(metadata: &fs::Metadata) -> u32 {
    metadata.mode()
}

#[cfg(not(unix))]
fn snapshot_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        1
    } else {
        0
    }
}

fn snapshot_metadata(path: &Path) -> SnapshotMetadataV1 {
    let metadata = fs::symlink_metadata(path).expect("snapshot path metadata must be readable");
    let file_type = metadata.file_type();
    SnapshotMetadataV1 {
        is_directory: file_type.is_dir(),
        is_regular_file: file_type.is_file(),
        is_symlink: file_type.is_symlink(),
        byte_length: metadata.len(),
        mode: snapshot_mode(&metadata),
        modified_at: metadata
            .modified()
            .expect("snapshot path modification time must be readable"),
    }
}

fn snapshot_runtime_tree(root: &Path) -> Vec<RuntimeTreeEntryV1> {
    let mut entries = Vec::new();
    snapshot_runtime_tree_entry(root, Path::new(""), &mut entries);
    entries
}

fn snapshot_runtime_tree_entry(
    path: &Path,
    relative_path: &Path,
    entries: &mut Vec<RuntimeTreeEntryV1>,
) {
    let metadata = snapshot_metadata(path);
    let payload = if metadata.is_directory {
        RuntimeTreePayloadV1::Directory
    } else if metadata.is_regular_file {
        RuntimeTreePayloadV1::File(read_file(path))
    } else if metadata.is_symlink {
        RuntimeTreePayloadV1::Symlink(
            fs::read_link(path).expect("runtime tree symlink must be readable"),
        )
    } else {
        panic!("runtime tree contains an unsupported filesystem entry: {path:?}");
    };
    let is_directory = metadata.is_directory;
    entries.push(RuntimeTreeEntryV1 {
        relative_path: relative_path.to_path_buf(),
        metadata,
        payload,
    });
    if is_directory {
        let mut children = fs::read_dir(path)
            .expect("runtime tree directory must be readable")
            .map(|entry| entry.expect("runtime tree entry must be readable").path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            let child_name = child
                .file_name()
                .expect("runtime tree child must have a file name");
            snapshot_runtime_tree_entry(&child, &relative_path.join(child_name), entries);
        }
    }
}

fn snapshot_file(path: &Path) -> FileSnapshotV1 {
    let metadata = snapshot_metadata(path);
    assert!(
        metadata.is_regular_file,
        "snapshot path must be a regular file"
    );
    FileSnapshotV1 {
        metadata,
        bytes: read_file(path),
    }
}

fn observation() -> WorkspaceRuntimeObservationV1 {
    WorkspaceRuntimeObservationV1::new(
        UnixMillis::new(1_700_000_000_123),
        Rfc3339MillisV1::new("2026-07-15T12:34:56.789Z")
            .expect("fixture registry timestamp must be valid"),
    )
}

fn manager(root: &Path) -> WorkspaceRuntimeManagerV1 {
    let application_support = root.join("Application Support");
    fs::create_dir_all(&application_support)
        .expect("fixture application-support parent must exist");
    #[cfg(unix)]
    fs::set_permissions(&application_support, fs::Permissions::from_mode(0o700))
        .expect("fixture application-support parent must be private");
    let paths = ServiceRuntimePathsV1::from_directories(
        root.join("LaunchAgents"),
        application_support.join("Podway"),
        root.join("Logs/Podway"),
        fixture_runtime_directory(root),
    )
    .expect("fixture service paths must be valid");
    WorkspaceRuntimeManagerV1::new(
        &paths,
        SqliteStoreOptionsV1::new(8).expect("fixture inspection options must be valid"),
    )
}

#[cfg(unix)]
fn make_runtime_private(root: &Path) {
    fs::set_permissions(
        root.join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("fixture runtime must be private before layout initialization");
}

#[cfg(not(unix))]
fn make_runtime_private(_root: &Path) {}

fn bootstrap_main(
    manager: &WorkspaceRuntimeManagerV1,
    root: &Path,
) -> Arc<
    podway_daemon::scheduler::WorkspaceSchedulerV1<
        podway_daemon::runtime_workspace::WorkspaceSchedulerContextV1,
    >,
> {
    make_runtime_private(root);
    manager
        .bootstrap(selector(root), observation())
        .expect("workspace bootstrap must compose a scheduler context")
}

#[test]
fn init_creates_only_workspace_state_not_a_task_or_session() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let context = scheduler.context_snapshot();

    assert_eq!(context.queue_limit(), 256);
    assert_eq!(context.config().job_queue.max_pending, 256);
    assert!(
        context
            .store()
            .read_session_aggregate(context.binding().identity())
            .expect("new Store must be readable")
            .is_none(),
        "workspace.init must not create a session"
    );
    assert!(context.database_path().is_file());
    assert_eq!(
        context.runtime_directory_path(),
        fs::canonicalize(fixture.main().join(".podway/runtime"))
            .expect("runtime directory must canonicalize")
    );
}
#[test]
fn cloned_contexts_expose_only_read_only_store_facades() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let context = scheduler.context_snapshot();
    let context_clone = context.as_ref().clone();

    assert_eq!(
        context.store().startup_recovery_report(),
        context_clone.store().startup_recovery_report(),
        "context clones expose the same generation's immutable recovery observation"
    );
    assert_eq!(
        context
            .store()
            .read_workspace_view(context.binding().identity())
            .expect("read facade must observe the active workspace")
            .identity()
            .workspace_uuid(),
        context.binding().identity().workspace_uuid()
    );
    assert!(matches!(
        context.store().claim_next(
            context.binding().identity(),
            WorkerIdV1::new("read-facade").expect("fixture worker ID must be valid"),
            podway_store::EpochMillisV1::new(1_700_000_000_123),
        ),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));
}

#[test]
fn repeated_existing_resolution_reuses_the_identity_keyed_scheduler_context() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let initial = bootstrap_main(&manager, fixture.main());
    let workspace_id = initial
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();

    let repeated = manager
        .resolve_existing(selector(fixture.main()), Some(&workspace_id), observation())
        .expect("existing Store binding must resolve");

    assert!(Arc::ptr_eq(&initial, &repeated));
    assert_eq!(initial.key(), repeated.key());
    assert_eq!(initial.generation(), repeated.generation());
    assert!(Arc::ptr_eq(
        &initial.context_snapshot(),
        &repeated.context_snapshot()
    ));
}

#[test]
fn root_and_nested_aliases_share_one_path_free_scheduler() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let root = bootstrap_main(&manager, fixture.main());
    let workspace_id = root
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let nested = fixture.main().join("nested/child");
    fs::create_dir_all(&nested).expect("nested selector fixture must be created");

    let nested_scheduler = manager
        .resolve_existing(selector(&nested), Some(&workspace_id), observation())
        .expect("nested worktree alias must resolve");

    assert!(Arc::ptr_eq(&root, &nested_scheduler));
    assert_eq!(root.key(), nested_scheduler.key());
    assert_eq!(root.generation(), nested_scheduler.generation());
}

#[test]
fn move_rebind_preserves_generation_and_updates_registry_with_git_provenance() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let initial = bootstrap_main(&manager, fixture.main());
    let initial_context = initial.context_snapshot();
    let workspace_id = initial_context
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let initial_key = initial.key().clone();
    let initial_generation = initial.generation();
    let relocated = fixture.temporary_path().join("relocated-main");
    fs::rename(fixture.main(), &relocated).expect("real Git worktree must move atomically");
    let canonical_relocated =
        fs::canonicalize(&relocated).expect("relocated worktree must canonicalize");

    let rebound = manager
        .resolve_existing(selector(&relocated), Some(&workspace_id), observation())
        .expect("moved worktree must rebind through the durable Store identity");
    let rebound_context = rebound.context_snapshot();

    assert!(Arc::ptr_eq(&initial, &rebound));
    assert_eq!(rebound.key(), &initial_key);
    assert_eq!(rebound.generation(), initial_generation);
    assert_ne!(
        initial_context.workspace_root(),
        rebound_context.workspace_root(),
        "a moved root must replace future scheduler context snapshots"
    );
    assert_eq!(
        rebound_context.workspace_root().to_path_buf(),
        canonical_relocated
    );
    let registry = manager
        .registry()
        .lookup(&workspace_id)
        .expect("metadata registry must remain readable")
        .expect("moved workspace metadata must remain registered");
    assert_eq!(
        registry.last_known_root().to_path_buf(),
        canonical_relocated
    );
}

#[test]
fn copied_workspace_uuid_conflict_never_adopts_or_mutates_the_copy() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let workspace_id = scheduler
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let original_root = fs::canonicalize(fixture.main()).expect("original root must canonicalize");
    let copied = fixture.temporary_path().join("copied-live-worktree");
    copy_tree(fixture.main(), &copied);
    let copied_runtime = copied.join(".podway/runtime");
    let copied_runtime_before = snapshot_runtime_tree(&copied_runtime);
    let registry_path = manager.registry().registry_path().to_path_buf();
    let registry_before = snapshot_file(&registry_path);

    let error =
        match manager.resolve_existing(selector(&copied), Some(&workspace_id), observation()) {
            Ok(_) => panic!("a copied live Store must not receive the original workspace UUID"),
            Err(error) => error,
        };

    match error {
        WorkspaceRuntimeErrorV1::Resolution(
            WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch {
                stored,
                observed_common_directory_fingerprint,
                observed_worktree_administration_fingerprint,
            },
        ) => {
            assert_eq!(
                stored.workspace_uuid(),
                &workspace_id,
                "the rejected copy must retain the original durable workspace UUID"
            );
            assert_ne!(
                stored.common_dir_identity(),
                &observed_common_directory_fingerprint,
                "the copied common Git directory must fail the durable identity check"
            );
            assert_ne!(
                stored.worktree_admin_identity(),
                &observed_worktree_administration_fingerprint,
                "the copied worktree administration must fail the durable identity check"
            );
        }
        other => {
            panic!("a copied workspace must fail with Git/Store fingerprint conflict: {other:?}")
        }
    }
    assert_eq!(
        snapshot_runtime_tree(&copied_runtime),
        copied_runtime_before,
        "copy rejection must not mutate any copied runtime path, bytes, or metadata"
    );
    assert_eq!(
        snapshot_file(&registry_path),
        registry_before,
        "copy rejection must not mutate registry bytes or metadata"
    );

    let registered = manager
        .registry()
        .lookup(&workspace_id)
        .expect("metadata registry must remain readable")
        .expect("original workspace must remain registered");
    assert_eq!(
        scheduler.context_snapshot().workspace_root().to_path_buf(),
        original_root.clone(),
        "the original scheduler context must remain authoritative"
    );
    assert_eq!(
        registered.last_known_root().to_path_buf(),
        original_root,
        "the original workspace must remain the UUID authority"
    );
    assert_ne!(
        registered.last_known_root().to_path_buf(),
        fs::canonicalize(&copied).expect("copied root must canonicalize"),
        "the copied workspace must not be adopted as UUID authority"
    );
}

#[test]
fn linked_worktree_uses_a_distinct_identity_scheduler_and_local_store() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let main = bootstrap_main(&manager, fixture.main());
    let linked = bootstrap_main(&manager, fixture.linked());

    assert_ne!(main.key(), linked.key());
    assert_ne!(
        main.context_snapshot().database_path(),
        linked.context_snapshot().database_path()
    );
    assert_eq!(
        manager
            .registry()
            .load()
            .expect("registry must contain both worktrees")
            .workspaces()
            .len(),
        2
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_runtime_root_round_trips_when_the_filesystem_supports_it() {
    let fixture = git_worktrees();
    let staging = fixture.temporary_path().join("non-utf8-staging");
    let status = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&staging)
        .status()
        .expect("git must be available");
    assert!(status.success());
    let target = non_utf8_child_path(fixture.temporary_path());
    if let Err(error) = fs::rename(&staging, &target) {
        if cfg!(target_os = "macos") && error.raw_os_error() == Some(92) {
            return;
        }
        panic!("non-UTF-8 fixture rename failed: {error}");
    }

    let manager = manager(fixture.temporary_path());
    let scheduler = manager
        .bootstrap(selector(&target), observation())
        .expect("runtime manager must preserve a supported non-UTF-8 root");
    assert_eq!(
        scheduler.context_snapshot().workspace_root().unix_bytes(),
        target.as_os_str().as_bytes()
    );
}

#[test]
fn deleted_root_emits_a_retirement_signal_without_registry_fallback() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let key = scheduler.key().clone();
    let generation = scheduler.generation();
    fs::remove_dir_all(fixture.main()).expect("fixture root must be removable");

    let signal = manager
        .revalidate_scheduler(&scheduler)
        .expect("a missing root must be a typed retirement signal");

    assert!(matches!(
        signal,
        WorkspaceSchedulerRevalidationV1::RetireRequired {
            key: actual_key,
            generation: actual_generation,
            source: WorkspaceResolutionErrorV1::Git { .. },
        } if actual_key == key && actual_generation == generation
    ));
}

#[cfg(unix)]
#[test]
fn missing_database_emits_a_retirement_signal_for_the_active_generation() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let database_path = scheduler.context_snapshot().database_path().to_path_buf();
    fs::remove_file(&database_path).expect("active database fixture must be removable");

    let signal = manager
        .revalidate_scheduler(&scheduler)
        .expect("a missing database must be a typed retirement signal");

    assert!(matches!(
        signal,
        WorkspaceSchedulerRevalidationV1::RetireRequired {
            source: WorkspaceResolutionErrorV1::ExistingBindingMissing,
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn replaced_database_identity_emits_a_retirement_signal() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let main = bootstrap_main(&manager, fixture.main());
    let linked = bootstrap_main(&manager, fixture.linked());
    let main_database = main.context_snapshot().database_path().to_path_buf();
    let replacement = main_database.with_extension("replacement");
    fs::copy(linked.context_snapshot().database_path(), &replacement)
        .expect("replacement database fixture must copy");
    fs::rename(&replacement, &main_database).expect("replacement database must publish atomically");

    let signal = manager
        .revalidate_scheduler(&main)
        .expect("a replaced database must be a typed retirement signal");

    assert!(matches!(
        signal,
        WorkspaceSchedulerRevalidationV1::RetireRequired {
            source: WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution
                | WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { .. }
                | WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch { .. }
                | WorkspaceResolutionErrorV1::RevalidatedStoreIdentityMismatch { .. }
                | WorkspaceResolutionErrorV1::BindingInspection { .. },
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn same_identity_database_replacement_is_rejected_and_requires_retirement() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let database = scheduler.context_snapshot().database_path().to_path_buf();
    let replacement = database.with_extension("same-identity-replacement");
    fs::copy(&database, &replacement).expect("same-identity replacement must copy");
    fs::rename(&replacement, &database).expect("same-identity replacement must publish atomically");

    let signal = manager
        .revalidate_scheduler(&scheduler)
        .expect("same-identity replacement must produce a retirement signal");
    assert!(matches!(
        signal,
        WorkspaceSchedulerRevalidationV1::RetireRequired {
            source: WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution,
            ..
        }
    ));

    assert!(matches!(
        manager.resolve_existing(selector(fixture.main()), None, observation()),
        Err(WorkspaceRuntimeErrorV1::Resolution(
            WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution
        ))
    ));
}

#[cfg(unix)]
#[test]
fn layout_and_store_are_private_at_runtime_boundaries() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let context = scheduler.context_snapshot();
    let runtime = context.runtime_directory_path();
    let config = fixture.main().join(".podway/config.yaml");

    assert_eq!(
        fs::metadata(runtime)
            .expect("runtime metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&config)
            .expect("config metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(context.database_path())
            .expect("database metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn registry_remains_metadata_only_and_never_contains_workspace_task_state() {
    let fixture = git_worktrees();
    let manager = manager(fixture.temporary_path());
    let scheduler = bootstrap_main(&manager, fixture.main());
    let workspace_id = scheduler
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();

    let registry = manager
        .registry()
        .load()
        .expect("registry must be readable after bootstrap");
    let entry = registry
        .lookup(&workspace_id)
        .expect("workspace registry entry must exist");
    assert_eq!(
        entry.last_known_root(),
        scheduler.context_snapshot().workspace_root()
    );

    let document: Value = serde_json::from_slice(
        &fs::read(manager.registry().registry_path()).expect("registry bytes must be readable"),
    )
    .expect("registry document must be JSON");
    let top_level = document
        .as_object()
        .expect("registry document must be an object");
    assert_eq!(top_level.len(), 2);
    assert!(top_level.contains_key("schema"));
    let workspaces = top_level
        .get("workspaces")
        .and_then(Value::as_array)
        .expect("registry must contain workspace metadata array");
    assert_eq!(workspaces.len(), 1);
    let entry = workspaces[0]
        .as_object()
        .expect("registry metadata entry must be an object");
    assert_eq!(entry.len(), 3);
    assert!(entry.contains_key("workspace_uuid"));
    assert!(entry.contains_key("last_known_root"));
    assert!(entry.contains_key("last_seen_at"));
    for forbidden in ["task", "session", "job", "queue", "request", "receipt"] {
        assert!(
            !entry.contains_key(forbidden),
            "metadata registry must not copy global or workspace task state"
        );
    }
}
