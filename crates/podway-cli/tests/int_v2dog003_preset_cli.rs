//! V2DOG-003 local CLI exposure for shipped Procedure v2 presets.

use std::process::{Command, Output};

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
