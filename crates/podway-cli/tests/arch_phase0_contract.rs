//! Direct CLI production boundaries (ARC-001, ADR-0038).

use std::{collections::BTreeSet, process::Command};

use serde_json::Value;

#[test]
fn arc_001_direct_cli_depends_on_engine_without_store_git_or_service_edges() {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    let cli = packages
        .iter()
        .find(|package| package["name"] == "podway-cli")
        .unwrap();
    let dependencies: BTreeSet<_> = cli["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|dependency| dependency["kind"].is_null())
        .filter_map(|dependency| dependency["name"].as_str())
        .filter(|name| name.starts_with("podway-"))
        .collect();
    assert_eq!(
        dependencies,
        BTreeSet::from([
            "podway-core",
            "podway-protocol",
            "podway-config",
            "podway-presets",
            "podway-engine",
        ])
    );
}
