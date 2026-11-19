//! Freeze the default successor observation surface through the real CLI.
//! The previous v5/v6 golden files remain archived byte-for-byte.

use podway_protocol::OutputEnvelopeV4;
use serde_json::Value;

use super::cli_fixture::Fixture;

pub(crate) struct Guide006CliActual {
    pub raw: String,
}

impl Guide006CliActual {
    pub fn normalized(&self) -> String {
        normalize(&self.raw)
    }
}

fn fixture(running: bool) -> Fixture {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Scoped observation baseline",
        "--idempotency-key",
        "start",
    ]);
    if running {
        fixture.run(&["begin", "--idempotency-key", "begin"]);
        fixture.run(&[
            "set",
            "scope-summary",
            "Record a fixture scope.",
            "--idempotency-key",
            "scope",
        ]);
    }
    fixture
}

fn read(fixture: &Fixture, extra: &[&str], command: &str) -> Guide006CliActual {
    let mut argv = vec!["--json"];
    argv.extend_from_slice(extra);
    argv.push(command);
    let output = fixture.raw_output(&argv);
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{argv:?}: {output:?}"
    );
    let raw = String::from_utf8(output.stdout).unwrap();
    let validated = serde_json::from_str::<OutputEnvelopeV4>(&raw).unwrap();
    assert_eq!(
        validated.as_value()["result"]["schema"],
        "podway.observation-result/v7"
    );
    Guide006CliActual { raw }
}

pub(crate) fn guide006_cli_observe_prepared(extra: &[&str]) -> Guide006CliActual {
    read(&fixture(false), extra, "observe")
}
pub(crate) fn guide006_cli_observe_running(extra: &[&str]) -> Guide006CliActual {
    read(&fixture(true), extra, "observe")
}
pub(crate) fn guide006_cli_next_prepared(extra: &[&str]) -> Guide006CliActual {
    read(&fixture(false), extra, "next")
}
pub(crate) fn guide006_cli_next_running(extra: &[&str]) -> Guide006CliActual {
    read(&fixture(true), extra, "next")
}
pub(crate) fn guide006_cli_status_running(extra: &[&str]) -> Guide006CliActual {
    read(&fixture(true), extra, "status")
}

fn normalize(raw: &str) -> String {
    let value: Value = serde_json::from_str(raw).unwrap();
    let root = value["workspace"]["root"]
        .as_str()
        .filter(|root| root.starts_with('/'))
        .unwrap();
    let uuid = |value: &Value| {
        let id = value.as_str().unwrap();
        assert!(uuid::Uuid::parse_str(id).is_ok(), "not a UUID: {id}");
        id.to_owned()
    };
    let mut identities = vec![
        (root.to_owned(), "<worktree>"),
        (value["generated_at"].as_str().unwrap().to_owned(), "<time>"),
        (uuid(&value["request_id"]), "<request>"),
        (uuid(&value["workspace"]["uuid"]), "<workspace>"),
        (uuid(&value["session"]["id"]), "<session>"),
    ];
    if !value["result"]["cursor"]["attempt"].is_null() {
        identities.push((
            uuid(&value["result"]["cursor"]["attempt"]["attempt"]),
            "<attempt>",
        ));
    }
    let normalized = identities
        .iter()
        .fold(raw.to_owned(), |text, (actual, token)| {
            text.replace(actual, token)
        });
    for (actual, token) in identities {
        assert!(normalized.contains(token));
        assert!(!normalized.contains(&actual));
    }
    for part in
        normalized.split(|character: char| !character.is_ascii_hexdigit() && character != '-')
    {
        assert!(
            part.len() != 36 || uuid::Uuid::parse_str(part).is_err(),
            "unrecognized UUID-shaped value survived normalization"
        );
    }
    normalized
}

pub(crate) fn guide006_cli_baseline(name: &str) -> &'static str {
    match name {
        "observe_prepared" => include_str!("guide006_scoped_baselines/observe_prepared.json"),
        "observe_running" => include_str!("guide006_scoped_baselines/observe_running.json"),
        "status_running" => include_str!("guide006_scoped_baselines/status_running.json"),
        "next_prepared" => include_str!("guide006_scoped_baselines/next_prepared.json"),
        "next_running" => include_str!("guide006_scoped_baselines/next_running.json"),
        _ => panic!("unknown baseline {name}"),
    }
}

fn verify(name: &str, actual: Guide006CliActual) {
    assert_eq!(actual.normalized(), guide006_cli_baseline(name), "{name}");
}

#[test]
fn guide006_cli_observe_prepared_baseline() {
    verify("observe_prepared", guide006_cli_observe_prepared(&[]));
}
#[test]
fn guide006_cli_observe_running_baseline() {
    verify("observe_running", guide006_cli_observe_running(&[]));
}
#[test]
fn guide006_cli_status_running_baseline() {
    verify("status_running", guide006_cli_status_running(&[]));
}
#[test]
fn guide006_cli_next_prepared_baseline() {
    verify("next_prepared", guide006_cli_next_prepared(&[]));
}
#[test]
fn guide006_cli_next_running_baseline() {
    verify("next_running", guide006_cli_next_running(&[]));
}
#[test]
fn guide006_cli_running_observe_is_deterministic_across_fresh_fixtures() {
    let left = guide006_cli_observe_running(&[]);
    let right = guide006_cli_observe_running(&[]);
    assert_ne!(left.raw, right.raw);
    assert_eq!(left.normalized(), right.normalized());
    assert_eq!(left.normalized(), guide006_cli_baseline("observe_running"));
}
#[test]
fn guide006_cli_perturbed_actuals_differ_from_their_baselines() {
    let fixture = fixture(true);
    let original = read(&fixture, &[], "observe");
    assert_eq!(
        original.normalized(),
        guide006_cli_baseline("observe_running")
    );
    fixture.run(&[
        "set",
        "scope-summary",
        "Changed the fixture item.",
        "--idempotency-key",
        "edit",
    ]);
    let after = read(&fixture, &[], "observe");
    assert_ne!(after.normalized(), original.normalized());
    let changed: Value = serde_json::from_str(&after.raw).unwrap();
    assert_eq!(changed["result"]["items"][0]["revision"], 2);
}
#[test]
fn guide006_cli_normalizer_requires_volatile_literals_and_hides_every_generated_value() {
    let scenario = guide006_cli_observe_running(&[]);
    let normalized = scenario.normalized();
    for placeholder in [
        "<worktree>",
        "<time>",
        "<request>",
        "<workspace>",
        "<session>",
        "<attempt>",
    ] {
        assert!(normalized.contains(placeholder));
    }
    let mut value: Value = serde_json::from_str(&scenario.raw).unwrap();
    value["workspace"].as_object_mut().unwrap().remove("root");
    assert!(std::panic::catch_unwind(|| normalize(&value.to_string())).is_err());
    let mut value: Value = serde_json::from_str(&scenario.raw).unwrap();
    value["unexpected_identity"] = Value::String(uuid::Uuid::new_v4().to_string());
    assert!(std::panic::catch_unwind(|| normalize(&value.to_string())).is_err());
}
