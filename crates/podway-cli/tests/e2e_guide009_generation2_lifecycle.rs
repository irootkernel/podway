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
