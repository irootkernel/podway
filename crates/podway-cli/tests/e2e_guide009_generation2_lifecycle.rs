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

use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4, validate_daemon_free_document_v1};
use serde_json::{Value, json};

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
    let output = fixture.output_with_stdin(args, stdin_bytes);
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

fn sha256_prefixed(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(7 + digest.len() * 2);
    hex.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    hex
}

struct CallCollector<'a> {
    fixture: &'a Fixture,
    calls: Vec<Vec<String>>,
    excluded_calls: Vec<(&'static str, Vec<String>)>,
}

impl<'a> CallCollector<'a> {
    fn new(fixture: &'a Fixture) -> Self {
        Self {
            fixture,
            calls: Vec::new(),
            excluded_calls: Vec::new(),
        }
    }

    fn checkpoint(&self) -> usize {
        self.calls.len()
    }

    fn record_external_work(&mut self, descriptor: &str) {
        self.excluded_calls
            .push(("external_work", vec![descriptor.to_owned()]));
    }

    fn run_args(&mut self, args: &[&str]) -> Value {
        self.calls
            .push(args.iter().map(|s| (*s).to_owned()).collect());
        let out = self.fixture.run(args);
        assert!(validate_daemon_free_document_v1("podway.output/v4", &out));
        assert!(out.get("ok").is_none(), "output/v4 must not contain ok");
        out
    }

    fn run_stdin(&mut self, args: &[&str], stdin_bytes: &[u8]) -> Value {
        self.calls
            .push(args.iter().map(|s| (*s).to_owned()).collect());
        let (code, out) = run_stdin_json(self.fixture, args, stdin_bytes);
        assert_eq!(code, Some(0), "stdin call failed for {args:?}: {out}");
        assert!(validate_daemon_free_document_v1("podway.output/v4", &out));
        assert!(out.get("ok").is_none(), "output/v4 must not contain ok");
        out
    }

    fn run_excluded_args(&mut self, category: &'static str, args: &[&str]) -> Value {
        self.excluded_calls
            .push((category, args.iter().map(|s| (*s).to_owned()).collect()));
        let out = self.fixture.run(args);
        assert!(validate_daemon_free_document_v1("podway.output/v4", &out));
        assert!(out.get("ok").is_none(), "output/v4 must not contain ok");
        out
    }

    fn run_excluded_stdin(
        &mut self,
        category: &'static str,
        args: &[&str],
        stdin_bytes: &[u8],
    ) -> Value {
        self.excluded_calls
            .push((category, args.iter().map(|s| (*s).to_owned()).collect()));
        let (code, out) = run_stdin_json(self.fixture, args, stdin_bytes);
        assert_eq!(code, Some(0), "stdin call failed for {args:?}: {out}");
        assert!(validate_daemon_free_document_v1("podway.output/v4", &out));
        assert!(out.get("ok").is_none(), "output/v4 must not contain ok");
        out
    }

    fn raw_excluded_stdin(
        &mut self,
        category: &'static str,
        args: &[&str],
        stdin_bytes: &[u8],
    ) -> (Option<i32>, Value) {
        self.excluded_calls
            .push((category, args.iter().map(|s| (*s).to_owned()).collect()));
        run_stdin_json(self.fixture, args, stdin_bytes)
    }

    fn raw_excluded_args(&mut self, category: &'static str, args: &[&str]) -> (Option<i32>, Value) {
        self.excluded_calls
            .push((category, args.iter().map(|s| (*s).to_owned()).collect()));
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let output = self.fixture.raw_output(&full);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        (output.status.code(), value)
    }

    fn assert_segment_commands(&self, start_idx: usize, expected_commands: &[&str]) {
        let segment = &self.calls[start_idx..];
        assert_eq!(
            segment.len(),
            expected_commands.len(),
            "unexpected CLI call count in segment: {segment:?}"
        );
        for (call, expected) in segment.iter().zip(expected_commands.iter()) {
            assert!(
                !call.iter().any(|token| matches!(
                    token.as_str(),
                    "observe" | "status" | "next" | "outcome"
                )),
                "measured segment must not include observe/status/next/outcome: {call:?}"
            );
            let route = subcommand_route(call);
            assert_eq!(route, *expected, "unexpected command route in {call:?}");
        }
    }
}

fn subcommand_route(call: &[String]) -> String {
    let mut tokens = Vec::new();
    let mut idx = 0;
    while idx < call.len() {
        match call[idx].as_str() {
            "--contract-generation"
            | "--timeout"
            | "--actor"
            | "--idempotency-key"
            | "--if-workspace-uuid"
            | "--if-session-id"
            | "--if-session-revision"
            | "--if-attempt"
            | "--if-item-revision"
            | "--if-goal-revision"
            | "--option"
            | "--reason"
            | "--preset"
            | "--task"
            | "--goal"
            | "--criterion" => {
                idx += 2;
            }
            "--json" | "--with-observation" | "--yes" | "--stdin" | "--dry-run" => {
                idx += 1;
            }
            token if token.starts_with("--") => {
                idx += 1;
            }
            token => {
                tokens.push(token);
                idx += 1;
                if tokens.len() == 1
                    && !matches!(
                        token,
                        "goal"
                            | "outcome"
                            | "workspace"
                            | "procedure"
                            | "preset"
                            | "archive"
                            | "evidence"
                            | "delegation"
                    )
                {
                    break;
                }
                if tokens.len() == 2 {
                    break;
                }
            }
        }
    }
    assert!(
        !tokens.is_empty(),
        "no subcommand route token found in {call:?}"
    );
    tokens.join(" ")
}

fn find_obs_template<'a>(obs: &'a Value, command: &str) -> &'a Value {
    assert_eq!(obs["schema"], "podway.observation-result/v6");
    assert!(
        obs.get("workspace").is_none(),
        "observation-result/v6 must not embed workspace.uuid"
    );
    obs["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["command"] == command)
        .unwrap_or_else(|| panic!("missing template {command} in {obs}"))
}

fn find_decide_template<'a>(obs: &'a Value, option_id: &str) -> &'a Value {
    assert_eq!(obs["schema"], "podway.observation-result/v6");
    obs["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| {
            t["command"] == "session.decide"
                && t["argv"]
                    .as_array()
                    .unwrap()
                    .windows(2)
                    .any(|w| w[0] == "--option" && w[1] == option_id)
        })
        .unwrap_or_else(|| {
            panic!("missing session.decide template for option {option_id} in {obs}")
        })
}

fn current_node_id(obs: &Value) -> &str {
    let id = obs["status"]["current"]["node"]["graph_node_id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing status.current.node.graph_node_id in {obs}"));
    assert_eq!(obs["guidance"]["node"]["graph_node_id"], id);
    id
}

fn build_argv_from_template(
    template: &Value,
    generation: &str,
    with_observation: bool,
    replacements: &[(&str, &str)],
    extra_args: &[&str],
) -> Vec<String> {
    let mut argv = vec!["--contract-generation".to_owned(), generation.to_owned()];
    if with_observation {
        argv.push("--with-observation".to_owned());
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
    for extra in extra_args {
        argv.push((*extra).to_owned());
    }
    argv
}

fn run_template_collected(
    collector: &mut CallCollector<'_>,
    template: &Value,
    generation: &str,
    with_observation: bool,
    replacements: &[(&str, &str)],
    extra_args: &[&str],
) -> Value {
    let built = build_argv_from_template(
        template,
        generation,
        with_observation,
        replacements,
        extra_args,
    );
    let refs: Vec<&str> = built.iter().map(String::as_str).collect();
    collector.run_args(&refs)
}

fn run_template_excluded(
    collector: &mut CallCollector<'_>,
    category: &'static str,
    template: &Value,
    generation: &str,
    with_observation: bool,
    replacements: &[(&str, &str)],
    extra_args: &[&str],
) -> Value {
    let built = build_argv_from_template(
        template,
        generation,
        with_observation,
        replacements,
        extra_args,
    );
    let refs: Vec<&str> = built.iter().map(String::as_str).collect();
    collector.run_excluded_args(category, &refs)
}

fn build_record_many_stdin(
    workspace_uuid: &str,
    obs: &Value,
    key: &str,
    ops: &[(&str, Value)],
) -> Vec<u8> {
    assert_eq!(obs["schema"], "podway.observation-result/v6");
    assert!(obs.get("workspace").is_none());
    let session_id = obs["status"]["session"]["id"].as_str().unwrap();
    let session_revision = obs["status"]["session"]["revision"].as_u64().unwrap();
    let attempt_id = obs["status"]["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap();
    let active_items = obs["active_items"].as_array().unwrap();
    let operations: Vec<Value> =
        ops.iter()
            .map(|(item_id, record)| {
                let item_rev = active_items
                    .iter()
                    .find(|item| item["item_id"] == *item_id)
                    .unwrap_or_else(|| panic!("missing active item {item_id} in {active_items:?}"))
                    ["revision"]
                    .as_u64()
                    .unwrap();
                json!({
                    "item_id": item_id,
                    "expected_item_revision": item_rev,
                    "record": record,
                })
            })
            .collect();
    serde_json::to_vec(&json!({
        "schema": "podway.item-record-many-input/v1",
        "workspace_uuid": workspace_uuid,
        "session_id": session_id,
        "session_revision": session_revision,
        "attempt_id": attempt_id,
        "idempotency_key": key,
        "operations": operations,
    }))
    .unwrap()
}

fn assert_wrapped_mutation<'a>(envelope: &'a Value, expected_inner_schema: &str) -> &'a Value {
    assert_eq!(
        envelope["result"]["schema"],
        "podway.mutation-observation-result/v1"
    );
    assert_eq!(
        envelope["result"]["mutation_result"]["schema"],
        expected_inner_schema
    );
    let obs = &envelope["result"]["observation"];
    assert_eq!(obs["schema"], "podway.observation-result/v6");
    assert!(obs.get("workspace").is_none());
    obs
}

#[test]
fn agent006_small_change_v2_integrated_caller_workflow_and_call_counts() {
    let fixture = Fixture::new();
    let mut collector = CallCollector::new(&fixture);

    // Setup calls (init, start, begin) are tracked in excluded_calls, separate from measured node segments.
    let init_out = collector.run_excluded_args(
        "setup",
        &[
            "--contract-generation",
            "2",
            "init",
            "--idempotency-key",
            "sc-init-1",
        ],
    );
    let workspace_uuid = init_out["workspace"]["uuid"].as_str().unwrap().to_owned();

    // Start small-change-v2 with --with-observation.
    let start_out = collector.run_excluded_args(
        "setup",
        &[
            "--contract-generation",
            "2",
            "--with-observation",
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "Tighten caller workflow documentation bounds",
            "--idempotency-key",
            "sc-start-1",
        ],
    );
    assert_eq!(start_out["workspace"]["uuid"], workspace_uuid);
    let start_obs = assert_wrapped_mutation(&start_out, "podway.session-start-result/v4");
    let begin_tpl = find_obs_template(start_obs, "session.begin");
    assert_eq!(begin_tpl["preconditions"]["workspace_uuid"], workspace_uuid);

    // Begin without --goal or --criterion (small-change-v2 has goal_tracking disabled).
    let begin_out = run_template_excluded(
        &mut collector,
        "setup",
        begin_tpl,
        "2",
        true,
        &[("<idempotency-key>", "sc-begin-1")],
        &[],
    );
    let obs = assert_wrapped_mutation(&begin_out, "podway.session-begin-result/v2");
    assert!(obs["guidance"]["goal"].is_null());
    assert_eq!(current_node_id(obs), "inspect");
    assert!(
        collector.calls.is_empty(),
        "setup calls must stay in excluded_calls before measured node segments"
    );

    // Action node 1: inspect (scope-summary text) -> 2 normal CLI calls (record + complete).
    assert!(
        obs["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["command"] != "item.record_many"),
        "inspect has no check_result item so item.record_many template is omitted"
    );
    collector.record_external_work("inspect-worktree-scope");
    let cp_inspect = collector.checkpoint();
    let inspect_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "sc-record-inspect",
        &[(
            "scope-summary",
            json!({"type": "text", "value": "Scoped to caller documentation and E2E tests."}),
        )],
    );
    let inspect_rec = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &inspect_stdin,
    );
    let obs = assert_wrapped_mutation(&inspect_rec, "podway.item-record-many-result/v2");
    let complete_inspect = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "sc-complete-inspect")],
        &[],
    );
    let obs = assert_wrapped_mutation(&complete_inspect, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp_inspect, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "implement");

    // Action node 2: implement (implementation-summary text) -> 2 normal CLI calls.
    collector.record_external_work("edit-caller-documentation");
    let cp_implement = collector.checkpoint();
    let implement_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "sc-record-implement",
        &[(
            "implementation-summary",
            json!({"type": "text", "value": "Updated runtime skill and workflow docs."}),
        )],
    );
    let implement_rec = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &implement_stdin,
    );
    let obs = assert_wrapped_mutation(&implement_rec, "podway.item-record-many-result/v2");
    let complete_implement = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "sc-complete-implement")],
        &[],
    );
    let obs = assert_wrapped_mutation(&complete_implement, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp_implement, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "verify");

    // Action node 3: verify (verification-command text + verification-exit-status integer) -> 2 normal CLI calls.
    assert!(
        obs["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["command"] != "item.record_many"),
        "small-change-v2 verify uses text + integer, not check_result"
    );
    collector.record_external_work("run-external-verification-command");
    let cp_verify = collector.checkpoint();
    let verify_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "sc-record-verify",
        &[
            (
                "verification-command",
                json!({"type": "text", "value": "make test-e2e-exact"}),
            ),
            (
                "verification-exit-status",
                json!({"type": "integer", "value": 0}),
            ),
        ],
    );
    let verify_rec = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &verify_stdin,
    );
    let obs = assert_wrapped_mutation(&verify_rec, "podway.item-record-many-result/v2");
    let complete_verify = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "sc-complete-verify")],
        &[],
    );
    let obs = assert_wrapped_mutation(&complete_verify, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp_verify, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "review");

    // Decision node 4: review (ready | changes-requested) -> 1 normal CLI call.
    let cp_review = collector.checkpoint();
    let decide_review = run_template_collected(
        &mut collector,
        find_decide_template(obs, "ready"),
        "2",
        true,
        &[
            (
                "<reason>",
                "Verification exited 0 and scope remained bounded.",
            ),
            ("<idempotency-key>", "sc-decide-review"),
        ],
        &[],
    );
    let obs = assert_wrapped_mutation(&decide_review, "podway.decision-result/v2");
    collector.assert_segment_commands(cp_review, &["decide"]);
    assert_eq!(current_node_id(obs), "closeout");

    // Action node 5: closeout (closeout-note text) -> 2 normal CLI calls.
    let cp_closeout = collector.checkpoint();
    let closeout_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "sc-record-closeout",
        &[(
            "closeout-note",
            json!({"type": "text", "value": "Small change completed and verified."}),
        )],
    );
    let closeout_rec = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &closeout_stdin,
    );
    let obs = assert_wrapped_mutation(&closeout_rec, "podway.item-record-many-result/v2");
    let complete_closeout = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "sc-complete-closeout")],
        &[],
    );
    let obs = assert_wrapped_mutation(&complete_closeout, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp_closeout, &["record", "complete"]);
    assert_eq!(obs["status"]["session"]["lifecycle"], "completed");
    assert!(obs["guidance"].is_null());

    // Total measured normal progression across small-change-v2's 4 action nodes + 1 decision node is 9 calls.
    assert_eq!(collector.calls.len(), 9);

    // Terminal disposition and independent verifier observation are tracked in excluded_calls.
    let disp_tpl = obs["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| {
            t["command"] == "session.terminal_disposition"
                && t["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tok| tok == "not-required")
        })
        .unwrap();
    let disp_out = run_template_excluded(
        &mut collector,
        "setup",
        disp_tpl,
        "2",
        false,
        &[
            (
                "<reason>",
                "No external handoff required for local fixture.",
            ),
            ("<idempotency-key>", "sc-disposition-1"),
        ],
        &[],
    );
    assert_eq!(disp_out["command"], "session.terminal_disposition");
    let verifier_obs = collector.run_excluded_args(
        "verifier_observation",
        &["--contract-generation", "2", "observe"],
    );
    assert_eq!(
        verifier_obs["result"]["status"]["session"]["lifecycle"],
        "completed"
    );
    assert_eq!(collector.calls.len(), 9);
    assert!(
        collector
            .excluded_calls
            .iter()
            .any(|(cat, _)| *cat == "setup")
    );
    assert!(
        collector
            .excluded_calls
            .iter()
            .any(|(cat, _)| *cat == "external_work")
    );
    assert!(
        collector
            .excluded_calls
            .iter()
            .any(|(cat, _)| *cat == "verifier_observation")
    );
}

#[test]
fn agent006_bug_fix_v2_integrated_caller_workflow_and_batch_goal_assessment() {
    let fixture = Fixture::new();
    let mut collector = CallCollector::new(&fixture);

    // Setup calls (init, start, begin) are tracked in excluded_calls, separate from measured node segments.
    let init_out = collector.run_excluded_args(
        "setup",
        &[
            "--contract-generation",
            "2",
            "init",
            "--idempotency-key",
            "bf-init-1",
        ],
    );
    let workspace_uuid = init_out["workspace"]["uuid"].as_str().unwrap().to_owned();

    let start_out = collector.run_excluded_args(
        "setup",
        &[
            "--contract-generation",
            "2",
            "--with-observation",
            "start",
            "--preset",
            "bug-fix-v2",
            "--task",
            "Fix duplicate login session on retry",
            "--idempotency-key",
            "bf-start-1",
        ],
    );
    let start_obs = assert_wrapped_mutation(&start_out, "podway.session-start-result/v4");
    let begin_tpl = find_obs_template(start_obs, "session.begin");

    // Begin with --goal and 3 criteria.
    let begin_out = run_template_excluded(
        &mut collector,
        "setup",
        begin_tpl,
        "2",
        true,
        &[("<idempotency-key>", "bf-begin-1")],
        &[
            "--goal",
            "Prevent duplicate login sessions on retry.",
            "--criterion",
            "reproduced=The duplicate-session defect is reproduced before the fix.",
            "--criterion",
            "verified=Automated verification passes for the fix.",
            "--criterion",
            "reviewed=Independent review approves the fix with zero unresolved valid findings.",
        ],
    );
    let obs = assert_wrapped_mutation(&begin_out, "podway.session-begin-result/v2");
    assert_eq!(obs["status"]["goal_revision"], 1);
    assert_eq!(obs["guidance"]["goal"]["revision"], 1);
    assert_eq!(
        obs["guidance"]["goal"]["criteria"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(current_node_id(obs), "reproduce");
    assert!(collector.calls.is_empty());

    // Node 1: reproduce (2 calls).
    collector.record_external_work("reproduce-defect-in-fixture");
    let cp = collector.checkpoint();
    let reproduce_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "bf-rec-reproduce",
        &[
            (
                "reproduction-status",
                json!({"type": "choice", "value": "reproduced"}),
            ),
            (
                "observed-behavior",
                json!({"type": "text", "value": "Retrying login created a second session row."}),
            ),
            (
                "expected-behavior",
                json!({"type": "text", "value": "Retrying login reuses the existing session row."}),
            ),
            (
                "regression-check",
                json!({"type": "text", "value": "bounded-fixture-check:fixture-input-readback"}),
            ),
        ],
    );
    let rec_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &reproduce_stdin,
    );
    let obs = assert_wrapped_mutation(&rec_out, "podway.item-record-many-result/v2");
    let comp_out = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "bf-comp-reproduce")],
        &[],
    );
    let obs = assert_wrapped_mutation(&comp_out, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "diagnose");

    // Node 2: diagnose (2 calls).
    collector.record_external_work("diagnose-root-cause");
    let cp = collector.checkpoint();
    let diagnose_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "bf-rec-diagnose",
        &[
            (
                "cause",
                json!({"type": "text", "value": "Missing idempotency lookup before session insert."}),
            ),
            (
                "affected-boundary",
                json!({"type": "text", "value": "crates/podway-store/src/direct_store/transaction.rs"}),
            ),
        ],
    );
    let rec_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &diagnose_stdin,
    );
    let obs = assert_wrapped_mutation(&rec_out, "podway.item-record-many-result/v2");
    let comp_out = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "bf-comp-diagnose")],
        &[],
    );
    let obs = assert_wrapped_mutation(&comp_out, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "implement");

    // Node 3: implement (2 calls).
    collector.record_external_work("implement-fix-in-fixture");
    let cp = collector.checkpoint();
    let implement_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "bf-rec-implement",
        &[
            (
                "fix-summary",
                json!({"type": "text", "value": "Check existing key before inserting session row."}),
            ),
            (
                "changed-boundaries",
                json!({"type": "list", "value": ["crates/podway-store/src/direct_store/transaction.rs"]}),
            ),
        ],
    );
    let rec_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &implement_stdin,
    );
    let obs = assert_wrapped_mutation(&rec_out, "podway.item-record-many-result/v2");
    let comp_out = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "bf-comp-implement")],
        &[],
    );
    let obs = assert_wrapped_mutation(&comp_out, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "verify");

    // Node 4: verify (has check_result target, so item.record_many stdin_template IS emitted; 2 calls).
    // Derive operation_id and operation_digest from the observed active_items check_result contract
    // and verify they match the item.record_many stdin_template.
    let verify_item = obs["active_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == "verification-result")
        .expect("verify node must declare verification-result active item");
    assert_eq!(verify_item["type"], "check_result");
    let observed_op_id = verify_item["constraints"]["operation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let observed_op_digest = verify_item["constraints"]["operation_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(observed_op_id, "bug-fix-verification");
    assert_eq!(
        observed_op_digest,
        "sha256:01122b6057efcfa3f22c453aa48fb647ccba9f7db437dd44473a9f32a854a979"
    );

    let record_many_tpl = find_obs_template(obs, "item.record_many");
    let mut verify_doc: Value =
        serde_json::from_str(record_many_tpl["stdin_template"].as_str().unwrap()).unwrap();
    assert_eq!(
        verify_doc["operations"][0]["record"]["operation_id"],
        observed_op_id
    );
    assert_eq!(
        verify_doc["operations"][0]["record"]["operation_digest"],
        observed_op_digest
    );

    // Outside the measured CLI segment, execute a bounded owned-fixture readback check
    // and hash its actual input and computed output. This checks file readback only;
    // no login regression or external suite is executed or attested by this fixture.
    collector.record_external_work("bounded-fixture-check:fixture-input-readback");
    let check_input_path = fixture.0.join("fixture-check-input.txt");
    let check_output_path = fixture.0.join("fixture-check-output.log");
    let expected_input = b"bounded owned fixture input\n";
    fs::write(&check_input_path, expected_input).unwrap();
    let check_input = fs::read(&check_input_path).unwrap();
    let check_passed = !check_input.is_empty()
        && check_input.len() <= 256
        && check_input.as_slice() == expected_input;
    assert!(check_passed, "bounded fixture input readback must match");
    let computed_output = serde_json::to_vec(&json!({
        "check": "fixture-input-readback",
        "input_bytes": check_input.len(),
        "matches_expected": check_passed,
        "external_suite_executed": false
    }))
    .unwrap();
    assert!(computed_output.len() <= 1024);
    fs::write(&check_output_path, &computed_output).unwrap();
    let check_output = fs::read(&check_output_path).unwrap();
    assert_eq!(check_output, computed_output);
    let input_digest = sha256_prefixed(&check_input);
    let output_digest = sha256_prefixed(&check_output);

    verify_doc["idempotency_key"] = json!("bf-rec-verify");
    verify_doc["operations"][0]["record"] = json!({
        "type": "check_result",
        "operation_id": observed_op_id,
        "operation_digest": observed_op_digest,
        "outcome": "pass",
        "summary": "Owned fixture input readback matched bounded expected bytes; no login regression or external suite was executed.",
        "input_basis": {
            "descriptor": "bounded-fixture-check-input:fixture-check-input.txt",
            "digest": input_digest
        },
        "executor": {
            "name": "agent006-fixture-readback",
            "version": "1"
        },
        "output_digest": output_digest
    });
    let cp = collector.checkpoint();
    let rec_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &serde_json::to_vec(&verify_doc).unwrap(),
    );
    let obs = assert_wrapped_mutation(&rec_out, "podway.item-record-many-result/v2");
    let comp_out = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "bf-comp-verify")],
        &[],
    );
    let obs = assert_wrapped_mutation(&comp_out, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "evaluate-verification");

    // Node 5: evaluate-verification (decision, 1 call).
    // Exercise --contract-generation 1 with --with-observation inside bug-fix-v2: it still embeds
    // fixed podway.observation-result/v6, and the next node (review) directly consumes its fences!
    let cp = collector.checkpoint();
    let decide_ev = run_template_collected(
        &mut collector,
        find_decide_template(obs, "passed"),
        "1",
        true,
        &[
            ("<reason>", "Bounded fixture verification check passed."),
            ("<idempotency-key>", "bf-decide-ev"),
        ],
        &[],
    );
    let obs = assert_wrapped_mutation(&decide_ev, "podway.decision-result/v2");
    collector.assert_segment_commands(cp, &["decide"]);
    assert_eq!(current_node_id(obs), "review");

    // Node 6: review (2 calls, consuming the embedded v6 observation returned by the generation-1 decide_ev call above).
    collector.record_external_work("review-fix-in-fixture");
    let cp = collector.checkpoint();
    let review_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "bf-rec-review",
        &[
            (
                "review-summary",
                json!({"type": "text", "value": "Reviewed transaction lookup and tests; no findings."}),
            ),
            (
                "unresolved-valid-findings",
                json!({"type": "integer", "value": 0}),
            ),
        ],
    );
    let rec_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &review_stdin,
    );
    let obs = assert_wrapped_mutation(&rec_out, "podway.item-record-many-result/v2");
    let comp_out = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "bf-comp-review")],
        &[],
    );
    let obs = assert_wrapped_mutation(&comp_out, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp, &["record", "complete"]);
    assert_eq!(current_node_id(obs), "evaluate-review");

    // Node 7: evaluate-review (decision, 1 call).
    let cp = collector.checkpoint();
    let decide_er = run_template_collected(
        &mut collector,
        find_decide_template(obs, "approved"),
        "2",
        true,
        &[
            (
                "<reason>",
                "Independent review approved with zero unresolved findings.",
            ),
            ("<idempotency-key>", "bf-decide-er"),
        ],
        &[],
    );
    let obs = assert_wrapped_mutation(&decide_er, "podway.decision-result/v2");
    collector.assert_segment_commands(cp, &["decide"]);
    assert_eq!(current_node_id(obs), "assess-goal");

    // Node 8: assess-goal. Verify single goal.assess_criterion template is present and no batch template exists.
    assert!(
        obs["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["command"] != "goal.assess_criteria")
    );
    let single_assess_tpl = find_obs_template(obs, "goal.assess_criterion");
    let pre = &single_assess_tpl["preconditions"];
    let goal_info = &obs["guidance"]["goal"];
    assert_eq!(goal_info["revision"], 1);
    assert_eq!(pre["goal_revision"], 1);
    let pre_session_rev = pre["session_revision"].as_u64().unwrap();
    assert_eq!(
        obs["status"]["session"]["revision"].as_u64().unwrap(),
        pre_session_rev
    );
    let seq_before_assess = decide_er["workspace"]["latest_workspace_sequence"]
        .as_u64()
        .unwrap();
    assert_eq!(
        decide_er["result"]["workspace_sequence"].as_u64().unwrap(),
        seq_before_assess
    );

    // Measured two-call segment at assess-goal: 3 criterion assessments + decision in 2 normal CLI calls!
    let cp_assess_goal = collector.checkpoint();
    let batch_assess_input = serde_json::to_vec(&json!({
        "schema": "podway.criterion-assessment-many-input/v1",
        "workspace_uuid": pre["workspace_uuid"],
        "session_id": pre["session_id"],
        "session_revision": pre["session_revision"],
        "attempt_id": pre["attempt_id"],
        "goal_revision": pre["goal_revision"],
        "idempotency_key": "bf-assess-all-1",
        "actor": "verifier",
        "assessments": [
            {
                "criterion_id": "reviewed",
                "status": "satisfied",
                "reason": "Review approved with zero unresolved findings.",
                "evidence": ["review", "evaluate-review"],
                "items": []
            },
            {
                "criterion_id": "verified",
                "status": "satisfied",
                "reason": "Verification check passed.",
                "evidence": ["verify"],
                "items": []
            },
            {
                "criterion_id": "reproduced",
                "status": "satisfied",
                "reason": "Reproduced before applying fix.",
                "evidence": ["reproduce"],
                "items": []
            }
        ]
    }))
    .unwrap();
    let assess_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "goal",
            "--with-observation",
            "assess-criteria",
            "--stdin",
        ],
        &batch_assess_input,
    );
    let assess_mut = &assess_out["result"]["mutation_result"];
    assert_eq!(
        assess_mut["schema"],
        "podway.criterion-assessment-many-result/v1"
    );
    assert_eq!(assess_mut["mode"], "assessment");
    assert_eq!(assess_mut["complete"], true);
    assert_eq!(assess_mut["determined_outcome"], "achieved");
    assert!(assess_mut.get("changed").is_none());
    assert!(assess_mut.get("assessed_count").is_none());
    // Batch assessment advances session revision and workspace sequence exactly once while goal_revision stays 1.
    assert_eq!(
        assess_mut["revision"].as_u64().unwrap(),
        pre_session_rev + 1
    );
    assert_eq!(assess_mut["goal_revision"], 1);
    assert_eq!(
        assess_out["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        seq_before_assess + 1
    );
    assert_eq!(
        assess_out["result"]["workspace_sequence"].as_u64().unwrap(),
        seq_before_assess + 1
    );
    // Results normalize to criterion declaration order (reproduced, verified, reviewed).
    let result_ids: Vec<&str> = assess_mut["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["criterion_id"].as_str().unwrap())
        .collect();
    assert_eq!(result_ids, vec!["reproduced", "verified", "reviewed"]);
    let obs = assert_wrapped_mutation(&assess_out, "podway.criterion-assessment-many-result/v1");
    assert_eq!(
        obs["status"]["session"]["revision"].as_u64().unwrap(),
        pre_session_rev + 1
    );
    assert_eq!(obs["status"]["goal_revision"], 1);
    assert_eq!(obs["guidance"]["goal"]["revision"], 1);
    assert_eq!(current_node_id(obs), "assess-goal");
    assert_eq!(
        obs["status"]["current"]["attempt"]["attempt_id"],
        pre["attempt_id"]
    );

    let decide_goal = run_template_collected(
        &mut collector,
        find_decide_template(obs, "achieved"),
        "2",
        true,
        &[
            ("<reason>", "All three bug-fix criteria are satisfied."),
            ("<idempotency-key>", "bf-decide-goal-1"),
        ],
        &[],
    );
    let obs = assert_wrapped_mutation(&decide_goal, "podway.decision-result/v2");
    collector.assert_segment_commands(cp_assess_goal, &["goal assess-criteria", "decide"]);
    assert_eq!(current_node_id(obs), "closeout");

    // Node 9: closeout (2 calls).
    let cp_closeout = collector.checkpoint();
    let closeout_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs,
        "bf-rec-closeout",
        &[(
            "closeout-note",
            json!({"type": "text", "value": "Bug-fix workflow completed and verified."}),
        )],
    );
    let rec_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &closeout_stdin,
    );
    let obs = assert_wrapped_mutation(&rec_out, "podway.item-record-many-result/v2");
    let comp_out = run_template_collected(
        &mut collector,
        find_obs_template(obs, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "bf-comp-closeout")],
        &[],
    );
    let obs = assert_wrapped_mutation(&comp_out, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp_closeout, &["record", "complete"]);
    assert_eq!(obs["status"]["session"]["lifecycle"], "completed");

    // Total measured normal progression across bug-fix-v2's 6 action nodes (12) + 2 single decisions (2) + 1 batch-assess+decide (2) is 16 calls.
    assert_eq!(collector.calls.len(), 16);
    let verifier_obs = collector.run_excluded_args(
        "verifier_observation",
        &["--contract-generation", "2", "observe"],
    );
    assert_eq!(
        verifier_obs["result"]["status"]["session"]["lifecycle"],
        "completed"
    );
    assert_eq!(collector.calls.len(), 16);
    assert!(
        collector
            .excluded_calls
            .iter()
            .any(|(cat, _)| *cat == "setup")
    );
    assert!(
        collector
            .excluded_calls
            .iter()
            .any(|(cat, _)| *cat == "external_work")
    );
    assert!(
        collector
            .excluded_calls
            .iter()
            .any(|(cat, _)| *cat == "verifier_observation")
    );
}

#[test]
fn agent006_integrated_caller_recovery_contention_and_fallback_paths() {
    let fixture = Fixture::new();
    let mut collector = CallCollector::new(&fixture);

    let init_out = collector.run_excluded_args(
        "setup",
        &[
            "--contract-generation",
            "2",
            "init",
            "--idempotency-key",
            "rec-init-1",
        ],
    );
    let workspace_uuid = init_out["workspace"]["uuid"].as_str().unwrap().to_owned();

    // 1. Unsupported --contract-generation is rejected with REQUEST_INVALID (exit 2) without silent downgrade.
    let (code, bad_gen) = collector.raw_excluded_args(
        "recovery_contention",
        &["--contract-generation", "99", "observe"],
    );
    assert_eq!(code, Some(2));
    assert_eq!(bad_gen["code"], "REQUEST_INVALID");

    // Start and begin small-change-v2.
    let start_out = collector.run_excluded_args(
        "setup",
        &[
            "--contract-generation",
            "2",
            "--with-observation",
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "Verify recovery, contention, and fallback rules",
            "--idempotency-key",
            "rec-start-1",
        ],
    );
    let start_obs = assert_wrapped_mutation(&start_out, "podway.session-start-result/v4");
    let begin_out = run_template_excluded(
        &mut collector,
        "setup",
        find_obs_template(start_obs, "session.begin"),
        "2",
        true,
        &[("<idempotency-key>", "rec-begin-1")],
        &[],
    );
    let obs_inspect = assert_wrapped_mutation(&begin_out, "podway.session-begin-result/v2");

    // 2. Record and complete `inspect` with --with-observation (measured 2-call segment).
    let cp_inspect = collector.checkpoint();
    let inspect_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs_inspect,
        "rec-inspect-record",
        &[(
            "scope-summary",
            json!({"type": "text", "value": "Initial inspect record."}),
        )],
    );
    let inspect_rec_out = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &inspect_stdin,
    );
    let obs_after_inspect_rec =
        assert_wrapped_mutation(&inspect_rec_out, "podway.item-record-many-result/v2");
    let complete_inspect_out = run_template_collected(
        &mut collector,
        find_obs_template(obs_after_inspect_rec, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "rec-inspect-complete")],
        &[],
    );
    let _obs_implement =
        assert_wrapped_mutation(&complete_inspect_out, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp_inspect, &["record", "complete"]);
    let seq_after_inspect = complete_inspect_out["workspace"]["latest_workspace_sequence"]
        .as_u64()
        .unwrap();

    // 3. Exact replay and `outcome lookup` of `rec-inspect-record` return the historical response from `inspect`
    // without advancing `latest_workspace_sequence`, and its embedded observation is historical (not current readiness!).
    let replay_rec_out = collector.run_excluded_stdin(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &inspect_stdin,
    );
    assert_eq!(replay_rec_out, inspect_rec_out);
    assert_eq!(
        current_node_id(&replay_rec_out["result"]["observation"]),
        "inspect"
    );
    let looked = collector.run_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "outcome",
            "lookup",
            "--idempotency-key",
            "rec-inspect-record",
        ],
    );
    assert_eq!(looked["result"]["found"], true);
    assert_eq!(looked["result"]["outcome"]["response"], inspect_rec_out);
    assert_eq!(
        looked["workspace"]["latest_workspace_sequence"],
        seq_after_inspect
    );

    // Attempting to derive a new mutation from the replayed/historical observation fails with conflict (exit 4)
    // and records no outcome in `outcome lookup`.
    let stale_complete_tpl =
        find_obs_template(&replay_rec_out["result"]["observation"], "session.complete");
    let stale_complete_argv = build_argv_from_template(
        stale_complete_tpl,
        "2",
        true,
        &[("<idempotency-key>", "rec-stale-from-replay")],
        &[],
    );
    let stale_refs: Vec<&str> = stale_complete_argv.iter().map(String::as_str).collect();
    let (code, stale_err) = collector.raw_excluded_args("recovery_contention", &stale_refs);
    assert_eq!(code, Some(4));
    assert_eq!(stale_err["code"], "SESSION_REVISION_CONFLICT");
    let stale_replay_lookup = collector.run_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "outcome",
            "lookup",
            "--idempotency-key",
            "rec-stale-from-replay",
        ],
    );
    assert_eq!(stale_replay_lookup["result"]["found"], false);
    assert_eq!(
        stale_replay_lookup["workspace"]["latest_workspace_sequence"],
        seq_after_inspect
    );

    // Mandatory fresh `observe` after replay/lookup/conflict restores current fences at `implement`.
    let fresh_obs_env = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "2", "observe"],
    );
    let obs_implement = &fresh_obs_env["result"];
    assert_eq!(current_node_id(obs_implement), "implement");

    // 4. SQLite BEGIN IMMEDIATE contention (`WORKSPACE_BUSY`, exit 4, timeout 20ms) records no outcome.
    let implement_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs_implement,
        "rec-implement-contended",
        &[(
            "implementation-summary",
            json!({"type": "text", "value": "Contended caller write."}),
        )],
    );
    let writer =
        rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (code, busy_err) = collector.raw_excluded_stdin(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "--timeout",
            "20ms",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &implement_stdin,
    );
    assert_eq!(code, Some(4));
    assert_eq!(busy_err["code"], "WORKSPACE_BUSY");
    assert_eq!(busy_err["retryable"], true);
    assert_eq!(busy_err["details"]["boundary"], "sqlite_write_lock");
    assert_eq!(busy_err["details"]["timeout_ms"], 20);
    writer.execute_batch("ROLLBACK").unwrap();

    let busy_lookup = collector.run_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "outcome",
            "lookup",
            "--idempotency-key",
            "rec-implement-contended",
        ],
    );
    assert_eq!(busy_lookup["result"]["found"], false);

    // 4b. Competing advancement + stale-fence refusal + refresh:
    // While the first caller still holds the pre-advancement observation `obs_implement`, a competing writer
    // advances `implement`. Submitting the pre-advancement session or item fence fails with exit 4 and
    // records no outcome; a fresh `observe` then supplies the updated fences so the retry succeeds.
    let competing_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs_implement,
        "rec-implement-competing-writer",
        &[(
            "implementation-summary",
            json!({"type": "text", "value": "Competing writer update at implement."}),
        )],
    );
    let competing_out = collector.run_excluded_stdin(
        "recovery_contention",
        &["--contract-generation", "2", "record", "--stdin"],
        &competing_stdin,
    );
    let advanced_session_rev = competing_out["result"]["revision"].as_u64().unwrap();

    // First caller submits its pre-advancement request -> fails with SESSION_REVISION_CONFLICT (exit 4) and no outcome.
    let stale_session_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs_implement,
        "rec-implement-stale-after-competing",
        &[(
            "implementation-summary",
            json!({"type": "text", "value": "Caller attempt with stale session fence."}),
        )],
    );
    let (code, stale_session_err) = collector.raw_excluded_stdin(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &stale_session_stdin,
    );
    assert_eq!(code, Some(4));
    assert_eq!(stale_session_err["code"], "SESSION_REVISION_CONFLICT");
    let stale_session_lookup = collector.run_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "outcome",
            "lookup",
            "--idempotency-key",
            "rec-implement-stale-after-competing",
        ],
    );
    assert_eq!(stale_session_lookup["result"]["found"], false);

    // Submitting updated session_revision with stale expected_item_revision (0 instead of 1) -> fails with ITEM_REVISION_CONFLICT (exit 4) and no outcome.
    let mut stale_item_doc: Value = serde_json::from_slice(&stale_session_stdin).unwrap();
    stale_item_doc["idempotency_key"] = json!("rec-implement-stale-item-fence");
    stale_item_doc["session_revision"] = json!(advanced_session_rev);
    let (code, stale_item_err) = collector.raw_excluded_stdin(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &serde_json::to_vec(&stale_item_doc).unwrap(),
    );
    assert_eq!(code, Some(4));
    assert_eq!(stale_item_err["code"], "ITEM_REVISION_CONFLICT");
    let stale_item_lookup = collector.run_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "outcome",
            "lookup",
            "--idempotency-key",
            "rec-implement-stale-item-fence",
        ],
    );
    assert_eq!(stale_item_lookup["result"]["found"], false);

    // Fresh `observe` after contention and competing advancement refreshes both session and item fences (`revision == 1`).
    let fresh_after_competing = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "2", "observe"],
    );
    assert_eq!(
        fresh_after_competing["result"]["active_items"][0]["revision"],
        1
    );
    let implement_stdin_fresh = build_record_many_stdin(
        &workspace_uuid,
        &fresh_after_competing["result"],
        "rec-implement-refreshed",
        &[(
            "implementation-summary",
            json!({"type": "text", "value": "Recorded after fresh observe following contention and competing write."}),
        )],
    );
    let impl_rec_out = collector.run_excluded_stdin(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &implement_stdin_fresh,
    );
    let obs_after_impl_rec =
        assert_wrapped_mutation(&impl_rec_out, "podway.item-record-many-result/v2");

    // 5. Simulated lost stdout on `complete --with-observation`: reconcile via `outcome lookup` + fresh `observe`.
    let _lost_stdout_ignored = run_template_excluded(
        &mut collector,
        "recovery_contention",
        find_obs_template(obs_after_impl_rec, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "rec-implement-complete-lost")],
        &[],
    );
    let reconciled = collector.run_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "outcome",
            "lookup",
            "--idempotency-key",
            "rec-implement-complete-lost",
        ],
    );
    assert_eq!(reconciled["result"]["found"], true);
    let fresh_after_lost = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "2", "observe"],
    );
    let obs_verify = &fresh_after_lost["result"];
    assert_eq!(current_node_id(obs_verify), "verify");

    // 6. Unsupported-route and unwrapped-response fallback paths:
    // 6a. Passing `--with-observation` on an unsupported route (`set`) is rejected with REQUEST_INVALID (exit 2) and no outcome.
    let (code, set_with_obs_err) = collector.raw_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "--with-observation",
            "set",
            "verification-command",
            "make test",
            "--idempotency-key",
            "rec-set-unsupported",
        ],
    );
    assert_eq!(code, Some(2));
    assert_eq!(set_with_obs_err["code"], "REQUEST_INVALID");
    let set_unsupported_lookup = collector.run_excluded_args(
        "recovery_contention",
        &[
            "--contract-generation",
            "2",
            "outcome",
            "lookup",
            "--idempotency-key",
            "rec-set-unsupported",
        ],
    );
    assert_eq!(set_unsupported_lookup["result"]["found"], false);

    // 6b. Execute a successful unsupported-route mutation (`set` without `--with-observation` using explicit
    // template fences from `obs_verify`), then run a mandatory fresh `observe`, and derive the next fenced
    // mutation (`record --stdin` for `verification-exit-status`) from that fresh observation!
    let set_cmd_tpl = obs_verify["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| {
            t["command"] == "item.set"
                && t["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tok| tok == "verification-command")
        })
        .expect("verify node must expose item.set template for verification-command");
    let set_cmd_out = run_template_excluded(
        &mut collector,
        "recovery_contention",
        set_cmd_tpl,
        "2",
        false,
        &[
            ("<value>", "make test-e2e-exact"),
            ("<idempotency-key>", "rec-verify-set-cmd"),
        ],
        &[],
    );
    assert_eq!(
        set_cmd_out["result"]["schema"],
        "podway.item-mutation-result/v3"
    );
    assert!(set_cmd_out["result"].get("observation").is_none());

    // Mandatory fresh `observe` after successful unsupported-route `set` mutation.
    let fresh_after_set = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "2", "observe"],
    );
    let obs_after_set = &fresh_after_set["result"];
    let verify_cmd_item = obs_after_set["active_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == "verification-command")
        .unwrap();
    assert_eq!(verify_cmd_item["revision"], 1);

    // Derive the next fenced mutation from `obs_after_set` (`verification-exit-status` at revision 0, updated session_revision).
    let verify_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs_after_set,
        "rec-verify-record",
        &[(
            "verification-exit-status",
            json!({"type": "integer", "value": 0}),
        )],
    );
    // Unwrapped record --stdin (without --with-observation) returns podway.item-record-many-result/v2 with no observation.
    let unwrapped_rec = collector.run_excluded_stdin(
        "recovery_contention",
        &["--contract-generation", "2", "record", "--stdin"],
        &verify_stdin,
    );
    assert_eq!(
        unwrapped_rec["result"]["schema"],
        "podway.item-record-many-result/v2"
    );
    assert!(unwrapped_rec["result"].get("observation").is_none());
    // Fallback to fresh observe when response is unwrapped.
    let fresh_after_unwrapped = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "2", "observe"],
    );
    let complete_verify_unwrapped = run_template_excluded(
        &mut collector,
        "recovery_contention",
        find_obs_template(&fresh_after_unwrapped["result"], "session.complete"),
        "2",
        false,
        &[("<idempotency-key>", "rec-verify-complete-unwrapped")],
        &[],
    );
    assert_eq!(
        complete_verify_unwrapped["result"]["schema"],
        "podway.stage-transition-result/v3"
    );
    assert!(
        complete_verify_unwrapped["result"]
            .get("observation")
            .is_none()
    );
    let fresh_gen1_at_review = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "1", "observe"],
    );
    assert_eq!(
        fresh_gen1_at_review["result"]["schema"],
        "podway.observation-result/v5"
    );
    let fresh_at_review = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "2", "observe"],
    );
    assert_eq!(current_node_id(&fresh_at_review["result"]), "review");

    // 7. Generation-1 --with-observation still embeds fixed podway.observation-result/v6, and its
    // fences drive the subsequent closeout action node!
    let cp_review = collector.checkpoint();
    let decide_gen1_with_obs = run_template_collected(
        &mut collector,
        find_decide_template(&fresh_at_review["result"], "ready"),
        "1",
        true,
        &[
            ("<reason>", "Verify generation-1 fixed embedded v6."),
            ("<idempotency-key>", "rec-decide-gen1-with-obs"),
        ],
        &[],
    );
    let obs_closeout = assert_wrapped_mutation(&decide_gen1_with_obs, "podway.decision-result/v2");
    collector.assert_segment_commands(cp_review, &["decide"]);
    assert_eq!(current_node_id(obs_closeout), "closeout");

    // Complete closeout from the generation-1 embedded v6 observation, then execute unsupported-route
    // terminal `disposition`, run a fresh `observe`, and derive the fenced `reset` mutation from it!
    let cp_closeout = collector.checkpoint();
    let closeout_stdin = build_record_many_stdin(
        &workspace_uuid,
        obs_closeout,
        "rec-closeout-record",
        &[(
            "closeout-note",
            json!({"type": "text", "value": "Completed recovery and fallback verification."}),
        )],
    );
    let closeout_rec = collector.run_stdin(
        &[
            "--contract-generation",
            "2",
            "record",
            "--stdin",
            "--with-observation",
        ],
        &closeout_stdin,
    );
    let obs_after_closeout_rec =
        assert_wrapped_mutation(&closeout_rec, "podway.item-record-many-result/v2");
    let complete_closeout = run_template_collected(
        &mut collector,
        find_obs_template(obs_after_closeout_rec, "session.complete"),
        "2",
        true,
        &[("<idempotency-key>", "rec-closeout-complete")],
        &[],
    );
    let obs_terminal =
        assert_wrapped_mutation(&complete_closeout, "podway.stage-transition-result/v3");
    collector.assert_segment_commands(cp_closeout, &["record", "complete"]);
    assert_eq!(obs_terminal["status"]["session"]["lifecycle"], "completed");

    let disp_tpl = obs_terminal["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| {
            t["command"] == "session.terminal_disposition"
                && t["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tok| tok == "not-required")
        })
        .unwrap();
    let disp_out = run_template_excluded(
        &mut collector,
        "recovery_contention",
        disp_tpl,
        "2",
        false,
        &[
            ("<reason>", "No external handoff required."),
            ("<idempotency-key>", "rec-disposition-1"),
        ],
        &[],
    );
    assert_eq!(disp_out["command"], "session.terminal_disposition");
    assert!(disp_out["result"].get("observation").is_none());

    // Mandatory fresh `observe` after terminal disposition (unsupported route for --with-observation),
    // then derive and execute the fenced `session.reset` template from that fresh observation.
    let fresh_after_disp = collector.run_excluded_args(
        "recovery_contention",
        &["--contract-generation", "2", "observe"],
    );
    let reset_tpl = find_obs_template(&fresh_after_disp["result"], "session.reset");
    let reset_out = run_template_excluded(
        &mut collector,
        "recovery_contention",
        reset_tpl,
        "2",
        false,
        &[("<idempotency-key>", "rec-reset-1")],
        &[],
    );
    assert_eq!(reset_out["command"], "session.reset");
    assert_eq!(collector.calls.len(), 5);
    assert!(
        collector
            .excluded_calls
            .iter()
            .any(|(cat, _)| *cat == "recovery_contention")
    );
}
