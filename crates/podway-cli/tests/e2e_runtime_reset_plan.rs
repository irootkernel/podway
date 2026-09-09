#![cfg(debug_assertions)]

use nix::unistd::geteuid;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            Path::new("/private/tmp").join(format!("pw-rc-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self(root);
        let launchctl = fixture.0.join("launchctl");
        fs::write(&launchctl, format!("#!/bin/sh\n[ \"$1\" = print ] || exit 99\nprintf 'Bad request.\\nCould not find service \"dev.podway.podwayd\" in domain for user gui: {}\\n' >&2\nexit 113\n", geteuid().as_raw())).unwrap();
        fs::set_permissions(launchctl, fs::Permissions::from_mode(0o700)).unwrap();
        fixture
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_podway"));
        command
            .env_clear()
            .current_dir(&self.0)
            .env("PODWAY_TEST_ACCOUNT_ROOT", &self.0)
            .env("PODWAY_TEST_LAUNCHCTL", self.0.join("launchctl"))
            .env("HOME", self.0.join("ignored-home"))
            .env("TMPDIR", self.0.join("ignored-temp"));
        command
    }
    fn run(&self, args: &[&str]) -> (Output, Value) {
        let output = self.command().arg("--json").args(args).output().unwrap();
        let value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {:?}", output));
        (output, value)
    }
    fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(relative);
        let mut parent = self.0.clone();
        for component in Path::new(relative).parent().unwrap().components() {
            parent.push(component);
            if !parent.exists() {
                fs::create_dir(&parent).unwrap();
            }
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Daemon(std::process::Child);
impl Daemon {
    fn start(fixture: &Fixture) -> Self {
        let daemon = Path::new(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd");
        let child = Command::new(daemon)
            .env_clear()
            .env("PODWAY_TEST_ACCOUNT_ROOT", &fixture.0)
            .args(["--mode", "dev"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(fs::File::create(fixture.0.join("daemon-stderr")).unwrap())
            .spawn()
            .unwrap();
        let mut daemon = Self(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let (output, status) = fixture.run(&["--mode", "dev", "daemon", "status"]);
            if output.status.success() && status["result"]["readiness_state"] == "ready" {
                assert_eq!(
                    status["result"]["pid"].as_u64(),
                    Some(u64::from(daemon.0.id()))
                );
                assert_eq!(
                    status["result"]["effective_socket_path"].as_str(),
                    fixture
                        .0
                        .join(".podway/modes/dev/run/podwayd.sock")
                        .to_str()
                );
                return daemon;
            }
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "daemon exited before readiness: {status}"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "daemon readiness timed out: {status}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn await_exit(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "reset shutdown failed: {status}");
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reset shutdown did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while self.0.try_wait().ok().flatten().is_none() && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
}

#[test]
fn absent_plan_needs_no_worktree_or_state_creation() {
    let fixture = Fixture::new();
    let before = fs::metadata(&fixture.0).unwrap();
    let (output, value) = fixture.run(&["runtime", "reset", "plan", "--all-modes"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["schema"], "podway.output/v3");
    assert_eq!(value["command"], "runtime.reset.plan");
    assert_eq!(value["result"]["status"], "no_change");
    assert!(value["result"]["plan_token"].is_null());
    assert!(value.get("workspace").is_none());
    assert!(!fixture.0.join(".podway").exists());
    assert!(!fixture.0.join("ignored-home").exists());
    assert_eq!(
        fs::metadata(&fixture.0).unwrap().mtime_nsec(),
        before.mtime_nsec()
    );
}

#[test]
fn live_idle_task_reserves_then_stops_only_after_durable_intent_and_blocks_restart() {
    use podway_protocol::{
        ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
        RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, decode_base64url_unpadded_v1,
        decode_response_payload_v2, encode_request_payload_v1, read_frame_v1, write_frame_v1,
    };
    use podway_service::{PodwayHomeV1, RuntimeResetLockV1, RuntimeResetPathV1};
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let mut daemon = Daemon::start(&fixture);
    let worktree = fixture.0.join("worktree");
    assert!(
        Command::new("/usr/bin/git")
            .args(["init", "--quiet"])
            .arg(&worktree)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&worktree)
            .args([
                "-c",
                "user.name=Podway Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "--quiet",
                "-m",
                "fixture"
            ])
            .status()
            .unwrap()
            .success()
    );
    let scope = worktree.to_str().unwrap();
    for command in [
        vec!["init"],
        vec![
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "Idle reset participant",
        ],
        vec!["begin"],
    ] {
        let mut args = vec!["--mode", "dev", "--worktree", scope];
        args.extend(command);
        let (output, value) = fixture.run(&args);
        assert!(output.status.success(), "{args:?}: {value}");
    }
    let (output, before) = fixture.run(&["--mode", "dev", "--worktree", scope, "status"]);
    assert!(output.status.success(), "{before}");
    let (output, plan) = fixture.run(&["--mode", "dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{plan}");
    assert_eq!(plan["result"]["status"], "ready", "{plan}");
    assert_eq!(plan["result"]["targets"][0]["state"], "live_idle");
    let token = plan["result"]["plan_token"].as_str().unwrap();
    let decoded: Value = serde_json::from_slice(
        &decode_base64url_unpadded_v1(token.split_once('.').unwrap().0).unwrap(),
    )
    .unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let digest = format!("sha256:{:x}", Sha256::digest(token.as_bytes()));
    let (_, status) = fixture.run(&["--mode", "dev", "daemon", "status"]);
    let process_id = status["result"]["process_id"].as_str().unwrap();
    let namespace = fixture.0.join(".podway/modes/dev");
    let socket = namespace.join("run/podwayd.sock");
    let mut stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut control = |action: &str, reservation: Option<&str>| {
        let request = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
            request_id: RequestIdV1::new(uuid::Uuid::new_v4().to_string()).unwrap(),
            client: ClientInfoV1::new("podway", env!("CARGO_PKG_VERSION"), std::process::id()).unwrap(),
            operation: OperationV1::Control, command: CommandNameV1::new("daemon.runtime_reset").unwrap(),
            workspace: None, idempotency_key: None, preconditions: PreconditionsV1::default(), options: RequestOptionsV1::new(false, 0).unwrap(),
            payload: serde_json::json!({
                "schema": "podway.runtime-reset-control-input/v1", "action": action, "mode": "dev",
                "namespace_root": RuntimeResetPathV1::new(&namespace).unwrap(), "expected_process_id": process_id,
                "operation_id": operation, "token_sha256": digest, "reservation_id": reservation,
            }).as_object().unwrap().clone(),
        }).unwrap();
        write_frame_v1(&mut stream, &encode_request_payload_v1(&request).unwrap()).unwrap();
        let bytes = read_frame_v1(&mut stream).unwrap().unwrap();
        decode_response_payload_v2(&bytes).unwrap();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response["request_id"], request.request_id().as_str());
        response
    };
    let reserved = control("reserve", None);
    assert_eq!(reserved["result"]["state"], "reserved", "{reserved}");
    let reservation = reserved["result"]["reservation_id"].as_str().unwrap();
    let (output, identity) = fixture.run(&["--mode", "dev", "daemon", "status"]);
    assert!(output.status.success(), "{identity}");
    assert_eq!(identity["result"]["process_id"], process_id);
    let (output, refused) = fixture.run(&["--mode", "dev", "terminate"]);
    assert!(!output.status.success(), "{refused}");
    assert_eq!(refused["code"], "DAEMON_SHUTTING_DOWN");
    assert_eq!(
        refused["message"],
        "Daemon is draining and not accepting work."
    );
    let (output, refused) = fixture.run(&["--mode", "dev", "--worktree", scope, "status"]);
    assert!(!output.status.success(), "{refused}");
    assert_eq!(refused["code"], "DAEMON_SHUTTING_DOWN");
    assert_eq!(
        refused["message"],
        "Daemon is draining and not accepting work."
    );
    let home = PodwayHomeV1::from_account_home(&fixture.0, geteuid().as_raw()).unwrap();
    let commit = RuntimeResetLockV1::acquire_commit(&home).unwrap();
    assert_eq!(
        control("snapshot", Some(reservation))["result"]["state"],
        "reserved"
    );
    let record = serde_json::json!({
        "schema": "podway.runtime-reset-record/v1", "operation_id": operation,
        "token_sha256": digest, "phase": "in_progress", "plan": decoded,
        "modes": [{"mode": "dev", "phase": "stop_intended", "reservation_id": reservation, "entries": []}], "result": null,
    });
    let record_path = fixture.file(
        ".podway/maintenance/runtime-reset.json",
        &serde_json::to_vec(&record).unwrap(),
    );
    fs::File::open(&record_path).unwrap().sync_all().unwrap();
    fs::File::open(record_path.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
    drop(commit);
    assert_eq!(
        control("snapshot", Some(reservation))["result"]["state"],
        "committed"
    );
    let stopped = control("shutdown", Some(reservation));
    assert_eq!(stopped["result"]["state"], "shutting_down", "{stopped}");
    daemon.await_exit();
    assert!(!socket.exists());
    let log = namespace.join("logs/podwayd-bootstrap.log");
    let log_before = fs::read(&log).unwrap();
    let output = Command::new(Path::new(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd"))
        .env_clear()
        .env("PODWAY_TEST_ACCOUNT_ROOT", &fixture.0)
        .args(["--mode", "dev"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!socket.exists());
    assert_eq!(fs::read(&log).unwrap(), log_before);
    let account_alias = fixture.0.join("account-alias");
    symlink(&fixture.0, &account_alias).unwrap();
    for explicit_root in [
        namespace.clone(),
        Path::new("/tmp").join(namespace.strip_prefix("/private/tmp").unwrap()),
        account_alias.join(".podway/modes/dev"),
    ] {
        let mut explicit = Daemon(
            Command::new(Path::new(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd"))
                .env_clear()
                .env("PODWAY_TEST_ACCOUNT_ROOT", &fixture.0)
                .env("PODWAY_DEV_HOME", explicit_root)
                .args(["--mode", "dev"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = explicit.0.try_wait().unwrap() {
                assert!(!status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "explicit ordinary namespace bypassed reset startup gate"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!socket.exists());
        assert_eq!(fs::read(&log).unwrap(), log_before);
    }
    assert!(worktree.join(".podway/runtime").exists());
    let (output, recovery) = fixture.run(&["--mode", "dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{recovery}");
    assert_eq!(recovery["result"]["status"], "recovery_required");
    assert_eq!(
        recovery["result"]["recovery_operation"]["operation_id"],
        operation
    );
}

#[test]
fn corrupt_offline_registry_yields_bounded_token_without_repair() {
    let fixture = Fixture::new();
    let lock = fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
    let registry = fixture.file(".podway/modes/dev/state/workspaces.json", b"broken-json");
    let inode = fs::metadata(&lock).unwrap().ino();
    let (output, value) = fixture.run(&["--dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["result"]["status"], "ready");
    assert_eq!(value["result"]["targets"][0]["state"], "offline");
    let token = value["result"]["plan_token"].as_str().unwrap();
    assert!(token.len() <= 262144);
    assert!(value["result"]["expires_at"].is_string());
    assert_eq!(fs::read(&registry).unwrap(), b"broken-json");
    assert_eq!(fs::metadata(lock).unwrap().ino(), inode);
    assert!(!fixture.0.join(".podway/maintenance").exists());
}

#[test]
fn selectors_and_daemon_flags_fail_before_discovery() {
    let fixture = Fixture::new();
    for args in [
        vec!["runtime", "reset", "plan"],
        vec!["--dev", "runtime", "reset", "plan", "--all-modes"],
        vec!["--mode", "dev", "--mode", "qa", "runtime", "reset", "plan"],
        vec!["--dev", "--dev", "runtime", "reset", "plan"],
        vec!["--dev", "runtime", "reset", "plan", "--worktree", "/tmp"],
        vec![
            "--dev",
            "runtime",
            "reset",
            "plan",
            "--socket",
            "/tmp/unused",
        ],
        vec!["--dev", "runtime", "reset", "plan", "--yes"],
        vec![
            "--dev",
            "runtime",
            "reset",
            "plan",
            "--if-goal-revision",
            "1",
        ],
        vec!["--dev", "runtime", "reset", "plan", "--detach"],
    ] {
        let (output, value) = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {value}");
        assert_eq!(value["code"], "REQUEST_INVALID", "{value}");
    }
    let output = fixture
        .command()
        .env("PODWAY_DEV_HOME", "")
        .args(["--dev", "runtime", "reset", "plan"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!fixture.0.join(".podway").exists());
}

#[test]
fn apply_requires_its_exact_selector_confirmation_and_single_mode_scope() {
    let fixture = Fixture::new();
    for args in [
        vec![
            "runtime",
            "reset",
            "apply",
            "--plan-token",
            "token",
            "--yes",
        ],
        vec![
            "--dev",
            "runtime",
            "reset",
            "apply",
            "--all-modes",
            "--plan-token",
            "token",
            "--yes",
        ],
        vec![
            "--dev",
            "runtime",
            "reset",
            "apply",
            "--plan-token",
            "token",
            "--yes",
            "--worktree",
            "/tmp",
        ],
    ] {
        let (output, value) = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {value}");
        assert_eq!(value["code"], "REQUEST_INVALID", "{value}");
    }

    let (output, value) = fixture.run(&[
        "--dev",
        "runtime",
        "reset",
        "apply",
        "--plan-token",
        "token",
    ]);
    assert_eq!(output.status.code(), Some(2), "{value}");
    assert_eq!(value["code"], "CONFIRMATION_REQUIRED", "{value}");

    let (output, value) = fixture.run(&[
        "runtime",
        "reset",
        "apply",
        "--all-modes",
        "--plan-token",
        "token",
        "--yes",
    ]);
    assert_eq!(output.status.code(), Some(3), "{value}");
    assert_eq!(value["code"], "RUNTIME_RESET_UNSUPPORTED", "{value}");
    assert!(!fixture.0.join(".podway").exists());

    let (output, value) = fixture.run(&["reset", "--all", "--yes"]);
    assert_eq!(output.status.code(), Some(2), "{value}");
    assert_eq!(value["code"], "REQUEST_INVALID", "{value}");
    assert!(value["message"].as_str().unwrap().contains("--force"));
}

#[test]
fn unsafe_layout_is_reported_without_following_target() {
    let fixture = Fixture::new();
    fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
    let outside = fixture.file("outside/keep", b"keep");
    symlink(
        fixture.0.join("outside"),
        fixture.0.join(".podway/modes/dev/state"),
    )
    .unwrap();
    let (output, value) = fixture.run(&["--mode", "dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["result"]["status"], "blocked");
    assert_eq!(value["result"]["targets"][0]["state"], "unsafe");
    assert!(value["result"]["plan_token"].is_null());
    assert_eq!(fs::read(outside).unwrap(), b"keep");
}

#[test]
fn account_discovery_errors_keep_catalogued_json_exit_codes() {
    let fixture = Fixture::new();
    symlink("missing", fixture.0.join(".podway")).unwrap();
    let (output, value) = fixture.run(&["runtime", "reset", "plan", "--all-modes"]);
    assert_eq!(output.status.code(), Some(5), "{value}");
    assert_eq!(value["code"], "RUNTIME_RESET_UNSAFE");
    assert_eq!(
        value["details"]["schema"],
        "podway.runtime-reset-error-details/v1"
    );
    fs::remove_file(fixture.0.join(".podway")).unwrap();
    for index in 0..64 {
        fixture.file(&format!(".podway/modes/m{index}/run/podwayd.lock"), b"");
    }
    let (output, value) = fixture.run(&["runtime", "reset", "plan", "--all-modes"]);
    assert_eq!(output.status.code(), Some(2), "{value}");
    assert_eq!(value["code"], "RUNTIME_RESET_LIMIT_EXCEEDED");
    assert!(value["details"]["retry"].is_null());
}

#[test]
fn failed_peer_exchange_is_unsafe_instead_of_unsupported() {
    use std::{os::unix::net::UnixListener, thread};

    for (responds, state, reason) in [
        (false, "unsafe", "io"),
        (true, "unsupported", "unsupported_peer"),
    ] {
        let fixture = Fixture::new();
        fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
        fixture.file(".podway/modes/dev/state/workspaces.json", b"broken-json");
        let socket = fixture.0.join(".podway/modes/dev/run/podwayd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let server = thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "peer probe did not connect"
                        );
                        thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let request: Value = serde_json::from_slice(
                &podway_protocol::read_single_frame_v1(&mut stream)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(request["command"], "daemon.status");
            if responds {
                stream
                    .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let response = serde_json::json!({
                    "schema": "podway.error/v1", "request_id": request["request_id"],
                    "command": "daemon.status", "generated_at": "2026-07-15T12:34:56.789Z",
                    "code": "DAEMON_UNAVAILABLE", "message": "daemon is restarting",
                    "retryable": true, "exit_code": 3,
                    "details": {"schema": "podway.endpoint-error-details/v2", "recovery": {
                        "action": "inspect_daemon", "command": "daemon.status",
                        "argv": ["podway", "--json", "daemon", "status"],
                        "reason": "Inspect daemon and contract health before deciding on a lifecycle action.", "requires_explicit_authorization": false
                    }}
                });
                let response = serde_json::to_vec(&response).unwrap();
                podway_protocol::decode_response_payload_v2(&response).unwrap();
                podway_protocol::write_frame_v1(&mut stream, &response).unwrap();
            }
        });
        let (output, value) = fixture.run(&["--dev", "runtime", "reset", "plan"]);
        server.join().unwrap();
        assert!(output.status.success(), "{value}");
        assert_eq!(value["result"]["status"], "blocked");
        assert_eq!(value["result"]["targets"][0]["state"], state);
        assert_eq!(value["result"]["targets"][0]["reason"], reason);
        assert!(value["result"]["plan_token"].is_null());
    }
}

#[test]
fn inspect_response_classification_is_fail_closed() {
    use std::{os::unix::net::UnixListener, thread};

    for (state, reason, identity_matches, error, expected_state, expected_reason) in [
        (
            "unsupported",
            None,
            true,
            None,
            "unsupported",
            "unsupported_peer",
        ),
        ("busy", Some("activity"), true, None, "busy", "activity"),
        (
            "idle",
            Some("activity"),
            true,
            None,
            "unsafe",
            "unknown_activity",
        ),
        ("idle", None, false, None, "unsafe", "identity_changed"),
        (
            "idle",
            None,
            true,
            Some(("RUNTIME_RESET_UNSUPPORTED", Some("managed_runtime"))),
            "unsupported",
            "unsupported_peer",
        ),
        (
            "idle",
            None,
            true,
            Some(("RUNTIME_RESET_UNSAFE", Some("unsafe_path"))),
            "unsafe",
            "unsafe_path",
        ),
        (
            "idle",
            None,
            true,
            Some(("RUNTIME_RESET_BUSY", Some("activity"))),
            "unsafe",
            "unknown_activity",
        ),
    ] {
        let fixture = Fixture::new();
        let lock = fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
        let _lock = nix::fcntl::Flock::lock(
            fs::File::open(lock).unwrap(),
            nix::fcntl::FlockArg::LockExclusiveNonblock,
        )
        .unwrap();
        fixture.file(".podway/modes/dev/state/workspaces.json", b"broken-json");
        let socket = fixture.0.join(".podway/modes/dev/run/podwayd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let namespace = fixture.0.join(".podway/modes/dev");
        let process_id = uuid::Uuid::new_v4().to_string();
        let response_process_id = process_id.clone();
        let server = thread::spawn(move || {
            let identity = podway_protocol::build_identity_v1();
            for exchange in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let request: Value = serde_json::from_slice(
                    &podway_protocol::read_single_frame_v1(&mut stream)
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
                let result = if exchange == 0 {
                    assert_eq!(request["command"], "daemon.status");
                    serde_json::json!({
                        "schema": "podway.daemon-status-result/v1",
                        "product": identity.product(),
                        "daemon_version": identity.version(),
                        "target": identity.target(),
                        "build_identity": identity.build_identity(),
                        "source_commit": identity.source_commit(),
                        "contract_manifest_schema": identity.contract_manifest_schema(),
                        "contract_manifest_digest": identity.contract_manifest_digest(),
                        "protocol_versions": identity.supported_ipc_ids(),
                        "pid": 42,
                        "process_id": if identity_matches {
                            response_process_id.clone()
                        } else {
                            uuid::Uuid::new_v4().to_string()
                        },
                        "executable_path": "/fixture/podwayd",
                        "started_at": "2026-07-15T12:34:56.789Z",
                        "uptime_ms": 1000,
                        "configured_socket_path": socket,
                        "effective_socket_path": socket,
                    })
                } else {
                    assert_eq!(request["command"], "daemon.runtime_reset");
                    serde_json::json!({
                        "schema": "podway.runtime-reset-control-result/v1",
                        "state": state,
                        "mode": "dev",
                        "namespace_root": podway_service::RuntimeResetPathV1::new(&namespace).unwrap(),
                        "process_id": response_process_id,
                        "operation_id": null,
                        "token_sha256": null,
                        "reservation_id": null,
                        "reason": reason,
                    })
                };
                let response = if exchange == 1 {
                    if let Some((code, error_reason)) = error {
                        serde_json::json!({
                            "schema": "podway.error/v1",
                            "request_id": request["request_id"],
                            "command": request["command"],
                            "generated_at": "2026-07-15T12:34:56.789Z",
                            "code": code,
                            "message": "fixture reset control error",
                            "retryable": false,
                            "exit_code": if code == "RUNTIME_RESET_UNSUPPORTED" { 3 } else if code == "RUNTIME_RESET_BUSY" { 4 } else { 5 },
                            "details": {
                                "schema": "podway.runtime-reset-error-details/v1",
                                "mode": "dev",
                                "reason": error_reason,
                                "result": null,
                                "retry": null,
                            },
                        })
                    } else {
                        serde_json::json!({
                            "schema": "podway.output/v3",
                            "request_id": request["request_id"],
                            "command": request["command"],
                            "generated_at": "2026-07-15T12:34:56.789Z",
                            "result": result,
                            "warnings": [],
                        })
                    }
                } else {
                    serde_json::json!({
                        "schema": "podway.output/v3",
                        "request_id": request["request_id"],
                        "command": request["command"],
                        "generated_at": "2026-07-15T12:34:56.789Z",
                        "result": result,
                        "warnings": [],
                    })
                };
                let response = serde_json::to_vec(&response).unwrap();
                podway_protocol::write_frame_v1(&mut stream, &response).unwrap();
            }
        });
        let (output, value) = fixture.run(&["--dev", "runtime", "reset", "plan"]);
        server.join().unwrap();
        assert!(output.status.success(), "{value}");
        assert_eq!(value["result"]["status"], "blocked");
        assert_eq!(
            value["result"]["targets"][0]["state"], expected_state,
            "{value}"
        );
        assert_eq!(
            value["result"]["targets"][0]["reason"], expected_reason,
            "{value}"
        );
        assert!(value["result"]["plan_token"].is_null());
    }
}

#[test]
fn on_disk_recovery_records_control_plans_without_mutation() {
    use podway_core::canonicalize_json_v1;
    use podway_protocol::encode_base64url_unpadded_v1;
    use podway_service::RuntimeResetPathV1;
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new();
    for name in [
        "runtime-reset.lock",
        "runtime-start.lock",
        "runtime-reset-commit.lock",
    ] {
        fixture.file(&format!(".podway/maintenance/{name}"), b"");
    }
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/compatibility/runtime-reset-contract-reservation.json"
    ))
    .unwrap();
    let mut record = fixtures["fixtures"]["podway.runtime-reset-record/v1"].clone();
    let root = fixture.0.join(".podway");
    let path = serde_json::to_value(RuntimeResetPathV1::new(&root).unwrap()).unwrap();
    let metadata = fs::metadata(&root).unwrap();
    let identity = serde_json::json!({
        "device": metadata.dev(), "inode": metadata.ino(), "uid": metadata.uid(),
        "mode": metadata.mode(), "links": metadata.nlink(),
    });
    record["plan"]["caller_uid"] = serde_json::json!(geteuid().as_raw());
    record["plan"]["account_root"] = path.clone();
    record["plan"]["account_identity"] = identity.clone();
    record["plan"]["targets"][0]["root"] = path;
    record["plan"]["targets"][0]["root_identity"] = identity;
    let bytes = canonicalize_json_v1(&record["plan"]).unwrap();
    let token = format!(
        "{}.{:x}",
        encode_base64url_unpadded_v1(bytes.as_bytes()),
        Sha256::digest(bytes.as_bytes())
    );
    record["token_sha256"] =
        serde_json::json!(format!("sha256:{:x}", Sha256::digest(token.as_bytes())));
    let record_path = fixture.file(
        ".podway/maintenance/runtime-reset.json",
        &serde_json::to_vec(&record).unwrap(),
    );
    let before = fs::read(&record_path).unwrap();
    let inode = fs::metadata(&record_path).unwrap().ino();
    let (output, value) = fixture.run(&["--mode", "prod", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["result"]["status"], "recovery_required");
    assert_eq!(
        value["result"]["recovery_operation"]["operation_id"],
        record["operation_id"]
    );
    assert_eq!(
        value["result"]["recovery_operation"]["token_sha256"],
        record["token_sha256"]
    );
    assert!(value["result"]["plan_token"].is_null());
    assert_eq!(
        value["result"]["preserved"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["class"] == "control_residue")
            .count(),
        4
    );
    assert_eq!(fs::read(&record_path).unwrap(), before);
    assert_eq!(fs::metadata(&record_path).unwrap().ino(), inode);
    let human = fixture
        .command()
        .args(["--mode", "prod", "runtime", "reset", "plan"])
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("Runtime reset plan: recovery_required"));
    assert!(text.contains(&format!(
        "Recovery required: {}",
        record["operation_id"].as_str().unwrap()
    )));
    assert!(!text.contains("Plan token:"));

    for args in [
        vec!["daemon", "install"],
        vec!["daemon", "start"],
        vec!["daemon", "restart"],
        vec!["daemon", "uninstall", "--yes"],
    ] {
        let (output, refused) = fixture.run(&args);
        assert_eq!(output.status.code(), Some(4), "{args:?}: {refused}");
        assert_eq!(refused["code"], "RUNTIME_RESET_IN_PROGRESS");
        assert_eq!(
            refused["message"],
            "runtime reset is in progress and fences the service lifecycle"
        );
        assert_eq!(
            refused["details"]["schema"],
            "podway.runtime-reset-error-details/v1"
        );
        assert_eq!(refused["details"]["reason"], "operation_in_progress");
        assert_eq!(refused["details"]["mode"], "prod");
        assert!(refused["details"]["retry"].is_null());
        podway_protocol::decode_response_payload_v2(&output.stdout).unwrap();
        assert_eq!(fs::read(&record_path).unwrap(), before);
        assert_eq!(fs::metadata(&record_path).unwrap().ino(), inode);
        assert!(!fixture.0.join("Library").exists());
    }

    record["phase"] = serde_json::json!("completed");
    record["plan"] = Value::Null;
    record["modes"] = serde_json::json!([]);
    record["result"] = fixtures["fixtures"]["podway.runtime-reset-result/v1"].clone();
    let completed = serde_json::to_vec(&record).unwrap();
    fs::write(&record_path, &completed).unwrap();
    let (output, value) = fixture.run(&["--mode", "prod", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["result"]["status"], "no_change");
    assert!(value["result"]["recovery_operation"].is_null());
    assert!(value["result"]["plan_token"].is_null());
    assert_eq!(fs::read(&record_path).unwrap(), completed);
    let (output, allowed) = fixture.run(&["daemon", "uninstall", "--yes"]);
    assert!(output.status.success(), "{allowed}");
    assert_eq!(fs::read(&record_path).unwrap(), completed);
    fixture.file(".podway/run/podwayd.lock", b"");
    let registry = fixture.file(".podway/state/workspaces.json", b"recreated registry");
    let (output, value) = fixture.run(&["--mode", "prod", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["result"]["status"], "ready");
    assert!(value["result"]["plan_token"].is_string());
    assert_eq!(fs::read(&registry).unwrap(), b"recreated registry");
    assert_eq!(fs::read(&record_path).unwrap(), completed);

    fs::write(&record_path, b"{corrupt}").unwrap();
    let (output, refused) = fixture.run(&["daemon", "start"]);
    assert_eq!(output.status.code(), Some(5), "{refused}");
    assert_eq!(refused["code"], "RUNTIME_RESET_UNSAFE");
    assert_eq!(
        refused["message"],
        "runtime reset found an unsafe service lifecycle path"
    );
    assert_eq!(refused["details"]["reason"], "unsafe_path");
    let (output, value) = fixture.run(&["--mode", "prod", "runtime", "reset", "plan"]);
    assert_eq!(output.status.code(), Some(5), "{value}");
    assert_eq!(value["code"], "RUNTIME_RESET_UNSAFE");
    assert_eq!(fs::read(&record_path).unwrap(), b"{corrupt}");
    assert_eq!(fs::metadata(&record_path).unwrap().ino(), inode);
}

#[test]
fn human_plans_disclose_status_preservation_and_token_availability() {
    let fixture = Fixture::new();
    for status in ["no_change", "ready", "blocked"] {
        match status {
            "ready" => {
                fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
                fixture.file(
                    ".podway/modes/dev/state/workspaces.json",
                    b"corrupt registry",
                );
            }
            "blocked" => {
                fixture.file(".podway/modes/dev/state/unrelated", b"keep");
            }
            _ => {}
        }
        let output = fixture
            .command()
            .args(["--dev", "runtime", "reset", "plan"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains(&format!("Runtime reset plan: {status}")),
            "{text}"
        );
        assert!(
            text.contains("exclude: external managed runtimes"),
            "{text}"
        );
        assert_eq!(text.contains("Plan token:"), status == "ready", "{text}");
        if status == "ready" {
            assert!(
                text.contains("remove ") && text.contains("workspaces.json"),
                "{text}"
            );
            assert!(
                text.contains("preserve ") && text.contains("podwayd.lock"),
                "{text}"
            );
            assert!(text.contains("Expires:"), "{text}");
            assert!(text.contains("(offline)"), "{text}");
            assert!(text.contains("remove registry:"), "{text}");
        } else if status == "blocked" {
            assert!(text.contains("blocked: unexpected_entry"), "{text}");
        }
    }
}

#[test]
fn confirmed_single_mode_apply_retires_offline_data_and_exact_replay_is_idempotent() {
    let fixture = Fixture::new();
    let namespace = fixture.0.join(".podway/modes/dev");
    fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
    fixture.file(
        ".podway/modes/dev/state/workspaces.json",
        b"corrupt registry must not be parsed",
    );
    fixture.file(".podway/modes/dev/logs/podwayd.log", b"retire me");
    let preserved = fixture.file("worktree/.podway/runtime.db", b"preserved");

    let (output, plan) = fixture.run(&["--dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{plan}");
    let token = plan["result"]["plan_token"].as_str().unwrap().to_owned();

    let (output, confirmation) =
        fixture.run(&["--dev", "runtime", "reset", "apply", "--plan-token", &token]);
    assert_eq!(output.status.code(), Some(2), "{confirmation}");
    assert_eq!(confirmation["code"], "CONFIRMATION_REQUIRED");
    assert!(namespace.join("state/workspaces.json").exists());

    let (output, applied) = fixture.run(&[
        "--dev",
        "runtime",
        "reset",
        "apply",
        "--plan-token",
        &token,
        "--yes",
    ]);
    assert!(output.status.success(), "{applied}");
    assert_eq!(applied["result"]["status"], "complete");
    assert!(
        applied["result"]["preserved"]
            .as_array()
            .unwrap()
            .iter()
            .any(|resource| resource["class"] == "lock_anchor"
                && resource["path"]["display"]
                    .as_str()
                    .unwrap()
                    .ends_with("run/podwayd.lock"))
    );
    assert_eq!(
        applied["result"]["preserved"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|resource| resource["class"] == "control_residue")
            .count(),
        4
    );
    assert!(!namespace.join("state/workspaces.json").exists());
    assert!(!namespace.join("logs").exists());
    assert!(namespace.join("run/podwayd.lock").exists());
    assert_eq!(fs::read(&preserved).unwrap(), b"preserved");

    let (output, replay) = fixture.run(&[
        "--dev",
        "runtime",
        "reset",
        "apply",
        "--plan-token",
        &token,
        "--yes",
    ]);
    assert!(output.status.success(), "{replay}");
    assert_eq!(replay["result"]["status"], "already_applied");
    assert_eq!(fs::read(preserved).unwrap(), b"preserved");
}

#[test]
fn production_apply_routes_exact_plist_retirement_through_the_service_owner() {
    let fixture = Fixture::new();
    fixture.file(".podway/run/podwayd.lock", b"");
    fixture.file(".podway/state/workspaces.json", b"offline registry");
    let plist = fixture.file(
        "Library/LaunchAgents/dev.podway.podwayd.plist",
        b"fixture service definition",
    );
    let unrelated = fixture.file("Library/LaunchAgents/example.keep.plist", b"preserved");

    let (output, plan) = fixture.run(&["--mode", "prod", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{plan}");
    assert_eq!(plan["result"]["status"], "ready");
    let token = plan["result"]["plan_token"].as_str().unwrap();
    let (output, applied) = fixture.run(&[
        "--mode",
        "prod",
        "runtime",
        "reset",
        "apply",
        "--plan-token",
        token,
        "--yes",
    ]);
    assert!(output.status.success(), "{applied}");
    assert_eq!(applied["result"]["status"], "complete");
    assert!(!plist.exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"preserved");
    assert!(fixture.0.join(".podway/run/podwayd.lock").exists());
}

#[test]
fn live_single_mode_apply_uses_reserved_orderly_shutdown_and_finishes_after_exact_exit() {
    let fixture = Fixture::new();
    let mut daemon = Daemon::start(&fixture);
    let namespace = fixture.0.join(".podway/modes/dev");
    let (output, plan) = fixture.run(&["--dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{plan}");
    assert_eq!(plan["result"]["targets"][0]["state"], "live_idle");
    let token = plan["result"]["plan_token"].as_str().unwrap();

    let mut apply = fixture.command();
    apply
        .arg("--json")
        .args([
            "--dev",
            "runtime",
            "reset",
            "apply",
            "--plan-token",
            token,
            "--yes",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = apply.output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={:?} stderr={:?}",
            output.stdout, output.stderr
        )
    });
    assert!(
        output.status.success(),
        "{value}; stderr={}; daemon={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&fs::read(fixture.0.join("daemon-stderr")).unwrap())
    );
    assert_eq!(value["result"]["status"], "complete");
    daemon.await_exit();
    assert!(!namespace.join("run/podwayd.sock").exists());
    assert!(namespace.join("run/podwayd.lock").exists());
    assert!(!namespace.join("logs").exists());
}

#[test]
fn activity_appearing_after_plan_is_reported_as_busy_before_commit() {
    let fixture = Fixture::new();
    let _daemon = Daemon::start(&fixture);
    let (output, plan) = fixture.run(&["--dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{plan}");
    let token = plan["result"]["plan_token"].as_str().unwrap();
    let socket = fixture.0.join(".podway/modes/dev/run/podwayd.sock");
    let _active_client = std::os::unix::net::UnixStream::connect(socket).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));

    let (output, value) = fixture.run(&[
        "--dev",
        "runtime",
        "reset",
        "apply",
        "--plan-token",
        token,
        "--yes",
    ]);
    assert_eq!(output.status.code(), Some(4), "{value}");
    assert_eq!(value["code"], "RUNTIME_RESET_BUSY", "{value}");
    assert_eq!(value["details"]["reason"], "activity", "{value}");
    assert!(
        !fixture
            .0
            .join(".podway/maintenance/runtime-reset.json")
            .exists()
    );
}

#[test]
fn unlistening_owned_socket_plans_offline_without_removal() {
    let fixture = Fixture::new();
    fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
    fixture.file(".podway/modes/dev/state/workspaces.json", b"registry");
    let socket = fixture.0.join(".podway/modes/dev/run/podwayd.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    drop(listener);
    let inode = fs::symlink_metadata(&socket).unwrap().ino();
    let (output, value) = fixture.run(&["--dev", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["result"]["status"], "ready");
    assert_eq!(value["result"]["targets"][0]["state"], "offline");
    assert!(value["result"]["plan_token"].is_string());
    assert_eq!(fs::symlink_metadata(&socket).unwrap().ino(), inode);
}
