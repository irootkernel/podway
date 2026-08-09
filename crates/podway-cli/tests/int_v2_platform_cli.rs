//! Process-boundary contracts for the Procedure v2 platform CLI surface.

use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

use podway_protocol::{
    RequestEnvelopeV1, decode_request_payload_v1, decode_single_frame_v1, encode_frame_v1,
};
use serde_json::{Value, json};

const WORKSPACE_ID: &str = "123e4567-e89b-42d3-a456-426614174001";
const SESSION_ID: &str = "123e4567-e89b-42d3-a456-426614174002";
const ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174003";
const JOB_ID: &str = "123e4567-e89b-42d3-a456-426614174004";
const PROCEDURE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    socket: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from("/tmp").join(format!("p8-{}-{sequence}", std::process::id()));
        fs::create_dir_all(&root).expect("fixture root must be creatable");
        fs::create_dir_all(root.join("account")).expect("fixture account root must be creatable");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("fixture root must be private");
        let socket = root.join("podwayd.sock");
        Self { root, socket }
    }

    fn run(&self, arguments: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(arguments)
            .env("PODWAY_TEST_ACCOUNT_ROOT", self.root.join("account"))
            .env_remove("HOME")
            .env_remove("TMPDIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("podway binary must run")
    }

    fn daemon_arguments(&self, command: &[&str]) -> Vec<String> {
        let mut arguments = vec![
            "--json".to_owned(),
            "--socket".to_owned(),
            self.socket.display().to_string(),
            "--worktree".to_owned(),
            self.root.display().to_string(),
            "--if-workspace-uuid".to_owned(),
            WORKSPACE_ID.to_owned(),
            "--if-session-id".to_owned(),
            SESSION_ID.to_owned(),
            "--if-session-revision".to_owned(),
            "7".to_owned(),
            "--idempotency-key".to_owned(),
            "v2plt008-recording-key".to_owned(),
        ];
        arguments.extend(command.iter().map(|argument| (*argument).to_owned()));
        arguments
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy)]
enum Reply {
    Unsupported,
    WorkspaceError,
    GoalDefinition,
}

struct RecordingDaemon {
    handle: JoinHandle<io::Result<Value>>,
}

impl RecordingDaemon {
    fn start(socket: &Path, reply: Reply) -> Self {
        let listener = UnixListener::bind(socket).expect("recording socket must bind");
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600))
            .expect("recording socket must be private");
        let handle = thread::spawn(move || {
            let (mut connection, _) = listener.accept()?;
            let mut frame = Vec::new();
            connection.read_to_end(&mut frame)?;
            let request = decode_request_payload_v1(
                decode_single_frame_v1(&frame)
                    .map_err(|error| io::Error::other(error.to_string()))?,
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
            let response = response_for(&request, reply);
            let frame = encode_frame_v1(
                &serde_json::to_vec(&response).expect("recording response must serialize"),
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
            connection.write_all(&frame)?;
            Ok(serde_json::to_value(request).expect("recorded request must serialize"))
        });
        Self { handle }
    }

    fn finish(self) -> Value {
        self.handle
            .join()
            .expect("recording daemon must not panic")
            .expect("recording daemon I/O must succeed")
    }
}

fn response_for(request: &RequestEnvelopeV1, reply: Reply) -> Value {
    match reply {
        Reply::Unsupported => json!({
            "schema": "podway.error/v1",
            "request_id": request.request_id().as_str(),
            "command": request.command().as_str(),
            "generated_at": "2026-08-09T12:34:56.789Z",
            "code": "UNSUPPORTED_V2_CAPABILITY",
            "message": "the registered Procedure v2 capability is not enabled",
            "retryable": false,
            "exit_code": 3,
            "details": {
                "schema": "podway.v2-runtime-error-details/v1",
                "kind": "UNSUPPORTED_V2_CAPABILITY",
                "capability": request.command().as_str(),
                "required_result_schema": required_result_schema(request.command().as_str()),
                "admission": { "admitted": false }
            }
        }),
        Reply::WorkspaceError => json!({
            "schema": "podway.error/v1",
            "request_id": request.request_id().as_str(),
            "command": request.command().as_str(),
            "generated_at": "2026-08-09T12:34:56.789Z",
            "code": "WORKSPACE_NOT_INITIALIZED",
            "message": "recording daemon rejection",
            "retryable": false,
            "exit_code": 5,
            "details": {}
        }),
        Reply::GoalDefinition => json!({
            "schema": "podway.output/v2",
            "request_id": request.request_id().as_str(),
            "command": "goal.define",
            "generated_at": "2026-08-09T12:34:56.789Z",
            "workspace": {
                "uuid": WORKSPACE_ID,
                "root": request.workspace().expect("goal definition selects a workspace").root(),
                "latest_workspace_sequence": 8
            },
            "job": {
                "id": JOB_ID,
                "sequence": 8,
                "state": "succeeded",
                "submitted_at": "2026-08-09T12:34:56.789Z",
                "claimed_at": "2026-08-09T12:34:56.789Z",
                "finished_at": "2026-08-09T12:34:56.789Z"
            },
            "result": {
                "schema": "podway.goal-definition-result/v1",
                "admission": { "admitted": true, "job_id": JOB_ID, "workspace_sequence": 8 },
                "goal_revision": 1,
                "statement": "Ship the v2 platform CLI.",
                "criteria": [{ "criterion_id": "stable-json", "statement": "JSON is stable." }],
                "actor": "reviewer",
                "recorded_at": "2026-08-09T12:34:56.789Z",
                "revision": 8
            },
            "warnings": []
        }),
    }
}

fn required_result_schema(command: &str) -> &'static str {
    match command {
        "session.start" | "session.start_replace" => "podway.session-start-result/v2",
        "session.decide" => "podway.decision-result/v1",
        "session.rework" => "podway.rework-result/v1",
        "goal.define" => "podway.goal-definition-result/v1",
        "goal.revise" => "podway.goal-revision-result/v1",
        "goal.assess_criterion" => "podway.criterion-assessment-result/v1",
        command => panic!("unexpected Procedure v2 command {command}"),
    }
}

fn one_json(output: &Output) -> Value {
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).lines().count(),
        1,
        "JSON mode must emit one object: {output:?}"
    );
    serde_json::from_slice(&output.stdout).expect("stdout must be JSON")
}

#[test]
fn reserved_v2_commands_shape_exact_requests_and_preserve_unsupported_errors() {
    let cases = [
        (
            "session.decide",
            vec![
                "--if-attempt",
                ATTEMPT_ID,
                "decide",
                "--option",
                "approve",
                "--reason",
                "evidence is sufficient",
                "--actor",
                "reviewer",
            ],
            json!({ "option_id": "approve", "reason": "evidence is sufficient", "actor": "reviewer" }),
            json!({ "session_id": SESSION_ID, "session_revision": 7, "attempt_id": ATTEMPT_ID }),
        ),
        (
            "session.rework",
            vec![
                "--if-attempt",
                ATTEMPT_ID,
                "rework",
                "--to",
                "implement",
                "--reason",
                "review found a gap",
                "--actor",
                "reviewer",
            ],
            json!({ "target_graph_node_id": "implement", "reason": "review found a gap", "actor": "reviewer" }),
            json!({ "session_id": SESSION_ID, "session_revision": 7, "attempt_id": ATTEMPT_ID }),
        ),
        (
            "goal.define",
            vec![
                "goal",
                "define",
                "--goal",
                "Ship safely.",
                "--criterion",
                "tested=Tests pass.",
                "--criterion",
                "reviewed=Review passes.",
                "--actor",
                "owner",
            ],
            json!({ "goal": "Ship safely.", "criteria": [{"criterion_id":"tested","statement":"Tests pass."},{"criterion_id":"reviewed","statement":"Review passes."}], "actor": "owner" }),
            json!({ "session_id": SESSION_ID, "session_revision": 7 }),
        ),
        (
            "goal.revise",
            vec![
                "--if-attempt",
                ATTEMPT_ID,
                "--if-goal-revision",
                "1",
                "goal",
                "revise",
                "--goal",
                "Ship after restart.",
                "--criterion",
                "restart-safe=Restart passes.",
                "--rework-to",
                "implement",
                "--reason",
                "Scope changed.",
                "--actor",
                "owner",
                "--reactivate",
            ],
            json!({ "goal": "Ship after restart.", "criteria": [{"criterion_id":"restart-safe","statement":"Restart passes."}], "target_graph_node_id": "implement", "reason": "Scope changed.", "actor": "owner", "reactivate": true }),
            json!({ "session_id": SESSION_ID, "session_revision": 7, "attempt_id": ATTEMPT_ID, "goal_revision": 1 }),
        ),
        (
            "goal.assess_criterion",
            vec![
                "--if-attempt",
                ATTEMPT_ID,
                "--if-goal-revision",
                "1",
                "goal",
                "assess-criterion",
                "restart-safe",
                "--status",
                "satisfied",
                "--reason",
                "The restart test passed.",
                "--evidence",
                "test",
                "--item",
                "assessment-note",
                "--actor",
                "reviewer",
            ],
            json!({ "criterion_id": "restart-safe", "status": "satisfied", "reason": "The restart test passed.", "evidence": ["test"], "items": ["assessment-note"], "actor": "reviewer" }),
            json!({ "session_id": SESSION_ID, "session_revision": 7, "attempt_id": ATTEMPT_ID, "goal_revision": 1 }),
        ),
    ];

    for (command, command_arguments, expected_payload, expected_preconditions) in cases {
        let fixture = Fixture::new();
        let daemon = RecordingDaemon::start(&fixture.socket, Reply::Unsupported);
        let arguments = fixture.daemon_arguments(&command_arguments);
        let output = fixture.run(&arguments);
        assert_eq!(output.status.code(), Some(3), "{command}: {output:?}");
        let error = one_json(&output);
        assert_eq!(error["code"], "UNSUPPORTED_V2_CAPABILITY", "{error}");
        let request = daemon.finish();
        assert_eq!(error["command"], command);
        assert_eq!(error["retryable"], false);
        assert_eq!(error["exit_code"], 3);
        assert_eq!(error["details"]["admission"]["admitted"], false);
        assert_eq!(request["operation"], "mutate");
        assert_eq!(request["command"], command);
        assert_eq!(
            request["payload"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| *key != "selector")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>(),
            expected_payload.as_object().unwrap().clone()
        );
        assert_eq!(request["preconditions"], expected_preconditions);
    }
}

#[test]
fn goal_bearing_start_is_typed_and_a_plain_v1_start_remains_unchanged() {
    let fixture = Fixture::new();
    let procedure = fixture.root.join("procedure.yaml");
    fs::write(&procedure, "schema: podway.procedure/v2\n").expect("fixture procedure must write");
    let daemon = RecordingDaemon::start(&fixture.socket, Reply::Unsupported);
    let mut arguments = fixture.daemon_arguments(&[
        "start",
        "--procedure",
        "procedure.yaml",
        "--expect-procedure-digest",
        PROCEDURE_DIGEST,
        "--task",
        "Ship v2",
        "--goal",
        "Ship safely.",
        "--criterion",
        "tested=Tests pass.",
        "--actor",
        "owner",
    ]);
    // A fresh start carries no session fences.
    for flag in ["--if-session-id", "--if-session-revision"] {
        let index = arguments
            .iter()
            .position(|argument| argument == flag)
            .unwrap();
        arguments.drain(index..=index + 1);
    }
    let output = fixture.run(&arguments);
    let request = daemon.finish();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(request["command"], "session.start");
    assert_eq!(request["payload"]["goal"], "Ship safely.");
    assert_eq!(
        request["payload"]["criteria"],
        json!([{"criterion_id":"tested","statement":"Tests pass."}])
    );
    assert_eq!(request["payload"]["actor"], "owner");
    assert!(request["payload"].get("initial_goal").is_none());

    let plain_fixture = Fixture::new();
    let plain_daemon = RecordingDaemon::start(&plain_fixture.socket, Reply::WorkspaceError);
    let mut plain_arguments = plain_fixture.daemon_arguments(&[
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Keep v1 unchanged",
    ]);
    for flag in ["--if-session-id", "--if-session-revision"] {
        let index = plain_arguments
            .iter()
            .position(|argument| argument == flag)
            .unwrap();
        plain_arguments.drain(index..=index + 1);
    }
    let plain_output = plain_fixture.run(&plain_arguments);
    let plain_request = plain_daemon.finish();
    assert_eq!(plain_output.status.code(), Some(5));
    assert_eq!(plain_request["command"], "session.start");
    for field in ["goal", "criteria", "actor", "initial_goal"] {
        assert!(plain_request["payload"].get(field).is_none(), "{field}");
    }
    assert_eq!(plain_request["payload"]["preset"], "sw-dev");
}

#[test]
fn v2_goal_success_has_stable_json_human_and_quiet_rendering() {
    for mode in ["json", "human", "quiet"] {
        let fixture = Fixture::new();
        let daemon = RecordingDaemon::start(&fixture.socket, Reply::GoalDefinition);
        let mut arguments = fixture.daemon_arguments(&[
            "goal",
            "define",
            "--goal",
            "Ship the v2 platform CLI.",
            "--criterion",
            "stable-json=JSON is stable.",
            "--actor",
            "reviewer",
        ]);
        match mode {
            "json" => {}
            "human" => arguments.retain(|argument| argument != "--json"),
            "quiet" => {
                arguments.retain(|argument| argument != "--json");
                arguments.insert(0, "--quiet".to_owned());
            }
            _ => unreachable!(),
        }
        let output = fixture.run(&arguments);
        daemon.finish();
        assert!(output.status.success(), "{mode}: {output:?}");
        match mode {
            "json" => {
                let envelope = one_json(&output);
                assert_eq!(envelope["schema"], "podway.output/v2");
                assert_eq!(envelope["command"], "goal.define");
                assert_eq!(
                    envelope["result"]["schema"],
                    "podway.goal-definition-result/v1"
                );
                assert_eq!(envelope["result"]["goal_revision"], 1);
            }
            "human" => {
                let text = String::from_utf8_lossy(&output.stdout);
                assert!(text.contains("Ship the v2 platform CLI."), "{text}");
                assert!(text.contains("stable-json"), "{text}");
            }
            "quiet" => assert!(output.stdout.is_empty()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn v2_grammar_rejects_invalid_shapes_before_contacting_a_daemon() {
    let invalid = [
        vec![
            "decide",
            "--option",
            "approve",
            "--reason",
            "valid but unfenced",
        ],
        vec![
            "goal",
            "define",
            "--goal",
            "Valid but unfenced.",
            "--criterion",
            "tested=Tests pass.",
        ],
        vec![
            "start",
            "--preset",
            "sw-dev",
            "--task",
            "Replace with a goal",
            "--goal",
            "Ship safely.",
            "--criterion",
            "tested=Tests pass.",
            "--replace",
            "--yes",
        ],
        vec!["goal", "define", "--goal", "Missing criteria."],
        vec!["goal", "define", "--criterion", "tested=Tests pass."],
        vec![
            "goal",
            "define",
            "--goal",
            "Bad criterion.",
            "--criterion",
            "missing-equals",
        ],
        vec![
            "goal",
            "define",
            "--goal",
            "Duplicate criteria.",
            "--criterion",
            "tested=First.",
            "--criterion",
            "tested=Second.",
        ],
        vec![
            "goal",
            "assess-criterion",
            "tested",
            "--status",
            "unknown",
            "--reason",
            "no",
        ],
        vec![
            "goal",
            "assess-criterion",
            "tested",
            "--status",
            "not_applicable",
            "--reason",
            "superseded",
            "--evidence",
            "test",
        ],
        vec![
            "goal",
            "assess-criterion",
            "tested",
            "--status",
            "satisfied",
            "--reason",
            "too many citations",
            "--evidence",
            "one",
            "--evidence",
            "two",
            "--evidence",
            "three",
            "--item",
            "four",
            "--item",
            "five",
        ],
        vec!["status", "--history-before", "3"],
        vec!["goal", "unknown"],
    ];
    for command in invalid {
        let fixture = Fixture::new();
        let arguments = command.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let output = fixture.run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
    }
}

#[test]
fn help_and_every_completion_target_publish_the_v2_routes_and_flags() {
    for topic in [
        "session.decide",
        "session.rework",
        "goal.define",
        "goal.revise",
        "goal.assess_criterion",
    ] {
        let fixture = Fixture::new();
        let output = fixture.run(&["--json".to_owned(), "help".to_owned(), topic.to_owned()]);
        assert!(output.status.success(), "{topic}: {output:?}");
        assert!(
            one_json(&output)["result"]["text"]
                .as_str()
                .unwrap()
                .contains("Usage:")
        );
    }
    for shell in ["bash", "zsh", "fish"] {
        let fixture = Fixture::new();
        let output = fixture.run(&["completions".to_owned(), shell.to_owned()]);
        assert!(output.status.success(), "{shell}: {output:?}");
        let script = String::from_utf8(output.stdout).expect("completion must be UTF-8");
        for token in [
            "decide",
            "rework",
            "goal",
            "assess-criterion",
            "if-goal-revision",
        ] {
            assert!(script.contains(token), "{shell} completion omits {token}");
        }
    }
}
