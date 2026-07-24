use std::{fs, path::Path, process::Command};

use serde_json::Value;

fn digest_is_canonical(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[test]
fn version_identity_is_static_complete_and_manifest_bound() {
    let output = Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(["--json", "version"])
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("podway version probe must run");
    assert!(output.status.success(), "version probe failed: {output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);

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
    assert!(result["source_commit"].is_null() || result["source_commit"].as_str().is_some());
    assert_eq!(
        result["contract_manifest_schema"],
        "podway.contract-manifest/v1"
    );
    assert_eq!(
        result["supported_ipc_ids"],
        serde_json::json!(["podway.ipc/v1"])
    );

    let manifest_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/contract-manifest-v1.json");
    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest_path).expect("checked contract manifest must be readable"),
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
fn matching_binary_identities_are_identical() {
    let cli = Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(["--json", "version"])
        .output()
        .expect("podway version probe must run");
    assert!(cli.status.success());
    let cli: Value = serde_json::from_slice(&cli.stdout).expect("podway version is JSON");

    let daemon_path = Path::new(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd");
    let daemon = Command::new(&daemon_path)
        .args(["--json", "version"])
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "podwayd version probe {} failed: {error}",
                daemon_path.display()
            )
        });
    assert!(
        daemon.status.success(),
        "podwayd version probe failed: {daemon:?}"
    );
    assert!(daemon.stderr.is_empty());
    let daemon: Value = serde_json::from_slice(&daemon.stdout).expect("podwayd version is JSON");
    assert_eq!(cli["result"], daemon);
}
