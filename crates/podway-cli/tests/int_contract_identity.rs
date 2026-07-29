#![forbid(unsafe_code)]

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
