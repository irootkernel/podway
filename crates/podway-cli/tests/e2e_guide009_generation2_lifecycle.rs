//! GUIDE-009: one conformance test covering generation selection and the
//! executable observation-v6 lifecycle templates.
//!
//! `generation2_lifecycle_conformance` executes every requirement of the
//! dossier's generation-2 CLI end-to-end list by itself, so it can run as one
//! packaged conformance scenario (`tools/qualify_distribution.py` runs each
//! registered scenario as a single `--exact` test).

use std::{
    fs,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::Value;

use super::cli_fixture::{Fixture, podway_binary};
use crate::e2e_guide006_generation1_baselines::{
    guide006_cli_baseline, guide006_cli_next_prepared, guide006_cli_next_running,
    guide006_cli_observe_prepared, guide006_cli_observe_running, guide006_cli_status_running,
};

/// One action node with a required confirm; the same shape the GUIDE-006
/// scenarios use, reachable through start, begin, cancel, complete and
/// disposition.
const WORK: &[u8] = br#"{"schema":"podway.procedure/v2","id":"guide009-work","version":"1",
    "name":"Guide009 work","purpose":"Exercise generation-2 lifecycle templates.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record work.",
        "items":[{"id":"confirm","type":"confirm","prompt":"Confirm.","required":true}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

const PRESET: &str = "small-change-v2";

fn write_work_procedure(fixture: &Fixture) -> String {
    fs::write(fixture.0.join("work.json"), WORK).unwrap();
    let preview = fixture.run(&["procedure", "preview", "work.json"]);
    preview["result"]["procedure_digest"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn observe(fixture: &Fixture, generation: &str) -> Value {
    fixture.run(&["--contract-generation", generation, "observe"])
}

fn begin(fixture: &Fixture) {
    fixture.run(&["begin", "--idempotency-key", "guide009-begin"]);
}

fn cancel(fixture: &Fixture) {
    fixture.run(&[
        "cancel",
        "--reason",
        "Guide009 cancellation for the terminal window.",
        "--idempotency-key",
        "guide009-cancel",
    ]);
}

fn disposed_terminal_fixture() -> (Fixture, Value) {
    let (fixture, _) = prepared_fixture();
    begin(&fixture);
    cancel(&fixture);
    fixture.run(&[
        "disposition",
        "handed-off",
        "--summary",
        "Guide009 hand-off.",
        "--reference",
        "guide009-reference",
        "--idempotency-key",
        "guide009-disposition",
    ]);
    let observation = observe(&fixture, "2");
    assert_eq!(
        observation["result"]["schema"],
        "podway.observation-result/v6"
    );
    (fixture, observation)
}

fn prepared_fixture() -> (Fixture, Value) {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "guide009-init"]);
    write_work_procedure(&fixture);
    fixture.run(&[
        "start",
        "--procedure",
        "work.json",
        "--expect-procedure-digest",
        &work_digest(&fixture),
        "--task",
        "Guide009 lifecycle",
        "--idempotency-key",
        "guide009-start",
    ]);
    let observation = observe(&fixture, "2");
    assert_eq!(
        observation["result"]["schema"],
        "podway.observation-result/v6"
    );
    (fixture, observation)
}

/// The preview digest of the current work procedure; preview is read-only, so
/// this never changes committed state between template executions.
fn work_digest(fixture: &Fixture) -> String {
    let preview = fixture.run(&["procedure", "preview", "work.json"]);
    preview["result"]["procedure_digest"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn undisposed_terminal_fixture() -> (Fixture, Value) {
    let (fixture, _) = prepared_fixture();
    begin(&fixture);
    cancel(&fixture);
    let observation = observe(&fixture, "2");
    assert_eq!(
        observation["result"]["schema"],
        "podway.observation-result/v6"
    );
    (fixture, observation)
}

fn templates_of(observation: &Value) -> &Vec<Value> {
    observation["result"]["mutation_templates"]
        .as_array()
        .unwrap()
}

fn assert_template_shape(template: &Value, command: &str) {
    assert_eq!(template["command"], command, "{template}");
    let argv = template["argv"].as_array().unwrap();
    assert_eq!(argv[0], "podway");
    // The argv closes with the exact R1 fence tail built from the
    // preconditions; the execution below replaces only the placeholders.
    let revision = template["preconditions"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let expected: Vec<&str> = vec![
        "--if-workspace-uuid",
        template["preconditions"]["workspace_uuid"]
            .as_str()
            .unwrap(),
        "--if-session-id",
        template["preconditions"]["session_id"].as_str().unwrap(),
        "--if-session-revision",
        revision.as_str(),
        "--idempotency-key",
        "<idempotency-key>",
    ];
    let tail: Vec<&str> = argv[argv.len() - 8..]
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(tail, expected, "{command}");
}

/// Launches the emitted argv after its executable token, adding only the
/// global options (`--json` and the fixture's `--worktree` via the harness)
/// and the caller's placeholder replacements, then returns the raw stdout.
fn run_template_under_generation(
    fixture: &Fixture,
    template: &Value,
    replacements: &[(&str, &str)],
    generation: Option<&str>,
) -> String {
    let mut argv: Vec<String> = vec!["--json".to_owned()];
    if let Some(generation) = generation {
        argv.push("--contract-generation".to_owned());
        argv.push(generation.to_owned());
    }
    for token in template["argv"].as_array().unwrap().iter().skip(1) {
        let token = token.as_str().unwrap();
        let replaced = replacements
            .iter()
            .find(|(placeholder, _)| *placeholder == token)
            .map(|(_, value)| (*value).to_owned())
            .unwrap_or_else(|| token.to_owned());
        argv.push(replaced);
    }
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    let output = fixture.raw_output(&borrowed);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{argv:?}: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty(), "{argv:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn run_template_once(fixture: &Fixture, template: &Value, replacements: &[(&str, &str)]) -> String {
    run_template_under_generation(fixture, template, replacements, None)
}

fn lookup(fixture: &Fixture, generation: &str, key: &str) -> Value {
    fixture.run(&[
        "--contract-generation",
        generation,
        "outcome",
        "lookup",
        "--idempotency-key",
        key,
    ])
}

/// Executes one template, replays its exact argv, looks the outcome up under
/// both generations, and requires the identical stored response bytes plus
/// the recorded command identity.
fn execute_template_with_replay_and_lookup(
    fixture: &Fixture,
    template: &Value,
    replacements: &[(&str, &str)],
    key: &str,
) -> Value {
    assert_template_shape(template, template["command"].as_str().unwrap());
    let first = run_template_once(fixture, template, replacements);
    let replay = run_template_once(fixture, template, replacements);
    assert_eq!(first, replay, "exact replay must return the original bytes");
    let original: OutputEnvelopeV4 = serde_json::from_str(&first).unwrap();
    let original = original.into_value();
    assert_eq!(
        original["command"], template["command"],
        "the executed command must be what the template names"
    );
    let under_one = lookup(fixture, "1", key);
    let under_two = lookup(fixture, "2", key);
    assert_eq!(
        under_one["result"]["outcome"]["request_digest"],
        under_two["result"]["outcome"]["request_digest"],
        "outcome lookup must not depend on the generation"
    );
    for lookup_envelope in [&under_one, &under_two] {
        let outcome = &lookup_envelope["result"]["outcome"];
        assert_eq!(outcome["response"]["request_id"], original["request_id"]);
        assert_eq!(outcome["response"]["command"], template["command"]);
        assert_eq!(outcome["response"]["result"], original["result"]);
    }
    original
}

struct GuardedChild(Option<Child>);

impl Drop for GuardedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The A1 open-stdin refusal: the invalid generation must fail before stdin
/// is read, so the producer stays open without sending any bytes.
fn invalid_generation_fails_before_stdin(fixture: &Fixture) {
    let mut command = Command::new(podway_binary());
    command
        .arg("--json")
        .arg("--contract-generation")
        .arg("3")
        .arg("--worktree")
        .arg(&fixture.0)
        .arg("record")
        .arg("--stdin")
        .current_dir(&fixture.0)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    let mut guard = GuardedChild(Some(child));
    // Keep the producer open without sending bytes: consuming stdin would block.
    let producer = guard.0.as_mut().unwrap().stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while guard.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "generation validation blocked on stdin"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let output = guard.0.take().unwrap().wait_with_output().unwrap();
    drop(producer);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    let value = error.as_value();
    assert_eq!(value["code"], "REQUEST_INVALID");
    assert_eq!(value["command"], "item.record_many");
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("--contract-generation")
    );
}

fn request_invalid(fixture: &Fixture, arguments: &[&str], command: &str) {
    let mut argv = vec!["--json"];
    argv.extend_from_slice(arguments);
    let output = fixture.raw_output(&argv);
    assert_eq!(output.status.code(), Some(2), "{argv:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{argv:?}: {output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    let value = error.as_value();
    assert_eq!(value["code"], "REQUEST_INVALID", "{argv:?}");
    assert_eq!(value["command"], command, "{argv:?}");
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("--contract-generation")
    );
}

#[test]
fn generation2_lifecycle_conformance() {
    // 1. Flag validation: invalid values fail closed with no state change,
    //    before local execution or stdin; local commands accept the flag.
    {
        let (fixture, _) = prepared_fixture();
        let before = fixture.podway_snapshot();
        for value in ["0", "3", "v2", ""] {
            request_invalid(
                &fixture,
                &["--contract-generation", value, "observe"],
                "session.observe",
            );
            request_invalid(&fixture, &["--contract-generation", value, "help"], "help");
            // A top-level help alias after the valued pair still reaches
            // validation attributed to `help`.
            request_invalid(&fixture, &["--contract-generation", value, "-h"], "help");
        }
        invalid_generation_fails_before_stdin(&fixture);
        // Retired syntax keeps COMMAND_RETIRED precedence over the value error.
        let output =
            fixture.raw_output(&["--json", "--contract-generation", "3", "daemon", "status"]);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "COMMAND_RETIRED");
        // A missing value stays a clap usage error, like every valued global:
        // the generic syntax message, not the generation message.
        let output = fixture.raw_output(&["--json", "--contract-generation"]);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "REQUEST_INVALID");
        let message = error.as_value()["message"].as_str().unwrap();
        assert_eq!(message, "invalid command syntax");
        assert!(!message.contains("--contract-generation"));
        // The equals form is accepted; plain `--json version` emits its bare
        // summary object rather than an output envelope.
        let output = fixture.raw_output(&["--json", "--contract-generation=2", "version"]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let version: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(version["name"], "podway");
        for local in [
            vec!["--contract-generation", "2", "help"],
            vec!["--contract-generation", "2", "completions", "bash"],
            vec!["--contract-generation=1", "preset", "list"],
            vec!["--contract-generation", "2", "procedure", "scaffold"],
        ] {
            let output = fixture.raw_output(&local);
            assert_eq!(output.status.code(), Some(0), "{local:?}: {output:?}");
        }
        // The root-level completion candidates offer the flag: the bash root
        // case lists it among its shared candidates, and the fish root route
        // completes it. Without --json the completions command prints the raw
        // script itself.
        let output = fixture.raw_output(&["--contract-generation", "2", "completions", "bash"]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let script = String::from_utf8(output.stdout).unwrap();
        let root_case = script
            .split("\"root\")")
            .nth(1)
            .unwrap()
            .split(";;")
            .next()
            .unwrap();
        assert!(root_case.contains("--contract-generation"));
        let output = fixture.raw_output(&["--contract-generation", "2", "completions", "fish"]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let script = String::from_utf8(output.stdout).unwrap();
        assert!(script.lines().any(|line| {
            line.contains("__podway_route_is \"root\"") && line.contains("-l contract-generation")
        }));
        assert_eq!(fixture.podway_snapshot(), before);
    }

    // 2. An explicit generation 1 equals the default: the frozen GUIDE-006
    //    CLI baselines reproduce with `--contract-generation 1`.
    {
        assert_eq!(
            guide006_cli_observe_prepared(&["--contract-generation", "1"]).normalized(),
            guide006_cli_baseline("guide006_cli_observe_prepared_baseline")
        );
        assert_eq!(
            guide006_cli_observe_running(&["--contract-generation", "1"]).normalized(),
            guide006_cli_baseline("guide006_cli_observe_running_baseline")
        );
        assert_eq!(
            guide006_cli_status_running(&["--contract-generation", "1"]).normalized(),
            guide006_cli_baseline("guide006_cli_status_running_baseline")
        );
        assert_eq!(
            guide006_cli_next_prepared(&["--contract-generation", "1"]).normalized(),
            guide006_cli_baseline("guide006_cli_next_prepared_baseline")
        );
        assert_eq!(
            guide006_cli_next_running(&["--contract-generation", "1"]).normalized(),
            guide006_cli_baseline("guide006_cli_next_running_baseline")
        );
    }

    // 3. Every prepared template executes after only its placeholders are
    //    replaced, from independently recreated eligible state.
    {
        let (_, prepared) = prepared_fixture();
        assert_eq!(
            prepared["result"]["status"]["session"]["lifecycle"],
            "prepared"
        );
        let templates = templates_of(&prepared);
        assert_eq!(templates.len(), 4);
        assert_eq!(templates[0]["command"], "session.begin");
        assert_eq!(templates[1]["command"], "session.reset");
        assert_eq!(templates[2]["command"], "session.start_replace");
        assert_eq!(templates[3]["command"], "session.start_replace");

        let (fixture, prepared) = prepared_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&prepared)[0],
            &[("<idempotency-key>", "guide009-begin-template")],
            "guide009-begin-template",
        );

        let (fixture, prepared) = prepared_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&prepared)[1],
            &[("<idempotency-key>", "guide009-reset-template")],
            "guide009-reset-template",
        );

        let (fixture, prepared) = prepared_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&prepared)[2],
            &[
                ("<preset>", PRESET),
                ("<task>", "Guide009 preset replacement"),
                ("<idempotency-key>", "guide009-start-preset"),
            ],
            "guide009-start-preset",
        );

        let (fixture, prepared) = prepared_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&prepared)[3],
            &[
                ("<procedure-file>", "work.json"),
                ("<procedure-digest>", &work_digest(&fixture)),
                ("<task>", "Guide009 procedure replacement"),
                ("<idempotency-key>", "guide009-start-procedure"),
            ],
            "guide009-start-procedure",
        );
    }

    // 4. Both undisposed-terminal disposition templates execute the same way.
    {
        let (_, undisposed) = undisposed_terminal_fixture();
        let templates = templates_of(&undisposed);
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0]["command"], "session.terminal_disposition");
        assert_eq!(templates[1]["command"], "session.terminal_disposition");

        let (fixture, undisposed) = undisposed_terminal_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&undisposed)[0],
            &[
                ("<summary>", "Guide009 handed off."),
                ("<reference>", "guide009-handoff"),
                ("<idempotency-key>", "guide009-disposition-handed-off"),
            ],
            "guide009-disposition-handed-off",
        );

        let (fixture, undisposed) = undisposed_terminal_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&undisposed)[1],
            &[
                ("<reason>", "No external hand-off required."),
                ("<idempotency-key>", "guide009-disposition-not-required"),
            ],
            "guide009-disposition-not-required",
        );
    }

    // 5. Every disposed-terminal template executes, including both fenced
    //    plain starts, whose execution, replay and outcome lookup record the
    //    `session.start` command while archiving the disposed session.
    {
        let (_, disposed) = disposed_terminal_fixture();
        let templates = templates_of(&disposed);
        assert_eq!(templates.len(), 4);
        assert_eq!(templates[0]["command"], "session.archive");
        assert_eq!(templates[1]["command"], "session.reset");
        assert_eq!(templates[2]["command"], "session.start");
        assert_eq!(templates[3]["command"], "session.start");

        let (fixture, disposed) = disposed_terminal_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&disposed)[0],
            &[("<idempotency-key>", "guide009-archive")],
            "guide009-archive",
        );

        let (fixture, disposed) = disposed_terminal_fixture();
        execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&disposed)[1],
            &[("<idempotency-key>", "guide009-disposed-reset")],
            "guide009-disposed-reset",
        );

        let (fixture, disposed) = disposed_terminal_fixture();
        let original_session = disposed["result"]["status"]["session"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let start = execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&disposed)[2],
            &[
                ("<preset>", PRESET),
                ("<task>", "Guide009 successor"),
                ("<idempotency-key>", "guide009-start-template"),
            ],
            "guide009-start-template",
        );
        assert_eq!(start["result"]["schema"], "podway.session-start-result/v4");
        // The fenced plain start reports the archive of the original disposed
        // session, which stays retained as the single archived session, and a
        // distinct prepared successor takes its place; the exact replay above
        // still returned the original success after that archival.
        assert_eq!(start["result"]["existing_session"]["action"], "archive");
        assert_eq!(
            start["result"]["existing_session"]["session_id"],
            original_session.as_str()
        );
        let list = fixture.run(&["archive", "list"]);
        assert_eq!(list["result"]["count"], 1);
        assert_eq!(
            list["result"]["sessions"][0]["session_id"],
            original_session
        );
        let successor = observe(&fixture, "2");
        assert_eq!(
            successor["result"]["status"]["session"]["lifecycle"],
            "prepared"
        );
        assert_ne!(
            successor["result"]["status"]["session"]["id"],
            original_session
        );

        let (fixture, disposed) = disposed_terminal_fixture();
        let original_session = disposed["result"]["status"]["session"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let start = execute_template_with_replay_and_lookup(
            &fixture,
            &templates_of(&disposed)[3],
            &[
                ("<procedure-file>", "work.json"),
                ("<procedure-digest>", &work_digest(&fixture)),
                ("<task>", "Guide009 procedure successor"),
                ("<idempotency-key>", "guide009-start-procedure-template"),
            ],
            "guide009-start-procedure-template",
        );
        assert_eq!(start["result"]["existing_session"]["action"], "archive");
        assert_eq!(
            start["result"]["existing_session"]["session_id"],
            original_session.as_str()
        );
        let list = fixture.run(&["archive", "list"]);
        assert_eq!(list["result"]["count"], 1);
        assert_eq!(
            list["result"]["sessions"][0]["session_id"],
            original_session
        );
        let successor = observe(&fixture, "2");
        assert_eq!(
            successor["result"]["status"]["session"]["lifecycle"],
            "prepared"
        );
        assert_ne!(
            successor["result"]["status"]["session"]["id"],
            original_session
        );
    }

    // 6. Presentation-only pin: a generation-2 mutation replays under
    //    generation 1 and the reverse once, outcome lookup agrees under both,
    //    and the committed state never changes across the replays.
    {
        // A generation-2 mutation, replayed under generation 1.
        let (fixture, prepared) = prepared_fixture();
        let begin = templates_of(&prepared)[0].clone();
        let sequence_before = prepared["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap();
        let under_two = run_template_under_generation(
            &fixture,
            &begin,
            &[("<idempotency-key>", "guide009-cross-generation")],
            Some("2"),
        );
        let under_one = run_template_under_generation(
            &fixture,
            &begin,
            &[("<idempotency-key>", "guide009-cross-generation")],
            Some("1"),
        );
        assert_eq!(under_one, under_two);
        // The generation-2-origin direction carries the same assertions as
        // the reverse below: each lookup must find the stored outcome and
        // return it unchanged under both generations, and the committed
        // sequence advanced exactly once for the mutation and not for the
        // replay.
        let original: OutputEnvelopeV4 = serde_json::from_str(&under_two).unwrap();
        let original = original.into_value();
        let lookup_one = lookup(&fixture, "1", "guide009-cross-generation");
        let lookup_two = lookup(&fixture, "2", "guide009-cross-generation");
        for lookup_envelope in [&lookup_one, &lookup_two] {
            assert_eq!(lookup_envelope["result"]["found"], true);
            let outcome = &lookup_envelope["result"]["outcome"];
            assert_eq!(outcome["response"]["request_id"], original["request_id"]);
            assert_eq!(outcome["response"]["command"], original["command"]);
            assert_eq!(outcome["response"]["result"], original["result"]);
        }
        assert_eq!(
            lookup_one["result"]["outcome"]["request_digest"],
            lookup_two["result"]["outcome"]["request_digest"]
        );
        assert_eq!(
            lookup_one["result"]["outcome"]["response"],
            lookup_two["result"]["outcome"]["response"]
        );
        let sequence_after = lookup_two["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap();
        assert_eq!(sequence_after, sequence_before + 1);
        // The reverse once: a generation-1 mutation replayed under generation 2.
        let (fixture, prepared) = prepared_fixture();
        let reset = templates_of(&prepared)[1].clone();
        let sequence_before = prepared["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap();
        let under_one = run_template_under_generation(
            &fixture,
            &reset,
            &[("<idempotency-key>", "guide009-reverse-direction")],
            Some("1"),
        );
        let under_two = run_template_under_generation(
            &fixture,
            &reset,
            &[("<idempotency-key>", "guide009-reverse-direction")],
            Some("2"),
        );
        assert_eq!(under_two, under_one);
        // Lookup under both generations must find the identical stored
        // outcome and return it unchanged, and the committed sequence
        // advanced exactly once per mutation, never across a replay.
        let original: OutputEnvelopeV4 = serde_json::from_str(&under_one).unwrap();
        let original = original.into_value();
        let lookup_one = lookup(&fixture, "1", "guide009-reverse-direction");
        let lookup_two = lookup(&fixture, "2", "guide009-reverse-direction");
        for lookup_envelope in [&lookup_one, &lookup_two] {
            assert_eq!(lookup_envelope["result"]["found"], true);
            let outcome = &lookup_envelope["result"]["outcome"];
            assert_eq!(outcome["response"]["request_id"], original["request_id"]);
            assert_eq!(outcome["response"]["command"], original["command"]);
            assert_eq!(outcome["response"]["result"], original["result"]);
        }
        assert_eq!(
            lookup_one["result"]["outcome"]["request_digest"],
            lookup_two["result"]["outcome"]["request_digest"]
        );
        assert_eq!(
            lookup_one["result"]["outcome"]["response"],
            lookup_two["result"]["outcome"]["response"]
        );
        assert_eq!(
            lookup_one["result"]["outcome"]["response"]["command"],
            reset["command"]
        );
        let sequence_after = lookup_two["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap();
        assert_eq!(sequence_after, sequence_before + 1);
    }

    // 7. The exact family per generation: envelope validation alone admits
    //    either family, so the result schema is asserted explicitly.
    {
        let (fixture, _) = prepared_fixture();
        assert_eq!(
            observe(&fixture, "1")["result"]["schema"],
            "podway.observation-result/v5"
        );
        assert_eq!(
            observe(&fixture, "2")["result"]["schema"],
            "podway.observation-result/v6"
        );
    }
}

const AGENT005_CLI_PROCEDURE: &[u8] = br#"schema: podway.procedure/v2
id: agent005-cli-observation
version: "1"
name: AGENT-005 CLI observation lifecycle
purpose: Exercise all six CLI --with-observation routes end to end.
description: AGENT-005 CLI test procedure.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Work
    intent: Record items before completing into the decision node.
    instructions:
      - Record items.
    items:
      - id: confirm
        type: confirm
        prompt: Confirm work.
        required: true
      - id: note
        type: text
        prompt: Record note.
        required: false
        min_length: 1
        max_length: 200
        multiline: false
  gate:
    type: decision
    title: Gate
    objective: Determine the goal outcome.
    prompt: Which outcome applies?
    options:
      - id: ship
        label: Ship
      - id: hold
        label: Hold
      - id: drop
        label: Drop
    reason:
      required: true
      prompt: Explain the assessment.
    assessment:
      target: session_goal
      outcomes:
        ship: achieved
        hold: not_achieved
        drop: superseded
  finish:
    type: action
    title: Finish
    intent: Complete the terminal node.
    instructions:
      - Complete finish.
    items:
      - id: done
        type: confirm
        prompt: Confirm finish.
        required: false
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: gate
    - id: gate
      use: gate
      evidence_from:
        - node: work
          required: true
          items:
            - confirm
      routes:
        ship:
          to: finish
          effect: advance
        hold:
          to: finish
          effect: advance
        drop:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - work
"#;

fn run_stdin_json(fixture: &Fixture, args: &[&str], stdin_bytes: &[u8]) -> (Option<i32>, Value) {
    use std::io::Write;
    let mut command = Command::new(podway_binary());
    command
        .arg("--json")
        .arg("--worktree")
        .arg(&fixture.0)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin_bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr for {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    (output.status.code(), value)
}

#[test]
fn agent005_with_observation_lifecycle_replay_lookup_and_conflicts() {
    use podway_protocol::validate_daemon_free_document_v1;
    use serde_json::json;

    let fixture = Fixture::new();
    let init = fixture.run(&["init", "--idempotency-key", "agent005-init"]);
    let workspace_uuid = init["workspace"]["uuid"].as_str().unwrap().to_owned();
    fs::write(fixture.0.join("agent005.yaml"), AGENT005_CLI_PROCEDURE).unwrap();
    let digest =
        fixture.run(&["procedure", "preview", "agent005.yaml"])["result"]["procedure_digest"]
            .as_str()
            .unwrap()
            .to_owned();

    // 1. Excluded routes and local commands reject --with-observation with exit 2 and zero state change.
    let snapshot_before_rejections = fixture.podway_snapshot();
    for (rejected_args, expected_cmd) in [
        (
            &[
                "start",
                "--procedure",
                "agent005.yaml",
                "--task",
                "dry",
                "--dry-run",
                "--with-observation",
            ][..],
            "session.start",
        ),
        (
            &[
                "start",
                "--procedure",
                "agent005.yaml",
                "--task",
                "replace",
                "--on-existing",
                "preserve",
                "--supersede-reason",
                "reason",
                "--with-observation",
                "--idempotency-key",
                "k-rej-1",
            ][..],
            "session.start_replace",
        ),
        (&["observe", "--with-observation"][..], "session.observe"),
        (&["status", "--with-observation"][..], "session.status"),
        (
            &["skip", "--with-observation", "--idempotency-key", "k-rej-2"][..],
            "session.skip",
        ),
        (
            &[
                "goal",
                "assess-criterion",
                "c1",
                "--status",
                "satisfied",
                "--reason",
                "r",
                "--with-observation",
                "--idempotency-key",
                "k-rej-3",
            ][..],
            "goal.assess_criterion",
        ),
        (
            &[
                "check",
                "confirm",
                "--with-observation",
                "--idempotency-key",
                "k-rej-4",
            ][..],
            "item.check",
        ),
        (&["help", "--with-observation"][..], "help"),
        (&["version", "--with-observation"][..], "version"),
        (&["preset", "list", "--with-observation"][..], "preset.list"),
    ] {
        let mut argv = vec!["--json"];
        argv.extend_from_slice(rejected_args);
        let output = fixture.raw_output(&argv);
        assert_eq!(output.status.code(), Some(2), "{argv:?}: {output:?}");
        let err: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(err.as_value()["code"], "REQUEST_INVALID", "{argv:?}");
        assert_eq!(err.as_value()["command"], expected_cmd, "{argv:?}");
    }
    assert_eq!(fixture.podway_snapshot(), snapshot_before_rejections);

    let assert_wrapped = |envelope: &Value, expected_inner: &str, expected_seq: u64| {
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            envelope
        ));
        assert!(validate_daemon_free_document_v1(
            "podway.mutation-observation-result/v1",
            &envelope["result"]
        ));
        assert_eq!(
            envelope["workspace"]["latest_workspace_sequence"],
            expected_seq
        );
        assert_eq!(envelope["result"]["workspace_sequence"], expected_seq);
        assert_eq!(
            envelope["result"]["mutation_result"]["schema"],
            expected_inner
        );
        assert_eq!(
            envelope["result"]["observation"]["schema"],
            "podway.observation-result/v6"
        );
        let fresh = observe(&fixture, "2");
        assert_eq!(envelope["result"]["observation"], fresh["result"]);
    };

    // 2. Plain start --with-observation under default generation 1 embeds v6 observation.
    let start_out = fixture.run(&[
        "start",
        "--procedure",
        "agent005.yaml",
        "--expect-procedure-digest",
        &digest,
        "--task",
        "AGENT-005 CLI lifecycle",
        "--with-observation",
        "--idempotency-key",
        "cli-start-1",
    ]);
    assert_wrapped(&start_out, "podway.session-start-result/v4", 2);
    // Default generation-1 observe remains v5 while --with-observation embedded v6.
    assert_eq!(
        observe(&fixture, "1")["result"]["schema"],
        "podway.observation-result/v5"
    );
    let session_id = start_out["session"]["id"].as_str().unwrap().to_owned();

    // True -> false conflict on start even after deleting agent005.yaml!
    fs::remove_file(fixture.0.join("agent005.yaml")).unwrap();
    let start_conflict = fixture.raw_output(&[
        "--json",
        "start",
        "--procedure",
        "agent005.yaml",
        "--expect-procedure-digest",
        &digest,
        "--task",
        "AGENT-005 CLI lifecycle",
        "--idempotency-key",
        "cli-start-1",
    ]);
    assert_eq!(start_conflict.status.code(), Some(2));
    let err: ErrorEnvelopeV2 = serde_json::from_slice(&start_conflict.stdout).unwrap();
    assert_eq!(err.as_value()["code"], "IDEMPOTENCY_KEY_REUSED");

    // 3. begin --with-observation.
    let begin_out = fixture.run(&[
        "begin",
        "--goal",
        "Complete AGENT-005 CLI test.",
        "--criterion",
        "c1=Work confirmed.",
        "--actor",
        "cli-tester",
        "--with-observation",
        "--idempotency-key",
        "cli-begin-1",
    ]);
    assert_wrapped(&begin_out, "podway.session-begin-result/v2", 3);
    let work_attempt = begin_out["result"]["mutation_result"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // True -> false conflict on begin.
    let begin_conflict = fixture.raw_output(&[
        "--json",
        "begin",
        "--goal",
        "Complete AGENT-005 CLI test.",
        "--criterion",
        "c1=Work confirmed.",
        "--actor",
        "cli-tester",
        "--if-workspace-uuid",
        &workspace_uuid,
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "0",
        "--idempotency-key",
        "cli-begin-1",
    ]);
    assert_eq!(begin_conflict.status.code(), Some(2));
    let err: ErrorEnvelopeV2 = serde_json::from_slice(&begin_conflict.stdout).unwrap();
    assert_eq!(err.as_value()["code"], "IDEMPOTENCY_KEY_REUSED");

    // 4. record --stdin --with-observation.
    let record_stdin = serde_json::to_vec(&json!({
        "schema": "podway.item-record-many-input/v1",
        "workspace_uuid": workspace_uuid,
        "session_id": session_id,
        "session_revision": 1,
        "attempt_id": work_attempt,
        "idempotency_key": "cli-record-1",
        "operations": [
            {
                "item_id": "confirm",
                "expected_item_revision": 0,
                "record": {"type": "confirm", "value": true}
            },
            {
                "item_id": "note",
                "expected_item_revision": 0,
                "record": {"type": "text", "value": "Recorded via CLI."}
            }
        ]
    }))
    .unwrap();
    let (code, record_out) = run_stdin_json(
        &fixture,
        &["record", "--stdin", "--with-observation"],
        &record_stdin,
    );
    assert_eq!(code, Some(0));
    assert_wrapped(&record_out, "podway.item-record-many-result/v2", 4);

    // True -> false conflict on record --stdin.
    let (code, record_conflict) = run_stdin_json(&fixture, &["record", "--stdin"], &record_stdin);
    assert_eq!(code, Some(2));
    assert_eq!(record_conflict["code"], "IDEMPOTENCY_KEY_REUSED");

    // 5. complete --with-observation (work -> gate).
    let complete_work_out = fixture.run(&[
        "complete",
        "--with-observation",
        "--if-workspace-uuid",
        &workspace_uuid,
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "2",
        "--if-attempt",
        &work_attempt,
        "--idempotency-key",
        "cli-complete-1",
    ]);
    assert_wrapped(&complete_work_out, "podway.stage-transition-result/v3", 5);
    let gate_attempt = complete_work_out["result"]["mutation_result"]["to_attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // True -> false conflict on complete.
    let complete_conflict = fixture.raw_output(&[
        "--json",
        "complete",
        "--if-workspace-uuid",
        &workspace_uuid,
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "2",
        "--if-attempt",
        &work_attempt,
        "--idempotency-key",
        "cli-complete-1",
    ]);
    assert_eq!(complete_conflict.status.code(), Some(2));
    let err: ErrorEnvelopeV2 = serde_json::from_slice(&complete_conflict.stdout).unwrap();
    assert_eq!(err.as_value()["code"], "IDEMPOTENCY_KEY_REUSED");

    // 6. goal assess-criteria --stdin --with-observation.
    let assess_stdin = serde_json::to_vec(&json!({
        "schema": "podway.criterion-assessment-many-input/v1",
        "workspace_uuid": workspace_uuid,
        "session_id": session_id,
        "session_revision": 3,
        "attempt_id": gate_attempt,
        "goal_revision": 1,
        "idempotency_key": "cli-assess-1",
        "actor": "cli-tester",
        "assessments": [
            {
                "criterion_id": "c1",
                "status": "satisfied",
                "reason": "Confirmed in work.",
                "evidence": ["work"],
                "items": []
            }
        ]
    }))
    .unwrap();
    let (code, assess_out) = run_stdin_json(
        &fixture,
        &["goal", "assess-criteria", "--stdin", "--with-observation"],
        &assess_stdin,
    );
    assert_eq!(code, Some(0));
    assert_wrapped(&assess_out, "podway.criterion-assessment-many-result/v1", 6);

    // True -> false conflict on goal assess-criteria --stdin.
    let (code, assess_conflict) = run_stdin_json(
        &fixture,
        &["goal", "assess-criteria", "--stdin"],
        &assess_stdin,
    );
    assert_eq!(code, Some(2));
    assert_eq!(assess_conflict["code"], "IDEMPOTENCY_KEY_REUSED");

    // 7. decide --with-observation (gate -> finish).
    let decide_out = fixture.run(&[
        "decide",
        "--option",
        "ship",
        "--reason",
        "Criterion c1 is satisfied.",
        "--actor",
        "cli-tester",
        "--with-observation",
        "--if-workspace-uuid",
        &workspace_uuid,
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "4",
        "--if-attempt",
        &gate_attempt,
        "--if-goal-revision",
        "1",
        "--idempotency-key",
        "cli-decide-1",
    ]);
    assert_wrapped(&decide_out, "podway.decision-result/v2", 7);
    let finish_attempt = decide_out["result"]["mutation_result"]["target_attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // True -> false conflict on decide.
    let decide_conflict = fixture.raw_output(&[
        "--json",
        "decide",
        "--option",
        "ship",
        "--reason",
        "Criterion c1 is satisfied.",
        "--actor",
        "cli-tester",
        "--if-workspace-uuid",
        &workspace_uuid,
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "4",
        "--if-attempt",
        &gate_attempt,
        "--if-goal-revision",
        "1",
        "--idempotency-key",
        "cli-decide-1",
    ]);
    assert_eq!(decide_conflict.status.code(), Some(2));
    let err: ErrorEnvelopeV2 = serde_json::from_slice(&decide_conflict.stdout).unwrap();
    assert_eq!(err.as_value()["code"], "IDEMPOTENCY_KEY_REUSED");

    // 8. Terminal complete --with-observation (finish -> completed) in human mode first, then replay in JSON!
    let human_complete = fixture.raw_output(&[
        "complete",
        "--with-observation",
        "--if-workspace-uuid",
        &workspace_uuid,
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "5",
        "--if-attempt",
        &finish_attempt,
        "--idempotency-key",
        "cli-complete-finish",
    ]);
    assert_eq!(human_complete.status.code(), Some(0));
    assert!(!human_complete.stdout.is_empty());

    let complete_finish_replay = fixture.run(&[
        "complete",
        "--with-observation",
        "--if-workspace-uuid",
        &workspace_uuid,
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "5",
        "--if-attempt",
        &finish_attempt,
        "--idempotency-key",
        "cli-complete-finish",
    ]);
    assert_wrapped(
        &complete_finish_replay,
        "podway.stage-transition-result/v3",
        8,
    );

    // 9. Outcome lookup and historical replay across all committed keys.
    for (key, expected) in [
        ("cli-start-1", &start_out),
        ("cli-begin-1", &begin_out),
        ("cli-record-1", &record_out),
        ("cli-complete-1", &complete_work_out),
        ("cli-assess-1", &assess_out),
        ("cli-decide-1", &decide_out),
    ] {
        let looked = lookup(&fixture, "2", key);
        assert_eq!(looked["result"]["found"], true);
        assert_eq!(looked["result"]["outcome"]["response"], *expected);
    }

    // Historical replay of cli-start-1 after agent005.yaml was deleted still returns the original start wrapper!
    let start_replay = fixture.run(&[
        "start",
        "--procedure",
        "agent005.yaml",
        "--expect-procedure-digest",
        &digest,
        "--task",
        "AGENT-005 CLI lifecycle",
        "--with-observation",
        "--idempotency-key",
        "cli-start-1",
    ]);
    assert_eq!(start_replay, start_out);

    // 10. Dispose terminal session and run plain disposed-terminal start (false -> true conflict check too).
    fixture.run(&[
        "disposition",
        "not-required",
        "--reason",
        "Completed AGENT-005 CLI test.",
        "--idempotency-key",
        "cli-disposition-1",
    ]);
    fs::write(fixture.0.join("agent005.yaml"), AGENT005_CLI_PROCEDURE).unwrap();
    let plain_disposed_start = fixture.run(&[
        "start",
        "--procedure",
        "agent005.yaml",
        "--expect-procedure-digest",
        &digest,
        "--task",
        "AGENT-005 false->true start check",
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "6",
        "--idempotency-key",
        "cli-start-false-to-true",
    ]);
    assert_eq!(
        plain_disposed_start["result"]["schema"],
        "podway.session-start-result/v4"
    );
    fs::remove_file(fixture.0.join("agent005.yaml")).unwrap();
    let start_ft_conflict = fixture.raw_output(&[
        "--json",
        "start",
        "--procedure",
        "agent005.yaml",
        "--expect-procedure-digest",
        &digest,
        "--task",
        "AGENT-005 false->true start check",
        "--if-session-id",
        &session_id,
        "--if-session-revision",
        "6",
        "--with-observation",
        "--idempotency-key",
        "cli-start-false-to-true",
    ]);
    assert_eq!(start_ft_conflict.status.code(), Some(2));
    let err: ErrorEnvelopeV2 = serde_json::from_slice(&start_ft_conflict.stdout).unwrap();
    assert_eq!(err.as_value()["code"], "IDEMPOTENCY_KEY_REUSED");
}
