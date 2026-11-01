//! Same-manifest settings changes preserve Direct outcomes and accepted receipts.

use std::{path::PathBuf, process::Command};

use serde_json::Value;

#[test]
fn integ005_permission_matrix_preserves_receipts_and_original_replay() {
    let qualifier = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/qualify_integration_settings.py");
    let output = Command::new("python3")
        .arg(qualifier)
        .arg("--podway")
        .arg(super::cli_fixture::podway_binary())
        .output()
        .expect("native settings qualifier launches");
    assert!(
        output.status.success(),
        "qualification failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["schema"],
        "podway.integration-settings-qualification/v1"
    );
    assert_eq!(report["current"]["matrix_cases"], 4);
    assert_eq!(report["current"]["passed"], true);
    assert_eq!(report["cleanup"]["fixture_removed"], true);
    assert_eq!(report["activation_qualified"], false);
}
