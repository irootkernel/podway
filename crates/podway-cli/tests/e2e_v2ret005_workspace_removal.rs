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

fn initialize_git_worktree(root: &Path) {
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
            "initial",
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
        .expect("workspace-removal E2E daemon must start");
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

fn cli_json(
    cli: &Path,
    home: &Path,
    paths: &ServiceRuntimePathsV1,
    worktree: Option<&Path>,
    arguments: &[&str],
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut command = Command::new(cli);
        command
            .arg("--json")
            .arg("--socket")
            .arg(paths.socket_path().as_path())
            .env("PODWAY_TEST_ACCOUNT_ROOT", home);
        if let Some(worktree) = worktree {
            command.arg("--worktree").arg(worktree);
        }
        command.args(arguments);
        let output = command.output().expect("Podway CLI command must execute");
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("CLI response must be JSON");
        if output.status.success() {
            return response;
        }
        if response["code"] == "DAEMON_STARTING" && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        panic!(
            "Podway CLI command failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn real_cli_and_daemon_remove_only_the_confirmed_workspace_state() {
    let root = PathBuf::from(format!("/tmp/podway-v2ret005-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    let worktree = root.join("worktree");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    initialize_git_worktree(&worktree);
    let outside = worktree.join("preserved.txt");
    fs::write(&outside, "preserved\n").unwrap();

    let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw()).unwrap();
    let cli = Path::new(env!("CARGO_BIN_EXE_podway"));
    let daemon = cli.with_file_name("podwayd");
    let child = spawn_daemon(&daemon, &home, &paths);
    let initialized = cli_json(cli, &home, &paths, Some(&worktree), &["init"]);
    let workspace_uuid = initialized["workspace"]["uuid"]
        .as_str()
        .expect("workspace.init must return its UUID")
        .to_owned();
    fs::create_dir_all(worktree.join(".podway/custom")).unwrap();
    fs::write(worktree.join(".podway/custom/unknown"), "remove me\n").unwrap();

    let removed = cli_json(
        cli,
        &home,
        &paths,
        Some(&worktree),
        &[
            "--yes",
            "--if-workspace-uuid",
            &workspace_uuid,
            "workspace",
            "remove",
            "--force",
        ],
    );
    assert_eq!(
        removed["result"]["schema"],
        "podway.workspace-removal-result/v1"
    );
    assert_eq!(removed["result"]["workspace_uuid"], workspace_uuid);
    assert!(!worktree.join(".podway").exists());
    assert!(worktree.join(".git").exists());
    assert_eq!(fs::read_to_string(outside).unwrap(), "preserved\n");

    let replayed = cli_json(
        cli,
        &home,
        &paths,
        Some(&worktree),
        &[
            "--yes",
            "--if-workspace-uuid",
            &workspace_uuid,
            "workspace",
            "remove",
            "--force",
        ],
    );
    assert_eq!(
        replayed["result"]["schema"],
        "podway.workspace-removal-result/v1"
    );
    assert_eq!(replayed["result"]["workspace_uuid"], Value::Null);
    assert_eq!(replayed["result"]["registry_entry_removed"], false);
    assert_eq!(replayed["result"]["podway_directory_removed"], false);
    assert_eq!(replayed["result"]["already_absent"], true);
    assert!(!worktree.join(".podway").exists());
    assert!(worktree.join(".git").exists());
    assert_eq!(
        fs::read_to_string(worktree.join("preserved.txt")).unwrap(),
        "preserved\n"
    );

    stop_daemon(child);
    fs::remove_dir_all(root).unwrap();
}
