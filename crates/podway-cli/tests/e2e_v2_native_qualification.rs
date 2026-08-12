use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
};

use serde_json::Value;

const REQUIRED_CHECKS: [&str; 13] = [
    "custom_preview_confirmation",
    "format_equivalence_restart",
    "preset_without_digest",
    "next_suggestions",
    "detached_replay",
    "concurrent_stale_fence",
    "sigkill_recovery",
    "response_loss_reconciliation",
    "completed_manual_reactivation",
    "completed_goal_reactivation",
    "cancelled_rejection",
    "endpoint_isolation",
    "release_public_admission",
];

fn required_path(variable: &str, executable: bool) -> PathBuf {
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{variable} must be exported by tools/run_e2e.py"));
    assert!(path.is_absolute(), "{variable} must be absolute: {path:?}");
    let metadata = fs::metadata(&path)
        .unwrap_or_else(|error| panic!("{variable} must identify a regular file: {error}"));
    assert!(
        metadata.is_file(),
        "{variable} must identify a regular file"
    );
    if executable {
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "{variable} must identify an executable file"
        );
    }
    path
}

#[test]
#[ignore = "run with tools/run_e2e.py so isolated v2 qualification binaries are supplied"]
fn v2rel003_native_runtime_qualification_emits_complete_compact_evidence() {
    let qualifier = required_path("PODWAY_V2REL003_QUALIFIER", false);
    let podway = required_path("PODWAY_V2REL003_CLI", true);
    let debug_daemon = required_path("PODWAY_V2REL003_DAEMON_DEBUG", true);
    let release_daemon = required_path("PODWAY_V2REL003_DAEMON_RELEASE", true);

    let output = Command::new("python3")
        .arg(qualifier)
        .arg("qualify-v2rel003")
        .arg("--podway")
        .arg(podway)
        .arg("--podwayd-debug")
        .arg(debug_daemon)
        .arg("--podwayd-release")
        .arg(release_daemon)
        .stdin(Stdio::null())
        .output()
        .expect("V2REL-003 native qualifier must launch");
    assert!(
        output.status.success(),
        "native qualifier failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "successful native qualification must not emit stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = std::str::from_utf8(&output.stdout).expect("qualification evidence must be UTF-8");
    let compact = stdout.trim_end_matches(['\r', '\n']);
    assert!(
        !compact.is_empty(),
        "qualification evidence must not be empty"
    );
    assert!(
        !compact.contains(['\r', '\n']),
        "qualification evidence must be exactly one compact JSON line"
    );
    let evidence: Value =
        serde_json::from_str(compact).expect("qualification evidence must be valid JSON");
    assert_eq!(
        evidence["schema"],
        "podway.v2rel003-native-qualification/v1"
    );
    assert_eq!(evidence["ok"], true);
    let checks = evidence["checks"]
        .as_object()
        .expect("qualification evidence must contain a checks object");
    assert!(
        !checks.is_empty(),
        "qualification evidence must report checks"
    );
    assert_eq!(
        checks.len(),
        REQUIRED_CHECKS.len(),
        "qualification evidence must report exactly the registered checks"
    );
    for name in REQUIRED_CHECKS {
        assert_eq!(
            checks.get(name),
            Some(&Value::Bool(true)),
            "{name} must pass"
        );
    }
    assert_eq!(
        serde_json::to_string(&evidence).expect("qualification evidence must re-encode"),
        compact,
        "qualification evidence must use the canonical compact representation"
    );
}
