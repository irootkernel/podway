//! Process-level proof that `podwayd` binds the production endpoint and shuts down cleanly.

#![forbid(unsafe_code)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::{Pid, geteuid},
};
use podway_service::ServiceRuntimePathsV1;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

struct ProcessFixtureV1 {
    root: PathBuf,
    home: PathBuf,
    temporary: PathBuf,
    paths: ServiceRuntimePathsV1,
}

impl ProcessFixtureV1 {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(format!("/tmp/pdb-{}-{sequence}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("h");
        let temporary = root.join("t");
        let application_support = home.join("Library/Application Support/Podway");
        fs::create_dir_all(&application_support).expect("application support fixture must exist");
        fs::create_dir_all(&temporary).expect("temporary fixture must exist");
        make_private(&home);
        make_private(&temporary);
        make_private(&application_support);
        let paths = ServiceRuntimePathsV1::for_user(&home, &temporary, geteuid().as_raw())
            .expect("short fixture service paths must be valid");
        Self {
            root,
            home,
            temporary,
            paths,
        }
    }

    fn spawn(&self) -> Child {
        Command::new(env!("CARGO_BIN_EXE_podwayd"))
            .env("HOME", &self.home)
            .env("TMPDIR", &self.temporary)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("podwayd process must start")
    }
}

impl Drop for ProcessFixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn make_private(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("fixture directory must be private");
}

#[test]
fn podwayd_sigterm_drains_and_removes_its_owned_socket() {
    let fixture = ProcessFixtureV1::new();
    let mut child = fixture.spawn();
    let socket = fixture.paths.socket_path().as_path();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !socket.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("podwayd status must be observable") {
            let output = child
                .wait_with_output()
                .expect("terminated podwayd output must be readable");
            panic!(
                "podwayd exited before binding ({status}): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "podwayd must bind its production socket");

    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM)
        .expect("SIGTERM must be delivered to podwayd");
    let output = child
        .wait_with_output()
        .expect("podwayd shutdown output must be readable");
    assert!(
        output.status.success(),
        "podwayd must exit successfully after SIGTERM: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !socket.exists(),
        "graceful shutdown must remove only the socket owned by this process"
    );
}

#[test]
fn podwayd_service_and_version_modes_are_explicit() {
    let version = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .arg("--version")
        .output()
        .expect("podwayd version process must run");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).expect("version output is UTF-8"),
        format!("podwayd {}\n", env!("CARGO_PKG_VERSION"))
    );

    let invalid = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .arg("--unknown")
        .output()
        .expect("podwayd invalid-argument process must run");
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("usage: podwayd"));
}
