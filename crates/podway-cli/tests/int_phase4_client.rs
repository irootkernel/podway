use std::{
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use podway_cli::client::{
    DaemonClientErrorV1, DaemonClientIoOperationV1, DaemonClientTimeoutsV1, DaemonClientV1,
};
use podway_protocol::{
    FrameErrorV1, OperationV1, ResponseEnvelopeV1, SliceCommandV1, SliceRequestV1,
    build_identity_v1, decode_request_payload_v1, decode_single_frame_v1, encode_frame_v1,
    encode_request_payload_v1,
};
use podway_service::ServiceRuntimePathsV1;

const REQUEST_ID: &str = "123e4567-e89b-12d3-a456-426614174000";
const WORKSPACE_ID: &str = "123e4567-e89b-12d3-a456-426614174001";
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct RuntimeFixture {
    root: PathBuf,
    runtime_root: PathBuf,
    paths: ServiceRuntimePathsV1,
}

impl RuntimeFixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "podway-cli-phase4-client-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("client fixture root must be created");
        let launch_agents = root.join("launch-agents");
        let application_support = root.join("application-support");
        let logs = root.join("logs");
        let runtime_root = short_runtime_directory(&root);
        for directory in [&launch_agents, &application_support, &logs, &runtime_root] {
            fs::create_dir(directory).expect("client fixture directory must be created");
        }
        fs::set_permissions(&runtime_root, fs::Permissions::from_mode(0o700))
            .expect("client runtime directory must be private");
        let paths = ServiceRuntimePathsV1::from_directories(
            launch_agents,
            application_support,
            logs,
            runtime_root.clone(),
        )
        .expect("client fixture service paths must be valid");
        Self {
            root,
            runtime_root,
            paths,
        }
    }

    fn socket_path(&self) -> &Path {
        self.paths.socket_path().as_path()
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.runtime_root);
    }
}

fn short_runtime_directory(root: &Path) -> PathBuf {
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for byte in root.as_os_str().as_encoded_bytes() {
        digest ^= u64::from(*byte);
        digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
    PathBuf::from(format!("/tmp/pw4r-{digest:016x}"))
}
enum ServerBehavior {
    FragmentedResponse(Vec<u8>),
    DelayedFragmentedResponse { response: Vec<u8>, delay: Duration },
    Response(Vec<u8>),
    Stall(Duration),
}

struct FakeSocketServer {
    request_receiver: mpsc::Receiver<Vec<u8>>,
    handle: JoinHandle<io::Result<()>>,
}

impl FakeSocketServer {
    fn start(fixture: &RuntimeFixture, behavior: ServerBehavior) -> Self {
        let listener = UnixListener::bind(fixture.socket_path())
            .expect("fake daemon socket must bind at the service-owned path");
        fs::set_permissions(fixture.socket_path(), fs::Permissions::from_mode(0o600))
            .expect("fake daemon socket must be private");
        let (request_sender, request_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut connection, _) = listener.accept()?;
            let mut request_wire = Vec::new();
            connection.read_to_end(&mut request_wire)?;
            request_sender
                .send(request_wire)
                .map_err(|_| io::Error::other("test did not receive request wire"))?;
            // Response delivery tolerates client aborts: clients that reject a
            // malformed or trailing-data response may drop the connection while
            // the fake daemon is still writing or shutting down, and the client
            // side of each test asserts the contract that matters.
            match behavior {
                ServerBehavior::FragmentedResponse(response) => {
                    for chunk in response.chunks(3) {
                        if connection.write_all(chunk).is_err() {
                            return Ok(());
                        }
                    }
                    let _ = connection.shutdown(Shutdown::Write);
                    Ok(())
                }
                ServerBehavior::DelayedFragmentedResponse { response, delay } => {
                    for chunk in response.chunks(3) {
                        if connection.write_all(chunk).is_err() {
                            return Ok(());
                        }
                        thread::sleep(delay);
                    }
                    let _ = connection.shutdown(Shutdown::Write);
                    Ok(())
                }
                ServerBehavior::Response(response) => {
                    if connection.write_all(&response).is_err() {
                        return Ok(());
                    }
                    let _ = connection.shutdown(Shutdown::Write);
                    Ok(())
                }
                ServerBehavior::Stall(duration) => {
                    thread::sleep(duration);
                    Ok(())
                }
            }
        });
        Self {
            request_receiver,
            handle,
        }
    }

    fn request_wire(&self) -> Vec<u8> {
        self.request_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("fake daemon must receive the request before responding")
    }

    fn join(self) {
        self.handle
            .join()
            .expect("fake daemon thread must not panic")
            .expect("fake daemon I/O must succeed");
    }
}

fn client(fixture: &RuntimeFixture) -> DaemonClientV1 {
    DaemonClientV1::new(fixture.paths.clone())
}

fn client_with_read_timeout(fixture: &RuntimeFixture, read: Duration) -> DaemonClientV1 {
    let timeouts =
        DaemonClientTimeoutsV1::new(Duration::from_secs(1), read, Duration::from_secs(1))
            .expect("non-zero client timeouts must be accepted");
    DaemonClientV1::with_timeouts(fixture.paths.clone(), timeouts)
}

fn transmitted_source(error: DaemonClientErrorV1) -> DaemonClientErrorV1 {
    assert!(error.request_may_have_been_transmitted());
    match error {
        DaemonClientErrorV1::RequestPossiblyTransmitted { source } => *source,
        other => panic!("expected a possibly-transmitted exchange failure, received {other:?}"),
    }
}

fn request() -> podway_protocol::RequestEnvelopeV1 {
    let identity = build_identity_v1();
    decode_request_payload_v1(
        format!(
            r#"{{"protocol":"podway.ipc/v1","request_id":"{REQUEST_ID}","client":{{"name":"podway","version":"0.1.0","pid":1,"product":"{}","contract_manifest_digest":"{}"}},"operation":"query","command":"session.status","workspace":{{"root":"/fixture/worktree","expected_uuid":"{WORKSPACE_ID}"}},"options":{{"detach":false,"wait_timeout_ms":30000}},"payload":{{"selector":{{"version":1,"path_bytes_base64url":"L2ZpeHR1cmUvd29ya3RyZWU","display":"/fixture/worktree","expected_uuid":"{WORKSPACE_ID}"}}}}}}"#,
            identity.product(),
            identity.contract_manifest_digest(),
        )
        .as_bytes(),
    )
    .expect("fixture request must satisfy the G006 session.status contract")
}

fn output_payload(command: &str) -> Vec<u8> {
    let result = if command == "session.status" {
        serde_json::json!({
            "task": {"title": "Fixture", "procedure": {"id": "fixture", "version": "1", "name": "Fixture", "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}},
            "session": {"id": "123e4567-e89b-42d3-a456-426614174010", "lifecycle": "running", "revision": 1, "created_at": "2026-07-15T12:34:56.789Z", "completed_at": null, "cancelled_at": null},
            "current": null,
            "stages": [],
            "items": [],
            "blockers": [],
            "queue": {"pending_mutations": false, "queued_count": 0, "running_job_id": null, "latest_workspace_sequence": 1}
        })
    } else {
        serde_json::json!({
            "admission": {"admitted": true, "job_id": "123e4567-e89b-42d3-a456-426614174011", "workspace_sequence": 1},
            "detached": true,
            "procedure_digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222"
        })
    };
    let mut envelope = serde_json::json!({
        "schema": "podway.output/v1",
        "request_id": REQUEST_ID,
        "command": command,
        "generated_at": "2026-07-15T12:34:56.789Z",
        "result": result,
        "warnings": []
    });
    if command == "session.start" {
        envelope["job"] = serde_json::json!({
            "id": "123e4567-e89b-42d3-a456-426614174011",
            "sequence": 1,
            "state": "queued",
            "submitted_at": "2026-07-15T12:34:56.789Z",
            "finished_at": null
        });
    }
    serde_json::to_vec(&envelope).expect("fixture output must serialize")
}
fn mutation_request() -> podway_protocol::RequestEnvelopeV1 {
    let identity = build_identity_v1();
    decode_request_payload_v1(
        format!(
            r#"{{"protocol":"podway.ipc/v1","request_id":"{REQUEST_ID}","client":{{"name":"podway","version":"0.1.0","pid":1,"product":"{}","contract_manifest_digest":"{}"}},"operation":"mutate","command":"session.start","workspace":{{"root":"/fixture/worktree","expected_uuid":"{WORKSPACE_ID}"}},"idempotency_key":"start-fixture","options":{{"detach":true,"wait_timeout_ms":1234}},"payload":{{"selector":{{"version":1,"path_bytes_base64url":"L2ZpeHR1cmUvd29ya3RyZWU","display":"/fixture/worktree","expected_uuid":"{WORKSPACE_ID}"}},"preset":"sw-dev","task_title":"A bounded fixture task"}}}}"#,
            identity.product(),
            identity.contract_manifest_digest(),
        )
        .as_bytes(),
    )
    .expect("fixture mutation request must satisfy the G006 session.start contract")
}

fn error_payload() -> Vec<u8> {
    format!(
        r#"{{"schema":"podway.error/v1","request_id":"{REQUEST_ID}","command":"session.status","generated_at":"2026-07-15T12:34:56.789Z","code":"DAEMON_UNAVAILABLE","message":"daemon is restarting","retryable":true,"exit_code":3,"details":{{}}}}"#
    )
    .into_bytes()
}

#[test]
fn fragmented_output_response_round_trips() {
    let fixture = RuntimeFixture::new();
    let frame =
        encode_frame_v1(&output_payload("session.status")).expect("output response must frame");
    let server = FakeSocketServer::start(&fixture, ServerBehavior::FragmentedResponse(frame));

    let response = client(&fixture)
        .request(&request())
        .expect("fragmented output response must decode");
    assert!(matches!(response, ResponseEnvelopeV1::Output(_)));

    let _ = server.request_wire();
    server.join();
}
#[test]
fn delayed_response_fragments_within_the_absolute_deadline_round_trip() {
    let fixture = RuntimeFixture::new();
    let frame =
        encode_frame_v1(&output_payload("session.status")).expect("output response must frame");
    let server = FakeSocketServer::start(
        &fixture,
        ServerBehavior::DelayedFragmentedResponse {
            response: frame,
            delay: Duration::from_millis(2),
        },
    );

    let response = client_with_read_timeout(&fixture, Duration::from_secs(3))
        .request(&request())
        .expect("fragments delivered before the full deadline must decode");
    assert!(matches!(response, ResponseEnvelopeV1::Output(_)));

    let _ = server.request_wire();
    server.join();
}

#[test]
fn delayed_response_fragments_cannot_extend_the_absolute_read_deadline() {
    let fixture = RuntimeFixture::new();
    let frame =
        encode_frame_v1(&output_payload("session.status")).expect("output response must frame");
    let server = FakeSocketServer::start(
        &fixture,
        ServerBehavior::DelayedFragmentedResponse {
            response: frame,
            delay: Duration::from_millis(15),
        },
    );

    let started = Instant::now();
    let result = client_with_read_timeout(&fixture, Duration::from_millis(60)).request(&request());
    let elapsed = started.elapsed();
    assert!(matches!(
        transmitted_source(result.expect_err("delayed response must time out")),
        DaemonClientErrorV1::Timeout {
            operation: DaemonClientIoOperationV1::Read
        }
    ));
    assert!(
        elapsed < Duration::from_millis(500),
        "the whole response must observe one absolute deadline; elapsed={elapsed:?}"
    );

    let _ = server.request_wire();
    server.join();
}

#[test]
fn error_response_round_trips_without_becoming_a_client_error() {
    let fixture = RuntimeFixture::new();
    let frame = encode_frame_v1(&error_payload()).expect("error response must frame");
    let server = FakeSocketServer::start(&fixture, ServerBehavior::Response(frame));

    let response = client(&fixture)
        .request(&request())
        .expect("daemon error envelopes are valid client responses");
    assert!(
        matches!(response, ResponseEnvelopeV1::Error(error) if error.code().as_str() == "DAEMON_UNAVAILABLE")
    );

    let _ = server.request_wire();
    server.join();
}
#[test]
fn malformed_framed_response_is_a_typed_response_decoding_failure() {
    let fixture = RuntimeFixture::new();
    let frame = encode_frame_v1(br#"{"schema":"podway.output/v1","invalid":true}"#)
        .expect("malformed response fixture must still frame");
    let server = FakeSocketServer::start(&fixture, ServerBehavior::Response(frame));

    let result = client(&fixture).request(&request());
    assert!(matches!(
        transmitted_source(result.expect_err("malformed response must fail")),
        DaemonClientErrorV1::ResponseDecoding { .. }
    ));

    let _ = server.request_wire();
    server.join();
}

#[test]
fn absent_daemon_is_a_typed_connection_failure() {
    let fixture = RuntimeFixture::new();

    let error = client(&fixture)
        .request(&request())
        .expect_err("an absent daemon must fail");
    assert!(!error.request_may_have_been_transmitted());
    assert!(matches!(
        error,
        DaemonClientErrorV1::Connection {
            operation: DaemonClientIoOperationV1::Connect,
            ..
        }
    ));
}

#[test]
fn unsafe_socket_parent_type_and_mode_are_rejected_before_request_io() {
    let insecure_parent = RuntimeFixture::new();
    fs::set_permissions(
        insecure_parent.paths.runtime_directory().as_path(),
        fs::Permissions::from_mode(0o755),
    )
    .expect("insecure parent mode fixture must be installed");
    assert!(matches!(
        client(&insecure_parent).request(&request()),
        Err(DaemonClientErrorV1::EndpointSecurity { .. })
    ));

    let regular_file = RuntimeFixture::new();
    fs::write(regular_file.socket_path(), "not a socket")
        .expect("regular endpoint fixture must be created");
    fs::set_permissions(
        regular_file.socket_path(),
        fs::Permissions::from_mode(0o600),
    )
    .expect("regular endpoint fixture mode must be private");
    assert!(matches!(
        client(&regular_file).request(&request()),
        Err(DaemonClientErrorV1::EndpointSecurity { .. })
    ));

    let linked_socket = RuntimeFixture::new();
    let target = linked_socket.root.join("socket-target");
    fs::write(&target, "not a socket").expect("socket symlink target must be created");
    symlink(&target, linked_socket.socket_path()).expect("socket symlink must be created");
    assert!(matches!(
        client(&linked_socket).request(&request()),
        Err(DaemonClientErrorV1::EndpointSecurity { .. })
    ));

    let wrong_mode = RuntimeFixture::new();
    let listener =
        UnixListener::bind(wrong_mode.socket_path()).expect("wrong-mode socket fixture must bind");
    fs::set_permissions(wrong_mode.socket_path(), fs::Permissions::from_mode(0o660))
        .expect("wrong-mode socket fixture must be installed");
    assert!(matches!(
        client(&wrong_mode).request(&request()),
        Err(DaemonClientErrorV1::EndpointSecurity { .. })
    ));
    drop(listener);
}

#[test]
fn response_timeout_is_a_typed_read_timeout() {
    let fixture = RuntimeFixture::new();
    let server =
        FakeSocketServer::start(&fixture, ServerBehavior::Stall(Duration::from_millis(100)));

    let result = client_with_read_timeout(&fixture, Duration::from_millis(20)).request(&request());
    assert!(matches!(
        transmitted_source(result.expect_err("stalled response must time out")),
        DaemonClientErrorV1::Timeout {
            operation: DaemonClientIoOperationV1::Read
        }
    ));

    let _ = server.request_wire();
    server.join();
}

#[test]
fn truncated_response_is_a_typed_framing_failure() {
    let fixture = RuntimeFixture::new();
    let frame =
        encode_frame_v1(&output_payload("session.status")).expect("output response must frame");
    let truncated = frame[..frame.len() - 1].to_vec();
    let server = FakeSocketServer::start(&fixture, ServerBehavior::Response(truncated));

    let result = client(&fixture).request(&request());
    assert!(matches!(
        transmitted_source(result.expect_err("truncated response must fail")),
        DaemonClientErrorV1::Framing {
            source: FrameErrorV1::UnexpectedEof { .. }
        }
    ));

    let _ = server.request_wire();
    server.join();
}

#[test]
fn trailing_response_data_is_a_typed_framing_failure() {
    let fixture = RuntimeFixture::new();
    let mut response =
        encode_frame_v1(&output_payload("session.status")).expect("output response must frame");
    response.push(0);
    let server = FakeSocketServer::start(&fixture, ServerBehavior::Response(response));

    let result = client(&fixture).request(&request());
    assert!(matches!(
        transmitted_source(result.expect_err("trailing response data must fail")),
        DaemonClientErrorV1::Framing {
            source: FrameErrorV1::TrailingData
        }
    ));

    let _ = server.request_wire();
    server.join();
}

#[test]
fn request_is_one_exact_frame_and_preserves_envelope_wait_preferences() {
    let fixture = RuntimeFixture::new();
    let frame =
        encode_frame_v1(&output_payload("session.start")).expect("output response must frame");
    let server = FakeSocketServer::start(&fixture, ServerBehavior::Response(frame));
    let request = mutation_request();
    let expected_payload = encode_request_payload_v1(&request).expect("request must encode");
    let expected_wire = encode_frame_v1(&expected_payload).expect("request must frame");

    client(&fixture)
        .request(&request)
        .expect("fake daemon output must round-trip");
    let request_wire = server.request_wire();
    assert_eq!(request_wire, expected_wire);
    let request_payload = decode_single_frame_v1(&request_wire)
        .expect("the client wire must contain exactly one frame");
    let decoded =
        decode_request_payload_v1(request_payload).expect("request wire must use protocol codec");
    let slice =
        SliceRequestV1::from_envelope(&decoded).expect("request must remain in the G006 slice");
    assert_eq!(decoded.operation(), OperationV1::Mutate);
    assert!(matches!(slice.command(), SliceCommandV1::SessionStart(_)));
    assert!(decoded.options().detach());
    assert_eq!(decoded.options().wait_timeout_ms(), 1_234);
    server.join();
}
