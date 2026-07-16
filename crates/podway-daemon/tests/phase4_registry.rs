//! Phase 4 private, metadata-only workspace-registry contracts.

#![forbid(unsafe_code)]

use std::{
    env, fs,
    os::unix::{
        fs::{PermissionsExt, symlink},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use podway_core::WorkspaceId;
use podway_daemon::registry;
use podway_daemon::registry::{
    MAX_WORKSPACE_REGISTRY_ENTRIES_V1, RegistryErrorV1, RegistryFailpointActionV1,
    RegistryFailpointV1, RegistryPathViolationV1, RegistryStoreV1, WorkspaceRegistryEntryV1,
    WorkspaceRegistryV1,
};
use podway_protocol::Rfc3339MillisV1;
use podway_service::ServiceRuntimePathsV1;
use podway_store::ValidatedWorkspaceRootV1;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const REGISTRY_CRASH_CHILD_TEST_NAME: &str = "registry_crash_child_aborts_at_configured_boundary";
const REGISTRY_CRASH_ROOT_ENV: &str = "PODWAY_PHASE4_REGISTRY_CRASH_ROOT";
const REGISTRY_CRASH_FAILPOINT_ENV: &str = "PODWAY_PHASE4_REGISTRY_CRASH_FAILPOINT";
const REGISTRY_CRASH_BEFORE_RENAME: &str = "before-rename";
const REGISTRY_CRASH_AFTER_RENAME: &str = "after-rename-before-parent-sync";

struct RegistryFixture {
    root: PathBuf,
    paths: ServiceRuntimePathsV1,
}

impl RegistryFixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "podway-daemon-phase4-registry-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture root must be created");

        let launch_agents = root.join("LaunchAgents");
        let application_support = root.join("ApplicationSupport");
        let logs = root.join("Logs");
        for directory in [&launch_agents, &application_support, &logs] {
            fs::create_dir(directory).expect("fixture service directory must be created");
        }
        set_mode(&application_support, 0o700);

        let paths = ServiceRuntimePathsV1::from_directories(
            launch_agents,
            application_support,
            logs,
            root.join("Runtime"),
        )
        .expect("fixture paths must be valid service paths");
        Self { root, paths }
    }

    fn registry_path(&self) -> &Path {
        self.paths.workspace_registry_path().as_path()
    }

    fn registry_parent(&self) -> &Path {
        self.registry_path()
            .parent()
            .expect("service-owned registry path has a parent")
    }

    fn registry_lock_path(&self) -> PathBuf {
        let mut file_name = self
            .registry_path()
            .file_name()
            .expect("registry path has a file name")
            .to_os_string();
        file_name.push(".lock");
        self.registry_parent().join(file_name)
    }
}

impl Drop for RegistryFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("fixture permissions must be set");
}

fn workspace(number: u64) -> WorkspaceId {
    WorkspaceId::new(format!("00000000-0000-0000-0000-{number:012x}"))
        .expect("fixture workspace UUID must be canonical")
}

fn root(path: &str) -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new(path))
        .expect("fixture workspace root must be losslessly encodable")
}

fn timestamp() -> Rfc3339MillisV1 {
    Rfc3339MillisV1::new("2026-07-15T12:34:56.789Z")
        .expect("fixture timestamp must have millisecond precision")
}

fn write_private_registry(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("corrupt registry fixture must be written");
    set_mode(path, 0o600);
}

fn service_runtime_paths_for_root(root: &Path) -> ServiceRuntimePathsV1 {
    ServiceRuntimePathsV1::from_directories(
        root.join("LaunchAgents"),
        root.join("ApplicationSupport"),
        root.join("Logs"),
        root.join("Runtime"),
    )
    .expect("child fixture paths must be valid service paths")
}

fn assert_aborted(status: ExitStatus, label: &str) {
    assert!(
        !status.success(),
        "{label} child unexpectedly returned without crashing"
    );
    assert_eq!(
        status.signal(),
        Some(6),
        "{label} child must terminate with SIGABRT after reaching its failpoint"
    );
}

fn run_registry_crash_child(
    fixture: &RegistryFixture,
    failpoint: RegistryFailpointV1,
) -> ExitStatus {
    let failpoint = match failpoint {
        RegistryFailpointV1::BeforeRename => REGISTRY_CRASH_BEFORE_RENAME,
        RegistryFailpointV1::AfterRenameBeforeParentSync => REGISTRY_CRASH_AFTER_RENAME,
    };
    Command::new(env::current_exe().expect("registry test executable path must be available"))
        .arg("--exact")
        .arg(REGISTRY_CRASH_CHILD_TEST_NAME)
        .arg("--nocapture")
        .env(REGISTRY_CRASH_ROOT_ENV, fixture.root.as_os_str())
        .env(REGISTRY_CRASH_FAILPOINT_ENV, failpoint)
        .status()
        .expect("registry crash child must start")
}

fn assert_private_directory(path: &Path, label: &str) {
    let metadata = fs::symlink_metadata(path).expect("private directory metadata must be readable");
    assert!(metadata.file_type().is_dir(), "{label} must be a directory");
    assert_eq!(
        metadata.permissions().mode() & 0o777,
        0o700,
        "{label} must be mode 0700"
    );
}

fn assert_private_file(path: &Path, label: &str) {
    let metadata = fs::symlink_metadata(path).expect("private file metadata must be readable");
    assert!(
        metadata.file_type().is_file(),
        "{label} must be a regular file"
    );
    assert_eq!(
        metadata.permissions().mode() & 0o777,
        0o600,
        "{label} must be mode 0600"
    );
}

fn assert_private_registry_paths(fixture: &RegistryFixture) {
    assert_private_directory(fixture.registry_parent(), "registry parent");
    assert_private_file(fixture.registry_path(), "registry document");
    assert_private_file(&fixture.registry_lock_path(), "registry lock");
}

fn temporary_registry_paths(parent: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(parent).expect("registry parent must be readable") {
        let entry = entry.expect("registry parent entry must be readable");
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".podway-registry-v1-")
        {
            paths.push(entry.path());
        }
    }
    paths
}

fn assert_crash_temporary_state(fixture: &RegistryFixture, failpoint: RegistryFailpointV1) {
    let temporary_paths = temporary_registry_paths(fixture.registry_parent());
    match failpoint {
        RegistryFailpointV1::BeforeRename => {
            assert_eq!(
                temporary_paths.len(),
                1,
                "pre-rename abort must leave only its private, unaccepted temporary"
            );
            for temporary_path in temporary_paths {
                assert_private_file(&temporary_path, "unaccepted registry temporary");
            }
        }
        RegistryFailpointV1::AfterRenameBeforeParentSync => {
            assert!(
                temporary_paths.is_empty(),
                "post-rename abort must not leave a temporary after publication"
            );
        }
    }
}

#[test]
fn registry_crash_child_aborts_at_configured_boundary() {
    let Some(fixture_root) = env::var_os(REGISTRY_CRASH_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let failpoint = match env::var(REGISTRY_CRASH_FAILPOINT_ENV)
        .expect("crash child failpoint must be configured")
        .as_str()
    {
        REGISTRY_CRASH_BEFORE_RENAME => RegistryFailpointV1::BeforeRename,
        REGISTRY_CRASH_AFTER_RENAME => RegistryFailpointV1::AfterRenameBeforeParentSync,
        failpoint => panic!("unknown registry crash failpoint {failpoint}"),
    };
    let paths = service_runtime_paths_for_root(&fixture_root);
    let store =
        RegistryStoreV1::with_failpoint(&paths, failpoint, RegistryFailpointActionV1::AbortProcess);
    let _ = store.insert_or_refresh(workspace(202), root("/tmp/registry-crash-new"), timestamp());
    panic!("configured registry failpoint returned instead of aborting");
}

#[test]
fn registry_emits_exact_schema_bytes_and_replays_identically() {
    let fixture = RegistryFixture::new();
    let store = RegistryStoreV1::new(&fixture.paths);
    let root = root("/tmp/registry-alpha");
    let workspace = workspace(1);
    let seen = timestamp();

    store
        .insert_or_refresh(workspace.clone(), root.clone(), seen.clone())
        .expect("initial metadata observation must persist");

    let expected = concat!(
        "{\"schema\":\"podway.registry/v1\",\"workspaces\":[{",
        "\"last_known_root\":\"podway.unix-path/v1:2f746d702f72656769737472792d616c706861\",",
        "\"last_seen_at\":\"2026-07-15T12:34:56.789Z\",",
        "\"workspace_uuid\":\"00000000-0000-0000-0000-000000000001\"}]}"
    );
    assert_eq!(
        fs::read(fixture.registry_path()).expect("registry bytes must be readable"),
        expected.as_bytes()
    );

    let reopened = RegistryStoreV1::new(&fixture.paths);
    let loaded = reopened.load().expect("canonical bytes must replay");
    assert_eq!(loaded.workspaces().len(), 1);
    assert_eq!(
        loaded
            .lookup(&workspace)
            .expect("workspace must be present")
            .last_known_root(),
        &root
    );
    reopened
        .insert_or_refresh(workspace, root, seen)
        .expect("same-root replay must only refresh metadata");
    assert_eq!(
        fs::read(fixture.registry_path()).expect("replayed registry bytes must be readable"),
        expected.as_bytes()
    );
}

#[test]
fn registry_rejects_more_than_ten_thousand_entries() {
    let root = root("/tmp/registry-bound");
    let seen = timestamp();
    let mut entries = Vec::with_capacity(MAX_WORKSPACE_REGISTRY_ENTRIES_V1 + 1);
    for number in 1..=(MAX_WORKSPACE_REGISTRY_ENTRIES_V1 as u64 + 1) {
        entries.push(
            WorkspaceRegistryEntryV1::new(workspace(number), root.clone(), seen.clone())
                .expect("fixture entry must be valid"),
        );
    }

    assert!(matches!(
        WorkspaceRegistryV1::new(entries),
        Err(
            registry::WorkspaceRegistryValidationErrorV1::TooManyWorkspaces {
                maximum: MAX_WORKSPACE_REGISTRY_ENTRIES_V1,
                ..
            }
        )
    ));
}

#[test]
fn corrupt_registry_documents_fail_closed_for_every_strictness_boundary() {
    let fixture = RegistryFixture::new();
    let store = RegistryStoreV1::new(&fixture.paths);
    let valid_root = "podway.unix-path/v1:2f746d702f7265676973747279";
    let valid_timestamp = "2026-07-15T12:34:56.789Z";
    let cases = [
        (
            "unknown field",
            "{\"extra\":true,\"schema\":\"podway.registry/v1\",\"workspaces\":[]}".to_owned(),
        ),
        (
            "duplicate field",
            "{\"schema\":\"podway.registry/v1\",\"schema\":\"podway.registry/v1\",\"workspaces\":[]}"
                .to_owned(),
        ),
        (
            "wrong schema",
            "{\"schema\":\"podway.registry/v2\",\"workspaces\":[]}".to_owned(),
        ),
        (
            "noncanonical whitespace",
            "{\"schema\": \"podway.registry/v1\",\"workspaces\":[]}".to_owned(),
        ),
        (
            "invalid root",
            format!(
                "{{\"schema\":\"podway.registry/v1\",\"workspaces\":[{{\"last_known_root\":\"podway.unix-path/v1:2F\",\"last_seen_at\":\"{valid_timestamp}\",\"workspace_uuid\":\"00000000-0000-0000-0000-000000000001\"}}]}}"
            ),
        ),
        (
            "invalid timestamp",
            format!(
                "{{\"schema\":\"podway.registry/v1\",\"workspaces\":[{{\"last_known_root\":\"{valid_root}\",\"last_seen_at\":\"2026-07-15T12:34:56Z\",\"workspace_uuid\":\"00000000-0000-0000-0000-000000000001\"}}]}}"
            ),
        ),
        (
            "unordered entries",
            format!(
                "{{\"schema\":\"podway.registry/v1\",\"workspaces\":[{{\"last_known_root\":\"{valid_root}\",\"last_seen_at\":\"{valid_timestamp}\",\"workspace_uuid\":\"00000000-0000-0000-0000-000000000002\"}},{{\"last_known_root\":\"{valid_root}\",\"last_seen_at\":\"{valid_timestamp}\",\"workspace_uuid\":\"00000000-0000-0000-0000-000000000001\"}}]}}"
            ),
        ),
    ];

    for (name, bytes) in cases {
        write_private_registry(fixture.registry_path(), bytes.as_bytes());
        assert!(
            matches!(
                store.load(),
                Err(RegistryErrorV1::InvalidRegistryDocument { .. })
            ),
            "{name} must be rejected"
        );
    }

    let oversized = vec![b' '; registry::MAX_WORKSPACE_REGISTRY_BYTES_V1 as usize + 1];
    write_private_registry(fixture.registry_path(), &oversized);
    assert!(matches!(
        store.load(),
        Err(RegistryErrorV1::RegistryTooLarge { .. })
    ));
}
#[test]
fn missing_registry_parent_is_created_as_a_private_real_directory() {
    let fixture = RegistryFixture::new();
    fs::remove_dir(fixture.registry_parent()).expect("empty registry parent must be removable");

    let registry = RegistryStoreV1::new(&fixture.paths)
        .load()
        .expect("missing service-owned parent must be created for an empty registry");
    assert!(registry.workspaces().is_empty());
    let metadata =
        fs::symlink_metadata(fixture.registry_parent()).expect("registry parent must be created");
    assert!(metadata.is_dir());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
}

#[test]
fn registry_rejects_nonprivate_parents_and_unsafe_path_nodes() {
    let fixture = RegistryFixture::new();
    let store = RegistryStoreV1::new(&fixture.paths);

    set_mode(fixture.registry_parent(), 0o755);
    assert!(matches!(
        store.load(),
        Err(RegistryErrorV1::UnsafeRegistryParent {
            violation: RegistryPathViolationV1::WrongMode { .. },
            ..
        })
    ));
    set_mode(fixture.registry_parent(), 0o700);
    write_private_registry(fixture.registry_path(), b"{}");
    set_mode(fixture.registry_path(), 0o640);
    assert!(matches!(
        store.load(),
        Err(RegistryErrorV1::UnsafeRegistryFile {
            violation: RegistryPathViolationV1::WrongMode { .. },
            ..
        })
    ));
    fs::remove_file(fixture.registry_path()).expect("wrong-mode registry file must be removed");

    let target = fixture.root.join("target");
    fs::write(&target, b"target").expect("symlink target must exist");
    symlink(&target, fixture.registry_path()).expect("registry symlink fixture must be created");
    assert!(matches!(
        store.load(),
        Err(RegistryErrorV1::UnsafeRegistryFile {
            violation: RegistryPathViolationV1::Symlink,
            ..
        })
    ));
    fs::remove_file(fixture.registry_path()).expect("registry symlink must be removed");

    fs::create_dir(fixture.registry_path()).expect("registry directory fixture must be created");
    assert!(matches!(
        store.load(),
        Err(RegistryErrorV1::UnsafeRegistryFile {
            violation: RegistryPathViolationV1::NotRegularFile,
            ..
        })
    ));
    fs::remove_dir(fixture.registry_path()).expect("registry directory fixture must be removed");
    fs::remove_file(fixture.registry_lock_path()).expect("registry lock must be removed");

    symlink(&target, fixture.registry_lock_path()).expect("registry lock symlink must be created");
    assert!(matches!(
        store.load(),
        Err(RegistryErrorV1::UnsafeRegistryLock {
            violation: RegistryPathViolationV1::Symlink,
            ..
        })
    ));

    let non_directory_fixture = RegistryFixture::new();
    fs::remove_dir(non_directory_fixture.registry_parent())
        .expect("empty registry parent must be removable");
    fs::write(non_directory_fixture.registry_parent(), b"not a directory")
        .expect("non-directory parent fixture must be created");
    assert!(matches!(
        RegistryStoreV1::new(&non_directory_fixture.paths).load(),
        Err(RegistryErrorV1::UnsafeRegistryParent {
            violation: RegistryPathViolationV1::NotDirectory,
            ..
        })
    ));

    let symlink_parent_fixture = RegistryFixture::new();
    let private_target = symlink_parent_fixture.root.join("private-target");
    fs::create_dir(&private_target).expect("private parent target must be created");
    set_mode(&private_target, 0o700);
    fs::remove_dir(symlink_parent_fixture.registry_parent())
        .expect("empty registry parent must be removable");
    symlink(&private_target, symlink_parent_fixture.registry_parent())
        .expect("registry parent symlink fixture must be created");
    assert!(matches!(
        RegistryStoreV1::new(&symlink_parent_fixture.paths).load(),
        Err(RegistryErrorV1::UnsafeRegistryParent {
            violation: RegistryPathViolationV1::Symlink,
            ..
        })
    ));
}

#[test]
fn publication_failpoints_have_deterministic_pre_and_post_rename_recovery() {
    let fixture = RegistryFixture::new();
    let workspace_id = workspace(3);
    let failpoint_root = root("/tmp/registry-failpoint");

    let before = RegistryStoreV1::with_failpoint(
        &fixture.paths,
        RegistryFailpointV1::BeforeRename,
        RegistryFailpointActionV1::ReturnError,
    );
    assert!(matches!(
        before.insert_or_refresh(workspace_id.clone(), failpoint_root.clone(), timestamp()),
        Err(RegistryErrorV1::Failpoint {
            point: RegistryFailpointV1::BeforeRename
        })
    ));
    assert!(
        RegistryStoreV1::new(&fixture.paths)
            .load()
            .expect("pre-rename failure must leave the old registry readable")
            .workspaces()
            .is_empty()
    );
    assert!(
        fs::read_dir(fixture.registry_parent())
            .expect("registry parent must be readable")
            .all(|entry| !entry
                .expect("private registry entry must be readable")
                .file_name()
                .to_string_lossy()
                .starts_with(".podway-registry-v1-")),
        "pre-rename cleanup may only leave no owned temporary behind"
    );

    let after = RegistryStoreV1::with_failpoint(
        &fixture.paths,
        RegistryFailpointV1::AfterRenameBeforeParentSync,
        RegistryFailpointActionV1::ReturnError,
    );
    assert!(matches!(
        after.insert_or_refresh(workspace_id.clone(), failpoint_root.clone(), timestamp()),
        Err(RegistryErrorV1::Failpoint {
            point: RegistryFailpointV1::AfterRenameBeforeParentSync
        })
    ));
    assert_eq!(
        RegistryStoreV1::new(&fixture.paths)
            .lookup(&workspace_id)
            .expect("post-rename recovery must load the replacement document")
            .expect("post-rename document must contain the observation")
            .last_known_root(),
        &failpoint_root
    );

    let barrier = Arc::new(Barrier::new(2));
    let barrier_store = Arc::new(RegistryStoreV1::with_failpoint(
        &fixture.paths,
        RegistryFailpointV1::BeforeRename,
        RegistryFailpointActionV1::Barrier(Arc::clone(&barrier)),
    ));
    let barrier_workspace = workspace(4);
    let writer = {
        let barrier_store = Arc::clone(&barrier_store);
        thread::spawn(move || {
            barrier_store.insert_or_refresh(
                barrier_workspace,
                root("/tmp/registry-barrier"),
                timestamp(),
            )
        })
    };
    barrier.wait();
    writer
        .join()
        .expect("barrier-controlled writer must not panic")
        .expect("barrier release must permit publication");
}
#[test]
fn abort_process_failpoints_recover_exact_documents_without_accepting_temporaries() {
    for (label, failpoint, seed_old_document) in [
        (
            "pre-rename abort with an absent document",
            RegistryFailpointV1::BeforeRename,
            false,
        ),
        (
            "pre-rename abort with an old document",
            RegistryFailpointV1::BeforeRename,
            true,
        ),
        (
            "post-rename abort with an old document",
            RegistryFailpointV1::AfterRenameBeforeParentSync,
            true,
        ),
    ] {
        let fixture = RegistryFixture::new();
        let old_entry = WorkspaceRegistryEntryV1::new(
            workspace(101),
            root("/tmp/registry-crash-old"),
            timestamp(),
        )
        .expect("old crash fixture entry must be valid");
        let crash_entry = WorkspaceRegistryEntryV1::new(
            workspace(202),
            root("/tmp/registry-crash-new"),
            timestamp(),
        )
        .expect("crash fixture entry must be valid");
        let recovery_entry = WorkspaceRegistryEntryV1::new(
            workspace(303),
            root("/tmp/registry-crash-recovery"),
            timestamp(),
        )
        .expect("recovery fixture entry must be valid");
        let mut expected_entries = Vec::new();

        if seed_old_document {
            RegistryStoreV1::new(&fixture.paths)
                .insert_or_refresh(
                    old_entry.workspace_uuid().clone(),
                    old_entry.last_known_root().clone(),
                    old_entry.last_seen_at().clone(),
                )
                .expect("old registry document must persist");
            expected_entries.push(old_entry.clone());
        }

        assert_aborted(run_registry_crash_child(&fixture, failpoint), label);

        if failpoint == RegistryFailpointV1::AfterRenameBeforeParentSync {
            expected_entries.push(crash_entry.clone());
        }
        let expected_after_abort = WorkspaceRegistryV1::new(expected_entries.clone())
            .expect("expected crash-recovery document must be strictly sorted");
        let reopened = RegistryStoreV1::new(&fixture.paths);
        let recovered_document = reopened
            .load()
            .expect("crash recovery must load one strict registry document");
        assert_eq!(
            recovered_document, expected_after_abort,
            "{label} must recover exactly the durable registry document"
        );
        for expected_entry in expected_after_abort.workspaces() {
            assert_eq!(
                recovered_document.lookup(expected_entry.workspace_uuid()),
                Some(expected_entry),
                "{label} must retain each workspace at its own root after reopening"
            );
        }
        match failpoint {
            RegistryFailpointV1::BeforeRename => {
                assert!(
                    recovered_document
                        .lookup(crash_entry.workspace_uuid())
                        .is_none(),
                    "{label} must not accept the unrenamed temporary observation"
                );
                if !seed_old_document {
                    assert!(
                        !fixture.registry_path().exists(),
                        "{label} must leave the registry document absent"
                    );
                }
            }
            RegistryFailpointV1::AfterRenameBeforeParentSync => {
                assert_eq!(
                    recovered_document.lookup(crash_entry.workspace_uuid()),
                    Some(&crash_entry),
                    "{label} must retain the renamed observation"
                );
            }
        }
        assert_crash_temporary_state(&fixture, failpoint);
        assert_private_directory(fixture.registry_parent(), "crash-recovered registry parent");
        assert_private_file(
            &fixture.registry_lock_path(),
            "crash-recovered registry lock",
        );
        if fixture.registry_path().exists() {
            assert_private_file(fixture.registry_path(), "crash-recovered registry document");
        }

        reopened
            .insert_or_refresh(
                recovery_entry.workspace_uuid().clone(),
                recovery_entry.last_known_root().clone(),
                recovery_entry.last_seen_at().clone(),
            )
            .expect("reopened registry must preserve observations while accepting a new workspace");
        expected_entries.push(recovery_entry);
        let expected_after_reopen = WorkspaceRegistryV1::new(expected_entries)
            .expect("post-recovery document must remain strictly sorted");
        let replayed_document = RegistryStoreV1::new(&fixture.paths)
            .load()
            .expect("post-recovery registry must reopen strictly");
        assert_eq!(
            replayed_document, expected_after_reopen,
            "{label} must not lose or cross workspace observations after reopening"
        );
        for expected_entry in expected_after_reopen.workspaces() {
            assert_eq!(
                replayed_document.lookup(expected_entry.workspace_uuid()),
                Some(expected_entry),
                "{label} must preserve exact UUID-to-root observations after reopening"
            );
        }
        assert_private_registry_paths(&fixture);
        assert_crash_temporary_state(&fixture, failpoint);
    }
}

#[test]
fn concurrent_registry_writers_serialize_without_losing_observations() {
    let fixture = RegistryFixture::new();
    let store = Arc::new(RegistryStoreV1::new(&fixture.paths));
    let start = Arc::new(Barrier::new(17));
    let mut writers = Vec::new();

    for number in 1..=16 {
        let store = Arc::clone(&store);
        let start = Arc::clone(&start);
        writers.push(thread::spawn(move || {
            start.wait();
            store.insert_or_refresh(
                workspace(number),
                root(&format!("/tmp/registry-concurrent-{number}")),
                timestamp(),
            )
        }));
    }
    start.wait();
    for writer in writers {
        writer
            .join()
            .expect("concurrent writer must not panic")
            .expect("concurrent writer must persist one observation");
    }

    let registry = store
        .load()
        .expect("serialized writes must leave valid JSON");
    assert_eq!(registry.workspaces().len(), 16);
    for number in 1..=16 {
        assert!(
            registry.lookup(&workspace(number)).is_some(),
            "workspace {number} must not be lost"
        );
    }
}

#[test]
fn moves_require_an_exact_previous_root_and_removal_is_exact() {
    let fixture = RegistryFixture::new();
    let store = RegistryStoreV1::new(&fixture.paths);
    let workspace = workspace(5);
    let original = root("/tmp/registry-original");
    let moved = root("/tmp/registry-moved");
    let unrelated = root("/tmp/registry-unrelated");

    store
        .insert_or_refresh(workspace.clone(), original.clone(), timestamp())
        .expect("initial observation must persist");
    assert!(matches!(
        store.insert_or_refresh(workspace.clone(), moved.clone(), timestamp()),
        Err(RegistryErrorV1::WorkspaceRootConflict { .. })
    ));
    assert!(matches!(
        store.move_workspace(&workspace, &unrelated, moved.clone(), timestamp()),
        Err(RegistryErrorV1::WorkspaceRootConflict { .. })
    ));
    store
        .move_workspace(&workspace, &original, moved.clone(), timestamp())
        .expect("exact compare-and-swap move must persist");
    assert_eq!(
        store
            .lookup(&workspace)
            .expect("moved workspace lookup must succeed")
            .expect("moved workspace must remain registered")
            .last_known_root(),
        &moved
    );
    assert!(
        store
            .remove(&workspace)
            .expect("exact removal must succeed")
            .is_some()
    );
    assert!(
        store
            .remove(&workspace)
            .expect("absent removal must still be exact")
            .is_none()
    );
}

#[test]
fn registry_document_contains_only_metadata_fields() {
    let fixture = RegistryFixture::new();
    let store = RegistryStoreV1::new(&fixture.paths);
    store
        .insert_or_refresh(workspace(6), root("/tmp/registry-metadata"), timestamp())
        .expect("metadata observation must persist");

    let document: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.registry_path()).expect("registry bytes must be readable"),
    )
    .expect("registry bytes must be JSON");
    let document = document
        .as_object()
        .expect("registry document must be an object");
    assert_eq!(
        document.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["schema", "workspaces"]
    );
    let entry = document["workspaces"]
        .as_array()
        .expect("workspaces must be an array")
        .first()
        .expect("fixture has one workspace")
        .as_object()
        .expect("workspace must be an object");
    assert_eq!(
        entry.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["last_known_root", "last_seen_at", "workspace_uuid"]
    );
    for forbidden in [
        "fingerprint",
        "task",
        "session",
        "job",
        "config",
        "store",
        "scheduler",
    ] {
        assert!(
            !entry.contains_key(forbidden),
            "registry must not store {forbidden}"
        );
    }
    assert_eq!(
        document["schema"],
        serde_json::Value::String("podway.registry/v1".to_owned())
    );
}
