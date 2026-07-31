//! Archive-builder acceptance for native Podway test-fixture binaries.

#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;

fn daemon_binary() -> PathBuf {
    std::env::var_os("PODWAYD_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd"))
}

fn package_with_artifact_class(output_directory: &Path, artifact_class: &str) -> Output {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Command::new("python3")
        .arg(root.join("tools/release_archive.py"))
        .args([
            "package",
            "--allow-dirty",
            "--artifact-class",
            artifact_class,
            "--podway",
        ])
        .arg(env!("CARGO_BIN_EXE_podway"))
        .arg("--podwayd")
        .arg(daemon_binary())
        .arg("--output-dir")
        .arg(output_directory)
        .current_dir(&root)
        .output()
        .expect("release archive builder must run")
}

fn package(output_directory: &Path) -> Value {
    let output = package_with_artifact_class(output_directory, "test-fixture");
    assert!(
        output.status.success(),
        "release archive builder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("release archive receipt must be JSON")
}

#[test]
fn pac_072_075_077_archive_fixture_is_complete_deterministic_and_documented() {
    let root = std::env::temp_dir().join(format!("podway-release-e2e-{}", std::process::id()));
    let first = root.join("first");
    let second = root.join("second");
    let first_receipt = package(&first);
    let second_receipt = package(&second);
    let rejected_directory = root.join("rejected-distribution");
    let rejected = package_with_artifact_class(&rejected_directory, "distribution");
    assert!(!rejected.status.success());
    let rejected_receipt: Value =
        serde_json::from_slice(&rejected.stdout).expect("rejection receipt must be JSON");
    assert_eq!(rejected_receipt["ok"], false);
    assert!(
        rejected_receipt["error"]
            .as_str()
            .is_some_and(|error| error.contains("distribution archives cannot use --allow-dirty"))
    );
    assert!(!rejected_directory.exists());

    assert_eq!(first_receipt["ok"], true);
    assert_eq!(first_receipt["mode"], "package");
    assert_eq!(
        first_receipt["archive_sha256"],
        second_receipt["archive_sha256"]
    );
    let entries = first_receipt["entries"]
        .as_array()
        .expect("archive entries must be an array");
    for required in [
        "/bin/podway",
        "/bin/podwayd",
        "/share/completions/podway.bash",
        "/share/completions/podway.fish",
        "/share/completions/podway.zsh",
        "/LICENSE",
        "/README.md",
        "/RELEASE_NOTES.md",
    ] {
        assert!(
            entries
                .iter()
                .filter_map(Value::as_str)
                .any(|entry| entry.ends_with(required)),
            "release archive omits {required}"
        );
    }
    assert!(
        entries
            .iter()
            .filter_map(Value::as_str)
            .any(|entry| entry.contains("/share/podway/presets/"))
    );
    assert!(
        entries
            .iter()
            .filter_map(Value::as_str)
            .any(|entry| entry.contains("/share/podway/schemas/"))
    );
    for required in [
        "/share/podway/spec/command-catalog.yaml",
        "/share/podway/spec/error-codes.json",
        "/share/podway/spec/state-transition-matrix.csv",
        "/share/podway/tests/fixtures/contract/canonicalization-v1.json",
        "/share/podway/docs/examples/json/status-result.json",
        "/share/podway/contracts/contract-manifest-v1.json",
    ] {
        assert!(
            entries
                .iter()
                .filter_map(Value::as_str)
                .any(|entry| entry.ends_with(required)),
            "release archive omits {required}"
        );
    }

    let provenance_path = first_receipt["provenance"]
        .as_str()
        .expect("provenance path must be present");
    let provenance: Value =
        serde_json::from_slice(&fs::read(provenance_path).expect("provenance must be readable"))
            .expect("provenance must be JSON");
    assert_eq!(provenance["schema"], "podway.release-provenance/v1");
    assert_eq!(provenance["artifact_class"], "test-fixture");
    assert_eq!(provenance["release_gate"], "test-fixture");
    assert_eq!(provenance["target"], "aarch64-apple-darwin");
    assert_eq!(provenance["version"], "0.1.0");
    assert!(
        provenance["build_identity"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:") && value.len() == 71)
    );
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest: Value = serde_json::from_slice(
        &fs::read(repository.join("contracts/contract-manifest-v1.json"))
            .expect("contract manifest must be readable"),
    )
    .expect("contract manifest must be JSON");
    assert_eq!(
        provenance["contract_manifest_schema"],
        manifest["schema_version"]
    );
    assert_eq!(provenance["contract_manifest_digest"], manifest["digest"]);
    assert_eq!(provenance["release_status"]["signing"], "unsigned");
    assert_eq!(
        provenance["release_status"]["notarization"],
        "not-attempted"
    );
    assert!(
        provenance["toolchain"]
            .as_str()
            .expect("toolchain must be text")
            .starts_with("rustc 1.97.1 ")
    );
    assert_eq!(
        provenance["archive"]["sha256"],
        first_receipt["archive_sha256"]
    );

    fs::remove_dir_all(root).expect("release fixture cleanup must succeed");
}
