use std::{
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixListener,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

use nix::unistd::geteuid;
use podway_core::{AttemptId, JobId, Revision, WorkspaceId};
use podway_protocol::{
    ErrorCodeV1, ErrorEnvelopeInputV1, ExitCodeV1, JobOutputV1, JobStateV1, OperationV1,
    OutputEnvelopeInputV1, RequestEnvelopeV1, ResponseEnvelopeV1, Rfc3339MillisV1, SliceCommandV1,
    SliceRequestV1, WorkspaceOutputV1, decode_request_payload_v1, decode_single_frame_v1,
    encode_frame_v1, encode_request_payload_v1, encode_response_payload_v1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const WORKSPACE_ID: &str = "123e4567-e89b-42d3-a456-426614174001";
const ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174002";
const JOB_ID: &str = "123e4567-e89b-42d3-a456-426614174003";
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    temporary: PathBuf,
    socket_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from("/tmp").join(format!("pwc-{}-{sequence}", std::process::id()));
        let home = root.join("home");
        let temporary = root.join("temporary");
        fs::create_dir_all(&home).expect("fixture home must be created");
        fs::create_dir_all(&temporary).expect("fixture temporary directory must be created");
        let paths = ServiceRuntimePathsV1::for_user(&home, &temporary, geteuid().as_raw())
            .expect("fixture paths must be valid");
        fs::create_dir(paths.runtime_directory().as_path())
            .expect("fixture daemon runtime directory must be created");
        Self {
            root,
            home,
            temporary,
            socket_path: paths.socket_path().as_path().to_path_buf(),
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(arguments)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.temporary)
            .output()
            .expect("podway binary must run")
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
    Error,
    MalformedFramedResponse,
}

struct FakeDaemon {
    handle: JoinHandle<io::Result<Vec<Vec<u8>>>>,
}

impl FakeDaemon {
    fn start(fixture: &Fixture, replies: Vec<Reply>) -> Self {
        let listener = UnixListener::bind(&fixture.socket_path)
            .expect("fake daemon must bind the service-owned socket path");
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
                    Reply::MalformedFramedResponse => {
                        encode_frame_v1(br#"{"schema":"podway.output/v1","invalid":true}"#)
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

impl Reply {
    fn response_for(self, request: &RequestEnvelopeV1) -> io::Result<ResponseEnvelopeV1> {
        match self {
            Self::Status => output_response(request, status_result(), None),
            Self::Output => output_response(
                request,
                Map::from_iter([("admitted".to_owned(), Value::Bool(true))]),
                Some(job()),
            ),
            Self::Error => error_response(request),
            Self::MalformedFramedResponse => Err(io::Error::other(
                "malformed framed response does not have an envelope",
            )),
        }
    }
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
    podway_protocol::ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: request.request_id().clone(),
        command: request.command().clone(),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("JOB_WAIT_TIMEOUT").expect("fixture code must be valid"),
        message: "The wait expired; the admitted job may continue.".to_owned(),
        retryable: true,
        exit_code: ExitCodeV1::new(4).expect("fixture exit code must be valid"),
        workspace: Some(Map::from_iter([
            ("uuid".to_owned(), Value::String(WORKSPACE_ID.to_owned())),
            ("root".to_owned(), Value::String("/fixture".to_owned())),
        ])),
        details: Map::from_iter([
            ("job_id".to_owned(), Value::String(JOB_ID.to_owned())),
            ("job_sequence".to_owned(), Value::from(7_u64)),
        ]),
    })
    .map(ResponseEnvelopeV1::Error)
    .map_err(|error| io::Error::other(error.to_string()))
}

fn status_result() -> Map<String, Value> {
    serde_json::from_value(json!({
        "task": { "title": "Fixture task" },
        "session": { "revision": 12 },
        "current": {
            "title": "Implement",
            "attempt_id": ATTEMPT_ID,
        },
        "items": [
            { "id": "goal", "revision": 3 },
            { "id": "artifact", "revision": 0 }
        ]
    }))
    .expect("fixture status result must be an object")
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
        "--workspace",
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
    assert!(matches!(slice.command(), SliceCommandV1::WorkspaceInit));
    assert_eq!(
        request.payload().len(),
        1,
        "init payload must not start a task"
    );
}

#[test]
fn item_mutation_preflights_status_and_replays_exact_wire() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);

    let output = fixture.run(&[
        "--json",
        "--workspace",
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
fn mutation_preflight_error_is_recorrelated_to_the_invoked_command() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::Error]);

    let output = fixture.run(&[
        "--json",
        "--workspace",
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
    assert_eq!(response["details"]["job_id"], JOB_ID);
    assert_eq!(response["details"]["job_sequence"], 7);

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
fn mutation_sync_and_detach_preferences_are_transport_fields() {
    for detach in [false, true] {
        let fixture = Fixture::new();
        let daemon = FakeDaemon::start(&fixture, vec![Reply::Status, Reply::Output]);
        let mut arguments = vec!["--workspace", "/fixture", "complete"];
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
    let output = fixture.run(&["--json", "--workspace", "/fixture", "status"]);
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
    assert_eq!(
        String::from_utf8(malformed.stdout)
            .expect("stdout must be UTF-8")
            .lines()
            .count(),
        1
    );
}
#[test]
fn malformed_framed_daemon_response_uses_the_stable_client_error_envelope() {
    let fixture = Fixture::new();
    let daemon = FakeDaemon::start(&fixture, vec![Reply::MalformedFramedResponse]);

    let output = fixture.run(&["--json", "--workspace", "/fixture", "status"]);
    assert_eq!(output.status.code(), Some(6));
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("client error stdout must be JSON");
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["command"], "cli");
    assert_eq!(response["code"], "DAEMON_RESPONSE_INVALID");
    assert_eq!(response["exit_code"], 6);
    assert_eq!(response["retryable"], false);

    let wires = daemon.finish();
    assert_eq!(wires.len(), 1);
}
