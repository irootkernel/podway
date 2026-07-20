#![cfg(unix)]

//! SQLite path-safety contracts at the Store filesystem boundary.

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use podway_core::{
    DomainCommand, DomainResult, JobId, Revision, Sha256Digest, UnixMillis, WorkspaceId,
};
use podway_store::schema::inspect_integrity_v1;
use podway_store::{
    AdmitRequestV1, DurableWorktreeIdentityV1, IdempotencyKeyV1, IntegrityModeV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreErrorV1,
    StoreFailpointV1, StoreUnavailableReasonV1, ValidatedWorkspaceRootV1,
};
use tempfile::TempDir;

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-path-safety")).unwrap()
}

fn options() -> SqliteStoreOptionsV1 {
    SqliteStoreOptionsV1::new(8).unwrap()
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}

fn open_store(path: &Path) -> Result<SqliteStoreV1, podway_store::StoreErrorV1> {
    SqliteStoreV1::open(path, &root(), identity(), options(), UnixMillis::new(10))
}

fn initialize(path: &Path) {
    drop(open_store(path).unwrap());
}

fn inspect(path: &Path) -> Result<podway_store::IntegrityReportV1, podway_store::StoreErrorV1> {
    inspect_integrity_v1(
        path,
        &identity(),
        &options(),
        IntegrityModeV1::Fast,
        UnixMillis::new(20),
    )
}
fn path_safety_error() -> StoreErrorV1 {
    StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::StorageIo,
    }
}

fn assert_path_safety_error<T>(result: Result<T, StoreErrorV1>) {
    match result {
        Err(error) => assert_eq!(error, path_safety_error()),
        Ok(_) => panic!("unsafe path must be rejected before SQLite is opened"),
    }
}

fn assert_open_rejected_without_blocking(path: &Path) {
    let (sender, receiver) = mpsc::channel();
    let path = path.to_path_buf();
    let handle = thread::spawn(move || {
        sender.send(open_store(&path).map(drop)).unwrap();
    });

    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("unsafe path must reject without blocking");
    handle.join().unwrap();
    assert_path_safety_error(result);
}

fn assert_inspect_rejected_without_blocking(path: &Path) {
    let (sender, receiver) = mpsc::channel();
    let path = path.to_path_buf();
    let handle = thread::spawn(move || {
        sender.send(inspect(&path).map(|_| ())).unwrap();
    });

    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("unsafe path must reject without blocking");
    handle.join().unwrap();
    assert_path_safety_error(result);
}

fn sqlite_artifact_snapshot(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ]
    .into_iter()
    .filter_map(|artifact| match fs::read(&artifact) {
        Ok(bytes) => Some((artifact, bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => panic!(
            "failed to read SQLite artifact {}: {error}",
            artifact.display()
        ),
    })
    .collect()
}

fn assert_sqlite_artifact_snapshot(path: &Path, expected: &[(PathBuf, Vec<u8>)]) {
    assert_eq!(sqlite_artifact_snapshot(path), expected);
}

fn owned_temporary(path: &Path, now: u64, attempt: u32) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap());
    name.push(".");
    name.push(std::process::id().to_string());
    name.push(".");
    name.push(now.to_string());
    name.push(".");
    name.push(attempt.to_string());
    name.push(".tmp");
    path.parent().unwrap().join(name)
}

#[derive(Clone, Copy, Debug)]
enum UnsafePathKind {
    Directory,
    Fifo,
    Socket,
    Symlink,
    HardLink,
}

fn replace_file_with_unsafe_path(
    path: &Path,
    kind: UnsafePathKind,
    alias_source: &Path,
) -> Option<UnixListener> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!("failed to remove {}: {error}", path.display()),
    }

    match kind {
        UnsafePathKind::Directory => {
            fs::create_dir(path).unwrap();
            fs::write(path.join("directory-sentinel"), b"directory sentinel").unwrap();
            None
        }
        UnsafePathKind::Fifo => {
            let status = Command::new("mkfifo").arg(path).status().unwrap();
            assert!(status.success());
            None
        }
        UnsafePathKind::Socket => Some(UnixListener::bind(path).unwrap()),
        UnsafePathKind::Symlink => {
            symlink(alias_source, path).unwrap();
            None
        }
        UnsafePathKind::HardLink => {
            fs::hard_link(alias_source, path).unwrap();
            None
        }
    }
}

fn assert_unsafe_path_preserved(path: &Path, kind: UnsafePathKind, alias_source: &Path) {
    let metadata = fs::symlink_metadata(path).unwrap();
    match kind {
        UnsafePathKind::Directory => {
            assert!(metadata.is_dir());
            assert_eq!(
                fs::read(path.join("directory-sentinel")).unwrap(),
                b"directory sentinel"
            );
        }
        UnsafePathKind::Fifo => assert!(metadata.file_type().is_fifo()),
        UnsafePathKind::Socket => assert!(metadata.file_type().is_socket()),
        UnsafePathKind::Symlink => {
            assert!(metadata.file_type().is_symlink());
            assert_eq!(fs::read_link(path).unwrap(), alias_source);
        }
        UnsafePathKind::HardLink => {
            let alias_metadata = fs::metadata(alias_source).unwrap();
            assert_eq!(metadata.ino(), alias_metadata.ino());
            assert_eq!(metadata.nlink(), 2);
            assert_eq!(alias_metadata.nlink(), 2);
        }
    }
}

fn reset_request() -> AdmitRequestV1 {
    AdmitRequestV1::new(
        DomainCommand::WorkspaceResetAll,
        IdempotencyKeyV1::new("path-safety-reset").unwrap(),
        JobId::new("00000000-0000-4000-8000-000000000002").unwrap(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('c'),
        UnixMillis::new(30),
    )
}

fn seed_reset(path: &Path) -> Result<podway_store::TerminalReceiptV1, podway_store::StoreErrorV1> {
    let workspace = identity();
    SqliteStoreV1::seed_or_verify_reset_target(
        path,
        &root(),
        workspace.clone(),
        options(),
        reset_request(),
        DomainResult::WorkspaceReset {
            workspace_id: workspace.workspace_uuid().clone(),
            revision: Revision::ZERO,
        },
        UnixMillis::new(40),
    )
}

fn assert_private(path: &Path) {
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn ordinary_open_rejects_final_and_intermediate_symlinks_without_touching_outside_bytes() {
    let temporary = TempDir::new().unwrap();
    let outside = temporary.path().join("outside.sqlite3");
    let _outside_store = open_store(&outside).unwrap();
    let outside_before = sqlite_artifact_snapshot(&outside);

    let final_link = temporary.path().join("state.sqlite3");
    symlink(&outside, &final_link).unwrap();
    assert_open_rejected_without_blocking(&final_link);
    assert_inspect_rejected_without_blocking(&final_link);
    assert_sqlite_artifact_snapshot(&outside, &outside_before);

    let outside_directory = temporary.path().join("outside-directory");
    fs::create_dir(&outside_directory).unwrap();
    let intermediate_target = outside_directory.join("state.sqlite3");
    let _intermediate_store = open_store(&intermediate_target).unwrap();
    let intermediate_before = sqlite_artifact_snapshot(&intermediate_target);
    let intermediate_link = temporary.path().join("runtime-link");
    symlink(&outside_directory, &intermediate_link).unwrap();
    let linked_target = intermediate_link.join("state.sqlite3");
    assert_open_rejected_without_blocking(&linked_target);
    assert_inspect_rejected_without_blocking(&linked_target);
    assert_sqlite_artifact_snapshot(&intermediate_target, &intermediate_before);
}

#[test]
fn reset_and_integrity_reject_a_database_replaced_before_open_or_verify() {
    let temporary = TempDir::new().unwrap();
    let target = temporary.path().join("state.sqlite3");
    let outside = temporary.path().join("outside.sqlite3");
    initialize(&target);
    initialize(&outside);
    let outside_before = sqlite_artifact_snapshot(&outside);

    fs::remove_file(&target).unwrap();
    symlink(&outside, &target).unwrap();

    assert_open_rejected_without_blocking(&target);
    assert_inspect_rejected_without_blocking(&target);
    assert_sqlite_artifact_snapshot(&outside, &outside_before);

    let reset_target = temporary.path().join("reset.sqlite3");
    let reset_outside = temporary.path().join("reset-outside.sqlite3");
    initialize(&reset_outside);
    let reset_outside_before = sqlite_artifact_snapshot(&reset_outside);
    symlink(&reset_outside, &reset_target).unwrap();
    assert_path_safety_error(seed_reset(&reset_target));
    assert_sqlite_artifact_snapshot(&reset_outside, &reset_outside_before);
}

#[test]
fn database_and_sidecars_reject_unsafe_path_kinds_without_touching_sentinels() {
    for affected_suffix in [None, Some("-wal"), Some("-shm")] {
        for kind in [
            UnsafePathKind::Directory,
            UnsafePathKind::Fifo,
            UnsafePathKind::Socket,
            UnsafePathKind::Symlink,
            UnsafePathKind::HardLink,
        ] {
            let temporary = TempDir::new().unwrap();
            let path = temporary.path().join("state.sqlite3");
            let outside = temporary.path().join("outside.sqlite3");
            let _target_store = open_store(&path).unwrap();
            let _outside_store = open_store(&outside).unwrap();
            let outside_before = sqlite_artifact_snapshot(&outside);
            let unsafe_path = affected_suffix
                .map(|suffix| sidecar(&path, suffix))
                .unwrap_or_else(|| path.clone());
            let alias_source = affected_suffix
                .map(|suffix| sidecar(&outside, suffix))
                .unwrap_or_else(|| outside.clone());
            assert!(
                alias_source.exists(),
                "initialized SQLite target must expose {}",
                alias_source.display()
            );

            let unrelated_sentinel = temporary.path().join("unrelated-sentinel");
            let unrelated_bytes = b"unrelated sentinel";
            fs::write(&unrelated_sentinel, unrelated_bytes).unwrap();

            let _socket = replace_file_with_unsafe_path(&unsafe_path, kind, &alias_source);
            assert_open_rejected_without_blocking(&path);
            assert_inspect_rejected_without_blocking(&path);

            assert_unsafe_path_preserved(&unsafe_path, kind, &alias_source);
            assert_sqlite_artifact_snapshot(&outside, &outside_before);
            assert_eq!(fs::read(&unrelated_sentinel).unwrap(), unrelated_bytes);
        }
    }
}
#[test]
fn failed_initialization_removes_only_owned_private_temporary_files() {
    let temporary = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let regular_sentinel = temporary.path().join("unrelated-regular");
    let regular_bytes = b"unrelated regular sentinel";
    fs::write(&regular_sentinel, regular_bytes).unwrap();
    fs::set_permissions(&regular_sentinel, fs::Permissions::from_mode(0o640)).unwrap();

    let symlink_target = outside.path().join("unrelated-symlink-target");
    let symlink_target_bytes = b"unrelated symlink target sentinel";
    fs::write(&symlink_target, symlink_target_bytes).unwrap();
    fs::set_permissions(&symlink_target, fs::Permissions::from_mode(0o400)).unwrap();
    let symlink_sentinel = temporary.path().join("unrelated-symlink");
    symlink(&symlink_target, &symlink_sentinel).unwrap();
    let symlink_mode = fs::symlink_metadata(&symlink_sentinel).unwrap().mode() & 0o777;

    let collision = owned_temporary(&path, 50, 0);
    let collision_paths = [
        collision.clone(),
        sidecar(&collision, "-wal"),
        sidecar(&collision, "-shm"),
    ];
    for (index, collision_path) in collision_paths.iter().enumerate() {
        fs::write(
            collision_path,
            format!("private collision sentinel {index}"),
        )
        .unwrap();
        fs::set_permissions(collision_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let collision_before = sqlite_artifact_snapshot(&collision);

    let error = match SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        options().with_failpoint(Some(
            StoreFailpointV1::SchemaAfterPragmasAndTemporaryCleanup,
        )),
        UnixMillis::new(50),
    ) {
        Err(error) => error,
        Ok(store) => {
            drop(store);
            panic!("schema failpoint must reject after temporary creation and cleanup");
        }
    };
    assert_eq!(
        error,
        StoreErrorV1::PrimaryOperationAndCleanupFailureV1 {
            primary: Box::new(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Recovery,
            }),
            cleanup: Box::new(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Recovery,
            }),
        }
    );

    let owned_temporary = owned_temporary(&path, 50, 1);
    let owned_marker = sidecar(&owned_temporary, ".owner");
    let owned_paths = [
        owned_temporary.clone(),
        sidecar(&owned_temporary, "-wal"),
        sidecar(&owned_temporary, "-shm"),
        owned_marker.clone(),
    ];
    assert_private(&owned_temporary);
    assert_private(&owned_marker);
    assert_sqlite_artifact_snapshot(&collision, &collision_before);
    for collision_path in &collision_paths {
        assert_private(collision_path);
    }

    assert_eq!(fs::read(&regular_sentinel).unwrap(), regular_bytes);
    assert_eq!(
        fs::metadata(&regular_sentinel).unwrap().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read(&symlink_sentinel).unwrap(), symlink_target_bytes);
    assert_eq!(fs::metadata(&symlink_target).unwrap().mode() & 0o777, 0o400);
    assert!(
        fs::symlink_metadata(&symlink_sentinel)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::symlink_metadata(&symlink_sentinel).unwrap().mode() & 0o777,
        symlink_mode
    );

    let mut surviving_entries: Vec<_> = fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    surviving_entries.sort();
    let mut expected_entries: Vec<_> = collision_paths
        .iter()
        .map(|path| path.file_name().unwrap().to_os_string())
        .collect();
    expected_entries.extend([
        OsString::from("unrelated-regular"),
        OsString::from("unrelated-symlink"),
    ]);
    expected_entries.extend([
        owned_temporary.file_name().unwrap().to_os_string(),
        owned_marker.file_name().unwrap().to_os_string(),
    ]);
    expected_entries.sort();
    assert_eq!(surviving_entries, expected_entries);
    drop(
        SqliteStoreV1::open(&path, &root(), identity(), options(), UnixMillis::new(51))
            .expect("the next open must recover the authenticated residual temporary"),
    );
    for owned_path in owned_paths {
        assert!(matches!(
            fs::symlink_metadata(owned_path),
            Err(error) if error.kind() == ErrorKind::NotFound
        ));
    }
}

#[test]
fn new_state_and_sidecars_are_private_and_existing_permissions_are_not_rewritten() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let _store = open_store(&path).unwrap();
    assert_private(&path);
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar(&path, suffix);
        assert!(sidecar.exists());
        assert_private(&sidecar);
    }

    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(inspect(&path).is_ok());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o400);

    let database_bytes = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert_inspect_rejected_without_blocking(&path);
    assert_open_rejected_without_blocking(&path);
    assert_eq!(fs::read(&path).unwrap(), database_bytes);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar(&path, suffix);
        let sidecar_bytes = fs::read(&sidecar).unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o640)).unwrap();

        assert_inspect_rejected_without_blocking(&path);
        assert_open_rejected_without_blocking(&path);
        assert_eq!(fs::read(&sidecar).unwrap(), sidecar_bytes);
        assert_eq!(fs::metadata(&sidecar).unwrap().mode() & 0o777, 0o640);

        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
