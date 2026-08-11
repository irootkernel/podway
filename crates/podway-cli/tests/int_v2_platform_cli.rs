//! Process-boundary contracts for the Procedure v2 platform CLI surface.

use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

use podway_core::WorkspaceId;
use podway_protocol::{
    RequestEnvelopeV1, WorktreeSelectorWireV1, decode_request_payload_v1, decode_single_frame_v1,
    encode_frame_v1,
};
use serde_json::{Value, json};

const WORKSPACE_ID: &str = "123e4567-e89b-42d3-a456-426614174001";
const SESSION_ID: &str = "123e4567-e89b-42d3-a456-426614174002";
const ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174003";
const JOB_ID: &str = "123e4567-e89b-42d3-a456-426614174004";
const NEXT_ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174005";
const BLOCKER_ID: &str = "123e4567-e89b-42d3-a456-426614174006";
const CURRENT_ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174007";
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
    StatusV2,
    StatusV2GoalDefined,
    StatusV2Verbose,
    SharedMutationV2,
    SharedMutationV1,
}

struct RecordingDaemon {
    handle: JoinHandle<io::Result<Value>>,
}

struct SequenceRecordingDaemon {
    handle: JoinHandle<io::Result<Vec<Value>>>,
}

impl SequenceRecordingDaemon {
    fn start(socket: &Path, replies: Vec<Reply>) -> Self {
        let listener = UnixListener::bind(socket).expect("recording socket must bind");
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600))
            .expect("recording socket must be private");
        let handle = thread::spawn(move || {
            let mut requests = Vec::with_capacity(replies.len());
            for reply in replies {
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
                requests
                    .push(serde_json::to_value(request).expect("recorded request must serialize"));
            }
            Ok(requests)
        });
        Self { handle }
    }

    fn finish(self) -> Vec<Value> {
        self.handle
            .join()
            .expect("recording daemon must not panic")
            .expect("recording daemon I/O must succeed")
    }
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
        Reply::StatusV2 | Reply::StatusV2GoalDefined => {
            let mut fixture: Value = serde_json::from_str(include_str!(
                "../../../tests/fixtures/v2/protocol/result-families.json"
            ))
            .expect("v2 result fixtures must parse");
            let mut result = fixture["fixtures"]["podway.status-result/v2"].take();
            result["session"]["id"] = json!(SESSION_ID);
            result["session"]["revision"] = json!(7);
            result["current"]["attempt"]["attempt_id"] = json!(ATTEMPT_ID);
            if matches!(reply, Reply::StatusV2GoalDefined) {
                result["current"]["node"] = json!({
                    "node_definition_id": "review",
                    "graph_node_id": "review",
                    "node_type": "decision"
                });
                result["goal_tracking"] = json!(true);
                result["goal_defined"] = json!(true);
                result["goal_revision"] = json!(1);
                result["goal"] = json!({
                    "revision": 1,
                    "statement": "Ship the v2 platform CLI.",
                    "criteria": [{
                        "criterion_id": "stable-json",
                        "statement": "JSON is stable.",
                        "status": "satisfied"
                    }],
                    "assessment_mode": "assessment",
                    "determined_outcome": "achieved"
                });
                result["allowed_option_ids"] = json!(["approve"]);
                result
                    .as_object_mut()
                    .expect("status result is an object")
                    .remove("terminal");
            }
            result["items"] = json!([{
                "item_id": "note",
                "type": "text",
                "required": true,
                "satisfied": false,
                "revision": 11
            }]);
            json!({
                "schema": "podway.output/v2",
                "request_id": request.request_id().as_str(),
                "command": "session.status",
                "generated_at": "2026-08-09T12:34:56.789Z",
                "workspace": {
                    "uuid": WORKSPACE_ID,
                    "root": request.workspace().expect("status selects a workspace").root(),
                    "latest_workspace_sequence": 8
                },
                "result": result,
                "warnings": []
            })
        }
        Reply::StatusV2Verbose => json!({
            "schema": "podway.output/v2",
            "request_id": request.request_id().as_str(),
            "command": "session.status",
            "generated_at": "2026-08-09T12:34:56.789Z",
            "workspace": {
                "uuid": WORKSPACE_ID,
                "root": request.workspace().expect("status selects a workspace").root(),
                "latest_workspace_sequence": 10
            },
            "result": verbose_status_result(),
            "warnings": []
        }),
        Reply::SharedMutationV2 => {
            let mut fixture: Value = serde_json::from_str(include_str!(
                "../../../tests/fixtures/v2/protocol/result-families.json"
            ))
            .expect("v2 result fixtures must parse");
            let schema = match request.command().as_str() {
                "session.complete" | "session.retry" | "session.skip" | "session.block"
                | "session.unblock" | "session.cancel" | "session.reset" => {
                    "podway.stage-transition-result/v2"
                }
                command if command.starts_with("item.") => "podway.item-mutation-result/v2",
                command => panic!("unexpected shared v2 mutation {command}"),
            };
            let mut result = fixture["fixtures"][schema].take();
            result["admission"]["job_id"] = json!(JOB_ID);
            result["admission"]["workspace_sequence"] = json!(8);
            if request.command().as_str().starts_with("item.") {
                result["attempt_id"] = json!(ATTEMPT_ID);
                result["item_id"] = request.payload()["item_id"].clone();
            } else {
                result["from_attempt_id"] = json!(ATTEMPT_ID);
                if request.command().as_str() == "session.retry" {
                    result["transition"] = json!("retry");
                    result["to_graph_node_id"] = result["from_graph_node_id"].clone();
                    result["to_attempt_id"] = json!(NEXT_ATTEMPT_ID);
                    result["reason"] = request.payload()["reason"].clone();
                    result["session_state"] = json!("running");
                } else if request.command().as_str() == "session.skip" {
                    result["transition"] = json!("skip");
                    result["to_graph_node_id"] = json!("finish");
                    result["to_attempt_id"] = json!(NEXT_ATTEMPT_ID);
                    if let Some(reason) = request.payload().get("reason") {
                        result["reason"] = reason.clone();
                    } else {
                        result.as_object_mut().unwrap().remove("reason");
                    }
                    result["session_state"] = json!("running");
                } else if request.command().as_str() == "session.block" {
                    result["transition"] = json!("block");
                    result["blocker_id"] = json!(BLOCKER_ID);
                    result["reason"] = request.payload()["reason"].clone();
                    result["session_state"] = json!("running");
                } else if request.command().as_str() == "session.unblock" {
                    result["transition"] = json!("unblock");
                    if request.payload()["all"] == Value::Bool(true) {
                        result["all"] = json!(true);
                    } else {
                        result["blocker_id"] = request.payload()["blocker_id"].clone();
                    }
                    result["session_state"] = json!("running");
                } else if request.command().as_str() == "session.cancel" {
                    result["transition"] = json!("cancel");
                    result["session_state"] = json!("cancelled");
                } else if request.command().as_str() == "session.reset" {
                    result["transition"] = json!("reset");
                    result["reset"] = json!(true);
                    result.as_object_mut().unwrap().remove("from_graph_node_id");
                    result.as_object_mut().unwrap().remove("from_attempt_id");
                    result.as_object_mut().unwrap().remove("session_state");
                }
            }
            json!({
                "schema": "podway.output/v2",
                "request_id": request.request_id().as_str(),
                "command": request.command().as_str(),
                "generated_at": "2026-08-09T12:34:56.789Z",
                "workspace": {
                    "uuid": WORKSPACE_ID,
                    "root": request.workspace().expect("mutation selects a workspace").root(),
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
                "result": result,
                "warnings": []
            })
        }
        Reply::SharedMutationV1 => json!({
            "schema": "podway.output/v1",
            "request_id": request.request_id().as_str(),
            "command": request.command().as_str(),
            "generated_at": "2026-08-09T12:34:56.789Z",
            "workspace": {
                "uuid": WORKSPACE_ID,
                "root": request.workspace().expect("mutation selects a workspace").root(),
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
                "schema": "podway.stage-transition-result/v1",
                "changed": true,
                "revision_before": 7,
                "revision_after": 8,
                "admission": {
                    "admitted": true,
                    "job_id": JOB_ID,
                    "workspace_sequence": 8
                }
            },
            "warnings": []
        }),
    }
}

fn verbose_status_result() -> Value {
    let mut fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/result-families.json"
    ))
    .expect("v2 result fixtures must parse");
    let mut result = fixture["fixtures"]["podway.status-result/v2"].take();
    result["tier"] = json!("verbose");
    result["session"]["id"] = json!(SESSION_ID);
    result["session"]["revision"] = json!(10);
    result["current"]["node"] = json!({
        "node_definition_id": "finish",
        "graph_node_id": "finish",
        "node_type": "action"
    });
    result["current"]["attempt"] = json!({
        "attempt_id": CURRENT_ATTEMPT_ID,
        "attempt_number": 3
    });
    result["trace_length"] = json!(8);
    result["counters"] = json!([
        {"graph_node_id": "work", "attempt_count": 2, "rework_traversal_count": 1},
        {"graph_node_id": "review", "attempt_count": 3, "rework_traversal_count": 1},
        {"graph_node_id": "finish", "attempt_count": 3, "rework_traversal_count": 0}
    ]);
    result["queue"]["latest_workspace_sequence"] = json!(10);
    result["allowed_manual_rework_targets"] = json!(["review"]);
    result["current_trace_history"] = json!({
        "entries": [
            {
                "trace_sequence": 8,
                "graph_node_id": "finish",
                "node_definition_id": "finish",
                "attempt_id": CURRENT_ATTEMPT_ID,
                "attempt_number": 3,
                "goal_revision": null,
                "lifecycle": "active",
                "validity": "valid",
                "started_at": "2026-08-09T12:34:55.000Z"
            },
            {
                "trace_sequence": 7,
                "graph_node_id": "review",
                "node_definition_id": "review",
                "attempt_id": NEXT_ATTEMPT_ID,
                "attempt_number": 3,
                "goal_revision": null,
                "lifecycle": "completed",
                "validity": "valid",
                "started_at": "2026-08-09T12:34:54.000Z",
                "finished_at": "2026-08-09T12:34:55.000Z"
            },
            {
                "trace_sequence": 4,
                "graph_node_id": "work",
                "node_definition_id": "work",
                "attempt_id": ATTEMPT_ID,
                "attempt_number": 2,
                "goal_revision": null,
                "lifecycle": "completed",
                "validity": "valid",
                "started_at": "2026-08-09T12:34:50.000Z",
                "finished_at": "2026-08-09T12:34:52.000Z"
            }
        ],
        "trace_truncated": false,
        "trace_window": {"first_sequence": 4, "last_sequence": 8}
    });
    result["stale_attempt_history"] = json!({
        "entries": [{
            "trace_sequence": 6,
            "graph_node_id": "finish",
            "node_definition_id": "finish",
            "attempt_id": JOB_ID,
            "attempt_number": 2,
            "goal_revision": null,
            "lifecycle": "completed",
            "validity": "stale",
            "started_at": "2026-08-09T12:34:53.000Z",
            "finished_at": "2026-08-09T12:34:54.000Z",
            "terminal_reason": null,
            "items": [],
            "items_total": 0,
            "items_truncated": false,
            "references": []
        }],
        "trace_truncated": true,
        "trace_window": {"first_sequence": 6, "last_sequence": 6}
    });
    for field in [
        "stale_goal_revision_history",
        "stale_goal_assessment_history",
    ] {
        result[field] = json!({"entries": [], "trace_truncated": false, "trace_window": null});
    }
    result["decision_history"] = json!({
        "entries": [{
            "trace_sequence": 7,
            "session_id": SESSION_ID,
            "session_revision": 9,
            "procedure_schema": "podway.procedure/v2",
            "procedure_snapshot_id": NEXT_ATTEMPT_ID,
            "procedure_digest": PROCEDURE_DIGEST,
            "graph_node_id": "review",
            "node_definition_id": "review",
            "attempt_id": NEXT_ATTEMPT_ID,
            "attempt_number": 3,
            "goal_revision": null,
            "option_id": "accept",
            "effect": "advance",
            "target_graph_node_id": "finish",
            "reason": "The immutable review record supports acceptance.",
            "actor": "V2DRW reviewer",
            "recorded_at": "2026-08-09T12:34:55.000Z",
            "references": []
        }],
        "trace_truncated": true,
        "trace_window": {"first_sequence": 7, "last_sequence": 7}
    });
    result["rework_history"] = json!({
        "entries": [
            {
                "trace_sequence": 7,
                "kind": "manual",
                "from_graph_node_id": "finish",
                "to_graph_node_id": "review",
                "target_attempt_id": NEXT_ATTEMPT_ID,
                "reason": "Review the latest evidence again.",
                "actor": "V2DRW reviewer",
                "reactivated": true,
                "recorded_at": "2026-08-09T12:34:54.000Z"
            },
            {
                "trace_sequence": 4,
                "kind": "declared",
                "from_graph_node_id": "review",
                "to_graph_node_id": "work",
                "target_attempt_id": ATTEMPT_ID,
                "reason": "The declared branch requires another work attempt.",
                "reactivated": false,
                "recorded_at": "2026-08-09T12:34:50.000Z"
            }
        ],
        "trace_truncated": false,
        "trace_window": {"first_sequence": 4, "last_sequence": 7}
    });
    result
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
fn typed_v2_commands_shape_exact_requests_and_preserve_capability_errors() {
    let cases = [
        (
            "session.decide",
            vec![
                "--if-attempt",
                ATTEMPT_ID,
                "--if-goal-revision",
                "1",
                "decide",
                "--option",
                "approve",
                "--reason",
                "the goal assessment determines this option",
            ],
            json!({ "option_id": "approve", "reason": "the goal assessment determines this option" }),
            json!({ "session_id": SESSION_ID, "session_revision": 7, "attempt_id": ATTEMPT_ID, "goal_revision": 1 }),
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
fn v2_status_preflight_supplies_omitted_attempt_fences() {
    let fixture = Fixture::new();
    let daemon =
        SequenceRecordingDaemon::start(&fixture.socket, vec![Reply::StatusV2, Reply::Unsupported]);
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--idempotency-key".to_owned(),
        "v2run002-preflight-key".to_owned(),
        "decide".to_owned(),
        "--option".to_owned(),
        "approve".to_owned(),
        "--reason".to_owned(),
        "ready".to_owned(),
    ];

    let output = fixture.run(&arguments);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(one_json(&output)["code"], "UNSUPPORTED_V2_CAPABILITY");
    let requests = daemon.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["command"], "session.status");
    assert_eq!(requests[0]["operation"], "query");
    assert_eq!(requests[1]["command"], "session.decide");
    assert_eq!(requests[1]["workspace"]["expected_uuid"], WORKSPACE_ID);
    assert_eq!(requests[1]["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(requests[1]["preconditions"]["session_revision"], 7);
    assert_eq!(requests[1]["preconditions"]["attempt_id"], ATTEMPT_ID);
    assert!(requests[1]["preconditions"].get("goal_revision").is_none());

    let fixture = Fixture::new();
    let daemon = SequenceRecordingDaemon::start(
        &fixture.socket,
        vec![Reply::StatusV2GoalDefined, Reply::Unsupported],
    );
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--idempotency-key".to_owned(),
        "v2gol003-assessment-decide-key".to_owned(),
        "decide".to_owned(),
        "--option".to_owned(),
        "approve".to_owned(),
        "--reason".to_owned(),
        "The recorded criteria determine this outcome.".to_owned(),
    ];
    let output = fixture.run(&arguments);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let requests = daemon.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["command"], "session.decide");
    assert_eq!(requests[1]["preconditions"]["goal_revision"], 1);

    let fixture = Fixture::new();
    let daemon =
        SequenceRecordingDaemon::start(&fixture.socket, vec![Reply::StatusV2, Reply::Unsupported]);
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--if-workspace-uuid".to_owned(),
        WORKSPACE_ID.to_owned(),
        "--if-session-id".to_owned(),
        SESSION_ID.to_owned(),
        "--if-session-revision".to_owned(),
        "7".to_owned(),
        "--idempotency-key".to_owned(),
        "v2run002-rework-preflight-key".to_owned(),
        "rework".to_owned(),
        "--to".to_owned(),
        "implement".to_owned(),
        "--reason".to_owned(),
        "review found a gap".to_owned(),
    ];

    let output = fixture.run(&arguments);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let requests = daemon.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["command"], "session.status");
    assert_eq!(requests[1]["command"], "session.rework");
    assert_eq!(requests[1]["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(requests[1]["preconditions"]["session_revision"], 7);
    assert_eq!(requests[1]["preconditions"]["attempt_id"], ATTEMPT_ID);
}

#[test]
fn v2gol003_fully_fenced_general_decide_skips_status_preflight() {
    let fixture = Fixture::new();
    let daemon = RecordingDaemon::start(&fixture.socket, Reply::Unsupported);
    let mut arguments = fixture.daemon_arguments(&[]);
    arguments.extend([
        "--if-attempt".to_owned(),
        ATTEMPT_ID.to_owned(),
        "decide".to_owned(),
        "--option".to_owned(),
        "approve".to_owned(),
        "--reason".to_owned(),
        "Replay the fully fenced general decision.".to_owned(),
    ]);

    let output = fixture.run(&arguments);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(one_json(&output)["code"], "UNSUPPORTED_V2_CAPABILITY");
    let request = daemon.finish();
    assert_eq!(request["command"], "session.decide");
    assert_eq!(request["operation"], "mutate");
    assert_eq!(request["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(request["preconditions"]["session_revision"], 7);
    assert_eq!(request["preconditions"]["attempt_id"], ATTEMPT_ID);
    assert!(request["preconditions"].get("goal_revision").is_none());
}

#[test]
fn shared_item_complete_retry_and_skip_use_v2_status_fences_and_render_output_v2() {
    for (command, arguments, expected_schema) in [
        (
            "item.set",
            vec!["set", "note", "verified"],
            "podway.item-mutation-result/v2",
        ),
        (
            "session.complete",
            vec!["complete"],
            "podway.stage-transition-result/v2",
        ),
        (
            "session.retry",
            vec!["retry", "--reason", "repeat from clean state"],
            "podway.stage-transition-result/v2",
        ),
        (
            "session.skip",
            vec!["skip", "--reason", "not applicable"],
            "podway.stage-transition-result/v2",
        ),
    ] {
        for json_mode in [true, false] {
            let fixture = Fixture::new();
            let daemon = SequenceRecordingDaemon::start(
                &fixture.socket,
                vec![Reply::StatusV2, Reply::SharedMutationV2],
            );
            let mut cli = vec![
                "--socket".to_owned(),
                fixture.socket.display().to_string(),
                "--worktree".to_owned(),
                fixture.root.display().to_string(),
                "--idempotency-key".to_owned(),
                format!("v2run003-{command}-{json_mode}"),
            ];
            if json_mode {
                cli.insert(0, "--json".to_owned());
            }
            cli.extend(arguments.iter().map(|argument| (*argument).to_owned()));

            let output = fixture.run(&cli);
            assert!(output.status.success(), "{command}: {output:?}");
            let requests = daemon.finish();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0]["command"], "session.status");
            assert_eq!(requests[1]["command"], command);
            assert_eq!(requests[1]["workspace"]["expected_uuid"], WORKSPACE_ID);
            assert_eq!(requests[1]["preconditions"]["session_id"], SESSION_ID);
            assert_eq!(requests[1]["preconditions"]["attempt_id"], ATTEMPT_ID);
            if command == "item.set" {
                assert!(
                    requests[1]["preconditions"]
                        .get("session_revision")
                        .is_none()
                );
                assert_eq!(requests[1]["preconditions"]["item_revision"], 11);
                assert_eq!(requests[1]["payload"]["item_id"], "note");
                assert_eq!(requests[1]["payload"]["value"], "verified");
            } else {
                assert_eq!(requests[1]["preconditions"]["session_revision"], 7);
                assert!(requests[1]["preconditions"].get("item_revision").is_none());
                if command == "session.retry" {
                    assert_eq!(requests[1]["payload"]["reason"], "repeat from clean state");
                } else if command == "session.skip" {
                    assert_eq!(requests[1]["payload"]["reason"], "not applicable");
                }
            }

            if json_mode {
                let envelope = one_json(&output);
                assert_eq!(envelope["schema"], "podway.output/v2");
                assert_eq!(envelope["command"], command);
                assert_eq!(envelope["result"]["schema"], expected_schema);
            } else {
                let text = String::from_utf8_lossy(&output.stdout);
                assert!(text.contains("workspace:"), "{text}");
                assert!(text.contains(expected_schema), "{text}");
            }
        }
    }
}

#[test]
fn fully_fenced_retry_skips_status_preflight_and_preserves_explicit_fences() {
    let fixture = Fixture::new();
    let daemon = RecordingDaemon::start(&fixture.socket, Reply::SharedMutationV2);
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--if-workspace-uuid".to_owned(),
        WORKSPACE_ID.to_owned(),
        "--if-session-id".to_owned(),
        SESSION_ID.to_owned(),
        "--if-session-revision".to_owned(),
        "7".to_owned(),
        "--if-attempt".to_owned(),
        ATTEMPT_ID.to_owned(),
        "--idempotency-key".to_owned(),
        "v2run004-fully-fenced-retry".to_owned(),
        "retry".to_owned(),
        "--reason".to_owned(),
        "repeat without a status probe".to_owned(),
    ];

    let output = fixture.run(&arguments);
    assert!(output.status.success(), "{output:?}");
    let request = daemon.finish();
    assert_eq!(request["command"], "session.retry");
    assert_eq!(request["workspace"]["expected_uuid"], WORKSPACE_ID);
    assert_eq!(request["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(request["preconditions"]["session_revision"], 7);
    assert_eq!(request["preconditions"]["attempt_id"], ATTEMPT_ID);
    assert_eq!(
        request["payload"]["reason"],
        "repeat without a status probe"
    );
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["result"]["transition"], "retry");
}

#[test]
fn fully_fenced_skip_skips_status_preflight_and_preserves_explicit_fences() {
    let fixture = Fixture::new();
    let daemon = RecordingDaemon::start(&fixture.socket, Reply::SharedMutationV2);
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--if-workspace-uuid".to_owned(),
        WORKSPACE_ID.to_owned(),
        "--if-session-id".to_owned(),
        SESSION_ID.to_owned(),
        "--if-session-revision".to_owned(),
        "7".to_owned(),
        "--if-attempt".to_owned(),
        ATTEMPT_ID.to_owned(),
        "--idempotency-key".to_owned(),
        "v2run005-fully-fenced-skip".to_owned(),
        "skip".to_owned(),
    ];

    let output = fixture.run(&arguments);
    assert!(output.status.success(), "{output:?}");
    let request = daemon.finish();
    assert_eq!(request["command"], "session.skip");
    assert_eq!(request["workspace"]["expected_uuid"], WORKSPACE_ID);
    assert_eq!(request["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(request["preconditions"]["session_revision"], 7);
    assert_eq!(request["preconditions"]["attempt_id"], ATTEMPT_ID);
    assert!(request["payload"].get("reason").is_none());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["result"]["transition"], "skip");
    assert!(envelope["result"].get("reason").is_none());
}

#[test]
fn fully_fenced_state_mutations_skip_status_preflight_and_render_output_v2() {
    let cases = [
        (
            "session.block",
            vec!["block", "--reason", "waiting for an external decision"],
            "block",
            true,
        ),
        (
            "session.unblock",
            vec!["unblock", BLOCKER_ID],
            "unblock",
            true,
        ),
        ("session.unblock", vec!["unblock", "--all"], "unblock", true),
        (
            "session.cancel",
            vec!["cancel", "--reason", "the task is no longer needed"],
            "cancel",
            true,
        ),
        ("session.reset", vec!["reset"], "reset", false),
    ];

    for (command, command_arguments, transition, requires_attempt) in cases {
        let fixture = Fixture::new();
        let daemon = RecordingDaemon::start(&fixture.socket, Reply::SharedMutationV2);
        let mut arguments = vec![
            "--json".to_owned(),
            "--socket".to_owned(),
            fixture.socket.display().to_string(),
            "--worktree".to_owned(),
            fixture.root.display().to_string(),
            "--if-workspace-uuid".to_owned(),
            WORKSPACE_ID.to_owned(),
            "--if-session-id".to_owned(),
            SESSION_ID.to_owned(),
            "--if-session-revision".to_owned(),
            "7".to_owned(),
            "--idempotency-key".to_owned(),
            format!("v2run006-fully-fenced-{transition}"),
        ];
        if requires_attempt {
            arguments.extend(["--if-attempt".to_owned(), ATTEMPT_ID.to_owned()]);
        } else {
            arguments.push("--yes".to_owned());
        }
        arguments.extend(command_arguments.into_iter().map(str::to_owned));

        let output = fixture.run(&arguments);
        assert!(output.status.success(), "{command}: {output:?}");
        let request = daemon.finish();
        assert_eq!(request["command"], command);
        assert_eq!(request["workspace"]["expected_uuid"], WORKSPACE_ID);
        assert_eq!(request["preconditions"]["session_id"], SESSION_ID);
        assert_eq!(request["preconditions"]["session_revision"], 7);
        if requires_attempt {
            assert_eq!(request["preconditions"]["attempt_id"], ATTEMPT_ID);
        } else {
            assert!(request["preconditions"].get("attempt_id").is_none());
            assert_eq!(request["payload"]["confirmed"], true);
        }
        let envelope = one_json(&output);
        assert_eq!(envelope["schema"], "podway.output/v2");
        assert_eq!(
            envelope["result"]["schema"],
            "podway.stage-transition-result/v2"
        );
        assert_eq!(envelope["result"]["transition"], transition);
    }
}

#[test]
fn state_mutations_with_omitted_fences_use_v2_status_preflight() {
    for (command, command_arguments, requires_attempt) in [
        (
            "session.block",
            vec!["block", "--reason", "waiting for review"],
            true,
        ),
        ("session.unblock", vec!["unblock", "--all"], true),
        (
            "session.cancel",
            vec!["cancel", "--reason", "cancel after review"],
            true,
        ),
        ("session.reset", vec!["reset"], false),
    ] {
        let fixture = Fixture::new();
        let daemon = SequenceRecordingDaemon::start(
            &fixture.socket,
            vec![Reply::StatusV2, Reply::SharedMutationV2],
        );
        let mut arguments = vec![
            "--json".to_owned(),
            "--socket".to_owned(),
            fixture.socket.display().to_string(),
            "--worktree".to_owned(),
            fixture.root.display().to_string(),
            "--idempotency-key".to_owned(),
            format!("v2run006-preflight-{command}"),
        ];
        if command == "session.reset" {
            arguments.push("--yes".to_owned());
        }
        arguments.extend(command_arguments.into_iter().map(str::to_owned));

        let output = fixture.run(&arguments);
        assert!(output.status.success(), "{command}: {output:?}");
        let requests = daemon.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["command"], "session.status");
        assert_eq!(requests[1]["command"], command);
        assert_eq!(requests[1]["workspace"]["expected_uuid"], WORKSPACE_ID);
        assert_eq!(requests[1]["preconditions"]["session_id"], SESSION_ID);
        assert_eq!(requests[1]["preconditions"]["session_revision"], 7);
        if requires_attempt {
            assert_eq!(requests[1]["preconditions"]["attempt_id"], ATTEMPT_ID);
        } else {
            assert!(requests[1]["preconditions"].get("attempt_id").is_none());
        }
        assert_eq!(one_json(&output)["schema"], "podway.output/v2");
    }
}

#[test]
fn state_mutation_preflight_preserves_retained_output_v1() {
    for (command, command_arguments) in [
        (
            "session.block",
            vec!["block", "--reason", "retained v1 blocker"],
        ),
        ("session.unblock", vec!["unblock", BLOCKER_ID]),
        ("session.unblock", vec!["unblock", "--all"]),
        (
            "session.cancel",
            vec!["cancel", "--reason", "retained v1 cancellation"],
        ),
        ("session.reset", vec!["reset"]),
    ] {
        let fixture = Fixture::new();
        let daemon = SequenceRecordingDaemon::start(
            &fixture.socket,
            vec![Reply::StatusV2, Reply::SharedMutationV1],
        );
        let mut arguments = vec![
            "--json".to_owned(),
            "--socket".to_owned(),
            fixture.socket.display().to_string(),
            "--worktree".to_owned(),
            fixture.root.display().to_string(),
            "--idempotency-key".to_owned(),
            format!("v2run006-retained-v1-{command}"),
        ];
        if command == "session.reset" {
            arguments.push("--yes".to_owned());
        }
        arguments.extend(command_arguments.into_iter().map(str::to_owned));

        let output = fixture.run(&arguments);
        assert!(output.status.success(), "{command}: {output:?}");
        let requests = daemon.finish();
        assert_eq!(requests[0]["command"], "session.status");
        assert_eq!(requests[1]["command"], command);
        let envelope = one_json(&output);
        assert_eq!(envelope["schema"], "podway.output/v1");
        assert_eq!(
            envelope["result"]["schema"],
            "podway.stage-transition-result/v1"
        );
    }
}

#[test]
fn shared_complete_accepts_retained_output_v1_after_preflight() {
    let fixture = Fixture::new();
    let daemon = SequenceRecordingDaemon::start(
        &fixture.socket,
        vec![Reply::StatusV2, Reply::SharedMutationV1],
    );
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--if-workspace-uuid".to_owned(),
        WORKSPACE_ID.to_owned(),
        "--if-session-id".to_owned(),
        SESSION_ID.to_owned(),
        "--if-session-revision".to_owned(),
        "7".to_owned(),
        "--if-attempt".to_owned(),
        ATTEMPT_ID.to_owned(),
        "--idempotency-key".to_owned(),
        "v2run003-retained-v1-complete".to_owned(),
        "complete".to_owned(),
    ];
    let output = fixture.run(&arguments);
    assert!(output.status.success(), "{output:?}");
    let requests = daemon.finish();
    assert_eq!(requests[0]["command"], "session.status");
    assert_eq!(requests[1]["command"], "session.complete");
    assert_eq!(requests[1]["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(requests[1]["preconditions"]["session_revision"], 7);
    assert_eq!(requests[1]["preconditions"]["attempt_id"], ATTEMPT_ID);
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v1");
    assert_eq!(
        envelope["result"]["schema"],
        "podway.stage-transition-result/v1"
    );
}

#[test]
fn shared_skip_accepts_retained_output_v1_after_preflight() {
    let fixture = Fixture::new();
    let daemon = SequenceRecordingDaemon::start(
        &fixture.socket,
        vec![Reply::StatusV2, Reply::SharedMutationV1],
    );
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--idempotency-key".to_owned(),
        "v2run005-retained-v1-skip".to_owned(),
        "skip".to_owned(),
        "--reason".to_owned(),
        "retained v1 session".to_owned(),
    ];
    let output = fixture.run(&arguments);
    assert!(output.status.success(), "{output:?}");
    let requests = daemon.finish();
    assert_eq!(requests[0]["command"], "session.status");
    assert_eq!(requests[1]["command"], "session.skip");
    assert_eq!(requests[1]["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(requests[1]["preconditions"]["session_revision"], 7);
    assert_eq!(requests[1]["preconditions"]["attempt_id"], ATTEMPT_ID);
    assert_eq!(requests[1]["payload"]["reason"], "retained v1 session");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v1");
    assert_eq!(
        envelope["result"]["schema"],
        "podway.stage-transition-result/v1"
    );
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
fn goal_bearing_start_replace_preflights_omitted_and_partial_identity_fences() {
    for explicit_session_id in [false, true] {
        let fixture = Fixture::new();
        let daemon = SequenceRecordingDaemon::start(
            &fixture.socket,
            vec![Reply::StatusV2, Reply::Unsupported],
        );
        let mut arguments = vec![
            "--json".to_owned(),
            "--socket".to_owned(),
            fixture.socket.display().to_string(),
            "--worktree".to_owned(),
            fixture.root.display().to_string(),
            "--idempotency-key".to_owned(),
            format!("v2gol-start-replace-preflight-{explicit_session_id}"),
        ];
        if explicit_session_id {
            arguments.extend(["--if-session-id".to_owned(), SESSION_ID.to_owned()]);
        }
        arguments.extend([
            "start".to_owned(),
            "--preset".to_owned(),
            "sw-dev".to_owned(),
            "--task".to_owned(),
            "Replace with a goal".to_owned(),
            "--goal".to_owned(),
            "Ship safely.".to_owned(),
            "--criterion".to_owned(),
            "tested=Tests pass.".to_owned(),
            "--replace".to_owned(),
            "--yes".to_owned(),
        ]);

        let output = fixture.run(&arguments);
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert_eq!(one_json(&output)["code"], "UNSUPPORTED_V2_CAPABILITY");
        let requests = daemon.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["command"], "session.status");
        assert_eq!(requests[0]["operation"], "query");
        if explicit_session_id {
            assert_eq!(requests[0]["preconditions"]["session_id"], SESSION_ID);
        } else {
            assert!(requests[0].get("preconditions").is_none());
        }
        assert_eq!(requests[1]["command"], "session.start_replace");
        assert_eq!(requests[1]["operation"], "mutate");
        assert_eq!(requests[1]["workspace"]["expected_uuid"], WORKSPACE_ID);
        assert_eq!(requests[1]["preconditions"]["session_id"], SESSION_ID);
        assert_eq!(requests[1]["preconditions"]["session_revision"], 7);
        assert_eq!(requests[1]["payload"]["goal"], "Ship safely.");
        assert_eq!(
            requests[1]["payload"]["criteria"],
            json!([{"criterion_id":"tested","statement":"Tests pass."}])
        );
        assert_eq!(requests[1]["payload"]["confirmed"], true);
    }
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
        vec!["status", "--verbose", "--history-before", "0"],
        vec![
            "status",
            "--verbose",
            "--history-before",
            "18446744073709551616",
        ],
        vec![
            "status",
            "--verbose",
            "--history-before",
            "3",
            "--wait-for-idle",
            "--compact",
        ],
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
fn verbose_status_sends_one_positive_exclusive_history_cursor() {
    let fixture = Fixture::new();
    let daemon = RecordingDaemon::start(&fixture.socket, Reply::WorkspaceError);
    let arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        fixture.socket.display().to_string(),
        "--worktree".to_owned(),
        fixture.root.display().to_string(),
        "--if-workspace-uuid".to_owned(),
        WORKSPACE_ID.to_owned(),
        "--if-session-id".to_owned(),
        SESSION_ID.to_owned(),
        "status".to_owned(),
        "--verbose".to_owned(),
        "--history-before".to_owned(),
        "42".to_owned(),
    ];
    let output = fixture.run(&arguments);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let request = daemon.finish();
    assert_eq!(request["command"], "session.status");
    assert_eq!(request["operation"], "query");
    assert_eq!(request["payload"]["verbose"], true);
    assert_eq!(request["payload"]["history_before"], 42);
    assert_eq!(request["preconditions"]["session_id"], SESSION_ID);
}

#[test]
fn v2drw005_verbose_status_preserves_decision_and_rework_history_in_json_and_text() {
    let expected_result = verbose_status_result();
    for json_mode in [true, false] {
        let fixture = Fixture::new();
        let daemon = RecordingDaemon::start(&fixture.socket, Reply::StatusV2Verbose);
        let mut arguments = vec![
            "--socket".to_owned(),
            fixture.socket.display().to_string(),
            "--worktree".to_owned(),
            fixture.root.display().to_string(),
            "--if-workspace-uuid".to_owned(),
            WORKSPACE_ID.to_owned(),
            "--if-session-id".to_owned(),
            SESSION_ID.to_owned(),
            "status".to_owned(),
            "--verbose".to_owned(),
            "--history-before".to_owned(),
            "9".to_owned(),
        ];
        if json_mode {
            arguments.insert(0, "--json".to_owned());
        }

        let output = fixture.run(&arguments);
        assert!(output.status.success(), "{output:?}");
        let request = daemon.finish();
        let canonical_root = fs::canonicalize(&fixture.root).unwrap();
        let selector = WorktreeSelectorWireV1::new(
            canonical_root.as_os_str().as_bytes(),
            canonical_root.display().to_string(),
            Some(WorkspaceId::new(WORKSPACE_ID).unwrap()),
        )
        .unwrap();
        assert_eq!(
            request["payload"],
            json!({
                "selector": selector,
                "verbose": true,
                "history_before": 9
            })
        );
        assert_eq!(request["command"], "session.status");
        assert_eq!(request["operation"], "query");
        assert_eq!(request["preconditions"], json!({"session_id": SESSION_ID}));

        if json_mode {
            assert_eq!(one_json(&output)["result"], expected_result);
        } else {
            let text = String::from_utf8(output.stdout).expect("text output must be UTF-8");
            let expected = serde_json::to_string_pretty(&expected_result).unwrap();
            assert!(text.ends_with(&format!("result: {expected}\n")), "{text}");
            let rendered = serde_json::from_str::<Value>(
                text.split_once("result: ")
                    .expect("text output must include the generic v2 result")
                    .1,
            )
            .expect("the rendered result must remain JSON");
            assert_eq!(
                rendered["decision_history"],
                expected_result["decision_history"]
            );
            assert_eq!(
                rendered["rework_history"],
                expected_result["rework_history"]
            );
            assert_eq!(
                rendered["rework_history"]["entries"][0]["trace_sequence"],
                7
            );
            assert_eq!(
                rendered["rework_history"]["entries"][1]["trace_sequence"],
                4
            );
            assert_eq!(rendered["rework_history"]["trace_truncated"], false);
            assert_eq!(
                rendered["rework_history"]["trace_window"],
                json!({"first_sequence": 4, "last_sequence": 7})
            );
        }
    }
}

#[test]
fn help_and_every_completion_target_publish_the_v2_routes_and_flags() {
    for topic in [
        "session.status",
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
    let fixture = Fixture::new();
    let replacement = fixture.run(&[
        "--json".to_owned(),
        "help".to_owned(),
        "session.start_replace".to_owned(),
    ]);
    assert!(replacement.status.success(), "{replacement:?}");
    let replacement_help = one_json(&replacement)["result"]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(replacement_help.contains(
        "--goal <text> --criterion <id>=<statement>... [--actor <text>] --replace \
         [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] \
         [--dry-run] [--yes]"
    ));
    for shell in ["bash", "zsh", "fish"] {
        let fixture = Fixture::new();
        let output = fixture.run(&["completions".to_owned(), shell.to_owned()]);
        assert!(output.status.success(), "{shell}: {output:?}");
        let script = String::from_utf8(output.stdout).expect("completion must be UTF-8");
        for token in [
            "history-before",
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
