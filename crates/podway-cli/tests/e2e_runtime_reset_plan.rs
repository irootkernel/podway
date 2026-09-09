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
    fixture.file(".podway/run/podwayd.lock", b"");
    let registry = fixture.file(".podway/state/workspaces.json", b"recreated registry");
    let (output, value) = fixture.run(&["--mode", "prod", "runtime", "reset", "plan"]);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["result"]["status"], "ready");
    assert!(value["result"]["plan_token"].is_string());
    assert_eq!(fs::read(&registry).unwrap(), b"recreated registry");
    assert_eq!(fs::read(&record_path).unwrap(), completed);

    fs::write(&record_path, b"{corrupt}").unwrap();
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
