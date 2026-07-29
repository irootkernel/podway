//! CLI process-boundary integration contracts using controlled daemon doubles.

use std::{
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

use nix::unistd::geteuid;
use podway_config::{
    MAX_PROCEDURE_DOCUMENT_BYTES_V1, ProcedureFormatV1, ProcedureWarningPolicyV1,
    parse_procedure_v1,
};
use podway_core::{
    AttemptId, CommandContextV1, CompleteSessionV1, DomainError, JobId, ProcedureSnapshotId,
    ProcedureSourceLabelV1, Revision, SessionAggregateV1, SessionCommandV1, SessionId, UnixMillis,
    WorkspaceId, apply_transition_v1,
};
use podway_protocol::{
    ErrorCodeV1, ErrorEnvelopeInputV1, ExitCodeV1, JobOutputV1, JobStateV1,
    MAX_SLICE_ITEM_TEXT_SCALARS_V1, MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1, OperationV1,
    OutputEnvelopeInputV1, RequestEnvelopeV1, ResponseEnvelopeV1, Rfc3339MillisV1, SliceCommandV1,
    SliceRequestV1, WorkspaceOutputV1, decode_request_payload_v1, decode_single_frame_v1,
    encode_frame_v1, encode_request_payload_v1, encode_response_payload_v1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const WORKSPACE_ID: &str = "123e4567-e89b-42d3-a456-426614174001";
const EXPLICIT_WORKSPACE_ID: &str = "123e4567-e89b-42d3-a456-426614174007";
const ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174002";
const SESSION_ID: &str = "123e4567-e89b-42d3-a456-426614174004";
const JOB_ID: &str = "123e4567-e89b-42d3-a456-426614174003";
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    socket_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("pwc-{}-{sequence}", std::process::id()));
        let home = root.join("home");
        fs::create_dir_all(&home).expect("fixture home must be created");
        let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw())
            .expect("fixture paths must be valid");
        fs::create_dir_all(paths.runtime_directory().as_path())
            .expect("fixture daemon runtime directory must be created");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("fixture root must be private");
        fs::set_permissions(
            paths.runtime_directory().as_path(),
            fs::Permissions::from_mode(0o700),
        )
        .expect("fixture runtime directory must be private");
        Self {
            root,
            socket_path: paths.socket_path().as_path().to_path_buf(),
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let arguments = self.arguments_with_explicit_endpoint(arguments);
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(&arguments)
            .env_remove("HOME")
            .env_remove("TMPDIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("podway binary must run")
    }
    fn run_with_stdin(&self, arguments: &[&str], input: &[u8]) -> Output {
        let arguments = self.arguments_with_explicit_endpoint(arguments);
        let mut child = Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(&arguments)
            .env_remove("HOME")
            .env_remove("TMPDIR")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("podway binary must start");
        let mut stdin = child.stdin.take().expect("podway stdin must be piped");
        stdin
            .write_all(input)
            .expect("stdin fixture must be writable");
        drop(stdin);
        child
            .wait_with_output()
            .expect("podway binary must complete")
    }

    fn arguments_with_explicit_endpoint(&self, arguments: &[&str]) -> Vec<String> {
        let mut resolved = Vec::with_capacity(arguments.len() + 2);
        if self.socket_path.exists() && !arguments.contains(&"--socket") {
            resolved.push("--socket".to_owned());
            resolved.push(self.socket_path.display().to_string());
        }
        resolved.extend(arguments.iter().map(|argument| (*argument).to_owned()));
        resolved
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy)]
enum Reply {
    Status,
    Output,
    StartOutput,
    Error,
    IdentityError,
    ContractMismatch,
    ResetUnreadable,
    CloseWithoutResponse,
    MalformedFramedResponse,
    MalformedStatusResult,
    MalformedNextResult,
}

struct FakeDaemon {
    handle: JoinHandle<io::Result<Vec<Vec<u8>>>>,
}

impl FakeDaemon {
    fn start(fixture: &Fixture, replies: Vec<Reply>) -> Self {
        Self::start_at(fixture.socket_path.clone(), replies)
    }

    fn start_at(socket_path: PathBuf, replies: Vec<Reply>) -> Self {
        let listener = UnixListener::bind(socket_path)
            .expect("fake daemon must bind the service-owned socket path");
        fs::set_permissions(
            listener
                .local_addr()
                .expect("fake daemon socket address must be readable")
                .as_pathname()
                .expect("fake daemon socket must be named"),
            fs::Permissions::from_mode(0o600),
        )
        .expect("fake daemon socket must be private");
        let handle = thread::spawn(move || {
            let mut wires = Vec::with_capacity(replies.len());
            for reply in replies {
                let (mut connection, _) = listener.accept()?;
                let mut wire = Vec::new();
                connection.read_to_end(&mut wire)?;
                let payload = decode_single_frame_v1(&wire)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let request = decode_request_payload_v1(payload)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                wires.push(wire);
                let frame = match reply {
                    Reply::CloseWithoutResponse => continue,
                    Reply::MalformedFramedResponse => {
                        encode_frame_v1(br#"{"schema":"podway.output/v1","invalid":true}"#)
                            .map_err(|error| io::Error::other(error.to_string()))?
                    }
                    Reply::MalformedStatusResult | Reply::MalformedNextResult => {
                        let payload = serde_json::to_vec(&json!({
                            "schema": "podway.output/v1",
                            "request_id": request.request_id().as_str(),
                            "command": request.command().as_str(),
                            "generated_at": "2026-07-15T12:34:56.789Z",
                            "workspace": {
                                "uuid": WORKSPACE_ID,
                                "root": "/fixture",
                                "latest_workspace_sequence": 9
                            },
                            "result": {"invalid": true},
                            "warnings": []
                        }))?;
                        encode_frame_v1(&payload)
                            .map_err(|error| io::Error::other(error.to_string()))?
                    }
                    _ => {
                        let response = reply.response_for(&request)?;
                        let payload = encode_response_payload_v1(&response)
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        encode_frame_v1(&payload)
                            .map_err(|error| io::Error::other(error.to_string()))?
                    }
                };
                connection.write_all(&frame)?;
                connection.shutdown(Shutdown::Write)?;
            }
            Ok(wires)
        });
        Self { handle }
    }

    fn finish(self) -> Vec<Vec<u8>> {
        self.handle
            .join()
            .expect("fake daemon thread must not panic")
            .expect("fake daemon I/O must succeed")
    }
}

#[test]
fn explicit_socket_selects_the_exact_daemon_endpoint_and_rejects_non_absolute_paths() {
    let fixture = Fixture::new();
    let explicit_socket = fixture.root.join("explicit.sock");
    let default_listener =
        UnixListener::bind(&fixture.socket_path).expect("default endpoint sentinel must bind");
    fs::set_permissions(&fixture.socket_path, fs::Permissions::from_mode(0o600))
        .expect("default endpoint sentinel must be private");
    default_listener
        .set_nonblocking(true)
        .expect("default endpoint sentinel must be nonblocking");
    let daemon = FakeDaemon::start_at(explicit_socket.clone(), vec![Reply::Status]);
    let explicit_socket_text = explicit_socket.display().to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_podway"))
        .args([
            "--json",
            "--socket",
            &explicit_socket_text,
            "--worktree",
            fixture.root.to_str().expect("fixture path must be UTF-8"),
            "status",
        ])
        .env_remove("HOME")
        .env_remove("TMPDIR")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("podway binary must run with a sanitized environment");
    assert!(
        output.status.success(),
        "explicit socket failed: {output:?}"
    );
    assert_eq!(daemon.finish().len(), 1);
    assert_eq!(
        default_listener
            .accept()
            .expect_err("an explicit endpoint must not fall back to the default socket")
            .kind(),
        io::ErrorKind::WouldBlock,
    );

    for (invalid, message) in [
        ("relative.sock", "socket path must be absolute"),
        ("~/podwayd.sock", "socket path must be absolute"),
        (
            "/tmp/../podwayd.sock",
            "socket path must be normalized and contain valid path characters",
        ),
    ] {
        let output = fixture.run(&["--json", "--socket", invalid, "status"]);
        assert_eq!(output.status.code(), Some(2), "{invalid}: {output:?}");
        let response: Value = serde_json::from_slice(&output.stdout).expect("typed JSON failure");
        assert_eq!(response["code"], "REQUEST_INVALID");
        assert_eq!(response["message"], message);
    }

    let local = fixture.run(&["--json", "--socket", &explicit_socket_text, "version"]);
    assert_eq!(local.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&local.stdout).expect("typed JSON failure");
    assert_eq!(response["code"], "REQUEST_INVALID");
}

#[test]
fn daemon_install_rejects_invalid_explicit_socket_before_resolving_the_daemon() {
    let fixture = Fixture::new();
    let overlong = format!("/tmp/{}.sock", "x".repeat(104));

    for (invalid, message) in [
        ("relative.sock", "socket path must be absolute"),
        ("~/podwayd.sock", "socket path must be absolute"),
        (
            "/tmp/../podwayd.sock",
            "socket path must be normalized and contain valid path characters",
        ),
        (
            overlong.as_str(),
            "socket path exceeds the macOS Unix socket path limit",
        ),
    ] {
        let output = fixture.run(&["--json", "--socket", invalid, "daemon", "install"]);
        assert_eq!(output.status.code(), Some(2), "{invalid}: {output:?}");
        let response: Value = serde_json::from_slice(&output.stdout).expect("typed JSON failure");
        assert_eq!(response["code"], "REQUEST_INVALID");
        assert_eq!(response["command"], "daemon.install");
        assert_eq!(response["message"], message);
    }
}

impl Reply {
    fn response_for(self, request: &RequestEnvelopeV1) -> io::Result<ResponseEnvelopeV1> {
        match self {
            Self::Status => output_response(request, status_result(), None),
            Self::Output | Self::StartOutput => {
                output_response(request, successful_mutation_result(request), Some(job()))
            }
            Self::Error => error_response(request),
            Self::IdentityError => identity_error_response(request),
            Self::ContractMismatch => contract_mismatch_response(request),
            Self::ResetUnreadable => error_response_with(
                request,
                "WORKSPACE_STATE_UNREADABLE",
                "Workspace state is corrupt or inaccessible.",
                5,
                false,
                false,
            ),
            Self::CloseWithoutResponse => Err(io::Error::other(
                "response-loss fixture closes without an envelope",
            )),
            Self::MalformedFramedResponse => Err(io::Error::other(
                "malformed framed response does not have an envelope",
            )),
            Self::MalformedStatusResult | Self::MalformedNextResult => Err(io::Error::other(
                "malformed result fixtures are emitted as raw response frames",
            )),
        }
    }
}

fn successful_mutation_result(request: &RequestEnvelopeV1) -> Map<String, Value> {
    let command = request.command().as_str();
    let admission = json!({
        "admitted": true,
        "job_id": JOB_ID,
        "workspace_sequence": 7,
    });
    if request.options().detach() {
        let mut result = Map::from_iter([
            ("admission".to_owned(), admission),
            ("detached".to_owned(), Value::Bool(true)),
        ]);
        if matches!(command, "session.start" | "session.start_replace") {
            result.insert(
                "procedure_digest".to_owned(),
                json!("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            );
        }
        return result;
    }
    if matches!(command, "session.start" | "session.start_replace") {
        return Map::from_iter([
            ("changed".to_owned(), Value::Bool(true)),
            ("revision_before".to_owned(), Value::from(0)),
            ("revision_after".to_owned(), Value::from(1)),
            (
                "procedure_digest".to_owned(),
                json!("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ),
            ("admission".to_owned(), admission),
        ]);
    }
    if command.starts_with("item.") {
        return Map::from_iter([
            ("changed".to_owned(), Value::Bool(true)),
            ("item_id".to_owned(), Value::String("item".to_owned())),
            ("revision_before".to_owned(), Value::from(1)),
            ("revision_after".to_owned(), Value::from(2)),
            ("admission".to_owned(), admission),
        ]);
    }
    if command == "session.reset" {
        return Map::from_iter([
            ("reset".to_owned(), Value::Bool(true)),
            ("revision".to_owned(), Value::from(2)),
            ("admission".to_owned(), admission),
        ]);
    }
    Map::from_iter([
        ("changed".to_owned(), Value::Bool(true)),
        ("revision_before".to_owned(), Value::from(1)),
        ("revision_after".to_owned(), Value::from(2)),
        ("admission".to_owned(), admission),
    ])
}

fn timestamp() -> Rfc3339MillisV1 {
    Rfc3339MillisV1::new("2026-07-15T12:34:56.789Z").expect("fixture timestamp must be valid")
}

fn workspace() -> WorkspaceOutputV1 {
    WorkspaceOutputV1::new(
        WorkspaceId::new(WORKSPACE_ID).expect("fixture workspace ID must be valid"),
        "/fixture",
        9,
    )
    .expect("fixture workspace must be valid")
}

fn job() -> JobOutputV1 {
    JobOutputV1::new(
        JobId::new(JOB_ID).expect("fixture job ID must be valid"),
        7,
        JobStateV1::Queued,
        timestamp(),
        None,
        None,
    )
    .expect("fixture job must be valid")
}

fn output_response(
    request: &RequestEnvelopeV1,
    result: Map<String, Value>,
    job: Option<JobOutputV1>,
) -> io::Result<ResponseEnvelopeV1> {
    podway_protocol::OutputEnvelopeV1::new(OutputEnvelopeInputV1 {
        request_id: request.request_id().clone(),
        command: request.command().clone(),
        generated_at: timestamp(),
        workspace: Some(workspace()),
        job,
        session: None,
        result,
        warnings: Vec::new(),
    })
    .map(ResponseEnvelopeV1::Output)
    .map_err(|error| io::Error::other(error.to_string()))
}

fn error_response(request: &RequestEnvelopeV1) -> io::Result<ResponseEnvelopeV1> {
    error_response_with(
        request,
        "JOB_WAIT_TIMEOUT",
        "The wait expired; the admitted job may continue.",
        4,
        true,
        true,
    )
}

fn identity_error_response(request: &RequestEnvelopeV1) -> io::Result<ResponseEnvelopeV1> {
    podway_protocol::ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: request.request_id().clone(),
        command: request.command().clone(),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("SESSION_ID_MISMATCH")
            .expect("identity mismatch code must be valid"),
        message: "The session ID differs from the expected identity.".to_owned(),
        retryable: false,
        exit_code: ExitCodeV1::new(4).expect("identity mismatch exit code must be valid"),
        workspace: Some(Map::from_iter([
            ("uuid".to_owned(), Value::String(WORKSPACE_ID.to_owned())),
            ("root".to_owned(), Value::String("/fixture".to_owned())),
        ])),
        details: serde_json::from_value(json!({
            "schema": "podway.session-id-mismatch-details/v1",
            "expected_session_id": SESSION_ID,
            "actual_session_id": null,
            "admission": { "admitted": false }
        }))
        .expect("identity mismatch details must be an object"),
    })
    .map(ResponseEnvelopeV1::Error)
    .map_err(|error| io::Error::other(error.to_string()))
}

fn contract_mismatch_response(request: &RequestEnvelopeV1) -> io::Result<ResponseEnvelopeV1> {
    let expected = podway_protocol::build_identity_v1();
    podway_protocol::ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: request.request_id().clone(),
        command: request.command().clone(),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("DAEMON_CONTRACT_MISMATCH")
            .expect("contract mismatch code must be valid"),
        message: "CLI and daemon contract identities differ.".to_owned(),
        retryable: false,
        exit_code: ExitCodeV1::new(3).expect("daemon mismatch exit code must be valid"),
        workspace: None,
        details: serde_json::from_value(json!({
            "expected": {
                "product": "podway",
                "contract_manifest_digest": format!("sha256:{}", "0".repeat(64)),
            },
            "actual": {
                "product": expected.product(),
                "contract_manifest_digest": expected.contract_manifest_digest(),
            },
            "admission": { "admitted": false },
        }))
        .expect("contract mismatch details must be an object"),
    })
    .map(ResponseEnvelopeV1::Error)
    .map_err(|error| io::Error::other(error.to_string()))
}

fn error_response_with(
    request: &RequestEnvelopeV1,
    code: &str,
    message: &str,
    exit_code: u8,
    retryable: bool,
    admitted: bool,
) -> io::Result<ResponseEnvelopeV1> {
    let mut details = if admitted {
        Map::from_iter([
            ("job_id".to_owned(), Value::String(JOB_ID.to_owned())),
            ("job_sequence".to_owned(), Value::from(7_u64)),
            (
                "admission".to_owned(),
                json!({
                    "admitted": true,
                    "job_id": JOB_ID,
                    "workspace_sequence": 7,
                }),
            ),
        ])
    } else {
        Map::from_iter([("admission".to_owned(), json!({"admitted": false}))])
    };
    if code == "ATTEMPT_NOT_CURRENT" {
        details.insert(
            "expected_attempt_id".to_owned(),
            Value::String(ATTEMPT_ID.to_owned()),
        );
    }
    podway_protocol::ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: request.request_id().clone(),
        command: request.command().clone(),
        generated_at: timestamp(),
        code: ErrorCodeV1::new(code).expect("fixture error code must be valid"),
        message: message.to_owned(),
        retryable,
        exit_code: ExitCodeV1::new(exit_code).expect("fixture exit code must be valid"),
        workspace: Some(Map::from_iter([
            ("uuid".to_owned(), Value::String(WORKSPACE_ID.to_owned())),
            ("root".to_owned(), Value::String("/fixture".to_owned())),
        ])),
        details,
    })
    .map(ResponseEnvelopeV1::Error)
    .map_err(|error| io::Error::other(error.to_string()))
}

fn status_result() -> Map<String, Value> {
    serde_json::from_value(json!({
        "task": {
            "title": "Fixture task",
            "procedure": {
                "id": "fixture",
                "version": "1.0.0",
                "name": "Fixture procedure",
                "digest": format!("sha256:{}", "a".repeat(64)),
            },
        },
        "session": {
            "id": SESSION_ID,
            "lifecycle": "running",
            "revision": 12,
            "created_at": "2026-07-15T12:34:56.789Z",
            "completed_at": null,
            "cancelled_at": null,
        },
        "current": {
            "stage_id": "implement",
            "stage_index": 0,
            "title": "Implement",
            "attempt_id": ATTEMPT_ID,
            "attempt_number": 1,
            "blocked": false,
            "ready_to_complete": false,
        },
        "stages": [{
            "id": "implement",
            "index": 0,
            "title": "Implement",
            "status": "current",
            "latest_attempt_number": 1,
        }],
        "items": [
            {
                "id": "goal",
                "type": "text",
                "prompt": "Goal",
                "required": true,
                "satisfied": true,
                "revision": 3,
                "value": "Fixture goal",
            },
            {
                "id": "artifact",
                "type": "artifact",
                "prompt": "Artifact",
                "required": false,
                "satisfied": false,
                "revision": 4,
                "value": null,
            },
        ],
        "blockers": [],
        "queue": {
            "pending_mutations": false,
            "queued_count": 0,
            "running_job_id": null,
            "latest_workspace_sequence": 9,
        },
    }))
    .expect("fixture status result must be an object")
}
fn seeded_session_with_advanced_cursor() -> (SessionAggregateV1, SessionAggregateV1) {
    let snapshot = parse_procedure_v1(
        r#"{
            "schema": "podway.procedure/v1",
            "id": "causal-drift",
            "version": "1",
            "name": "Causal drift fixture",
            "stages": [
                {"id": "implement", "title": "Implement", "instructions": [], "items": []},
                {"id": "verify", "title": "Verify", "instructions": [], "items": []}
            ],
            "rework": {"allow_return_to": "any_previous"}
        }"#,
        ProcedureFormatV1::Json,
    )
    .expect("stateful evaluator procedure must parse")
    .into_snapshot_v1(
        ProcedureSnapshotId::new("123e4567-e89b-42d3-a456-426614174097")
            .expect("fixture snapshot ID must be valid"),
        ProcedureSourceLabelV1::file("causal-drift").expect("fixture source label must be valid"),
        UnixMillis::new(1),
        ProcedureWarningPolicyV1::Accept,
    )
    .expect("stateful evaluator snapshot must build");
    let seeded = SessionAggregateV1::start(
        SessionId::new(SESSION_ID).expect("fixture session ID must be valid"),
        "Causal drift fixture",
        snapshot,
        AttemptId::new(ATTEMPT_ID).expect("fixture attempt ID must be valid"),
        UnixMillis::new(2),
    )
    .expect("stateful evaluator must seed a running session");
    let advanced = apply_transition_v1(
        Some(&seeded),
        &SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: seeded
                .active_attempt_id()
                .expect("seeded session must expose an active attempt")
                .clone(),
            next_attempt_id: Some(
                AttemptId::new("123e4567-e89b-42d3-a456-426614174099")
                    .expect("advanced attempt ID must be valid"),
            ),
            local_artifact_verifications: Vec::new(),
        }),
        CommandContextV1 {
            expected_revision: seeded.revision(),
            now: UnixMillis::new(3),
        },
    )
    .expect("stateful evaluator must advance the active stage")
    .next_aggregate()
    .expect("advancing a non-final stage must retain an aggregate")
    .clone();
    (seeded, advanced)
}

fn status_result_for_authoritative_cursor(aggregate: &SessionAggregateV1) -> Map<String, Value> {
    let mut result = status_result();
    let active_stage_id = aggregate
        .active_stage_id()
        .expect("running aggregate must expose an active stage");
    let active_attempt_id = aggregate
        .active_attempt_id()
        .expect("running aggregate must expose an active attempt");
    let stage_index = aggregate
        .snapshot()
        .stages()
        .iter()
        .position(|stage| stage.id() == active_stage_id)
        .expect("active stage must belong to the snapshot");
    let attempt = aggregate
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == active_attempt_id)
        .expect("active attempt must be durable");

    result["session"]
        .as_object_mut()
        .expect("fixture status must have a session")
        .insert(
            "revision".to_owned(),
            Value::from(aggregate.revision().get()),
        );
    let current = result["current"]
        .as_object_mut()
        .expect("fixture status must have a current cursor");
    current.insert(
        "stage_id".to_owned(),
        Value::String(active_stage_id.as_str().to_owned()),
    );
    current.insert("stage_index".to_owned(), Value::from(stage_index));
    current.insert(
        "title".to_owned(),
        Value::String(
            aggregate.snapshot().stages()[stage_index]
                .title()
                .to_owned(),
        ),
    );
    current.insert(
        "attempt_id".to_owned(),
        Value::String(active_attempt_id.as_str().to_owned()),
    );
    current.insert("attempt_number".to_owned(), Value::from(attempt.number()));
    result.insert(
        "stages".to_owned(),
        Value::Array(
            aggregate
                .snapshot()
                .stages()
                .iter()
                .enumerate()
                .map(|(index, stage)| {
                    json!({
                        "id": stage.id().as_str(),
                        "index": index,
                        "title": stage.title(),
                        "status": if stage.id() == active_stage_id { "current" } else { "done" },
                        "latest_attempt_number": aggregate.stage_progress()[index].latest_attempt_number(),
                    })
                })
                .collect(),
        ),
    );
    result
}

struct StatefulCursorEvaluator {
    aggregate_before: SessionAggregateV1,
    durable: SessionAggregateV1,
    revision_before: Revision,
}

impl StatefulCursorEvaluator {
    fn new(durable: SessionAggregateV1) -> Self {
        let revision_before = durable.revision();
        Self {
            aggregate_before: durable.clone(),
            durable,
            revision_before,
        }
    }

    fn reject_stale_complete(&self, stale_attempt_id: AttemptId) {
        let outcome = apply_transition_v1(
            Some(&self.durable),
            &SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: stale_attempt_id,
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            CommandContextV1 {
                expected_revision: self.durable.revision(),
                now: UnixMillis::new(4),
            },
        );
        assert!(
            matches!(outcome, Err(DomainError::AttemptNotCurrent { .. })),
            "the production transition evaluator must reject the stale active attempt: {outcome:?}"
        );
    }
}

struct StatefulCursorDaemon {
    handle: JoinHandle<io::Result<StatefulCursorEvaluator>>,
}

impl StatefulCursorDaemon {
    fn start(fixture: &Fixture, seeded: SessionAggregateV1, advanced: SessionAggregateV1) -> Self {
        let listener = UnixListener::bind(&fixture.socket_path)
            .expect("stateful evaluator must bind the service-owned socket path");
        fs::set_permissions(&fixture.socket_path, fs::Permissions::from_mode(0o600))
            .expect("stateful evaluator socket must be private");
        let handle = thread::spawn(move || {
            let evaluator = StatefulCursorEvaluator::new(advanced);
            for request_index in 0..2 {
                let (mut connection, _) = listener.accept()?;
                let mut wire = Vec::new();
                connection.read_to_end(&mut wire)?;
                let request = decode_request(&wire);
                let response = match request_index {
                    0 => {
                        assert_eq!(request.command().as_str(), "session.status");
                        output_response(
                            &request,
                            status_result_for_authoritative_cursor(&evaluator.durable),
                            None,
                        )?
                    }
                    1 => {
                        assert_eq!(request.command().as_str(), "session.complete");
                        let stale_attempt_id = request
                            .preconditions()
                            .attempt_id()
                            .expect("stale CLI mutation must carry its captured attempt")
                            .clone();
                        assert_eq!(
                            stale_attempt_id,
                            *seeded
                                .active_attempt_id()
                                .expect("seeded session must expose its captured attempt")
                        );
                        evaluator.reject_stale_complete(stale_attempt_id);
                        error_response_with(
                            &request,
                            "ATTEMPT_NOT_CURRENT",
                            "The target attempt is no longer active.",
                            4,
                            true,
                            true,
                        )?
                    }
                    _ => unreachable!(),
                };
                let payload = encode_response_payload_v1(&response)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                connection.write_all(
                    &encode_frame_v1(&payload)
                        .map_err(|error| io::Error::other(error.to_string()))?,
                )?;
                connection.shutdown(Shutdown::Write)?;
            }
            Ok(evaluator)
        });
        Self { handle }
    }

    fn finish(self) -> StatefulCursorEvaluator {
        self.handle
            .join()
            .expect("stateful evaluator thread must not panic")
            .expect("stateful evaluator I/O must succeed")
    }
}

fn decode_request(wire: &[u8]) -> RequestEnvelopeV1 {
    let payload = decode_single_frame_v1(wire).expect("wire must contain one frame");
    decode_request_payload_v1(payload).expect("wire must contain a valid request")
}

#[test]
fn init_uses_bootstrap_without_a_task_or_expected_uuid() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "init",
        "--idempotency-key",
        "init-replay",
        "--detach",
    ]);
    assert!(output.status.success(), "init failed: {output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(response["command"], "workspace.init");
    assert_eq!(response["job"]["id"], JOB_ID);

    let wires = daemon.finish();
    assert_eq!(wires.len(), 1, "init must not issue a status preflight");
    let request = decode_request(&wires[0]);
    let slice = SliceRequestV1::from_envelope(&request).expect("init request must be in the slice");
    assert_eq!(request.operation(), OperationV1::Bootstrap);
    assert_eq!(
        request.idempotency_key().map(|key| key.as_str()),
        Some("init-replay")
    );
    assert!(
        request
            .workspace()
            .and_then(|workspace| workspace.expected_uuid())
            .is_none()
    );
    assert!(slice.selector().expected_uuid().is_none());
    assert!(matches!(slice.command(), SliceCommandV1::WorkspaceInit(_)));
    assert_eq!(
        request.payload().len(),
        1,
        "init payload must contain only its selector"
    );
}
#[test]
fn init_repair_uses_the_durable_bootstrap_authority() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "init",
        "--repair",
        "--idempotency-key",
        "repair-replay",
        "--detach",
    ]);
    assert!(output.status.success(), "init repair failed: {output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(response["command"], "workspace.init");
    assert_eq!(response["job"]["id"], JOB_ID);

    let wires = daemon.finish();
    assert_eq!(wires.len(), 1, "init repair must not status-preflight");
    let request = decode_request(&wires[0]);
    let slice = SliceRequestV1::from_envelope(&request)
        .expect("init repair must remain admitted by the durable bootstrap slice");
    assert!(matches!(
        slice.command(),
        SliceCommandV1::WorkspaceInit(init) if init.repair
    ));
    assert_eq!(request.command().as_str(), "workspace.init");
    assert_eq!(request.operation(), OperationV1::Bootstrap);
    assert_eq!(
        request.idempotency_key().map(|key| key.as_str()),
        Some("repair-replay")
    );
    assert!(request.options().detach());
    assert_eq!(
        request.payload().get("repair"),
        Some(&Value::Bool(true)),
        "init --repair must transmit a typed repair marker omitted by ordinary init",
    );
    assert_eq!(
        request.payload().len(),
        2,
        "init --repair must differ from ordinary init's selector-only payload",
    );
}

#[test]
fn pac_053_item_mutation_preflights_status_and_replays_exact_wire() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--timeout",
        "1s",
        "set",
        "goal",
        "bounded value",
        "--idempotency-key",
        "stable-replay-key",
        "--detach",
    ]);
    assert!(output.status.success(), "set failed: {output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
    assert_eq!(
        stdout.lines().count(),
        1,
        "JSON mode must emit one envelope"
    );
    let response: Value = serde_json::from_str(&stdout).expect("stdout must contain JSON");
    assert_eq!(response["command"], "item.set");
    assert_eq!(response["job"]["id"], JOB_ID);

    let wires = daemon.finish();
    assert_eq!(wires.len(), 2);
    let preflight = decode_request(&wires[0]);
    assert_eq!(preflight.command().as_str(), "session.status");
    assert_eq!(preflight.operation(), OperationV1::Query);

    let mutation = decode_request(&wires[1]);
    assert_eq!(mutation.command().as_str(), "item.set");
    assert_eq!(
        mutation.idempotency_key().map(|key| key.as_str()),
        Some("stable-replay-key")
    );
    assert!(mutation.options().detach());
    assert_eq!(mutation.options().wait_timeout_ms(), 1_000);
    assert_eq!(
        mutation.preconditions().attempt_id().map(AttemptId::as_str),
        Some(ATTEMPT_ID)
    );
    assert_eq!(
        mutation.preconditions().item_revision().map(Revision::get),
        Some(3)
    );
    assert!(mutation.preconditions().session_revision().is_none());
    assert_eq!(
        mutation
            .workspace()
            .and_then(|workspace| workspace.expected_uuid())
            .map(WorkspaceId::as_str),
        Some(WORKSPACE_ID)
    );

    let slice = SliceRequestV1::from_envelope(&mutation).expect("mutation must remain admitted");
    assert_eq!(
        slice.selector().expected_uuid().map(WorkspaceId::as_str),
        Some(WORKSPACE_ID)
    );
    assert!(matches!(slice.command(), SliceCommandV1::ItemSet(_)));
    let payload = encode_request_payload_v1(&mutation).expect("request must encode");
    let replay_wire = encode_frame_v1(&payload).expect("request must frame");
    assert_eq!(
        wires[1], replay_wire,
        "client must send one exact protocol frame"
    );
}

#[test]
fn pac_022_cursor_mutation_is_rejected_after_authoritative_active_stage_drift() {
    const DRIFTED_ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174099";

    let fixture = Fixture::new();
    let (seeded, advanced) = seeded_session_with_advanced_cursor();
    let captured_attempt = seeded
        .active_attempt_id()
        .expect("seeded session must expose a cursor")
        .clone();
    assert_eq!(captured_attempt.as_str(), ATTEMPT_ID);
    assert_eq!(
        advanced
            .active_attempt_id()
            .expect("advanced session must expose a cursor")
            .as_str(),
        DRIFTED_ATTEMPT_ID
    );
    assert_ne!(seeded.active_stage_id(), advanced.active_stage_id());
    let daemon = StatefulCursorDaemon::start(&fixture, seeded, advanced);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--if-attempt",
        ATTEMPT_ID,
        "complete",
    ]);

    assert_eq!(output.status.code(), Some(4));
    let error: Value =
        serde_json::from_slice(&output.stdout).expect("rejected cursor mutation must be JSON");
    assert_eq!(
        error,
        json!({
            "schema": "podway.error/v1",
            "request_id": error["request_id"],
            "command": "session.complete",
            "generated_at": "2026-07-15T12:34:56.789Z",
            "code": "ATTEMPT_NOT_CURRENT",
            "message": "The target attempt is no longer active.",
            "retryable": true,
            "exit_code": 4,
            "workspace": { "uuid": WORKSPACE_ID, "root": "/fixture" },
            "details": {
                "schema": "podway.attempt-conflict-details/v1",
                "expected_attempt_id": ATTEMPT_ID,
                "job_id": JOB_ID,
                "job_sequence": 7,
                "admission": {
                    "admitted": true,
                    "job_id": JOB_ID,
                    "workspace_sequence": 7
                }
            }
        }),
        "the stale cursor must retain the daemon's exact typed rejection",
    );

    let evaluator = daemon.finish();
    assert_eq!(
        evaluator.durable.revision(),
        evaluator.revision_before,
        "a stale mutation must not change the durable aggregate revision"
    );
    assert_eq!(
        evaluator.durable, evaluator.aggregate_before,
        "a stale mutation must leave the durable aggregate unchanged"
    );
    assert_eq!(
        evaluator.durable.active_attempt_id().map(AttemptId::as_str),
        Some(DRIFTED_ATTEMPT_ID),
        "a stale mutation must not replace the authoritative active cursor"
    );
}

#[test]
fn pac_053_explicit_revision_attempt_item_revision_and_idempotency_reach_exact_wire_fields() {
    const EXPLICIT_ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174098";

    let cursor_fixture = Fixture::new();
    let cursor_daemon = FakeDaemon::start(&cursor_fixture, vec![Reply::Status, Reply::Output]);
    let cursor_output = cursor_fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--idempotency-key",
        "cursor-explicit-key",
        "--if-session-revision",
        "41",
        "--if-attempt",
        EXPLICIT_ATTEMPT_ID,
        "complete",
    ]);
    assert!(
        cursor_output.status.success(),
        "explicit cursor mutation failed"
    );
    let cursor_wires = cursor_daemon.finish();
    let cursor_mutation = decode_request(&cursor_wires[1]);
    assert_eq!(
        cursor_mutation
            .preconditions()
            .session_revision()
            .map(Revision::get),
        Some(41)
    );
    assert_eq!(
        cursor_mutation
            .preconditions()
            .attempt_id()
            .map(AttemptId::as_str),
        Some(EXPLICIT_ATTEMPT_ID)
    );
    assert_eq!(
        cursor_mutation.idempotency_key().map(|key| key.as_str()),
        Some("cursor-explicit-key")
    );

    let item_fixture = Fixture::new();
    let item_daemon = FakeDaemon::start(&item_fixture, vec![Reply::Status, Reply::Output]);
    let item_output = item_fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--idempotency-key",
        "item-explicit-key",
        "--if-attempt",
        EXPLICIT_ATTEMPT_ID,
        "--if-item-revision",
        "73",
        "set",
        "goal",
        "explicit value",
    ]);
    assert!(
        item_output.status.success(),
        "explicit item mutation failed"
    );
    let item_wires = item_daemon.finish();
    let item_mutation = decode_request(&item_wires[1]);
    assert_eq!(
        item_mutation
            .preconditions()
            .attempt_id()
            .map(AttemptId::as_str),
        Some(EXPLICIT_ATTEMPT_ID)
    );
    assert_eq!(
        item_mutation
            .preconditions()
            .item_revision()
            .map(Revision::get),
        Some(73)
    );
    assert!(
        item_mutation.preconditions().session_revision().is_none(),
        "item mutations must not infer a session revision",
    );
    assert_eq!(
        item_mutation.idempotency_key().map(|key| key.as_str()),
        Some("item-explicit-key")
    );
}
#[test]
fn mutation_preflight_error_is_recorrelated_and_marked_not_admitted() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Error]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "set",
        "goal",
        "bounded value",
    ]);
    assert_eq!(output.status.code(), Some(4));
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("preflight error stdout must be JSON");
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["command"], "item.set");
    assert_eq!(response["code"], "JOB_WAIT_TIMEOUT");
    assert_eq!(response["retryable"], true);
    assert_eq!(response["exit_code"], 4);
    assert_eq!(response["workspace"]["uuid"], WORKSPACE_ID);
    assert_eq!(response["workspace"]["root"], "/fixture");
    assert_eq!(
        response["details"],
        json!({
            "schema": "podway.job-wait-timeout-details/v1",
            "admission": {"admitted": false}
        })
    );

    let wires = daemon.finish();
    assert_eq!(
        wires.len(),
        1,
        "a failed preflight must not send a mutation"
    );
    let preflight = decode_request(&wires[0]);
    assert_eq!(preflight.command().as_str(), "session.status");
    assert_eq!(
        response["request_id"],
        preflight.request_id().as_str(),
        "re-correlated errors retain the response request identity"
    );
}

#[test]
fn mutation_local_validation_error_is_marked_not_admitted_without_transport() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, Vec::new());

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--idempotency-key",
        "",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Reject an invalid mutation key",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("local mutation failure must be JSON");
    assert_eq!(response["command"], "session.start");
    assert_eq!(response["code"], "REQUEST_INVALID");
    assert_eq!(
        response["details"],
        json!({"admission": {"admitted": false}})
    );
    assert!(
        daemon.finish().is_empty(),
        "local mutation validation must not contact the daemon"
    );
}

#[test]
fn mutation_sync_and_detach_preferences_are_transport_fields() {
    for detach in [false, true] {
        let fixture = Fixture::new();
        let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);
        let mut arguments = vec!["--worktree", "/fixture", "complete"];
        if detach {
            arguments.extend(["--idempotency-key", "complete-replay", "--detach"]);
        }
        let output = fixture.run(&arguments);
        assert!(output.status.success(), "complete failed: {output:?}");
        let wires = daemon.finish();
        let mutation = decode_request(&wires[1]);
        assert_eq!(mutation.options().detach(), detach);
        let key = mutation
            .idempotency_key()
            .expect("mutation must include an idempotency key")
            .as_str();
        if detach {
            assert_eq!(key, "complete-replay");
        } else {
            assert_eq!(
                Uuid::parse_str(key)
                    .expect("generated idempotency key must be a UUID")
                    .get_version_num(),
                4
            );
        }
        assert_eq!(
            mutation
                .preconditions()
                .session_revision()
                .map(Revision::get),
            Some(12)
        );
        assert_eq!(
            mutation.preconditions().attempt_id().map(AttemptId::as_str),
            Some(ATTEMPT_ID)
        );
    }
}

#[test]
fn daemon_and_parser_errors_preserve_stable_exit_and_json_contracts() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Error]);
    let output = fixture.run(&["--json", "--worktree", "/fixture", "status"]);
    assert_eq!(output.status.code(), Some(4));
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("error stdout must be JSON");
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["code"], "JOB_WAIT_TIMEOUT");
    assert_eq!(response["details"]["job_id"], JOB_ID);
    let wires = daemon.finish();
    assert_eq!(wires.len(), 1);

    let malformed = fixture.run(&["--json", "start", "--preset", "sw-dev"]);
    assert_eq!(malformed.status.code(), Some(2));
    let malformed_json: Value =
        serde_json::from_slice(&malformed.stdout).expect("parser error stdout must be JSON");
    assert_eq!(malformed_json["schema"], "podway.error/v1");
    assert_eq!(malformed_json["exit_code"], 2);
    assert_eq!(malformed_json["command"], "session.start");
    assert_eq!(
        malformed_json["details"],
        json!({"admission": {"admitted": false}})
    );
    assert_eq!(
        String::from_utf8(malformed.stdout)
            .expect("stdout must be UTF-8")
            .lines()
            .count(),
        1
    );
    let invalid_applicability = fixture.run(&["--json", "next", "--verbose"]);
    assert_eq!(invalid_applicability.status.code(), Some(2));
    let invalid_applicability_json: Value = serde_json::from_slice(&invalid_applicability.stdout)
        .expect("validation error stdout must be JSON");
    assert_eq!(invalid_applicability_json["schema"], "podway.error/v1");
    assert_eq!(invalid_applicability_json["command"], "session.next");
    assert_eq!(invalid_applicability_json["code"], "REQUEST_INVALID");
}

#[test]
fn guarded_read_identity_conflict_preserves_request_and_closed_public_details() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::IdentityError]);
    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--if-session-id",
        SESSION_ID,
        "status",
    ]);
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stderr.is_empty());
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("identity conflict must be JSON");
    assert_eq!(response["code"], "SESSION_ID_MISMATCH");
    assert_eq!(response["retryable"], false);
    assert_eq!(response["exit_code"], 4);
    assert_eq!(
        response["details"],
        json!({
            "schema": "podway.session-id-mismatch-details/v1",
            "expected_session_id": SESSION_ID,
            "actual_session_id": null,
            "admission": { "admitted": false }
        })
    );
    let wires = daemon.finish();
    assert_eq!(wires.len(), 1);
    let request = decode_request(&wires[0]);
    assert_eq!(request.command().as_str(), "session.status");
    assert_eq!(
        request.preconditions().session_id().map(SessionId::as_str),
        Some(SESSION_ID)
    );
}

#[test]
fn contract_mismatch_response_is_propagated_without_fabricated_status() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::ContractMismatch]);
    let output = fixture.run(&["--json", "--worktree", "/fixture", "status"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.is_empty());
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("contract mismatch must be JSON");
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["command"], "session.status");
    assert_eq!(response["code"], "DAEMON_CONTRACT_MISMATCH");
    assert_eq!(response["exit_code"], 3);
    assert_eq!(response["retryable"], false);
    assert_eq!(response["details"]["admission"]["admitted"], false);
    let wires = daemon.finish();
    assert_eq!(wires.len(), 1);
    assert_eq!(
        response["request_id"],
        decode_request(&wires[0]).request_id().as_str()
    );
}
#[test]
fn malformed_framed_daemon_response_uses_the_stable_client_error_envelope() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::MalformedFramedResponse]);

    let output = fixture.run(&["--json", "--worktree", "/fixture", "status"]);
    assert_eq!(output.status.code(), Some(6));
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("client error stdout must be JSON");
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["command"], "session.status");
    assert_eq!(response["code"], "INTERNAL_ERROR");
    assert_eq!(response["exit_code"], 6);
    assert_eq!(response["retryable"], false);

    let wires = daemon.finish();
    assert_eq!(wires.len(), 1);
    assert_eq!(
        response["request_id"],
        decode_request(&wires[0]).request_id().as_str(),
        "client-envelope failures preserve the request correlation identifier"
    );
}

#[test]
fn mutation_response_loss_reports_unknown_outcome_with_reconciliation_key() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::CloseWithoutResponse]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--idempotency-key",
        "response-loss-key",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Response loss task",
    ]);
    assert_eq!(
        output.status.code(),
        Some(4),
        "unexpected output: {output:?}"
    );
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("unknown outcome must be JSON");
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["command"], "session.start");
    assert_eq!(response["code"], "MUTATION_OUTCOME_UNKNOWN");
    assert_eq!(response["exit_code"], 4);
    assert_eq!(response["retryable"], true);
    assert_eq!(
        response["details"],
        json!({
            "schema": "podway.mutation-outcome-unknown-details/v1",
            "outcome": "unknown",
            "idempotency_key": "response-loss-key",
            "reconcile": {
                "command": "job.lookup",
                "idempotency_key": "response-loss-key",
            },
        })
    );

    let wires = daemon.finish();
    assert_eq!(
        wires.len(),
        1,
        "response loss must not trigger an automatic retry"
    );
    let request = decode_request(&wires[0]);
    assert_eq!(request.command().as_str(), "session.start");
    assert_eq!(
        request.idempotency_key().unwrap().as_str(),
        "response-loss-key"
    );
    assert_eq!(response["request_id"], request.request_id().as_str());
}

#[test]
fn mutation_connect_failure_reports_not_admitted_daemon_unavailable() {
    let fixture = Fixture::new();
    let socket = fixture.socket_path.display().to_string();
    let output = fixture.run(&[
        "--json",
        "--socket",
        &socket,
        "--worktree",
        "/fixture",
        "--idempotency-key",
        "connect-failure-key",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Connect failure task",
    ]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "unexpected output: {output:?}"
    );
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("connect failure must be JSON");
    assert_eq!(response["command"], "session.start");
    assert_eq!(response["code"], "DAEMON_UNAVAILABLE");
    assert_eq!(response["retryable"], true);
    assert_eq!(
        response["details"],
        json!({
            "schema": "podway.endpoint-error-details/v1",
            "admission": {"admitted": false}
        })
    );
}
#[test]
fn malformed_typed_read_results_fail_closed_in_json_and_text() {
    for (argument, command, reply) in [
        ("status", "session.status", Reply::MalformedStatusResult),
        ("next", "session.next", Reply::MalformedNextResult),
    ] {
        let fixture = Fixture::new();
        let daemon = FakeDaemon::start(&fixture, vec![reply]);
        let output = fixture.run(&["--json", "--worktree", "/fixture", argument]);
        assert_eq!(
            output.status.code(),
            Some(6),
            "{command} malformed result must fail",
        );
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("failure stdout must be JSON");
        assert_eq!(response["schema"], "podway.error/v1");
        assert_eq!(response["command"], command);
        assert_eq!(response["code"], "INTERNAL_ERROR");
        assert_eq!(response["exit_code"], 6);

        let wires = daemon.finish();
        assert_eq!(wires.len(), 1);
        assert_eq!(
            response["request_id"],
            decode_request(&wires[0]).request_id().as_str(),
            "{command} malformed result must preserve request correlation",
        );

        let text_fixture = Fixture::new();
        let text_daemon = FakeDaemon::start(&text_fixture, vec![reply]);
        let text_output = text_fixture.run(&["--worktree", "/fixture", argument]);
        assert_eq!(text_output.status.code(), Some(6));
        assert!(
            text_output.stdout.is_empty(),
            "{command} malformed result must not fall back to generic text output: {text_output:?}",
        );
        let stderr = String::from_utf8(text_output.stderr).expect("stderr must be UTF-8");
        assert!(
            stderr.contains("the daemon returned an invalid response"),
            "{command} text failure must report malformed typed output: {stderr}",
        );
        assert_eq!(text_daemon.finish().len(), 1);
    }
}
#[test]
fn malformed_status_preflight_cannot_authorize_mutation_or_reset() {
    for (arguments, command) in [
        (
            &[
                "--json",
                "--worktree",
                "/fixture",
                "set",
                "goal",
                "bounded value",
            ][..],
            "item.set",
        ),
        (
            &[
                "--json",
                "--worktree",
                "/fixture",
                "reset",
                "--all",
                "--force",
                "--yes",
            ][..],
            "workspace.reset_all",
        ),
    ] {
        let fixture = Fixture::new();
        let daemon = FakeDaemon::start(&fixture, vec![Reply::MalformedStatusResult]);

        let output = fixture.run(arguments);
        assert_eq!(
            output.status.code(),
            Some(6),
            "{command} must fail closed on malformed status"
        );
        let response: Value =
            serde_json::from_slice(&output.stdout).expect("failure stdout must be JSON");
        assert_eq!(response["command"], command);
        assert_eq!(response["code"], "INTERNAL_ERROR");

        let wires = daemon.finish();
        assert_eq!(
            wires.len(),
            1,
            "{command} must not issue a request after malformed status"
        );
        assert_eq!(
            response["request_id"],
            decode_request(&wires[0]).request_id().as_str(),
            "{command} must correlate malformed preflight failure to its status request"
        );
    }
}

#[test]
fn set_stdin_is_bounded_before_preflight_at_the_authoritative_value_limit() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);
    let exact_value = "x".repeat(MAX_SLICE_ITEM_TEXT_SCALARS_V1);

    let output = fixture.run_with_stdin(
        &["--json", "--worktree", "/fixture", "set", "goal", "--stdin"],
        exact_value.as_bytes(),
    );
    assert!(
        output.status.success(),
        "exact stdin limit failed: {output:?}"
    );

    let wires = daemon.finish();
    assert_eq!(
        wires.len(),
        2,
        "exact stdin limit must preflight then mutate"
    );
    let mutation = decode_request(&wires[1]);
    let slice = SliceRequestV1::from_envelope(&mutation)
        .expect("exact stdin limit must remain admitted by the typed item-set slice");
    assert!(matches!(
        slice.command(),
        SliceCommandV1::ItemSet(item) if item.value.chars().count() == MAX_SLICE_ITEM_TEXT_SCALARS_V1
    ));
    assert_eq!(
        mutation.payload()["value"].as_str().map(str::len),
        Some(MAX_SLICE_ITEM_TEXT_SCALARS_V1)
    );

    let overflow_fixture = Fixture::new();
    let overflow = "x".repeat(MAX_SLICE_ITEM_TEXT_SCALARS_V1 + 1);
    let overflow_output = overflow_fixture.run_with_stdin(
        &["--json", "--worktree", "/fixture", "set", "goal", "--stdin"],
        overflow.as_bytes(),
    );
    assert_eq!(
        overflow_output.status.code(),
        Some(2),
        "stdin overflow must fail before daemon access"
    );
    let error: Value =
        serde_json::from_slice(&overflow_output.stdout).expect("overflow stdout must be JSON");
    assert_eq!(error["command"], "item.set");
    assert_eq!(error["code"], "REQUEST_TOO_LARGE");
}

#[test]
fn text_success_uses_stdout_and_daemon_errors_use_stderr() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status]);

    let output = fixture.run(&["--worktree", "/fixture", "status"]);
    assert!(output.status.success(), "text status failed: {output:?}");
    assert!(
        String::from_utf8(output.stdout)
            .expect("status stdout must be UTF-8")
            .contains("task: Fixture task")
    );
    assert!(output.stderr.is_empty(), "text success must not use stderr");
    assert_eq!(daemon.finish().len(), 1);

    let error_fixture = Fixture::new();
    let error_daemon = FakeDaemon::start(&error_fixture, vec![Reply::Error]);
    let error_output = error_fixture.run(&["--worktree", "/fixture", "status"]);
    assert_eq!(error_output.status.code(), Some(4));
    assert!(
        error_output.stdout.is_empty(),
        "daemon text errors must not use stdout"
    );
    assert!(
        String::from_utf8(error_output.stderr)
            .expect("error stderr must be UTF-8")
            .contains("error: JOB_WAIT_TIMEOUT:")
    );
    assert_eq!(error_daemon.finish().len(), 1);
}

#[test]
fn pstrt004_start_rendering_preserves_the_admitted_procedure_digest() {
    const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let json_fixture = Fixture::new();
    let json_daemon = FakeDaemon::start(&json_fixture, vec![Reply::StartOutput]);
    let json_output = json_fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Digest rendering",
        "--detach",
    ]);
    assert!(
        json_output.status.success(),
        "JSON start failed: {json_output:?}"
    );
    let rendered: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(rendered["result"]["procedure_digest"], DIGEST);
    assert_eq!(json_daemon.finish().len(), 1);

    let text_fixture = Fixture::new();
    let text_daemon = FakeDaemon::start(&text_fixture, vec![Reply::StartOutput]);
    let text_output = text_fixture.run(&[
        "--worktree",
        "/fixture",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Digest rendering",
        "--detach",
    ]);
    assert!(
        text_output.status.success(),
        "text start failed: {text_output:?}"
    );
    assert!(
        String::from_utf8(text_output.stdout)
            .unwrap()
            .contains(DIGEST),
        "text start must render the admitted Procedure digest"
    );
    assert_eq!(text_daemon.finish().len(), 1);
}

#[test]
fn resolved_routes_attribute_post_parse_failures() {
    fn assert_route(output: Output, command: &str) {
        assert_eq!(
            output.status.code(),
            Some(2),
            "{command} must be a usage failure"
        );
        let error: Value =
            serde_json::from_slice(&output.stdout).expect("failure stdout must be JSON");
        assert_eq!(error["command"], command);
        assert_eq!(error["code"], "REQUEST_INVALID");
    }

    let fixture = Fixture::new();
    assert_route(
        fixture.run(&["--json", "--worktree", "/fixture", "help"]),
        "help",
    );
    assert_route(
        fixture.run(&["--json", "--worktree", "/dev/null/unresolvable", "status"]),
        "session.status",
    );
    assert_route(
        fixture.run(&[
            "--json",
            "--worktree",
            "/fixture",
            "attach",
            "artifact",
            "--reference",
            "build:42",
            "--digest",
            "not-a-digest",
            "--size",
            "42",
            "--media-type",
            "text/plain",
        ]),
        "item.attach",
    );

    let key = "x".repeat(257);
    assert_route(
        fixture.run(&[
            "--json",
            "--worktree",
            "/fixture",
            "--idempotency-key",
            &key,
            "start",
            "--preset",
            "sw-dev",
            "--task",
            "too long replay key",
        ]),
        "session.start",
    );

    let long_worktree = format!("/{}", "x".repeat(MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1));
    assert_route(
        fixture.run(&["--json", "--worktree", &long_worktree, "status"]),
        "session.status",
    );
}
#[test]
fn session_start_uses_the_canonical_wire_name_and_source_payload() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Canonical task",
    ]);
    assert!(output.status.success(), "start failed: {output:?}");

    let wires = daemon.finish();
    assert_eq!(
        wires.len(),
        1,
        "start must not preflight a nonexistent session"
    );
    let request = decode_request(&wires[0]);
    assert_eq!(request.command().as_str(), "session.start");
    assert_eq!(request.operation(), OperationV1::Mutate);
    assert_eq!(request.payload()["preset"], "sw-dev");
    assert_eq!(request.payload()["task_title"], "Canonical task");
    assert!(request.idempotency_key().is_some());
    assert!(request.preconditions().session_revision().is_none());
    assert!(request.preconditions().attempt_id().is_none());
}

#[test]
fn pstrt001_session_start_forwards_the_expected_procedure_digest() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Output]);
    let digest = format!("sha256:{}", "a".repeat(64));

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "start",
        "--procedure",
        "procedures/custom.yaml",
        "--expect-procedure-digest",
        &digest,
        "--task",
        "Guarded task",
    ]);
    assert!(output.status.success(), "guarded start failed: {output:?}");
    let wires = daemon.finish();
    let request = decode_request(&wires[0]);
    assert_eq!(request.payload()["expected_procedure_digest"], digest);

    let invalid = fixture.run(&[
        "--json",
        "start",
        "--preset",
        "sw-dev",
        "--expect-procedure-digest",
        &format!("sha256:{}", "b".repeat(64)),
        "--task",
        "Invalid preset guard",
    ]);
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn reference_attachment_uses_size_bytes_and_item_preconditions() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);
    let digest = format!("sha256:{}", "a".repeat(64));

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "attach",
        "artifact",
        "--reference",
        "build:42",
        "--digest",
        &digest,
        "--size",
        "42",
        "--media-type",
        "text/plain",
    ]);
    assert!(
        output.status.success(),
        "reference attach failed: {output:?}"
    );

    let wires = daemon.finish();
    assert_eq!(wires.len(), 2);
    let request = decode_request(&wires[1]);
    assert_eq!(request.command().as_str(), "item.attach");
    assert_eq!(request.payload()["item_id"], "artifact");
    assert_eq!(request.payload()["reference"], "build:42");
    assert_eq!(request.payload()["digest"], digest);
    assert_eq!(request.payload()["size_bytes"], 42);
    assert_eq!(request.payload()["media_type"], "text/plain");
    assert_eq!(
        request.preconditions().attempt_id().map(AttemptId::as_str),
        Some(ATTEMPT_ID)
    );
    assert_eq!(
        request.preconditions().item_revision().map(Revision::get),
        Some(4)
    );
}

#[test]
fn start_replace_supplies_confirmation_and_identity_preconditions() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Replacement",
        "--replace",
        "--yes",
    ]);
    assert!(output.status.success(), "start replace failed: {output:?}");

    let wires = daemon.finish();
    assert_eq!(wires.len(), 2);
    let request = decode_request(&wires[1]);
    assert_eq!(request.command().as_str(), "session.start_replace");
    assert_eq!(request.payload()["confirmed"], true);
    assert_eq!(
        request.preconditions().session_id().map(|id| id.as_str()),
        Some(SESSION_ID)
    );
    assert_eq!(
        request
            .preconditions()
            .session_revision()
            .map(Revision::get),
        Some(12)
    );
    assert!(request.preconditions().attempt_id().is_none());
}

#[test]
fn pstrt_start_replace_with_complete_explicit_identity_skips_preflight_for_exact_replay() {
    const PROCEDURE_DIGEST: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::StartOutput]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--if-workspace-uuid",
        WORKSPACE_ID,
        "--if-session-id",
        SESSION_ID,
        "--if-session-revision",
        "12",
        "--idempotency-key",
        "pstrt-start-replace-exact-replay",
        "start",
        "--procedure",
        "procedure.yaml",
        "--expect-procedure-digest",
        PROCEDURE_DIGEST,
        "--task",
        "Replacement",
        "--replace",
        "--yes",
    ]);
    assert!(
        output.status.success(),
        "fully fenced start replace failed: {output:?}"
    );

    let wires = daemon.finish();
    assert_eq!(
        wires.len(),
        1,
        "a fully fenced replacement must not issue a status preflight"
    );
    let request = decode_request(&wires[0]);
    assert_eq!(request.command().as_str(), "session.start_replace");
    assert_eq!(request.operation(), OperationV1::Mutate);
    assert_eq!(
        request.preconditions().session_id().map(SessionId::as_str),
        Some(SESSION_ID)
    );
    assert_eq!(
        request
            .preconditions()
            .session_revision()
            .map(Revision::get),
        Some(12)
    );
    assert_eq!(
        request.payload()["expected_procedure_digest"],
        PROCEDURE_DIGEST
    );
}

#[test]
fn start_replace_dry_run_uses_the_readonly_preflighted_preview_contract() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--if-session-revision",
        "12",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "Replacement preview",
        "--replace",
        "--dry-run",
    ]);
    assert!(
        output.status.success(),
        "start replace dry run failed: {output:?}"
    );

    let wires = daemon.finish();
    assert_eq!(
        wires.len(),
        2,
        "replace preview must preflight current state"
    );
    let preview = decode_request(&wires[1]);
    assert_eq!(preview.command().as_str(), "session.start_replace");
    assert_eq!(preview.operation(), OperationV1::Query);
    assert_eq!(preview.payload()["dry_run"], true);
    assert!(preview.payload().get("confirmed").is_none());
    assert!(preview.idempotency_key().is_none());
    assert_eq!(
        preview.preconditions().session_id().map(|id| id.as_str()),
        Some(SESSION_ID)
    );
    assert_eq!(
        preview
            .preconditions()
            .session_revision()
            .map(Revision::get),
        Some(12)
    );
}

#[test]
fn reset_all_binds_readable_workspace_identity_without_session_preconditions() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "reset",
        "--all",
        "--force",
        "--yes",
    ]);
    assert!(output.status.success(), "reset all failed: {output:?}");

    let wires = daemon.finish();
    assert_eq!(wires.len(), 2, "reset all must probe readable state");
    let reset = decode_request(&wires[1]);
    assert_eq!(reset.command().as_str(), "workspace.reset_all");
    assert_eq!(reset.operation(), OperationV1::Bootstrap);
    assert_eq!(reset.payload()["expected_workspace_uuid"], WORKSPACE_ID);
    assert_eq!(
        reset
            .workspace()
            .and_then(|workspace| workspace.expected_uuid())
            .map(WorkspaceId::as_str),
        Some(WORKSPACE_ID)
    );
    assert!(reset.preconditions().session_id().is_none());
    assert!(reset.preconditions().session_revision().is_none());
}

#[test]
fn reset_all_prefers_explicit_workspace_identity_for_probe_and_mutation() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "--if-workspace-uuid",
        EXPLICIT_WORKSPACE_ID,
        "reset",
        "--all",
        "--force",
        "--yes",
    ]);
    assert!(
        output.status.success(),
        "explicitly guarded reset all failed: {output:?}"
    );

    let wires = daemon.finish();
    assert_eq!(wires.len(), 2, "reset all must issue one guarded probe");
    let probe = decode_request(&wires[0]);
    assert_eq!(probe.command().as_str(), "session.status");
    assert_eq!(
        probe
            .workspace()
            .and_then(|workspace| workspace.expected_uuid())
            .map(WorkspaceId::as_str),
        Some(EXPLICIT_WORKSPACE_ID)
    );

    let reset = decode_request(&wires[1]);
    assert_eq!(reset.command().as_str(), "workspace.reset_all");
    assert_eq!(
        reset.payload()["expected_workspace_uuid"],
        EXPLICIT_WORKSPACE_ID
    );
    assert_eq!(
        reset
            .workspace()
            .and_then(|workspace| workspace.expected_uuid())
            .map(WorkspaceId::as_str),
        Some(EXPLICIT_WORKSPACE_ID)
    );
    assert!(reset.preconditions().session_id().is_none());
    assert!(reset.preconditions().session_revision().is_none());
}

#[test]
fn reset_all_continues_only_after_the_documented_unreadable_state_probe_error() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::ResetUnreadable, Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "reset",
        "--all",
        "--force",
        "--yes",
    ]);
    assert!(
        output.status.success(),
        "unreadable-state reset all failed: {output:?}"
    );

    let wires = daemon.finish();
    assert_eq!(wires.len(), 2);
    let reset = decode_request(&wires[1]);
    assert_eq!(reset.command().as_str(), "workspace.reset_all");
    assert!(reset.payload().get("expected_workspace_uuid").is_none());
    assert!(
        reset
            .workspace()
            .and_then(|workspace| workspace.expected_uuid())
            .is_none()
    );
    assert!(reset.preconditions().session_id().is_none());
    assert!(reset.preconditions().session_revision().is_none());
}
#[test]
fn offline_procedure_validation_rejects_oversized_files_before_reading_them() {
    let fixture = Fixture::new();
    let procedure = fixture.root.join("oversized.yaml");
    fs::File::create(&procedure)
        .expect("oversized procedure file must be created")
        .set_len((MAX_PROCEDURE_DOCUMENT_BYTES_V1 + 1) as u64)
        .expect("oversized procedure file length must be set");

    let output = fixture.run(&[
        "--json",
        "procedure",
        "validate",
        procedure.to_str().expect("fixture procedure path is UTF-8"),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(error["schema"], "podway.error/v1");
    assert_eq!(error["command"], "procedure.validate");
    assert_eq!(error["code"], "PROCEDURE_INVALID");
}
#[test]
fn offline_procedure_validation_rejects_symlink_sources() {
    let fixture = Fixture::new();
    let source = fixture.root.join("source.yaml");
    let symlinked = fixture.root.join("source-link.yaml");
    fs::write(&source, "schema: podway.procedure/v1\n").expect("source procedure must be written");
    symlink(&source, &symlinked).expect("source procedure symlink must be created");

    let output = fixture.run(&[
        "--json",
        "procedure",
        "validate",
        symlinked.to_str().expect("fixture procedure path is UTF-8"),
    ]);
    assert_eq!(output.status.code(), Some(5));
    let error: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(error["command"], "procedure.validate");
    assert_eq!(error["code"], "PATH_OUTSIDE_WORKTREE");
}

#[test]
fn session_start_help_documents_a_parseable_dry_run() {
    let fixture = Fixture::new();
    let help = fixture.run(&["--json", "help", "session.start"]);
    assert!(help.status.success(), "session.start help failed: {help:?}");
    let help_json: Value = serde_json::from_slice(&help.stdout).expect("help stdout must be JSON");
    let help_text = help_json["result"]["text"]
        .as_str()
        .expect("session.start help must have text");
    assert!(
        help_text.contains("[--dry-run]") && help_text.contains("--dry-run"),
        "session.start help must document its dry-run parser flag: {help_text}",
    );

    let preview = fixture.run(&[
        "--json",
        "--worktree",
        "/fixture",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "preview",
        "--dry-run",
    ]);
    assert!(
        preview.status.success(),
        "documented session.start dry-run must parse and execute locally: {preview:?}",
    );
    let preview_json: Value =
        serde_json::from_slice(&preview.stdout).expect("preview stdout must be JSON");
    assert_eq!(preview_json["command"], "session.start");
    assert_eq!(preview_json["result"]["dry_run"], true);
}

#[test]
fn local_custom_procedure_dry_run_rejects_worktree_symlinks() {
    let fixture = Fixture::new();
    let worktree = fixture.root.join("worktree");
    fs::create_dir(&worktree).expect("worktree directory must be created");
    let external = fixture.root.join("outside.yaml");
    fs::write(&external, "schema: podway.procedure/v1\n")
        .expect("outside procedure must be written");
    symlink(&external, worktree.join("escape.yaml")).expect("procedure symlink must be created");

    let output = fixture.run(&[
        "--json",
        "--worktree",
        worktree.to_str().expect("fixture worktree path is UTF-8"),
        "start",
        "--procedure",
        "escape.yaml",
        "--task",
        "unsafe preview",
        "--dry-run",
    ]);
    assert_eq!(output.status.code(), Some(5));
    let error: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(error["code"], "PATH_OUTSIDE_WORKTREE");
    assert_eq!(error["command"], "session.start");
}
