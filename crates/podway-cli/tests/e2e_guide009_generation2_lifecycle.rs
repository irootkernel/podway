//! Successor lifecycle and commit-bound observation through the real CLI.

use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::{Value, json};

use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::cli_fixture::{Fixture, GuardedChild, podway_binary};
use super::e2e_guide006_generation1_baselines::{
    guide006_cli_baseline, guide006_cli_next_prepared, guide006_cli_next_running,
    guide006_cli_observe_prepared, guide006_cli_observe_running, guide006_cli_status_running,
};

fn prepared() -> (Fixture, Value) {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let start = fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Scoped lifecycle",
        "--idempotency-key",
        "start",
        "--with-observation",
    ]);
    assert_eq!(
        start["result"]["schema"],
        "podway.mutation-observation-result/v2"
    );
    assert_eq!(
        start["result"]["observation"],
        observe(&fixture, "2")["result"]
    );
    (fixture, start)
}

fn observe(fixture: &Fixture, generation: &str) -> Value {
    fixture.run(&["--contract-generation", generation, "observe"])
}

fn lookup(fixture: &Fixture, key: &str) -> Value {
    fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"].clone()
}

fn result(output: std::process::Output) -> Value {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{output:?}"
    );
    serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
        .unwrap()
        .into_value()
}

fn error(output: std::process::Output, code: &str) {
    assert!(
        !output.status.success() && output.stderr.is_empty(),
        "{output:?}"
    );
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], code, "{}", error.as_value());
}

fn template<'a>(observation: &'a Value, route: &str) -> &'a Value {
    observation["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == route)
        .unwrap_or_else(|| panic!("missing {route}"))
}

fn template_argv(template: &Value, key: &str) -> Vec<String> {
    template["argv"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|arg| {
            let arg = arg.as_str().unwrap();
            if arg == "<idempotency-key>" {
                key.to_owned()
            } else {
                arg.to_owned()
            }
        })
        .collect()
}

fn template_input(template: &Value, key: &str) -> Value {
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!(key);
    input
}

fn scoped(fixture: &Fixture, before: &Value, args: &[&str], key: &str) -> Value {
    let cursor = before["result"]["cursor"].to_string();
    let mut argv = args.to_vec();
    argv.extend([
        "--if-cursor",
        &cursor,
        "--if-workspace-uuid",
        before["workspace"]["uuid"].as_str().unwrap(),
        "--idempotency-key",
        key,
    ]);
    fixture.run(&argv)
}

fn complete(fixture: &Fixture, key: &str) -> Value {
    fixture.run(&["complete", "--idempotency-key", key, "--with-observation"])
}

fn activate_default(fixture: &Fixture, key: &str) -> Value {
    let observed = observe(fixture, "2");
    let mut input = template_input(template(&observed, "group.activate"), key);
    input["working_copy_basis"] = json!({
        "descriptor":"Owned deterministic fixture",
        "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111",
    });
    result(fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        input.to_string().as_bytes(),
    ))
}

fn set(fixture: &Fixture, item: &str, value: &str, key: &str) {
    fixture.run(&["set", item, value, "--idempotency-key", key]);
}

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
    let mut guard = GuardedChild::new(child);
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

#[test]
fn generation2_lifecycle_conformance() {
    invalid_generation_fails_before_stdin(&Fixture::new());
    for (name, actual) in [
        (
            "observe_prepared",
            guide006_cli_observe_prepared(&["--contract-generation", "1"]),
        ),
        (
            "observe_running",
            guide006_cli_observe_running(&["--contract-generation", "1"]),
        ),
        (
            "next_prepared",
            guide006_cli_next_prepared(&["--contract-generation", "1"]),
        ),
        (
            "next_running",
            guide006_cli_next_running(&["--contract-generation", "1"]),
        ),
        (
            "status_running",
            guide006_cli_status_running(&["--contract-generation", "1"]),
        ),
    ] {
        assert_eq!(actual.normalized(), guide006_cli_baseline(name), "{name}");
    }
    let (fixture, _) = prepared();
    let snapshot = fixture.podway_snapshot();
    let first = observe(&fixture, "1");
    let second = observe(&fixture, "2");
    assert_eq!(first["result"]["cursor"], second["result"]["cursor"]);
    assert!(first["result"].get("mutation_templates").is_none());
    fixture.assert_podway_snapshot(&snapshot);

    let begin = template(&second, "session.begin");
    assert_eq!(begin["authority"], "optimistic_concurrency_only");
    let argv = template_argv(begin, "template-begin");
    let mut args: Vec<&str> = argv.iter().map(String::as_str).collect();
    args.extend(["--contract-generation", "2", "--with-observation"]);
    let original = result(fixture.raw_output(&args));
    assert_eq!(
        lookup(&fixture, "template-begin")["outcome"]["response"],
        original
    );
    assert_eq!(result(fixture.raw_output(&args)), original);

    scoped(
        &fixture,
        &observe(&fixture, "2"),
        &["cancel", "--reason", "Close the fixture."],
        "cancel",
    );
    let terminal = observe(&fixture, "2");
    assert!(
        template(&terminal, "session.terminal_disposition")["requires_explicit_authorization"]
            .is_boolean()
    );
    fixture.run(&[
        "disposition",
        "not-required",
        "--reason",
        "The fixture has no handoff.",
        "--idempotency-key",
        "disposition",
    ]);
    let disposed = observe(&fixture, "2");
    let argv = template_argv(template(&disposed, "session.archive"), "template-archive");
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let archived = result(fixture.raw_output(&args));
    assert_eq!(archived["result"]["activity"], "inactive");
    assert_eq!(
        lookup(&fixture, "template-archive")["outcome"]["response"],
        archived
    );
}

#[test]
fn agent005_with_observation_lifecycle_replay_lookup_and_conflicts() {
    let (fixture, start) = prepared();
    let same = fixture.run(&[
        "--contract-generation",
        "2",
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Scoped lifecycle",
        "--idempotency-key",
        "start",
        "--with-observation",
    ]);
    assert_eq!(same, start);
    error(
        fixture.output(&[
            "start",
            "--preset",
            "small-change-v3",
            "--task",
            "Scoped lifecycle",
            "--idempotency-key",
            "start",
        ]),
        "IDEMPOTENCY_KEY_REUSED",
    );
    let before = observe(&fixture, "2");
    let begin = scoped(&fixture, &before, &["begin", "--with-observation"], "begin");
    assert_eq!(
        begin["result"]["observation"],
        observe(&fixture, "2")["result"]
    );

    let running = observe(&fixture, "2");
    let mut cursor = running["result"]["cursor"].as_object().unwrap().clone();
    cursor.remove("session");
    let batch=json!({
        "schema":"podway.scoped-item-record-many-input/v1",
        "context":{"workspace_uuid":running["workspace"]["uuid"],"idempotency_key":"batch","cursor":cursor},
        "operations":[{"item_id":"scope-summary","expected_item_revision":0,"record":{"type":"text","value":"Pinned plan."}}],
    }).to_string();
    let original = result(fixture.output_with_stdin(
        &["record", "--stdin", "--with-observation"],
        batch.as_bytes(),
    ));
    assert_eq!(
        original["result"]["observation"],
        observe(&fixture, "2")["result"]
    );
    let replay = result(fixture.output_with_stdin(
        &[
            "record",
            "--stdin",
            "--with-observation",
            "--contract-generation",
            "2",
        ],
        batch.as_bytes(),
    ));
    assert_eq!(original, replay);
    assert_eq!(lookup(&fixture, "batch")["outcome"]["response"], original);
    error(
        fixture.output_with_stdin(&["record", "--stdin"], batch.as_bytes()),
        "IDEMPOTENCY_KEY_REUSED",
    );
    let rejected = fixture.output_with_stdin(
        &["record", "--stdin", "--contract-generation", "3"],
        b"invalid",
    );
    error(rejected, "REQUEST_INVALID");
}

struct Caller<'a> {
    fixture: &'a Fixture,
    calls: Vec<String>,
}

impl<'a> Caller<'a> {
    fn new(fixture: &'a Fixture) -> Self {
        Self {
            fixture,
            calls: Vec::new(),
        }
    }

    fn run(&mut self, argv: &[&str], input: Option<&Value>) -> Value {
        let response = result(match input {
            Some(input) => self
                .fixture
                .output_with_stdin(argv, input.to_string().as_bytes()),
            None => self.fixture.output(argv),
        });
        self.calls
            .push(response["command"].as_str().unwrap().to_owned());
        response
    }

    fn advance(&mut self, previous: &Value, args: &[&str], key: &str) -> Value {
        let cursor = previous["result"]["observation"]["cursor"].to_string();
        let mut argv = args.to_vec();
        argv.extend([
            "--if-workspace-uuid",
            previous["workspace"]["uuid"].as_str().unwrap(),
            "--if-cursor",
            &cursor,
            "--idempotency-key",
            key,
            "--with-observation",
        ]);
        self.run(&argv, None)
    }

    fn record_then_complete(
        &mut self,
        state: &Value,
        operations: Value,
        record_key: &str,
        complete_key: &str,
    ) -> Value {
        let record = state["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["command"] == "item.record_many")
            .unwrap();
        let mut input = template_input(record, record_key);
        input["operations"] = operations;
        let written = self.run(&["record", "--stdin", "--with-observation"], Some(&input));
        let advanced = self.advance(&written, &["complete"], complete_key);
        self.assert_segment(["item.record_many", "session.complete"]);
        advanced
    }

    fn assert_segment<const N: usize>(&mut self, expected: [&str; N]) {
        assert_eq!(self.calls, expected, "measured caller segment");
        self.calls.clear();
    }
}

fn record_item(item: &str, kind: &str, value: impl Into<Value>) -> Value {
    json!({
        "item_id": item, "expected_item_revision": 0,
        "record": {"type":kind,"value":value.into()},
    })
}

#[test]
fn agent006_small_change_v3_integrated_caller_workflow_and_call_counts() {
    let (fixture, _) = prepared();
    let initial = fixture.run(&["begin", "--idempotency-key", "begin"]);
    let sequence = initial["workspace"]["latest_workspace_sequence"]
        .as_u64()
        .unwrap();
    set(
        &fixture,
        "scope-summary",
        "Bound the fixture work.",
        "scope",
    );
    let activation = activate_default(&fixture, "activate");
    assert_eq!(activation["result"]["active_attempt"]["node"], "implement");
    let started = observe(&fixture, "2");

    let mut caller = Caller::new(&fixture);
    let implementation = record_item(
        "implementation-summary",
        "text",
        "Record fixture implementation",
    );
    let verification = record_item("verification-command", "text", "fixture verification");
    let exit = record_item("verification-exit-status", "integer", 0);
    let work = caller.record_then_complete(
        &started["result"],
        json!([implementation]),
        "work",
        "done-work",
    );
    assert_eq!(
        work["result"]["observation"]["cursor"]["attempt"]["node"],
        "verify"
    );
    let checks = caller.record_then_complete(
        &work["result"]["observation"],
        json!([verification, exit]),
        "checks",
        "done-checks",
    );
    assert_eq!(
        checks["result"]["observation"]["cursor"]["attempt"]["node"],
        "assess-review"
    );
    let review = caller.record_then_complete(
        &checks["result"]["observation"],
        json!([
            record_item("assessment-verdict", "choice", "success"),
            record_item("assessment-stage", "choice", "not-scored"),
            record_item("assessment-score", "integer", 0),
        ]),
        "assessment",
        "assessed",
    );
    assert_eq!(
        review["result"]["observation"]["cursor"]["attempt"]["node"],
        "review"
    );
    assert_eq!(review["result"]["observation"]["decision_entry"], 1);
    let chosen = caller.advance(
        &review,
        &[
            "decide",
            "--option",
            "ready",
            "--reason",
            "The fixture evidence supports it.",
        ],
        "choice",
    );
    caller.assert_segment(["session.decide"]);
    assert_eq!(
        chosen["result"]["observation"]["cursor"]["attempt"]["node"],
        "return-group-result"
    );
    let returned = caller.record_then_complete(
        &chosen["result"]["observation"],
        json!([implementation, verification, exit]),
        "return-items",
        "return",
    );
    assert_eq!(
        returned["result"]["observation"]["cursor"]["attempt"]["scope"]["kind"],
        "base"
    );
    let closed = caller.record_then_complete(
        &returned["result"]["observation"],
        json!([record_item(
            "closeout-note",
            "text",
            "The fixture work is finished."
        )]),
        "closeout",
        "finish",
    );
    assert_eq!(closed["result"]["observation"]["lifecycle"], "completed");
    assert_eq!(lookup(&fixture, "finish")["outcome"]["response"], closed);
    assert_eq!(
        closed["workspace"]["latest_workspace_sequence"],
        sequence + 13
    );
}

#[test]
fn agent006_scoped_goal_assessment_and_closeout() {
    let fixture = Fixture::new();
    let result_item = json!({
        "id":"finding","type":"text","prompt":"Record the scoped finding.","required":true,
    });
    let plan_item = json!({
        "id":"plan-note","type":"text","prompt":"Record the fixture plan.","required":true,
    });
    let base = json!({
        "schema":"podway.procedure/v3","id":"scoped-goal-fixture","version":"1",
        "name":"Scoped goal","purpose":"Exercise exact criterion assessment on return.",
        "goal_tracking":true,
        "node_definitions":{
            "planning":{"type":"action","title":"Plan","intent":"Bind the fixture plan.","items":[plan_item]},
            "assess":{"type":"decision","title":"Assess","objective":"Determine whether the goal is achieved.","prompt":"Which goal outcome applies?",
                "options":[
                    {"id":"achieved","label":"Achieved","criteria":"The scoped criterion is satisfied."},
                    {"id":"not-achieved","label":"Not achieved","criteria":"The scoped criterion is unsatisfied."},
                    {"id":"superseded","label":"Superseded","criteria":"All criteria are no longer applicable."},
                ],
                "reason":{"required":true,"prompt":"Explain the selected goal outcome."},
                "assessment":{"target":"session_goal","outcomes":{
                    "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"
                }},
            },
            "closeout":{"type":"action","title":"Close","intent":"Finish the fixture."}
        },
        "graph":{"entry":"plan","nodes":[
            {"id":"plan","use":"planning","next":"assessment"},
            {"id":"assessment","use":"assess",
                "evidence_from":[{"scope":"execution_result","required":true,"items":["finding"]}],
                "routes":{
                    "achieved":{"to":"close","effect":"advance"},
                    "not-achieved":{"to":"close","effect":"advance"},
                    "superseded":{"to":"close","effect":"advance"}
                }
            },
            {"id":"close","use":"closeout","terminal":true},
        ]},
        "execution_group":{"anchor":"plan","success_return":"assessment","plan_inputs":["plan-note"],"result_items":[result_item],"replan_decisions":[]}
    });
    let group = json!({
        "schema":"podway.execution-group/v1","id":"scoped-goal-group","version":"1",
        "name":"Scoped goal work","purpose":"Produce one bounded finding.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Write the fixture finding.","items":[result_item]}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]},
        "plan_inputs":[plan_item]
    });
    std::fs::write(fixture.0.join("base.json"), base.to_string()).unwrap();
    std::fs::write(fixture.0.join("group.json"), group.to_string()).unwrap();
    let base_preview = fixture.run(&["procedure", "preview", "base.json"]);
    let group_preview = fixture.run(&["procedure", "preview", "group.json"]);
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--procedure",
        "base.json",
        "--expect-procedure-digest",
        base_preview["result"]["digest"].as_str().unwrap(),
        "--task",
        "Scoped criterion",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&[
        "begin",
        "--goal",
        "Accept the fixture finding.",
        "--criterion",
        "qualified=The finding is admitted.",
        "--criterion",
        "current=The finding is current.",
        "--criterion",
        "scoped=The finding is scoped.",
        "--idempotency-key",
        "begin",
    ]);
    set(&fixture, "plan-note", "State the fixture plan.", "plan");
    let observation = observe(&fixture, "2");
    let mut activation = template_input(template(&observation, "group.activate"), "activate");
    activation["source"] = json!({"kind":"document","format":"json","source":group.to_string()});
    activation["expected_definition"] = group_preview["result"]["digest"].clone();
    activation["working_copy_basis"] = json!({"descriptor":"Owned criterion fixture","digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111"});
    result(fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        activation.to_string().as_bytes(),
    ));
    set(
        &fixture,
        "finding",
        "The current fixture produced the required finding.",
        "finding",
    );
    assert_eq!(
        complete(&fixture, "return")["result"]["observation"]["cursor"]["attempt"]["node"],
        "assessment"
    );

    let observed = observe(&fixture, "2");
    let mut assessment = template_input(template(&observed, "goal.assess_criteria"), "assess");
    assessment["assessments"] = json!(["qualified", "current", "scoped"].map(|criterion| json!({
        "criterion":criterion,"status":"satisfied","reason":"The current scoped result contains the finding.",
        "citations":[{"kind":"execution_result"}],
    })));
    let mut caller = Caller::new(&fixture);
    let accepted = caller.run(
        &["goal", "assess-criteria", "--stdin", "--with-observation"],
        Some(&assessment),
    );
    assert!(
        accepted["result"]["observation"]["goal"]["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .all(|criterion| criterion["status"] == "satisfied")
    );
    let decided = caller.advance(
        &accepted,
        &[
            "decide",
            "--option",
            "achieved",
            "--reason",
            "The scoped criteria are satisfied.",
        ],
        "decision",
    );
    caller.assert_segment(["goal.assess_criteria", "session.decide"]);
    assert_eq!(
        decided["result"]["observation"]["cursor"]["attempt"]["node"],
        "close"
    );
    assert_eq!(lookup(&fixture, "assess")["outcome"]["response"], accepted);
    let final_state = complete(&fixture, "finish");
    assert_eq!(
        final_state["result"]["observation"]["goal"]["outcome"],
        "achieved"
    );
    assert_eq!(
        final_state["result"]["observation"]["lifecycle"],
        "completed"
    );
}

#[test]
fn agent006_integrated_caller_recovery_contention_and_fallback_paths() {
    let (fixture, _) = prepared();
    let original = observe(&fixture, "2");
    let cursor = original["result"]["cursor"].to_string();
    let workspace = original["workspace"]["uuid"].as_str().unwrap();
    let begin = [
        "begin",
        "--idempotency-key",
        "recover",
        "--if-workspace-uuid",
        workspace,
        "--if-cursor",
        &cursor,
        "--with-observation",
    ];
    assert_eq!(lookup(&fixture, "recover")["found"], false);
    let lock = rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let busy = [
        "--timeout",
        "50ms",
        begin[0],
        begin[1],
        begin[2],
        begin[3],
        begin[4],
        begin[5],
        begin[6],
        begin[7],
    ];
    error(fixture.output(&busy), "WORKSPACE_BUSY");
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(lookup(&fixture, "recover")["found"], false);
    let response = fixture.run(&begin);
    assert_eq!(lookup(&fixture, "recover")["outcome"]["response"], response);
    assert_eq!(fixture.run(&begin), response);
    let fallback = observe(&fixture, "2");
    assert_eq!(fallback["result"], response["result"]["observation"]);
}
