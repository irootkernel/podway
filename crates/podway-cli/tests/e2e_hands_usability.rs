//! Reproduce the HANDS usability changes through the native CLI.

use std::process::Output;

use podway_protocol::ErrorEnvelopeV2;
use serde_json::{Value, json};

use super::cli_fixture::Fixture;

const BASIS_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_DIGEST: &str =
    "sha256:2222222222222222222222222222222222222222222222222222222222222222";
const GROUP_MISMATCH: &str =
    "The supplied or retained group definition does not match the expected digest.";
const GROUP_ACTIVATION: &str =
    "Group activation requires the current unselected base anchor and valid prior work.";
const REQUEST_INVALID: &str = "Request is malformed or violates schema.";
const ACTIVATE: &[&str] = &["group", "activate", "--stdin"];

fn refuse(output: Output, code: &str, exit: i32, message: &str) {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout).unwrap();
    let error = error.as_value();
    assert_eq!(error["code"], code);
    assert_eq!(error["exit_code"], exit);
    assert_eq!(error["retryable"], exit == 4);
    assert_eq!(error["details"], json!({}));
    assert_eq!(error["message"], message);
}

fn observe(fixture: &Fixture) -> Value {
    fixture.run(&["--contract-generation", "2", "observe"])
}

fn template(fixture: &Fixture, command: &str, key: &str) -> Value {
    let observation = observe(fixture);
    let template = observation["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == command)
        .unwrap();
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!(key);
    input["working_copy_basis"] = json!({"descriptor":"Owned test fixture","digest":BASIS_DIGEST});
    input
}

fn small_change() -> Fixture {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Exercise HANDS refusals",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "set",
        "scope-summary",
        "Record fixture scope.",
        "--idempotency-key",
        "scope",
    ]);
    fixture
}

fn activate(fixture: &Fixture, input: &Value) -> Output {
    fixture.output_with_stdin(ACTIVATE, input.to_string().as_bytes())
}

fn activate_first(fixture: &Fixture) -> Value {
    let output = activate(
        fixture,
        &template(fixture, "group.activate", "activate-first"),
    );
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn group_review(fixture: &Fixture, verdict: &str, stage: &str, score: &str, exit: &str) {
    fn set(fixture: &Fixture, item: &str, text: &str) {
        fixture.run(&["set", item, text, "--idempotency-key", item]);
    }
    fn complete(fixture: &Fixture, key: &str) {
        fixture.run(&["complete", "--idempotency-key", key]);
    }
    set(
        fixture,
        "implementation-summary",
        "Bounded native fixture work.",
    );
    complete(fixture, "complete-implement");
    set(
        fixture,
        "verification-command",
        "Record fixture verification.",
    );
    set(fixture, "verification-exit-status", exit);
    complete(fixture, "complete-verify");
    set(fixture, "assessment-verdict", verdict);
    set(fixture, "assessment-stage", stage);
    set(fixture, "assessment-score", score);
    if stage == "scored" {
        set(
            fixture,
            "scored-basis",
            "Exercise the scored replan route in an isolated fixture.",
        );
    }
    complete(fixture, "complete-assessment");
}

fn replacement_fixture() -> Fixture {
    let fixture = small_change();
    activate_first(&fixture);
    group_review(&fixture, "failure", "scored", "80", "1");
    let mut replan = template(&fixture, "group.replan", "replan");
    replan["reason"] = json!("Exercise replacement activation refusals.");
    replan["feedback"] = json!("Revisit the isolated fixture scope.");
    let output = fixture.output_with_stdin(
        &["group", "replan", "--stdin"],
        replan.to_string().as_bytes(),
    );
    assert!(output.status.success(), "{output:?}");
    fixture.run(&[
        "set",
        "scope-summary",
        "Record the replacement scope.",
        "--idempotency-key",
        "scope-revised",
    ]);
    fixture
}

fn assert_no_outcome(fixture: &Fixture, key: &str) {
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["found"],
        false
    );
}

#[test]
fn hands001_blank_reasons_name_the_cli_option() {
    let fixture = small_change();
    let before = observe(&fixture);
    for route in ["block", "retry", "cancel"] {
        refuse(
            fixture.output(&[route, "--reason", "   ", "--idempotency-key", route]),
            "REQUEST_INVALID",
            2,
            "--reason must contain a non-whitespace character",
        );
        assert_no_outcome(&fixture, route);
    }
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_stdin_reports_the_replacement_placeholder_and_pointer() {
    let fixture = replacement_fixture();
    let input = template(&fixture, "group.activate", "unfilled-prior-work");
    assert_eq!(input["prior_work"], "<prior-work-dispositions-array>");
    let before = observe(&fixture);
    refuse(
        activate(&fixture, &input),
        "REQUEST_INVALID",
        2,
        "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<prior-work-dispositions-array>\" at /prior_work",
    );
    assert_no_outcome(&fixture, "unfilled-prior-work");
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_stdin_pointer_preserves_json_and_escapes_human_output() {
    let fixture = Fixture::new();
    let diagnostic = "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\"";
    for (key, escaped) in [
        ("row\nname", r"row\nname"),
        (r"row\nname", r"row\\nname"),
        ("row\u{1b}[31m", r"row\u{1b}[31m"),
    ] {
        let document = json!({
            "schema": "podway.group-activate-input/v1",
            (key): "<first>",
        });
        let input = document.to_string();
        let output = fixture.human_output_with_stdin(ACTIVATE, input.as_bytes());
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("error: {diagnostic} at /{escaped}\n"),
        );
        refuse(
            activate(&fixture, &document),
            "REQUEST_INVALID",
            2,
            &format!("{diagnostic} at /{key}"),
        );
    }
}

#[test]
fn hands001_stdin_pointer_overflow_keeps_the_same_refusal_class() {
    let fixture = Fixture::new();
    let key = "x".repeat(65_536);
    let document = json!({
        "schema": "podway.group-activate-input/v1",
        (key): "<first>"
    });
    refuse(
        activate(&fixture, &document),
        "REQUEST_INVALID",
        2,
        "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\" (RFC 6901 pointer exceeds the error message length limit)",
    );
}

#[test]
fn hands001_human_pointer_escape_expansion_stays_bounded() {
    let fixture = Fixture::new();
    let diagnostic = "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\"";
    for key in ["\u{1b}".repeat(20_000), "\\".repeat(40_000)] {
        let document = json!({
            "schema": "podway.group-activate-input/v1",
            (&key): "<first>",
        });
        let input = document.to_string();
        let output = fixture.human_output_with_stdin(ACTIVATE, input.as_bytes());
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        let length = stderr.chars().count();
        assert!(
            length <= 65_536,
            "human diagnostic output was {length} scalars"
        );
        assert_eq!(
            stderr,
            format!(
                "error: {diagnostic} (escaped RFC 6901 pointer exceeds the error message length limit)\n"
            ),
        );
        refuse(
            activate(&fixture, &document),
            "REQUEST_INVALID",
            2,
            &format!("{diagnostic} at /{key}"),
        );
    }
}

#[test]
fn hands001_native_unknown_criterion_refusal_leaves_no_state_or_outcome() {
    let (fixture, before, _) = super::e2e_adopt001_scoped_cli::scoped_criterion_fixture(true);
    let template = before["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == "goal.assess_criteria")
        .unwrap();
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!("unknown-criterion");
    input["actor"] = Value::Null;
    input["assessments"] = json!([{
        "criterion": "unknown",
        "status": "satisfied",
        "reason": "Exercise the unknown criterion refusal.",
        "citations": []
    }]);
    refuse(
        fixture.output_with_stdin(
            &["goal", "assess-criteria", "--stdin"],
            input.to_string().as_bytes(),
        ),
        "REQUEST_INVALID",
        2,
        &format!("{REQUEST_INVALID} (criterion assessment repeats or names an unknown criterion)."),
    );
    assert_no_outcome(&fixture, "unknown-criterion");
    let after = observe(&fixture);
    assert_eq!(after["result"], before["result"]);
    assert_eq!(after["workspace"], before["workspace"]);
}

#[test]
fn hands001_native_off_anchor_activation_refusal_leaves_no_state_or_outcome() {
    let fixture = small_change();
    let mut input = template(&fixture, "group.activate", "off-anchor");
    activate_first(&fixture);
    let before = observe(&fixture);
    assert!(
        before["result"]["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["command"] != "group.activate")
    );
    let mut cursor = before["result"]["cursor"].clone();
    cursor.as_object_mut().unwrap().remove("session");
    input["context"]["cursor"] = cursor;
    refuse(
        activate(&fixture, &input),
        "GROUP_ACTIVATION_NOT_ALLOWED",
        1,
        &format!(
            "{GROUP_ACTIVATION} The current attempt is not the unselected base anchor; observe and use the current group.activate template."
        ),
    );
    assert_no_outcome(&fixture, "off-anchor");
    let after = observe(&fixture);
    assert_eq!(after["result"], before["result"]);
    assert_eq!(after["workspace"], before["workspace"]);
}

#[test]
fn hands001_base_history_requires_the_group_execution_for_retained_group_digests() {
    let fixture = small_change();
    let activated = activate_first(&fixture);
    let execution = activated["result"]["execution_id"].as_str().unwrap();
    let selected = observe(&fixture);
    let digest = selected["result"]["selection"]["definition_digest"]
        .as_str()
        .unwrap();
    group_review(&fixture, "success", "not-scored", "0", "0");
    fixture.run(&[
        "decide",
        "--option",
        "ready",
        "--reason",
        "Fixture verification passed.",
        "--idempotency-key",
        "ready",
    ]);
    for (item, value) in [
        ("implementation-summary", "Return current fixture work."),
        (
            "verification-command",
            "Record successful fixture verification.",
        ),
        ("verification-exit-status", "0"),
    ] {
        fixture.run(&[
            "set",
            item,
            value,
            "--idempotency-key",
            &format!("return-{item}"),
        ]);
    }
    fixture.run(&["group", "return", "--idempotency-key", "return"]);
    let before = observe(&fixture);
    let session = before["session"]["id"].as_str().unwrap();
    for generation in ["1", "2"] {
        for route in ["dag", "attempts"] {
            let mut args = vec![
                "--contract-generation",
                generation,
                "history",
                route,
                "--session",
                session,
                "--definition-digest",
                digest,
            ];
            refuse(
                fixture.output(&args),
                "REQUEST_INVALID",
                2,
                &format!(
                    "{REQUEST_INVALID} --definition-digest names a retained group definition; add --group-execution <uuid> from group list."
                ),
            );
            *args.last_mut().unwrap() = OTHER_DIGEST;
            refuse(
                fixture.output(&args),
                "GROUP_DEFINITION_MISMATCH",
                4,
                GROUP_MISMATCH,
            );
            *args.last_mut().unwrap() = digest;
            args.extend(["--group-execution", execution]);
            fixture.run(&args);
        }
    }
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_wrong_activation_digest_reports_admitted_source_identity() {
    let fixture = small_change();
    let mut input = template(&fixture, "group.activate", "wrong-definition");
    let digest = input["expected_definition"].as_str().unwrap().to_owned();
    input["expected_definition"] = json!(OTHER_DIGEST);
    let before = observe(&fixture);
    refuse(
        activate(&fixture, &input),
        "GROUP_DEFINITION_MISMATCH",
        4,
        &format!("{GROUP_MISMATCH} Supplied source digest: {digest}."),
    );
    assert_no_outcome(&fixture, "wrong-definition");
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_replacement_refuses_empty_and_unknown_prior_work_with_distinct_reasons() {
    let fixture = replacement_fixture();
    let mut input = template(&fixture, "group.activate", "empty-prior-work");
    input["prior_work"] = json!([]);
    let before = observe(&fixture);
    refuse(
        activate(&fixture, &input),
        "GROUP_ACTIVATION_NOT_ALLOWED",
        1,
        &format!(
            "{GROUP_ACTIVATION} Prior work is invalid (activation history requires one to 128 prior-work dispositions after the first group)."
        ),
    );
    assert_no_outcome(&fixture, "empty-prior-work");
    input["context"]["idempotency_key"] = json!("unknown-prior-work");
    input["prior_work"] = json!([{
        "group_execution_id":"00000000-0000-4000-8000-000000000012",
        "choice":"revalidate","rationale":"Exercise an unknown fixture execution."
    }]);
    refuse(
        activate(&fixture, &input),
        "GROUP_ACTIVATION_NOT_ALLOWED",
        1,
        &format!(
            "{GROUP_ACTIVATION} Prior work is invalid (prior work does not identify a retained same-session group)."
        ),
    );
    assert_no_outcome(&fixture, "unknown-prior-work");
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_removal_reports_every_missing_confirmation_in_order() {
    let fixture = Fixture::new();
    let init = fixture.run(&["init", "--idempotency-key", "init"]);
    let workspace = init["workspace"]["uuid"].as_str().unwrap();
    let before = fixture.podway_snapshot();
    let prefix = "JSON workspace removal requires --force, --yes and --if-workspace-uuid; missing:";
    for (flags, missing) in [
        (vec![], "--force, --yes, --if-workspace-uuid"),
        (vec!["--force"], "--yes, --if-workspace-uuid"),
        (vec!["--yes"], "--force, --if-workspace-uuid"),
        (vec!["--if-workspace-uuid", workspace], "--force, --yes"),
        (vec!["--force", "--yes"], "--if-workspace-uuid"),
        (vec!["--force", "--if-workspace-uuid", workspace], "--yes"),
        (vec!["--yes", "--if-workspace-uuid", workspace], "--force"),
    ] {
        let mut args = vec![
            "workspace",
            "remove",
            "--idempotency-key",
            "refused-removal",
        ];
        args.extend(flags);
        refuse(
            fixture.output(&args),
            "REQUEST_INVALID",
            2,
            &format!("{prefix} {missing}"),
        );
    }
    let prefix =
        "non-interactive workspace removal requires --yes and --if-workspace-uuid; missing:";
    for (flags, missing) in [
        (vec![], "--yes, --if-workspace-uuid"),
        (vec!["--yes"], "--if-workspace-uuid"),
        (vec!["--if-workspace-uuid", workspace], "--yes"),
    ] {
        let mut args = vec![
            "workspace",
            "remove",
            "--idempotency-key",
            "refused-removal",
        ];
        args.extend(flags);
        let output = fixture.raw_output(&args);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr, format!("error: {prefix} {missing}\n"));
    }
    assert_eq!(fixture.podway_snapshot(), before);
    assert_no_outcome(&fixture, "refused-removal");
}
