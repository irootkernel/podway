//! Native supervised-delegation qualification with a deterministic external fixture.

use std::{path::PathBuf, process::Command};

use serde_json::Value;

#[test]
#[ignore = "run with tools/run_e2e.py to supply the actual daemon binary"]
fn v2sdg008_native_lifecycle_migration_and_host_qualification() {
    let qualifier = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/qualify_supervised_delegation.py");
    let output = Command::new("python3")
        .arg(qualifier)
        .arg("--podway")
        .arg(env!("CARGO_BIN_EXE_podway"))
        .arg("--podwayd")
        .arg(std::env::var_os("PODWAYD_TEST_BINARY").expect("native daemon supplied by E2E runner"))
        .output()
        .expect("native qualifier launches");
    assert!(
        output.status.success(),
        "qualification failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let evidence: Value = serde_json::from_slice(&output.stdout).expect("qualification JSON");
    assert_eq!(evidence["schema"], "podway.v2sdg008-qualification/v1");
    assert_eq!(evidence["live_adapter"], "not_performed");
    assert_eq!(evidence["live_model"], "not_performed");
    assert_eq!(evidence["runtime_cleanup"], true);
    assert_eq!(
        evidence["example"]["source"],
        "docs/examples/procedures/supervised-review-v2.yaml"
    );
    assert!(
        evidence["example"]["procedure_digest"]
            .as_str()
            .expect("example digest")
            .starts_with("sha256:")
    );
    for check in [
        "example",
        "external",
        "managed",
        "decision_evidence",
        "recovery",
        "migration",
    ] {
        assert_eq!(evidence["checks"][check], true, "{check}");
    }
}
