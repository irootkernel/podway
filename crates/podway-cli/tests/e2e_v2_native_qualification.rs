//! Release-profile maintenance hooks must remain disabled in the real CLI.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
};

use podway_protocol::OutputEnvelopeV4;
use sha2::{Digest, Sha256};

use super::cli_fixture::{Fixture, GuardedChild, bounded_output, podway_binary};

#[test]
#[ignore = "run with tools/run_e2e.py to supply the actual release CLI"]
fn release_binary_ignores_every_debug_maintenance_hook() {
    let supplied = PathBuf::from(std::env::var_os("PODWAY_RELEASE_TEST_BINARY").unwrap());
    assert!(supplied.is_absolute());
    assert!(fs::symlink_metadata(&supplied).unwrap().is_file());
    assert_eq!(supplied.canonicalize().unwrap(), supplied);
    if std::env::var_os("PODWAY_TEST_CLI_BINARY").is_some() {
        assert_eq!(
            supplied,
            podway_binary(),
            "qualify the supplied packaged snapshot"
        );
    }
    let original = Sha256::digest(fs::read(&supplied).unwrap());
    let fixture = Fixture::new();
    let binary = fixture.0.join("podway-release");
    fs::copy(&supplied, &binary).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o500)).unwrap();
    assert_eq!(Sha256::digest(fs::read(&binary).unwrap()), original);
    let child = Command::new(&binary)
        .arg("--podway-test-isolation-probe")
        .current_dir(&fixture.0)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PODWAY_TEST_ISOLATION_PROBE", "podway-test-isolation-v1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = bounded_output(GuardedChild::new(child), &["disabled isolation probe"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    for stage in [
        "lease-acquired",
        "intent-published",
        "marker-published",
        "runtime-cleared",
        "phase-advanced",
        "before-final-removal",
        "after-first-removal-marker",
        "after-last-removal-marker",
        "directories-removed",
    ] {
        let invoke = |arguments: &[&str]| {
            let mut command = Command::new(&binary);
            if arguments.first() != Some(&"version") {
                command.arg("--worktree").arg(&fixture.0);
            }
            let child = command
                .arg("--json")
                .args(arguments)
                .current_dir(&fixture.0)
                .env_remove("HOME")
                .env_remove("XDG_CONFIG_HOME")
                .env(
                    "PODWAY_TEST_DIRECT_MAINTENANCE_TOKEN",
                    "podway-direct-maintenance-test-v1",
                )
                .env("PODWAY_TEST_DIRECT_MAINTENANCE_PAUSE", stage)
                .env("PODWAY_TEST_ISOLATION_PROBE", "podway-test-isolation-v1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let output = bounded_output(GuardedChild::new(child), arguments);
            assert_eq!(output.status.code(), Some(0), "{stage}: {output:?}");
            assert!(output.stderr.is_empty(), "{stage}: {output:?}");
            serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
                .unwrap()
                .into_value()
        };
        if stage == "lease-acquired" {
            let identity = invoke(&["version", "--identity"]);
            assert_eq!(identity["schema"], "podway.output/v4");
            assert_eq!(identity["result"]["schema"], "podway.version-result/v2");
            assert_eq!(
                identity["result"]["contract_manifest_schema"],
                "podway.contract-manifest/v2"
            );
            assert!(identity["result"].get("supported_ipc_ids").is_none());
        }
        let initialized = invoke(&["init", "--idempotency-key", &format!("init-{stage}")]);
        let reset = invoke(&[
            "reset",
            "--all",
            "--yes",
            "--idempotency-key",
            &format!("reset-{stage}"),
        ]);
        assert_eq!(reset["result"]["reset"], true);
        assert_ne!(initialized["workspace"]["uuid"], reset["workspace"]["uuid"]);
        let removed = invoke(&[
            "workspace",
            "remove",
            "--force",
            "--yes",
            "--if-workspace-uuid",
            reset["workspace"]["uuid"].as_str().unwrap(),
            "--idempotency-key",
            &format!("remove-{stage}"),
        ]);
        assert_eq!(removed["command"], "workspace.remove");
        assert!(!fixture.0.join(".podway").exists());
        assert!(fixture.0.join(".git").is_dir());
    }
    assert_eq!(Sha256::digest(fs::read(&binary).unwrap()), original);
    assert_eq!(Sha256::digest(fs::read(&supplied).unwrap()), original);
}
