use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::{Pid, geteuid},
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::Value;

fn run(command: &mut Command, label: &str) -> std::process::Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label} must execute: {error}"));
    assert!(
        output.status.success(),
        "{label} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn initialize_git_worktree(root: &Path, message: &str) {
    run(
        Command::new("git").arg("init").arg("--quiet").arg(root),
        "git init",
    );
    run(
        Command::new("git").arg("-C").arg(root).args([
            "config",
            "user.email",
            "podway-tests@example.invalid",
        ]),
        "git email configuration",
    );
    run(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.name", "Podway Tests"]),
        "git name configuration",
    );
    run(
        Command::new("git").arg("-C").arg(root).args([
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            message,
        ]),
        "initial git commit",
    );
}

fn spawn_daemon(daemon: &Path, home: &Path, paths: &ServiceRuntimePathsV1) -> Child {
    let child = Command::new(daemon)
        .args(["--service", "--socket"])
        .arg(paths.socket_path().as_path())
        .env_clear()
        .env("PODWAY_TEST_ACCOUNT_ROOT", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("reset-all recovery E2E daemon must start");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !paths.socket_path().as_path().exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(paths.socket_path().as_path().exists());
    child
}

fn stop_daemon(child: Child) {
    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "daemon shutdown failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cli_output(
    cli: &Path,
    home: &Path,
    paths: &ServiceRuntimePathsV1,
    worktree: &Path,
    arguments: &[&str],
) -> (std::process::ExitStatus, Value) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = Command::new(cli)
            .arg("--json")
            .arg("--socket")
            .arg(paths.socket_path().as_path())
            .arg("--worktree")
            .arg(worktree)
            .args(arguments)
            .env("PODWAY_TEST_ACCOUNT_ROOT", home)
            .output()
            .expect("Podway CLI command must execute");
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("CLI response must be JSON");
        if response["code"] == "DAEMON_STARTING" && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        return (output.status, response);
    }
}

#[test]
fn confirmed_reset_all_recovers_after_git_identity_replacement() {
    let root = PathBuf::from(format!(
        "/tmp/podway-reset-all-recovery-e2e-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    let worktree = root.join("worktree");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    initialize_git_worktree(&worktree, "first identity");

    let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw()).unwrap();
    let cli = Path::new(env!("CARGO_BIN_EXE_podway"));
    let daemon = cli.with_file_name("podwayd");
    let child = spawn_daemon(&daemon, &home, &paths);
    let (status, initialized) = cli_output(cli, &home, &paths, &worktree, &["init"]);
    assert!(status.success(), "workspace.init failed: {initialized}");
    let old_workspace_uuid = initialized["workspace"]["uuid"]
        .as_str()
        .expect("workspace.init must return its UUID")
        .to_owned();
    let retained = worktree.join(".podway/retained-by-reset-all");
    fs::write(&retained, "retained\n").unwrap();
    stop_daemon(child);

    fs::remove_dir_all(worktree.join(".git")).unwrap();
    initialize_git_worktree(&worktree, "replacement identity");
    let child = spawn_daemon(&daemon, &home, &paths);
    let (status, rejected) = cli_output(cli, &home, &paths, &worktree, &["workspace", "show"]);
    assert!(!status.success());
    assert_eq!(rejected["code"], "WORKSPACE_ID_CONFLICT");

    let (status, reset) = cli_output(
        cli,
        &home,
        &paths,
        &worktree,
        &[
            "--if-workspace-uuid",
            &old_workspace_uuid,
            "--idempotency-key",
            "reset-all-detached-git-e2e",
            "reset",
            "--all",
            "--force",
            "--yes",
        ],
    );
    assert!(status.success(), "workspace.reset_all failed: {reset}");
    assert_eq!(reset["command"], "workspace.reset_all");
    let new_workspace_uuid = reset["workspace"]["uuid"]
        .as_str()
        .expect("workspace.reset_all must return its replacement UUID")
        .to_owned();
    assert_ne!(new_workspace_uuid, old_workspace_uuid);
    assert_eq!(fs::read_to_string(&retained).unwrap(), "retained\n");

    stop_daemon(child);
    let child = spawn_daemon(&daemon, &home, &paths);
    let (status, shown) = cli_output(cli, &home, &paths, &worktree, &["workspace", "show"]);
    assert!(status.success(), "workspace.show failed: {shown}");
    assert_eq!(shown["workspace"]["uuid"], new_workspace_uuid);

    stop_daemon(child);
    fs::remove_dir_all(root).unwrap();
}
