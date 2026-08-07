#![forbid(unsafe_code)]

use std::{fs, path::Path, process::Command};

use podway_protocol::ResponseEnvelopeV1;
use serde_json::Value;

fn digest_is_canonical(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Detaches a spawned product process from the developer's own account state.
///
/// Account resolution reads the operating-system account database (ADR-0012), so clearing the
/// environment does not detach it. Neither override path is ever created: identity probes must not
/// reach account state or `launchctl` at all, so an unexpected attempt fails loudly.
fn configure_test_isolation(command: &mut Command) {
    let root = std::env::temp_dir().join(format!("pci-{}", std::process::id()));
    command
        .env("PODWAY_TEST_ACCOUNT_ROOT", &root)
        .env("PODWAY_TEST_LAUNCHCTL", root.join("launchctl-must-not-run"));
}

#[test]
fn version_identity_is_static_complete_and_manifest_bound() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_podway"));
    command
        .args(["--json", "version", "--identity"])
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    configure_test_isolation(&mut command);
    let output = command.output().expect("podway version probe must run");
    assert!(output.status.success(), "version probe failed: {output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);

    let ResponseEnvelopeV1::Output(cli_envelope) = serde_json::from_slice(&output.stdout)
        .expect("version output satisfies the typed public protocol")
    else {
        panic!("version identity must be a success envelope");
    };
    assert_eq!(cli_envelope.command().as_str(), "version");
    assert_eq!(cli_envelope.result()["schema"], "podway.version-result/v1");

    let daemon = Path::new(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd");
    let mut daemon_command = Command::new(daemon);
    daemon_command
        .args(["version", "--json", "--identity"])
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    configure_test_isolation(&mut daemon_command);
    let daemon_output = daemon_command
        .output()
        .expect("podwayd version probe must run");
    assert!(
        daemon_output.status.success(),
        "daemon version probe failed: {daemon_output:?}"
    );
    assert!(daemon_output.stderr.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&daemon_output.stdout)
            .lines()
            .count(),
        1
    );
    let ResponseEnvelopeV1::Output(daemon_envelope) = serde_json::from_slice(&daemon_output.stdout)
        .expect("daemon version output satisfies the typed public protocol")
    else {
        panic!("daemon version identity must be a success envelope");
    };
    assert_eq!(daemon_envelope.command().as_str(), "version");
    assert_eq!(
        cli_envelope.result(),
        daemon_envelope.result(),
        "both release binaries must report one identical closed identity"
    );

    let envelope: Value = serde_json::from_slice(&output.stdout).expect("version output is JSON");
    let result = envelope["result"]
        .as_object()
        .expect("version result is an object");
    assert_eq!(result["product"], "podway");
    assert_eq!(result["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        result["target"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        result["build_identity"]
            .as_str()
            .is_some_and(digest_is_canonical)
    );

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let expected_source_commit = option_env!("PODWAY_SOURCE_COMMIT")
        .map(str::to_owned)
        .or_else(|| {
            let output = Command::new("git")
                .args(["-C", workspace.to_str()?, "rev-parse", "--verify", "HEAD"])
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        });
    match expected_source_commit {
        Some(expected) => assert_eq!(result["source_commit"], expected),
        None => assert!(result["source_commit"].is_null()),
    }
    assert_eq!(
        result["contract_manifest_schema"],
        "podway.contract-manifest/v1"
    );
    assert_eq!(
        result["supported_ipc_ids"],
        serde_json::json!(["podway.ipc/v1"])
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(workspace.join("contracts/contract-manifest-v1.json"))
            .expect("checked contract manifest must be readable"),
    )
    .expect("checked contract manifest must be JSON");
    assert_eq!(result["contract_manifest_digest"], manifest["digest"]);
    assert!(
        result["contract_manifest_digest"]
            .as_str()
            .is_some_and(digest_is_canonical)
    );
}

#[test]
fn public_version_json_matches_the_compact_name_and_version_contract() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_podway"));
    command
        .args(["version", "--json"])
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    configure_test_isolation(&mut command);
    let output = command.output().expect("podway version summary must run");
    assert!(
        output.status.success(),
        "version summary failed: {output:?}"
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        format!(
            "{{\"name\":\"podway\",\"version\":\"v{}\"}}\n",
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes()
    );
}
