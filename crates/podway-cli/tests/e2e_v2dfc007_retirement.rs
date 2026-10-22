//! Real-binary retirement must precede stdin, worktree discovery and side effects.

use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use podway_protocol::ErrorEnvelopeV2;
use serde_json::Value;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("podway-retirement-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        super::cli_fixture::assert_packaged_fixture(&root);
        fs::write(root.join("sentinel"), b"unchanged").unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct GuardedChild(Option<Child>);

impl Drop for GuardedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn retired_surface_returns_valid_guidance_without_reading_open_stdin_or_worktree() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["record", "--stdin", "--detach"],
        vec!["delegation", "accept", "--stdin", "--mode", "dev"],
        vec!["daemon", "status"],
        vec![
            "daemon",
            "install",
            "--daemon-path",
            "/missing/podwayd",
            "--json",
        ],
        vec!["daemon", "mystery"],
        vec!["runtime", "reset", "apply"],
        vec!["workspace", "mode", "apply"],
        vec!["job", "lookup"],
        vec!["terminate"],
        vec!["status", "--wait-for-idle"],
        vec!["observe", "--after-job"],
        vec!["status", "--dev=false"],
        vec!["--socket"],
        vec!["help", "goal", "define", "--detach"],
    ] {
        let mut command = Command::new(super::cli_fixture::podway_binary());
        if !arguments.contains(&"--json") {
            command.arg("--json");
        }
        let child = command
            .arg("--worktree")
            .arg(fixture.0.join("missing-worktree"))
            .args(&arguments)
            .current_dir(&fixture.0)
            .env("PODWAY_TEST_ACCOUNT_ROOT", fixture.0.join("unused-account"))
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut guard = GuardedChild(Some(child));
        // Keep the producer open without sending bytes: consuming stdin would block.
        let producer = guard.0.as_mut().unwrap().stdin.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while guard.0.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "retirement blocked: {arguments:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let output = guard.0.take().unwrap().wait_with_output().unwrap();
        drop(producer);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{arguments:?}: {output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        let value = error.as_value();
        assert_eq!(value["code"], "COMMAND_RETIRED", "{arguments:?}");
        if arguments == ["help", "goal", "define", "--detach"] {
            assert_eq!(value["command"], "help", "{arguments:?}");
        }
        assert_eq!(value["retryable"], false);
        assert!(output.stdout.ends_with(b"\n"));
        let details = value["details"].as_object().unwrap();
        assert_eq!(details.len(), 4);
        if arguments == ["job", "lookup"] {
            assert_eq!(details["replacement"], "outcome.lookup");
            assert!(
                details["guidance"]
                    .as_str()
                    .unwrap()
                    .contains("outcome.lookup")
            );
        } else {
            assert_eq!(details["replacement"], Value::Null);
            assert!(
                details["guidance"]
                    .as_str()
                    .unwrap()
                    .contains("No replacement")
            );
        }
        assert_eq!(fs::read(fixture.0.join("sentinel")).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }
}
