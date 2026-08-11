//! V2DOG-003 local CLI exposure for shipped Procedure v2 presets.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(arguments)
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("static preset command must run")
}

fn run_in(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("local dry-run command must run")
}

fn json(arguments: &[&str]) -> Value {
    let output = run(arguments);
    assert!(output.status.success(), "command failed: {output:?}");
    assert!(output.stderr.is_empty(), "unexpected stderr: {output:?}");
    serde_json::from_slice(&output.stdout).expect("command output must be JSON")
}

#[test]
fn preset_metadata_commands_expose_both_v2_presets_without_changing_v1_entries() {
    let list = json(&["--json", "preset", "list"]);
    let ids: Vec<&str> = list["result"]["presets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|preset| preset["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![
            "analysis",
            "bug-fix",
            "bug-fix-v2",
            "docs-only",
            "sw-dev",
            "sw-dev-v2"
        ]
    );

    for (id, digest) in [
        (
            "bug-fix-v2",
            "sha256:53e249a158bdbec6e8437595378509a35cb05288b48db9505cad25d04ef8f768",
        ),
        (
            "sw-dev-v2",
            "sha256:810d438bde83d3055d5d8ab49eec59d60f0c3de61610f74e73c5815fd0087854",
        ),
    ] {
        let shown = json(&["--json", "preset", "show", id]);
        assert_eq!(shown["result"]["preset"], id);
        assert_eq!(shown["result"]["digest"], digest);
        assert_eq!(
            shown["result"]["procedure"]["schema"],
            "podway.procedure/v2"
        );
        assert_eq!(shown["result"]["warnings"], Value::Array(Vec::new()));

        let explained = json(&["--json", "preset", "explain", id]);
        assert_eq!(explained["result"]["preset"]["id"], id);
        assert!(
            explained["result"]["nodes"]
                .as_array()
                .is_some_and(|nodes| !nodes.is_empty())
        );
    }
}

#[test]
fn v2_preset_dry_run_uses_the_v2_start_result_and_shipped_digest() {
    let output = json(&[
        "--json",
        "start",
        "--preset",
        "bug-fix-v2",
        "--task",
        "Repair a defect",
        "--dry-run",
    ]);
    assert_eq!(output["schema"], "podway.output/v2");
    assert_eq!(output["command"], "session.start");
    assert_eq!(output["result"]["schema"], "podway.session-start-result/v2");
    assert_eq!(output["result"]["procedure_schema"], "podway.procedure/v2");
    assert_eq!(
        output["result"]["procedure_digest"],
        "sha256:53e249a158bdbec6e8437595378509a35cb05288b48db9505cad25d04ef8f768"
    );
    assert_eq!(output["result"]["dry_run"], true);
    assert_eq!(output["result"]["goal_tracking"], true);
    assert_eq!(output["result"]["goal_defined"], false);
}

#[test]
fn custom_v2_dry_run_dispatches_locally_and_enforces_the_canonical_digest() {
    let root = std::env::temp_dir().join(format!(
        "podway-v2dog-custom-dry-run-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("custom dry-run fixture root must be created");
    fs::write(
        root.join("bug-fix-v2.yaml"),
        include_str!("../../../assets/presets/bug-fix-v2.yaml"),
    )
    .expect("custom Procedure v2 YAML must be written");
    fs::write(
        root.join("sw-dev-v1.yaml"),
        include_str!("../../../assets/presets/sw-dev.yaml"),
    )
    .expect("custom Procedure v1 YAML must be written");
    let shown = json(&["--json", "preset", "show", "bug-fix-v2"]);
    fs::write(
        root.join("bug-fix-v2.json"),
        serde_json::to_vec_pretty(&shown["result"]["procedure"]).unwrap(),
    )
    .expect("equivalent custom Procedure v2 JSON must be written");
    fs::write(
        root.join("no-goal-v2.yaml"),
        "schema: podway.procedure/v2\nid: no-goal\nversion: \"1\"\nname: No Goal\npurpose: Verify goal admission.\nnode_definitions:\n  work:\n    type: action\n    title: Work\n    intent: Do the work.\ngraph:\n  entry: work\n  nodes:\n    - id: work\n      use: work\n      terminal: true\n",
    )
    .expect("goal-disabled Procedure v2 fixture must be written");
    fs::write(
        root.join("unreachable-v2.yaml"),
        "schema: podway.procedure/v2\nid: unreachable\nversion: \"1\"\nname: Unreachable\npurpose: Verify vet admission.\nnode_definitions:\n  work:\n    type: action\n    title: Work\n    intent: Do the work.\ngraph:\n  entry: first\n  nodes:\n    - id: first\n      use: work\n      terminal: true\n    - id: stranded\n      use: work\n      terminal: true\n",
    )
    .expect("vet-invalid Procedure v2 fixture must be written");
    fs::write(
        root.join("broken-v2.yaml"),
        "schema: podway.procedure/v2\nid: broken\ngraph: [\n",
    )
    .expect("malformed Procedure v2 fixture must be written");

    let digest = "sha256:53e249a158bdbec6e8437595378509a35cb05288b48db9505cad25d04ef8f768";
    let success = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "bug-fix-v2.yaml",
            "--expect-procedure-digest",
            digest,
            "--task",
            "Repair a defect",
            "--goal",
            "Repair the defect.",
            "--criterion",
            "verified=The fix is verified.",
            "--dry-run",
        ],
    );
    assert!(
        success.status.success(),
        "custom v2 dry-run failed: {success:?}"
    );
    let success: Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(success["schema"], "podway.output/v2");
    assert_eq!(
        success["result"]["schema"],
        "podway.session-start-result/v2"
    );
    assert_eq!(success["result"]["procedure_digest"], digest);
    assert_eq!(success["result"]["goal_defined"], true);

    let json_success = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "bug-fix-v2.json",
            "--expect-procedure-digest",
            digest,
            "--task",
            "Repair a defect",
            "--dry-run",
        ],
    );
    assert!(
        json_success.status.success(),
        "equivalent custom v2 JSON dry-run failed: {json_success:?}"
    );
    let json_success: Value = serde_json::from_slice(&json_success.stdout).unwrap();
    assert_eq!(json_success["schema"], "podway.output/v2");
    assert_eq!(json_success["result"]["procedure_digest"], digest);

    let missing = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "bug-fix-v2.yaml",
            "--task",
            "Repair a defect",
            "--dry-run",
        ],
    );
    assert_eq!(missing.status.code(), Some(2));
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing["code"], "DIGEST_CONFIRMATION_REQUIRED");
    assert_eq!(missing["details"]["procedure_digest"], digest);
    assert_eq!(missing["details"]["admission"]["admitted"], false);

    let mismatch = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "bug-fix-v2.yaml",
            "--expect-procedure-digest",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--task",
            "Repair a defect",
            "--dry-run",
        ],
    );
    assert_eq!(mismatch.status.code(), Some(4));
    let mismatch: Value = serde_json::from_slice(&mismatch.stdout).unwrap();
    assert_eq!(mismatch["code"], "PROCEDURE_DIGEST_MISMATCH");
    assert_eq!(mismatch["details"]["actual_procedure_digest"], digest);

    let no_goal_validation = run_in(
        &root,
        &["--json", "procedure", "validate", "no-goal-v2.yaml"],
    );
    assert!(no_goal_validation.status.success());
    let no_goal_validation: Value = serde_json::from_slice(&no_goal_validation.stdout).unwrap();
    let no_goal_digest = no_goal_validation["result"]["digest"]
        .as_str()
        .expect("validation must report the canonical digest")
        .to_owned();

    let no_goal_missing_digest = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "no-goal-v2.yaml",
            "--task",
            "Reject a goal",
            "--goal",
            "This Procedure does not enable goals.",
            "--criterion",
            "checked=The boundary is checked.",
            "--dry-run",
        ],
    );
    assert_eq!(no_goal_missing_digest.status.code(), Some(2));
    let no_goal_missing_digest: Value =
        serde_json::from_slice(&no_goal_missing_digest.stdout).unwrap();
    assert_eq!(
        no_goal_missing_digest["code"],
        "DIGEST_CONFIRMATION_REQUIRED"
    );

    let no_goal_mismatch = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "no-goal-v2.yaml",
            "--expect-procedure-digest",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--task",
            "Reject a goal",
            "--goal",
            "This Procedure does not enable goals.",
            "--criterion",
            "checked=The boundary is checked.",
            "--dry-run",
        ],
    );
    assert_eq!(no_goal_mismatch.status.code(), Some(4));
    let no_goal_mismatch: Value = serde_json::from_slice(&no_goal_mismatch.stdout).unwrap();
    assert_eq!(no_goal_mismatch["code"], "PROCEDURE_DIGEST_MISMATCH");

    let no_goal_tracking = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "no-goal-v2.yaml",
            "--expect-procedure-digest",
            no_goal_digest.as_str(),
            "--task",
            "Reject a goal",
            "--goal",
            "This Procedure does not enable goals.",
            "--criterion",
            "checked=The boundary is checked.",
            "--dry-run",
        ],
    );
    assert_eq!(no_goal_tracking.status.code(), Some(1));
    let no_goal_tracking: Value = serde_json::from_slice(&no_goal_tracking.stdout).unwrap();
    assert_eq!(no_goal_tracking["code"], "GOAL_TRACKING_NOT_ENABLED");
    assert_eq!(
        no_goal_tracking["details"]["kind"],
        "GOAL_TRACKING_NOT_ENABLED"
    );

    let vet_invalid = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "unreachable-v2.yaml",
            "--task",
            "Reject an invalid graph",
            "--dry-run",
        ],
    );
    assert_eq!(vet_invalid.status.code(), Some(1));
    let vet_invalid: Value = serde_json::from_slice(&vet_invalid.stdout).unwrap();
    assert_eq!(vet_invalid["code"], "PROCEDURE_INVALID");

    let malformed = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "broken-v2.yaml",
            "--task",
            "Reject malformed input",
            "--goal",
            "Stay local.",
            "--criterion",
            "checked=The boundary is checked.",
            "--dry-run",
        ],
    );
    assert_eq!(malformed.status.code(), Some(1));
    let malformed: Value = serde_json::from_slice(&malformed.stdout).unwrap();
    assert_eq!(malformed["code"], "PROCEDURE_INVALID");

    let v1 = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "sw-dev-v1.yaml",
            "--task",
            "Retain v1",
            "--dry-run",
        ],
    );
    assert!(v1.status.success(), "v1 dry-run regressed: {v1:?}");
    let v1: Value = serde_json::from_slice(&v1.stdout).unwrap();
    assert_eq!(v1["schema"], "podway.output/v1");
    assert_eq!(v1["result"]["dry_run"], true);

    let v1_goal = run_in(
        &root,
        &[
            "--json",
            "start",
            "--procedure",
            "sw-dev-v1.yaml",
            "--task",
            "Reject a v1 goal",
            "--goal",
            "Goals require v2.",
            "--criterion",
            "checked=The boundary is checked.",
            "--dry-run",
        ],
    );
    assert_eq!(v1_goal.status.code(), Some(2));
    let v1_goal: Value = serde_json::from_slice(&v1_goal.stdout).unwrap();
    assert_eq!(v1_goal["code"], "REQUEST_INVALID");
    assert!(
        !root.join(".podway").exists(),
        "dry-run must not create state"
    );

    fs::remove_dir_all(&root).expect("custom dry-run fixture root must be removed");
}
