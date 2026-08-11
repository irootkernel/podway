//! V2DOG-004 discoverability checks for the complete Procedure v2 CLI surface.

use std::process::{Command, Output};

use serde_json::Value;

const V2_WORKFLOW: &str = include_str!("../../../docs/examples/v2-workflow.md");
const OUTPUT_V2_SCHEMA: &str = include_str!("../../../assets/schemas/output-v2.schema.json");
const STATUS_V2_SCHEMA: &str = include_str!("../../../assets/schemas/status-result-v2.schema.json");
const NEXT_V2_SCHEMA: &str = include_str!("../../../assets/schemas/next-result-v2.schema.json");
const COMPONENTS_SCHEMA: &str =
    include_str!("../../../assets/schemas/v2-result-components-v1.schema.json");
const CRITERION_RESULT_SCHEMA: &str =
    include_str!("../../../assets/schemas/criterion-assessment-result-v1.schema.json");

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(arguments)
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("static guidance command must run")
}

fn help(topic: &str) -> String {
    let output = run(&["--json", "help", topic]);
    assert!(
        output.status.success(),
        "help failed for {topic}: {output:?}"
    );
    assert!(output.stderr.is_empty(), "unexpected stderr: {output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).expect("help must be JSON");
    value["result"]["text"]
        .as_str()
        .expect("help result must contain text")
        .to_owned()
}

#[test]
fn every_v2_route_and_route_specific_flag_is_documented() {
    for (topic, tokens) in [
        ("procedure.format", &["--check", "--write"][..]),
        ("procedure.vet", &[][..]),
        ("procedure.lint", &["--warnings-as-errors"][..]),
        ("procedure.check", &["--warnings-as-errors"][..]),
        (
            "procedure.graph",
            &["--format", "mermaid", "puml", "dot"][..],
        ),
        ("procedure.preview", &[][..]),
        ("procedure.scaffold", &["--template", "minimal"][..]),
        ("procedure.convert", &["<file>"][..]),
        (
            "session.decide",
            &["--option", "--reason", "--actor", "--if-goal-revision"][..],
        ),
        ("session.rework", &["--to", "--reason", "--actor"][..]),
        ("goal.define", &["--goal", "--criterion", "--actor"][..]),
        (
            "goal.revise",
            &[
                "--goal",
                "--criterion",
                "--rework-to",
                "--reason",
                "--actor",
                "--reactivate",
                "--if-goal-revision",
            ][..],
        ),
        (
            "goal.assess_criterion",
            &[
                "--status",
                "--reason",
                "--evidence",
                "--item",
                "--actor",
                "--if-goal-revision",
            ][..],
        ),
    ] {
        let text = help(topic);
        assert!(text.contains("Usage:"), "{topic} omits usage");
        for token in tokens {
            assert!(text.contains(token), "{topic} omits {token}: {text}");
        }
    }
}

#[test]
fn completion_and_overview_expose_the_shipped_v2_presets() {
    for shell in ["bash", "zsh", "fish"] {
        let output = run(&["completions", shell]);
        assert!(
            output.status.success(),
            "{shell} completion failed: {output:?}"
        );
        let script = String::from_utf8(output.stdout).expect("completion must be UTF-8");
        for preset in ["bug-fix-v2", "sw-dev-v2"] {
            assert!(script.contains(preset), "{shell} completion omits {preset}");
        }
    }

    let overview = help("overview");
    for token in [
        "sw-dev-v2",
        "session.decide",
        "session.rework",
        "goal.define",
        "goal.revise",
        "goal.assess_criterion",
        "does not judge their semantic truth",
    ] {
        assert!(overview.contains(token), "overview omits {token}");
    }
}

#[test]
fn v2_operator_example_uses_contract_owned_json_fields_and_denies_semantic_authority() {
    let output: Value = serde_json::from_str(OUTPUT_V2_SCHEMA).unwrap();
    let status: Value = serde_json::from_str(STATUS_V2_SCHEMA).unwrap();
    let next: Value = serde_json::from_str(NEXT_V2_SCHEMA).unwrap();
    let components: Value = serde_json::from_str(COMPONENTS_SCHEMA).unwrap();
    let criterion: Value = serde_json::from_str(CRITERION_RESULT_SCHEMA).unwrap();

    for path in [
        "workspace.uuid",
        "result.session.id",
        "result.session.revision",
        "result.current.attempt.attempt_id",
        "result.goal_revision",
        "result.items[].item_id",
        "result.items[].revision",
        "result.allowed_option_ids[]",
        "result.allowed_manual_rework_targets[]",
        "result.queue.pending_mutations",
        "result.node.graph_node_id",
        "result.attempt.attempt_id",
        "result.revision",
        "result.allowed_actions[]",
        "result.suggestions[].argv",
        "result.effect",
        "result.target_graph_node_id",
        "result.target_attempt_id",
        "result.result.status",
        "result.complete",
        "result.determined_outcome",
    ] {
        assert!(V2_WORKFLOW.contains(path), "v2 workflow omits {path}");
    }

    assert!(output["properties"]["workspace"].is_object());
    for field in [
        "session",
        "current",
        "goal_revision",
        "items",
        "allowed_option_ids",
        "allowed_manual_rework_targets",
        "queue",
    ] {
        assert!(
            status["properties"][field].is_object(),
            "status omits {field}"
        );
    }
    for field in [
        "node",
        "attempt",
        "revision",
        "allowed_actions",
        "suggestions",
    ] {
        assert!(next["properties"][field].is_object(), "next omits {field}");
    }
    for (definition, fields) in [
        ("sessionIdentity", &["id", "revision"][..]),
        ("currentIdentity", &["attempt"][..]),
        ("attemptIdentity", &["attempt_id"][..]),
        ("compactItem", &["item_id", "revision"][..]),
        ("queue", &["pending_mutations"][..]),
        ("nodeIdentity", &["graph_node_id"][..]),
        ("suggestion", &["argv"][..]),
    ] {
        for field in fields {
            assert!(
                components["$defs"][definition]["properties"][field].is_object(),
                "{definition} omits {field}"
            );
        }
    }
    for field in ["goal_revision", "result", "complete", "determined_outcome"] {
        assert!(
            criterion["properties"][field].is_object(),
            "criterion result omits {field}"
        );
    }
    assert!(components["$defs"]["criterionResult"]["properties"]["status"].is_object());

    for disclaimer in [
        "Podway\nenforces the declared progression rules, but it does not run checks or decide",
        "truth determination made by Podway",
        "not a second semantic authority",
    ] {
        assert!(
            V2_WORKFLOW.contains(disclaimer),
            "v2 workflow omits disclaimer: {disclaimer}"
        );
    }
}
