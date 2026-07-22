//! Shared real-Git workspace fixtures for Phase 4 daemon resolution tests.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};

static TEMPORARY_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct TemporaryDirectoryV1 {
    path: PathBuf,
}

impl TemporaryDirectoryV1 {
    pub fn new(prefix: &str) -> Self {
        let parent = std::env::temp_dir();
        for _ in 0..1024 {
            let sequence = TEMPORARY_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{prefix}-{}-{sequence}", std::process::id(),));
            match fs::create_dir(&path) {
                Ok(()) => return Self { path },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    panic!("temporary workspace fixture directory must be created: {error}")
                }
            }
        }
        panic!("temporary workspace fixture directory names must not be exhausted");
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectoryV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub struct GitWorktreeFixtureV1 {
    temporary: TemporaryDirectoryV1,
    main: PathBuf,
    linked: PathBuf,
}

impl GitWorktreeFixtureV1 {
    pub fn main(&self) -> &Path {
        &self.main
    }

    pub fn linked(&self) -> &Path {
        &self.linked
    }

    pub fn temporary_path(&self) -> &Path {
        self.temporary.path()
    }
}

/// Creates a real non-bare Git worktree and a real linked worktree.
pub fn git_worktrees() -> GitWorktreeFixtureV1 {
    let temporary = TemporaryDirectoryV1::new("podway-daemon-phase4-workspace");
    let main = temporary.path().join("main");
    let linked = temporary.path().join("linked");

    let mut initialize = Command::new("git");
    initialize.arg("init").arg("--quiet").arg(&main);
    run_git(&mut initialize, "initialize non-bare main worktree");

    let mut email = Command::new("git");
    email
        .arg("-C")
        .arg(&main)
        .arg("config")
        .arg("user.email")
        .arg("podway-tests@example.invalid");
    run_git(&mut email, "configure fixture author email");

    let mut name = Command::new("git");
    name.arg("-C")
        .arg(&main)
        .arg("config")
        .arg("user.name")
        .arg("Podway Tests");
    run_git(&mut name, "configure fixture author name");

    let mut commit = Command::new("git");
    commit
        .arg("-C")
        .arg(&main)
        .arg("commit")
        .arg("--quiet")
        .arg("--allow-empty")
        .arg("-m")
        .arg("initial fixture commit");
    run_git(&mut commit, "create fixture commit for linked worktree");

    let mut add_linked = Command::new("git");
    add_linked
        .arg("-C")
        .arg(&main)
        .arg("worktree")
        .arg("add")
        .arg("--quiet")
        .arg("-b")
        .arg("linked")
        .arg(&linked);
    run_git(&mut add_linked, "create linked worktree");

    prepare_runtime(&main);
    prepare_runtime(&linked);

    GitWorktreeFixtureV1 {
        temporary,
        main,
        linked,
    }
}

pub fn prepare_runtime(root: &Path) {
    fs::create_dir_all(root.join(".podway/runtime"))
        .expect("worktree-local runtime directory must be created");
}

pub fn selector(path: &Path) -> WorktreeSelectorV1 {
    let canonical = fs::canonicalize(path).expect("fixture selector path must be canonical");
    let display = DiagnosticPathDisplayV1::new("phase4 workspace fixture")
        .expect("fixed fixture display is valid");
    #[cfg(unix)]
    let lossless = LosslessPathV1::from_raw_bytes(canonical.as_os_str().as_bytes(), display)
        .expect("fixture selector path must be an absolute canonical Unix path");
    #[cfg(not(unix))]
    let lossless = {
        let _ = display;
        panic!("phase 4 workspace fixtures require Unix native paths")
    };
    WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, lossless)
        .expect("fixture selector must use the supported version")
}

pub fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).expect("copied workspace root must be created");
    copy_tree_contents(source, destination);
}

fn copy_tree_contents(source: &Path, destination: &Path) {
    for entry in fs::read_dir(source).expect("workspace copy source must be readable") {
        let entry = entry.expect("workspace copy directory entry must be readable");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .expect("workspace copy file type must be readable");
        if file_type.is_dir() {
            fs::create_dir(&destination_path).expect("workspace copy directory must be created");
            copy_tree_contents(&source_path, &destination_path);
            fs::set_permissions(
                &destination_path,
                fs::metadata(&source_path)
                    .expect("workspace copy source directory metadata must be readable")
                    .permissions(),
            )
            .expect("workspace copy directory permissions must be preserved");
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).expect("workspace copy file must be copied");
            fs::set_permissions(
                &destination_path,
                fs::metadata(&source_path)
                    .expect("workspace copy source file metadata must be readable")
                    .permissions(),
            )
            .expect("workspace copy file permissions must be preserved");
        } else {
            panic!("workspace copy fixture does not support non-file entries");
        }
    }
    #[cfg(unix)]
    fs::set_permissions(
        destination,
        fs::metadata(source)
            .expect("workspace copy source directory metadata must be readable")
            .permissions(),
    )
    .expect("workspace copy root permissions must be preserved");
}

pub fn read_file(path: &Path) -> Vec<u8> {
    fs::read(path).expect("fixture file must be readable")
}

#[cfg(unix)]
pub fn non_utf8_child_path(parent: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;

    let mut bytes = parent.as_os_str().as_bytes().to_vec();
    if bytes != b"/" {
        bytes.push(b'/');
    }
    bytes.extend_from_slice(b"non-utf8-");
    bytes.push(0xff);
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

fn run_git(command: &mut Command, action: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("Git must be available to {action}: {error}"));
    assert!(status.success(), "Git must {action}");
}
