//! Tracked config initialization uses opaque bytes and descriptor-relative placement.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    ValidatedWorktreeV1, WORKTREE_SELECTOR_VERSION_V1, WorkspaceLayoutElementStatusV1,
    WorkspaceLayoutInitializerV1, WorktreeSelectorV1,
};
use tempfile::TempDir;

fn lossless(path: &Path) -> LosslessPathV1 {
    let display = DiagnosticPathDisplayV1::new("fixture").expect("fixture display");
    LosslessPathV1::from_raw_bytes(path.as_os_str().as_bytes(), display)
        .expect("temporary fixture path is absolute and canonical")
}

fn resolve(path: &Path) -> ValidatedWorktreeV1 {
    let path = fs::canonicalize(path).expect("canonical fixture selection");
    NativeGitResolverV1::new()
        .resolve(
            WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, lossless(&path))
                .expect("fixture selector"),
        )
        .expect("fixture worktree resolves")
}

fn create_main(root: &Path) {
    let administration = root.join(".git");
    fs::create_dir_all(administration.join("objects")).expect("objects directory");
    fs::create_dir_all(administration.join("refs")).expect("refs directory");
    fs::write(administration.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
}

fn temporary() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn admin_bytes(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if path.is_file() {
            output.insert(
                path.strip_prefix(root)
                    .expect("administration child")
                    .to_path_buf(),
                fs::read(path).expect("administration bytes"),
            );
            return;
        }
        for entry in fs::read_dir(path).expect("administration directory") {
            collect(root, &entry.expect("administration entry").path(), output);
        }
    }

    let mut output = BTreeMap::new();
    collect(path, path, &mut output);
    output
}

fn assert_no_layout_temporary_files(podway: &Path) {
    for entry in fs::read_dir(podway).expect("podway directory") {
        let name = entry.expect("podway entry").file_name();
        let bytes = name.as_os_str().as_bytes();
        assert!(
            !bytes.starts_with(b".podway-ignore-") && !bytes.starts_with(b".podway-config-"),
            "retained temporary layout entry: {name:?}"
        );
    }
}

#[test]
fn absent_config_uses_exact_opaque_bytes_and_preserves_git_administration() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let validated = resolve(&worktree);
    let admin_before = admin_bytes(&worktree.join(".git"));
    let default = vec![b'\xff'; 64 * 1024];

    let first = WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&validated, &default)
        .expect("first initialization");
    assert_eq!(
        first.config_file(),
        Some(WorkspaceLayoutElementStatusV1::Created)
    );
    assert_eq!(
        fs::read(worktree.join(".podway/config.yaml")).expect("created config"),
        default
    );
    assert_no_layout_temporary_files(&worktree.join(".podway"));

    let replay = WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&validated, b"different opaque defaults")
        .expect("replayed initialization");
    assert_eq!(
        replay.config_file(),
        Some(WorkspaceLayoutElementStatusV1::AlreadyValid)
    );
    assert_eq!(
        fs::read(worktree.join(".podway/config.yaml")).expect("replayed config"),
        default
    );
    assert_no_layout_temporary_files(&worktree.join(".podway"));
    assert_eq!(admin_before, admin_bytes(&worktree.join(".git")));
}

#[test]
fn existing_custom_config_bytes_and_permissions_are_preserved_idempotently() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let podway = worktree.join(".podway");
    fs::create_dir(&podway).expect("podway directory");
    let config = podway.join("config.yaml");
    let custom = b"not yaml \xff [preserve exactly]";
    fs::write(&config, custom).expect("custom config");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).expect("custom mode");
    let validated = resolve(&worktree);

    let defaults: [&[u8]; 2] = [b"first default", b"second default"];
    for default in defaults {
        let report = WorkspaceLayoutInitializerV1::new()
            .initialize_with_config(&validated, default)
            .expect("initialize existing config");
        assert_eq!(
            report.config_file(),
            Some(WorkspaceLayoutElementStatusV1::AlreadyValid)
        );
        assert_eq!(fs::read(&config).expect("custom config bytes"), custom);
        assert_eq!(
            fs::metadata(&config)
                .expect("custom config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
    assert_no_layout_temporary_files(&podway);
}

#[test]
fn invalid_default_config_bytes_fail_before_layout_mutation() {
    for default in [Vec::new(), vec![b'x'; 64 * 1024 + 1]] {
        let temporary = temporary();
        let worktree = temporary.path().join("worktree");
        create_main(&worktree);
        let validated = resolve(&worktree);
        let admin_before = admin_bytes(&worktree.join(".git"));

        assert!(
            WorkspaceLayoutInitializerV1::new()
                .initialize_with_config(&validated, &default)
                .is_err()
        );
        assert!(!worktree.join(".podway").exists());
        assert_eq!(admin_before, admin_bytes(&worktree.join(".git")));
    }
}

#[test]
fn config_conflicting_node_kinds_fail_without_layout_mutation() {
    for kind in ["symlink", "directory", "fifo", "socket"] {
        let temporary = temporary();
        let worktree = temporary.path().join(kind);
        create_main(&worktree);
        let podway = worktree.join(".podway");
        fs::create_dir(&podway).expect("podway directory");
        let config = podway.join("config.yaml");
        let outside = worktree.join("outside-config");
        let _socket = match kind {
            "symlink" => {
                fs::write(&outside, b"outside config").expect("outside config");
                symlink(&outside, &config).expect("config symlink");
                None
            }
            "directory" => {
                fs::create_dir(&config).expect("config directory");
                None
            }
            "fifo" => {
                let status = Command::new("mkfifo")
                    .arg(&config)
                    .status()
                    .expect("mkfifo must be available on Unix");
                assert!(status.success(), "mkfifo must create config fixture");
                None
            }
            "socket" => Some(UnixListener::bind(&config).expect("config socket")),
            _ => unreachable!("fixed node kind"),
        };
        let validated = resolve(&worktree);

        assert!(
            WorkspaceLayoutInitializerV1::new()
                .initialize_with_config(&validated, b"opaque default")
                .is_err(),
            "{kind} must be rejected"
        );
        assert!(!podway.join("procedures").exists());
        assert!(!podway.join("runtime").exists());
        assert!(!podway.join(".gitignore").exists());
        let file_type = fs::symlink_metadata(&config)
            .expect("conflicting config entry")
            .file_type();
        match kind {
            "symlink" => {
                assert!(file_type.is_symlink());
                assert_eq!(
                    fs::read(&outside).expect("outside config"),
                    b"outside config"
                );
            }
            "directory" => assert!(file_type.is_dir()),
            "fifo" => assert!(file_type.is_fifo()),
            "socket" => assert!(file_type.is_socket()),
            _ => unreachable!("fixed node kind"),
        }
        assert_no_layout_temporary_files(&podway);
    }
}
