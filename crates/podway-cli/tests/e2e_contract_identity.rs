use std::{fs, path::Path, process::Command};

use nix::unistd::geteuid;
use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LocalPlatformPathV1, MacosServiceCommandRunnerV1,
    ServiceErrorV1, ServiceManagerContractV1, ServiceManagerV1, ServiceOperationV1,
    ServiceRuntimePathsV1, StdServiceFilesystemV1, SystemLaunchctlRunnerV1,
};
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

#[test]
fn mismatched_install_fails_before_service_publication_or_launchctl() {
    let root = std::path::PathBuf::from(format!("/tmp/pci-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw())
        .expect("fixture service paths");
    let daemon = Path::new(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd");
    let runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        SystemLaunchctlRunnerV1::new(root.join("launchctl-must-not-run")),
        FixedServiceClockV1::new(UnixMillis::new(1)),
        geteuid().as_raw(),
    )
    .expect("service runner");
    let manager = ServiceManagerV1::new(
        runner,
        FixedServiceClockV1::new(UnixMillis::new(1)),
        paths.clone(),
    );
    let spec = InstallSpecV1::new(
        LocalPlatformPathV1::new(&daemon).expect("daemon binary path"),
        podway_service::ServiceLabelV1::podwayd(),
        paths.clone(),
    )
    .with_expected_contract_identity(
        "podway",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    );

    assert!(matches!(
        manager.install(spec),
        Err(ServiceErrorV1::OperationFailureV1 {
            operation: ServiceOperationV1::Install,
            source,
        }) if matches!(
            source.as_ref(),
            ServiceErrorV1::ContractMismatchV1 {
                actual_product: Some(product),
                actual_manifest_digest: Some(digest),
                ..
            } if product == "podway" && digest != "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        )
    ));
    assert!(!paths.launch_agent_path().as_path().exists());
    assert!(!paths.metadata_index_path().as_path().exists());
    assert!(!root.join("launchctl-must-not-run").exists());
    assert!(
        !root.exists(),
        "contract rejection must precede service directory creation"
    );
}
