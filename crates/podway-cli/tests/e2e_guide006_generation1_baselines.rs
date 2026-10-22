//! GUIDE-006: frozen generation-1 CLI baselines.
//!
//! Pins the complete `podway.output/v4` stdout envelopes of the default CLI
//! `observe`, `status` (standard tier) and `next` (prepared
//! `podway.prepared-next-result/v3` and running `podway.next-result/v5`)
//! commands through the real binary in isolated fixtures.
//!
//! Freeze rule (Coordinator decision D4): the expected-value files under
//! `guide006_baselines/` are frozen at the GUIDE-006 commit. Through GUIDE-011,
//! `git diff --exit-code <guide006-commit> -- crates/podway-cli/tests/guide006_baselines`
//! must stay empty, and every hunk of this module's own diff must be a
//! mechanical dossier :302-304 update (for example the GUIDE-009 generation
//! fields, dossier :446-448) or an added explicit-generation-1 reuse call. Any
//! change to the normalizer rules, the fixture Procedure source, the scenario
//! steps or the assertions is a freeze violation that needs Task-Evaluator
//! review. This module stays registered in `e2e_suite.rs`, so every later
//! `make test` reruns these cases unchanged.
//!
//! Baseline policy (Coordinator decisions D1/D2): each expected value is the
//! exact normalized stdout (compact JSON with the emitted trailing newline),
//! loaded read-only with `include_str!`. On mismatch the normalized actual is
//! written to `CARGO_TARGET_TMPDIR/guide006/<test>.actual`; installing a
//! baseline is a Coordinator-approved `cp` of that file. Nothing in this module
//! writes into `guide006_baselines/`.
//!
//! Interpretation (Coordinator decision D7): "the default observe, status and
//! next CLI outputs" means the default-generation `--json` envelopes (default
//! tier); the human leg is intentionally not pinned because the dossier lists
//! human-surface text among intended changes (:580-588).
//!
//! Negative tests (amendment A3) run their scenarios live inside the test; no
//! raw actual is committed. The scenario helpers take extra global CLI options
//! inserted after `--json` and before the command word (Coordinator decision
//! D6), so GUIDE-009 can pass `--contract-generation 1` and compare against the
//! same baseline files without changing any helper signature.

use std::{fs, path::Path};

use podway_protocol::OutputEnvelopeV4;
use serde_json::Value;

use super::cli_fixture::Fixture;

const GUIDE006_CLI_PROCEDURE: &str = r#"{"schema":"podway.procedure/v2","id":"guide006-work","version":"1",
    "name":"Guide006 work","purpose":"Exercise generation-1 observation states.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record work.",
        "items":[
            {"id":"confirm","type":"confirm","prompt":"Confirm.","required":true},
            {"id":"note","type":"text","prompt":"Record a note.","required":false,"max_length":100},
            {"id":"pick","type":"choice","prompt":"Pick.","required":false,"choices":["yes","no"]},
            {"id":"count","type":"integer","prompt":"Count.","required":false,"minimum":0,"maximum":9}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

/// The raw stdout of one scenario run; the normalizer derives every volatile
/// value from the envelope itself.
pub(crate) struct Guide006CliActual {
    pub raw: String,
}

impl Guide006CliActual {
    pub fn normalized(&self) -> String {
        guide006_normalize_cli(&self.raw)
    }
}

fn guide006_cli_run(fixture: &Fixture, global_extra: &[&str], command: &[&str]) -> Value {
    let mut argv: Vec<&str> = vec!["--json"];
    argv.extend_from_slice(global_extra);
    argv.extend_from_slice(command);
    let output = fixture.raw_output(&argv);
    assert_eq!(output.status.code(), Some(0), "{argv:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{argv:?}: {output:?}");
    serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
        .unwrap_or_else(|error| panic!("{argv:?}: {error:?}"))
        .into_value()
}

fn guide006_cli_stdout(fixture: &Fixture, global_extra: &[&str], command: &[&str]) -> String {
    let mut argv: Vec<&str> = vec!["--json"];
    argv.extend_from_slice(global_extra);
    argv.extend_from_slice(command);
    let output = fixture.raw_output(&argv);
    assert_eq!(output.status.code(), Some(0), "{argv:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{argv:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn guide006_cli_error_stdout(
    fixture: &Fixture,
    global_extra: &[&str],
    command: &[&str],
    exit: i32,
) -> String {
    let mut argv: Vec<&str> = vec!["--json"];
    argv.extend_from_slice(global_extra);
    argv.extend_from_slice(command);
    let output = fixture.raw_output(&argv);
    assert_eq!(output.status.code(), Some(exit), "{argv:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{argv:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn guide006_cli_start(fixture: &Fixture, global_extra: &[&str]) {
    guide006_cli_run(
        fixture,
        global_extra,
        &["init", "--idempotency-key", "guide006-init"],
    );
    fs::write(fixture.0.join("procedure.json"), GUIDE006_CLI_PROCEDURE).unwrap();
    let preview = guide006_cli_run(
        fixture,
        global_extra,
        &["procedure", "preview", "procedure.json"],
    );
    let digest = preview["result"]["procedure_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    guide006_cli_run(
        fixture,
        global_extra,
        &[
            "start",
            "--procedure",
            "procedure.json",
            "--expect-procedure-digest",
            &digest,
            "--task",
            "Guide006 generation-1 baseline",
            "--idempotency-key",
            "guide006-start",
        ],
    );
}

pub(crate) fn guide006_cli_observe_prepared(global_extra: &[&str]) -> Guide006CliActual {
    let fixture = Fixture::new();
    guide006_cli_start(&fixture, global_extra);
    Guide006CliActual {
        raw: guide006_cli_stdout(&fixture, global_extra, &["observe"]),
    }
}

pub(crate) fn guide006_cli_observe_running(global_extra: &[&str]) -> Guide006CliActual {
    let fixture = Fixture::new();
    guide006_cli_start(&fixture, global_extra);
    guide006_cli_run(
        &fixture,
        global_extra,
        &["begin", "--idempotency-key", "guide006-begin"],
    );
    guide006_cli_run(
        &fixture,
        global_extra,
        &[
            "set",
            "note",
            "A recorded note.",
            "--idempotency-key",
            "guide006-set-note",
        ],
    );
    Guide006CliActual {
        raw: guide006_cli_stdout(&fixture, global_extra, &["observe"]),
    }
}

pub(crate) fn guide006_cli_status_running(global_extra: &[&str]) -> Guide006CliActual {
    let fixture = Fixture::new();
    guide006_cli_start(&fixture, global_extra);
    guide006_cli_run(
        &fixture,
        global_extra,
        &["begin", "--idempotency-key", "guide006-begin"],
    );
    guide006_cli_run(
        &fixture,
        global_extra,
        &[
            "set",
            "note",
            "A recorded note.",
            "--idempotency-key",
            "guide006-set-note",
        ],
    );
    Guide006CliActual {
        raw: guide006_cli_stdout(&fixture, global_extra, &["status"]),
    }
}

pub(crate) fn guide006_cli_next_prepared(global_extra: &[&str]) -> Guide006CliActual {
    let fixture = Fixture::new();
    guide006_cli_start(&fixture, global_extra);
    Guide006CliActual {
        raw: guide006_cli_stdout(&fixture, global_extra, &["next"]),
    }
}

pub(crate) fn guide006_cli_next_running(global_extra: &[&str]) -> Guide006CliActual {
    let fixture = Fixture::new();
    guide006_cli_start(&fixture, global_extra);
    guide006_cli_run(
        &fixture,
        global_extra,
        &["begin", "--idempotency-key", "guide006-begin"],
    );
    guide006_cli_run(
        &fixture,
        global_extra,
        &[
            "set",
            "note",
            "A recorded note.",
            "--idempotency-key",
            "guide006-set-note",
        ],
    );
    Guide006CliActual {
        raw: guide006_cli_stdout(&fixture, global_extra, &["next"]),
    }
}

fn is_uuid_shaped(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let mut rest = value;
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            let (dash, tail) = rest.split_at(1);
            if dash != "-" {
                return false;
            }
            rest = tail;
        }
        if rest.len() < *width {
            return false;
        }
        let (group, tail) = rest.split_at(*width);
        if !group
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return false;
        }
        rest = tail;
    }
    rest.is_empty()
}

fn is_timestamp_shaped(value: &str) -> bool {
    fn digits(bytes: &[u8]) -> bool {
        !bytes.is_empty() && bytes.iter().all(|byte| byte.is_ascii_digit())
    }
    let bytes = value.as_bytes();
    bytes.len() == 24
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
        && digits(&bytes[0..4])
        && digits(&bytes[5..7])
        && digits(&bytes[8..10])
        && digits(&bytes[11..13])
        && digits(&bytes[14..16])
        && digits(&bytes[17..19])
        && digits(&bytes[20..23])
}

/// Text-level scan: a shaped literal anywhere, even embedded inside a longer
/// string value, means normalization hid something.
fn text_has_shaped_window(text: &str, width: usize, shaped: fn(&str) -> bool) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= width
        && (0..=bytes.len() - width).any(|start| {
            std::str::from_utf8(&bytes[start..start + width])
                .map(shaped)
                .unwrap_or(false)
        })
}

fn assert_no_generated_literals(text: &str) {
    assert!(
        !text_has_shaped_window(text, 36, is_uuid_shaped),
        "a UUID-shaped literal survived CLI normalization"
    );
    assert!(
        !text_has_shaped_window(text, 24, is_timestamp_shaped),
        "a timestamp-shaped literal survived CLI normalization"
    );
}

fn collect_generated_literals(
    value: &Value,
    reserved: &[String],
    uuids: &mut Vec<String>,
    timestamps: &mut Vec<String>,
) {
    match value {
        Value::String(text) => {
            if reserved.iter().any(|literal| literal == text) {
                return;
            }
            if is_uuid_shaped(text) && !uuids.contains(text) {
                uuids.push(text.clone());
            }
            if is_timestamp_shaped(text) && !timestamps.contains(text) {
                timestamps.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_generated_literals(item, reserved, uuids, timestamps);
            }
        }
        Value::Object(members) => {
            for member in members.values() {
                collect_generated_literals(member, reserved, uuids, timestamps);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Self-contained four-pass normalizer (Coordinator decision D3): the envelope
/// `request_id` and `generated_at` first, then the worktree root parsed from
/// `workspace.root` (required on success envelopes; error envelopes carry no
/// workspace member), then first-appearance placeholders for the remaining
/// generated UUID and timestamp literals in document order. Equal literals
/// share one placeholder, distinct literals get distinct ordinals, and the
/// final text must contain no shaped literal at all.
pub(crate) fn guide006_normalize_cli(stdout: &str) -> String {
    let trailing_newline = stdout.ends_with('\n');
    let body = stdout.strip_suffix('\n').unwrap_or(stdout);
    let envelope: Value = serde_json::from_str(body).expect("the CLI actual is one envelope");
    let request_id = envelope["request_id"]
        .as_str()
        .expect("the CLI envelope has a request_id")
        .to_owned();
    let generated_at = envelope["generated_at"]
        .as_str()
        .expect("the CLI envelope has a generated_at")
        .to_owned();
    let mut reserved = vec![request_id.clone(), generated_at.clone()];
    let mut text = body
        .replace(
            &format!("\"request_id\":\"{request_id}\""),
            "\"request_id\":\"<request-id>\"",
        )
        .replace(
            &format!("\"generated_at\":\"{generated_at}\""),
            "\"generated_at\":\"<generated-at>\"",
        );
    let root = match envelope.get("code") {
        None => Some(
            envelope["workspace"]["root"]
                .as_str()
                .expect("a CLI success envelope has a workspace root")
                .to_owned(),
        ),
        Some(_) => envelope
            .get("workspace")
            .and_then(|workspace| workspace.get("root"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    if let Some(root) = root {
        assert_eq!(
            body.matches(&root).count(),
            1,
            "the CLI envelope must contain exactly one worktree-root occurrence"
        );
        reserved.push(root.clone());
        text = text.replace(
            &format!("\"root\":\"{root}\""),
            "\"root\":\"<worktree-root>\"",
        );
    }
    let mut uuids = Vec::new();
    let mut timestamps = Vec::new();
    collect_generated_literals(&envelope, &reserved, &mut uuids, &mut timestamps);
    for (index, literal) in uuids.iter().enumerate() {
        text = text.replace(literal, &format!("<id-{}>", index + 1));
    }
    for (index, literal) in timestamps.iter().enumerate() {
        text = text.replace(literal, &format!("<ts-{}>", index + 1));
    }
    assert_no_generated_literals(&text);
    let mut normalized = text;
    if trailing_newline {
        normalized.push('\n');
    }
    normalized
}

/// Baseline loader for GUIDE-009 reuse (Coordinator decision D6); read-only.
pub(crate) fn guide006_cli_baseline(name: &str) -> &'static str {
    match name {
        "guide006_cli_observe_prepared_baseline" => {
            include_str!("guide006_baselines/guide006_cli_observe_prepared_baseline.json")
        }
        "guide006_cli_observe_running_baseline" => {
            include_str!("guide006_baselines/guide006_cli_observe_running_baseline.json")
        }
        "guide006_cli_status_running_baseline" => {
            include_str!("guide006_baselines/guide006_cli_status_running_baseline.json")
        }
        "guide006_cli_next_prepared_baseline" => {
            include_str!("guide006_baselines/guide006_cli_next_prepared_baseline.json")
        }
        "guide006_cli_next_running_baseline" => {
            include_str!("guide006_baselines/guide006_cli_next_running_baseline.json")
        }
        unknown => panic!("unknown GUIDE-006 CLI baseline: {unknown}"),
    }
}

fn guide006_assert_baseline(name: &str, actual: &Guide006CliActual) {
    let baseline = guide006_cli_baseline(name);
    let normalized = actual.normalized();
    if baseline != normalized {
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("guide006");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{name}.actual"));
        fs::write(&path, &normalized).unwrap();
        panic!(
            "GUIDE-006 CLI baseline mismatch for {name}; the normalized actual was written to {}",
            path.display()
        );
    }
}

#[test]
fn guide006_cli_observe_prepared_baseline() {
    let actual = guide006_cli_observe_prepared(&[]);
    guide006_assert_baseline("guide006_cli_observe_prepared_baseline", &actual);
}

#[test]
fn guide006_cli_observe_running_baseline() {
    let actual = guide006_cli_observe_running(&[]);
    guide006_assert_baseline("guide006_cli_observe_running_baseline", &actual);
}

#[test]
fn guide006_cli_status_running_baseline() {
    let actual = guide006_cli_status_running(&[]);
    guide006_assert_baseline("guide006_cli_status_running_baseline", &actual);
}

#[test]
fn guide006_cli_next_prepared_baseline() {
    let actual = guide006_cli_next_prepared(&[]);
    guide006_assert_baseline("guide006_cli_next_prepared_baseline", &actual);
}

#[test]
fn guide006_cli_next_running_baseline() {
    let actual = guide006_cli_next_running(&[]);
    guide006_assert_baseline("guide006_cli_next_running_baseline", &actual);
}

#[test]
fn guide006_cli_running_observe_is_deterministic_across_fresh_fixtures() {
    let first = guide006_cli_observe_running(&[]);
    let second = guide006_cli_observe_running(&[]);
    assert_eq!(
        first.normalized(),
        second.normalized(),
        "the running observe scenario must normalize identically across fresh fixtures"
    );
    assert_eq!(
        first.normalized(),
        guide006_cli_baseline("guide006_cli_observe_running_baseline")
    );
}

#[test]
fn guide006_cli_perturbed_actuals_differ_from_their_baselines() {
    let running = guide006_cli_observe_running(&[]);
    let running_baseline = guide006_cli_baseline("guide006_cli_observe_running_baseline");
    assert_eq!(running.normalized(), running_baseline);
    let envelope: Value = serde_json::from_str(running.raw.trim_end_matches('\n')).unwrap();
    let reparsed = |value: Value| {
        format!(
            "{}\n",
            guide006_normalize_cli(&serde_json::to_string(&value).unwrap())
        )
    };
    // Control: the unperturbed envelope through the same parse-and-reserialize
    // path still normalizes to the baseline.
    assert_eq!(reparsed(envelope.clone()), running_baseline);
    let session_id = envelope["result"]["status"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let attempt_id = envelope["result"]["status"]["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // One mutation-template argv element changed.
    let mut perturbed = envelope.clone();
    perturbed["result"]["mutation_templates"][0]["argv"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("--guide006-perturbation".into()));
    assert_ne!(reparsed(perturbed), running_baseline);

    // One active-item revision changed.
    let mut perturbed = envelope.clone();
    perturbed["result"]["active_items"][0]["revision"] = Value::Number(9.into());
    assert_ne!(reparsed(perturbed), running_baseline);

    // One identifier occurrence replaced by another distinct identifier from
    // the same actual (amendment A2: the multiplicities of the two first-
    // appearance placeholders change, so the normalized bytes differ). The
    // control runs the identical call unperturbed first.
    assert_eq!(guide006_normalize_cli(&running.raw), running_baseline);
    let swapped = guide006_normalize_cli(&running.raw.replacen(&session_id, &attempt_id, 1));
    assert_ne!(swapped, running_baseline);

    // One result member removed.
    let mut perturbed = envelope.clone();
    perturbed["result"]
        .as_object_mut()
        .unwrap()
        .remove("active_items_total");
    assert_ne!(reparsed(perturbed), running_baseline);

    // One error-details field changed on a live error envelope. The CLI error
    // envelopes are GUIDE-004's freeze, so this class compares against the
    // unperturbed live control instead of a committed baseline: a wrong item
    // revision yields `ITEM_REVISION_CONFLICT` details with numeric fields.
    let fixture = Fixture::new();
    guide006_cli_start(&fixture, &[]);
    guide006_cli_run(
        &fixture,
        &[],
        &["begin", "--idempotency-key", "guide006-begin"],
    );
    let error = guide006_cli_error_stdout(
        &fixture,
        &[],
        &[
            "set",
            "note",
            "Conflicting value.",
            "--if-item-revision",
            "9",
            "--idempotency-key",
            "guide006-conflict",
        ],
        4,
    );
    let control = guide006_normalize_cli(&error);
    let mut perturbed: Value = serde_json::from_str(error.trim_end_matches('\n')).unwrap();
    assert_eq!(perturbed["code"], "ITEM_REVISION_CONFLICT");
    let details = perturbed["details"].as_object().unwrap().clone();
    let revision_field = details
        .keys()
        .find(|field| details[field.as_str()].is_number())
        .unwrap()
        .clone();
    perturbed["details"][revision_field.as_str()] = Value::Number(8.into());
    assert_ne!(
        guide006_normalize_cli(&format!("{}\n", serde_json::to_string(&perturbed).unwrap())),
        control
    );
    assert_eq!(
        guide006_normalize_cli(&format!(
            "{}\n",
            serde_json::to_string(
                &serde_json::from_str::<Value>(error.trim_end_matches('\n')).unwrap()
            )
            .unwrap()
        )),
        control,
        "the unperturbed re-serialized control must normalize identically"
    );
}

#[test]
fn guide006_cli_normalizer_requires_volatile_literals_and_hides_every_generated_value() {
    fn rejection_reason(rejected: std::thread::Result<()>) -> String {
        let payload = rejected.expect_err("the normalizer must reject this input");
        if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_owned()
        } else {
            panic!("the rejection payload carried no message");
        }
    }

    // (a) A success-shaped envelope with valid volatile members but no
    // workspace root is rejected for exactly that reason.
    let reason = rejection_reason(std::panic::catch_unwind(|| {
        guide006_normalize_cli(
            "{\"schema\":\"podway.output/v4\",\"request_id\":\"00000000-0000-4000-8000-000000000901\",\"command\":\"session.observe\",\"generated_at\":\"2026-10-23T00:00:00.000Z\",\"result\":{},\"warnings\":[]}\n",
        );
    }));
    assert!(
        reason.contains("a CLI success envelope has a workspace root"),
        "{reason}"
    );

    // (b) An error-shaped envelope whose only violation is a shaped literal
    // embedded inside a longer string value is rejected for exactly that
    // reason.
    let reason = rejection_reason(std::panic::catch_unwind(|| {
        guide006_normalize_cli(&format!(
            "{{\"schema\":\"podway.error/v2\",\"request_id\":\"00000000-0000-4000-8000-000000000902\",\"command\":\"item.set\",\"generated_at\":\"2026-10-23T00:00:00.000Z\",\"code\":\"ITEM_REVISION_CONFLICT\",\"message\":\"The item changed after it was observed.\",\"details\":{{\"path\":\"embedded-{}\"}},\"retryable\":true,\"exit_code\":4}}\n",
            "00000000-0000-4000-8000-00000000099a"
        ));
    }));
    assert!(
        reason.contains("a UUID-shaped literal survived CLI normalization"),
        "{reason}"
    );

    // (c) A real scenario normalizes to exactly its frozen baseline.
    let actual = guide006_cli_observe_running(&[]);
    assert_eq!(
        actual.normalized(),
        guide006_cli_baseline("guide006_cli_observe_running_baseline")
    );
}
