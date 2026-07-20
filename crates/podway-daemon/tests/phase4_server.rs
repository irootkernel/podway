//! Phase 4 Unix-stream server transport contracts.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read, Write},
    net::Shutdown,
    num::NonZeroUsize,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use podway_daemon::{
    observability::{
        ClockErrorV1, ClockV1, LogSinkV1, ObservabilityFinalizationV1, ObservabilityV1,
    },
    peer::{FixedPeerCredentialSourceV1, PeerUidVerificationErrorV1, PeerUidVerifierV1},
    server::{
        BoundedAcceptLoopV1, ConnectionHandlerSpawnerV1, FixedResponseMetadataSourceV1,
        RequestDispatcherV1, ResponseMetadataClockErrorV1, ResponseMetadataClockV1,
        ResponseMetadataErrorV1, ServerAcceptLoopErrorV1, ServerConnectionErrorV1,
        ServerTransportTimeoutsV1, ShutdownAdmissionV1, SystemResponseMetadataSourceV1,
        UnixServerTransportV1,
    },
};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, ErrorCodeV1, ErrorEnvelopeInputV1, ErrorEnvelopeV1, ExitCodeV1,
    FrameIoPhaseV1, MAX_FRAME_PAYLOAD_BYTES_V1, OperationV1, OutputEnvelopeInputV1,
    OutputEnvelopeV1, PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1,
    RequestOptionsV1, ResponseEnvelopeV1, Rfc3339MillisV1, SliceRequestV1, WorkspaceContextV1,
    WorktreeSelectorWireV1, decode_response_payload_v1, encode_frame_v1, encode_request_payload_v1,
    read_single_frame_v1,
};
use serde_json::{Map, Value, json};

const REQUEST_ID: &str = "2037d76d-6ea8-42c2-a11f-883248bb8774";
const FALLBACK_REQUEST_ID: &str = "2037d76d-6ea8-42c2-a11f-883248bb8775";
const GENERATED_AT: &str = "2026-07-15T12:34:56.789Z";
const EXPECTED_UID: u32 = 501;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Default)]
struct CapturingObservabilitySink {
    events: Mutex<Vec<String>>,
}

impl LogSinkV1 for CapturingObservabilitySink {
    fn write_event(&self, event: &str) -> io::Result<()> {
        self.events
            .lock()
            .expect("observability events lock must not be poisoned")
            .push(event.to_owned());
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

struct FixedObservabilityClock;

impl ClockV1 for FixedObservabilityClock {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1> {
        Ok(42)
    }
}

struct SocketFixture {
    root: PathBuf,
    listener: UnixListener,
    socket_path: PathBuf,
}

impl SocketFixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "podway-daemon-server-phase4-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("server fixture root must be created");
        let socket_path = root.join("daemon.sock");
        let listener = UnixListener::bind(&socket_path).expect("server fixture listener must bind");
        Self {
            root,
            listener,
            socket_path,
        }
    }
}

impl Drop for SocketFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy)]
enum DispatcherOutcome {
    Success,
    Error,
    InvalidResponse,
}

struct TestDispatcher {
    outcome: DispatcherOutcome,
    calls: Arc<AtomicUsize>,
}

impl TestDispatcher {
    fn new(outcome: DispatcherOutcome, calls: Arc<AtomicUsize>) -> Self {
        Self { outcome, calls }
    }
}

impl RequestDispatcherV1 for TestDispatcher {
    fn dispatch(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> ResponseEnvelopeV1 {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            slice_request.command().command_name(),
            request.command().as_str()
        );
        match self.outcome {
            DispatcherOutcome::Success => success_response(request),
            DispatcherOutcome::Error => ResponseEnvelopeV1::Error(
                ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
                    request_id: request.request_id().clone(),
                    command: request.command().clone(),
                    generated_at: timestamp(),
                    code: ErrorCodeV1::new("REQUEST_INVALID").expect("fixture error code is valid"),
                    message: "The dispatcher rejected the request.".to_owned(),
                    retryable: false,
                    exit_code: ExitCodeV1::new(2).expect("fixture exit code is valid"),
                    workspace: None,
                    details: Map::new(),
                })
                .expect("fixture error is valid"),
            ),
            DispatcherOutcome::InvalidResponse => invalid_response(request),
        }
    }
}

struct BlockingDispatcher {
    gate: Arc<(Mutex<GateState>, Condvar)>,
    calls: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
}

#[derive(Default)]
struct GateState {
    entered: bool,
    release: bool,
}

impl RequestDispatcherV1 for BlockingDispatcher {
    fn dispatch(
        &self,
        request: &RequestEnvelopeV1,
        _slice_request: &SliceRequestV1,
    ) -> ResponseEnvelopeV1 {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (lock, changed) = &*self.gate;
        let mut state = lock.lock().expect("test gate lock must not be poisoned");
        state.entered = true;
        changed.notify_all();
        while !state.release {
            state = changed
                .wait(state)
                .expect("test gate lock must not be poisoned");
        }
        self.completed.fetch_add(1, Ordering::SeqCst);
        success_response(request)
    }
}
#[derive(Debug)]
struct PreEpochMetadataClock;

impl ResponseMetadataClockV1 for PreEpochMetadataClock {
    fn now_since_unix_epoch(&self) -> Result<Duration, ResponseMetadataClockErrorV1> {
        Err(ResponseMetadataClockErrorV1::BeforeUnixEpoch)
    }
}

#[derive(Default)]
struct FailSecondConnectionHandlerSpawner {
    attempts: AtomicUsize,
}

impl ConnectionHandlerSpawnerV1 for FailSecondConnectionHandlerSpawner {
    fn spawn(
        &self,
        handler: Box<dyn FnOnce() + Send + 'static>,
    ) -> io::Result<thread::JoinHandle<()>> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 1 {
            return Err(io::Error::other("injected handler spawn failure"));
        }
        thread::Builder::new().spawn(handler)
    }
}

fn timestamp() -> Rfc3339MillisV1 {
    Rfc3339MillisV1::new(GENERATED_AT).expect("fixture timestamp is valid")
}

fn fallback_request_id() -> RequestIdV1 {
    RequestIdV1::new(FALLBACK_REQUEST_ID).expect("fixture request ID is valid")
}

fn request() -> RequestEnvelopeV1 {
    let selector =
        WorktreeSelectorWireV1::new(b"/tmp/podway-worktree", "/tmp/podway-worktree", None)
            .expect("fixture selector is valid");
    let payload = serde_json::from_value(json!({ "selector": selector }))
        .expect("fixture payload is an object");
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("fixture request ID is valid"),
        client: ClientInfoV1::new("podway-test", "1.0.0", 42).expect("fixture client is valid"),
        operation: OperationV1::Query,
        command: CommandNameV1::new("session.status").expect("fixture command is valid"),
        workspace: Some(
            WorkspaceContextV1::new("/tmp/podway-worktree", None)
                .expect("fixture workspace context is valid"),
        ),
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 30_000).expect("fixture options are valid"),
        payload,
    })
    .expect("fixture request is valid")
}

fn request_frame(request: &RequestEnvelopeV1) -> Vec<u8> {
    let payload = encode_request_payload_v1(request).expect("fixture request must encode");
    encode_frame_v1(&payload).expect("fixture request frame must encode")
}

fn metadata() -> FixedResponseMetadataSourceV1 {
    FixedResponseMetadataSourceV1::new(timestamp(), fallback_request_id())
}

fn transport(
    dispatcher: TestDispatcher,
    actual_uid: u32,
    timeouts: ServerTransportTimeoutsV1,
) -> Arc<
    UnixServerTransportV1<
        FixedPeerCredentialSourceV1,
        TestDispatcher,
        FixedResponseMetadataSourceV1,
    >,
> {
    Arc::new(UnixServerTransportV1::with_metadata(
        PeerUidVerifierV1::new(EXPECTED_UID, FixedPeerCredentialSourceV1::uid(actual_uid)),
        dispatcher,
        timeouts,
        metadata(),
    ))
}

fn read_response(client: &mut UnixStream) -> ResponseEnvelopeV1 {
    let payload = read_single_frame_v1(client)
        .expect("server response must be one complete frame")
        .expect("server must send one response frame");
    decode_response_payload_v1(&payload).expect("server response payload must be valid")
}

fn send_and_half_close(client: &mut UnixStream, frame: &[u8]) {
    client
        .write_all(frame)
        .expect("client must write request bytes");
    client
        .shutdown(Shutdown::Write)
        .expect("client must half-close request stream");
}

fn assert_error_code(response: ResponseEnvelopeV1, code: &str) -> podway_protocol::ErrorEnvelopeV1 {
    match response {
        ResponseEnvelopeV1::Error(error) => {
            assert_eq!(error.code().as_str(), code);
            error
        }
        ResponseEnvelopeV1::Output(_) => panic!("expected protocol error response"),
    }
}

#[test]
fn fragmented_same_uid_request_dispatches_once_and_returns_one_framed_output() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::new(Duration::from_secs(30), Duration::from_secs(5))
            .expect("fragmentation test timeouts are valid"),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    let frame = request_frame(&request());
    for byte in frame {
        client.write_all(&[byte]).expect("fragment must write");
    }
    client
        .shutdown(Shutdown::Write)
        .expect("fragmented client must half-close");

    match read_response(&mut client) {
        ResponseEnvelopeV1::Output(output) => {
            assert_eq!(output.request_id().to_string(), REQUEST_ID);
            assert_eq!(output.command().as_str(), "session.status");
        }
        ResponseEnvelopeV1::Error(error) => panic!("unexpected transport error: {error:?}"),
    }
    assert!(handler.join().expect("handler must not panic").is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn uid_mismatch_rejects_before_dispatch_or_frame_read() {
    let (_client, server) = UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        EXPECTED_UID + 1,
        ServerTransportTimeoutsV1::default(),
    );

    let result = transport.handle_connection(server);
    assert!(matches!(
        result,
        Err(ServerConnectionErrorV1::Peer(
            PeerUidVerificationErrorV1::UidMismatch {
                expected_uid: EXPECTED_UID,
                actual_uid,
            }
        )) if actual_uid == EXPECTED_UID + 1
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn invalid_json_returns_a_framed_request_invalid_error_with_a_generated_id() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::default(),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    let frame = encode_frame_v1(b"{not-json").expect("invalid JSON still has a valid frame");
    send_and_half_close(&mut client, &frame);
    let error = assert_error_code(read_response(&mut client), "REQUEST_INVALID");
    assert_eq!(error.request_id(), &fallback_request_id());
    assert_eq!(
        error.details().get("request_id_recovered"),
        Some(&Value::Bool(false))
    );
    assert!(handler.join().expect("handler must not panic").is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[test]
fn pre_epoch_metadata_clock_returns_typed_transport_failure_without_epoch_fallback() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = Arc::new(UnixServerTransportV1::with_metadata(
        PeerUidVerifierV1::new(EXPECTED_UID, FixedPeerCredentialSourceV1::uid(EXPECTED_UID)),
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        ServerTransportTimeoutsV1::default(),
        SystemResponseMetadataSourceV1::with_clock(PreEpochMetadataClock),
    ));
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    let frame = encode_frame_v1(b"{not-json").expect("invalid JSON still has a valid frame");
    send_and_half_close(&mut client, &frame);
    let mut response = [0_u8; 1];
    assert_eq!(
        client
            .read(&mut response)
            .expect("server must close after metadata failure"),
        0
    );
    assert!(matches!(
        handler.join().expect("handler must not panic"),
        Err(ServerConnectionErrorV1::ResponseMetadata(
            ResponseMetadataErrorV1::Clock(ResponseMetadataClockErrorV1::BeforeUnixEpoch)
        ))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn trailing_data_is_rejected_without_dispatch_and_preserves_a_recoverable_request_id() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::default(),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    let mut frame = request_frame(&request());
    frame.push(0xff);
    send_and_half_close(&mut client, &frame);
    let error = assert_error_code(read_response(&mut client), "REQUEST_INVALID");
    assert_eq!(error.request_id().to_string(), REQUEST_ID);
    assert!(handler.join().expect("handler must not panic").is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn oversize_prefix_returns_request_too_large_without_allocating_or_dispatching() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::default(),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    let oversized = u32::try_from(MAX_FRAME_PAYLOAD_BYTES_V1 + 1)
        .expect("protocol limit and one must fit a v1 prefix")
        .to_be_bytes();
    send_and_half_close(&mut client, &oversized);
    let error = assert_error_code(read_response(&mut client), "REQUEST_TOO_LARGE");
    assert_eq!(error.request_id(), &fallback_request_id());
    assert!(handler.join().expect("handler must not panic").is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn unsupported_protocol_returns_typed_error_and_preserves_request_id() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::default(),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    let mut unsupported = serde_json::to_value(request()).expect("fixture request serializes");
    unsupported["protocol"] = Value::String("podway.ipc/v2".to_owned());
    let payload = serde_json::to_vec(&unsupported).expect("unsupported request serializes");
    let frame = encode_frame_v1(&payload).expect("unsupported request frame encodes");
    send_and_half_close(&mut client, &frame);
    let error = assert_error_code(read_response(&mut client), "PROTOCOL_VERSION_UNSUPPORTED");
    assert_eq!(error.request_id().to_string(), REQUEST_ID);
    assert_eq!(
        error.details().get("supported_protocols"),
        Some(&Value::Array(vec![Value::String(
            "podway.ipc/v1".to_owned()
        )]))
    );
    assert!(handler.join().expect("handler must not panic").is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn dispatcher_error_is_framed_and_returned_without_transport_rewriting() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Error, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::default(),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    send_and_half_close(&mut client, &request_frame(&request()));
    let error = assert_error_code(read_response(&mut client), "REQUEST_INVALID");
    assert_eq!(error.message(), "The dispatcher rejected the request.");
    assert!(handler.join().expect("handler must not panic").is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn invalid_dispatcher_response_is_sanitized_and_retains_categorical_evidence() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::InvalidResponse, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::default(),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    send_and_half_close(&mut client, &request_frame(&request()));
    let error = assert_error_code(read_response(&mut client), "INTERNAL_ERROR");
    assert_eq!(error.request_id().to_string(), REQUEST_ID);
    assert_eq!(error.command().as_str(), "session.status");
    assert_eq!(error.message(), "An unexpected internal error occurred.");
    assert!(error.details().is_empty());
    assert!(matches!(
        handler.join().expect("handler must not panic"),
        Err(ServerConnectionErrorV1::InvalidDispatcherResponse)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn read_timeout_returns_sanitized_internal_error_and_retains_frame_io_evidence() {
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let calls = Arc::new(AtomicUsize::new(0));
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("client read timeout must configure");
    let transport = transport(
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        EXPECTED_UID,
        ServerTransportTimeoutsV1::new(Duration::from_millis(10), Duration::from_secs(1))
            .expect("positive timeouts are valid"),
    );
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    let error = assert_error_code(read_response(&mut client), "INTERNAL_ERROR");
    assert_eq!(error.request_id(), &fallback_request_id());
    assert_eq!(
        error.details().get("request_id_recovered"),
        Some(&Value::Bool(false))
    );
    assert!(matches!(
        handler.join().expect("handler must not panic"),
        Err(ServerConnectionErrorV1::RequestFrameIo {
            phase: FrameIoPhaseV1::LengthPrefix,
            ..
        })
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn disconnected_client_response_failure_is_emitted_or_explicitly_accounted() {
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(CapturingObservabilitySink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedObservabilityClock));
    let transport = Arc::new(UnixServerTransportV1::with_metadata_and_observability(
        PeerUidVerifierV1::new(EXPECTED_UID, FixedPeerCredentialSourceV1::uid(EXPECTED_UID)),
        BlockingDispatcher {
            gate: Arc::clone(&gate),
            calls,
            completed: Arc::new(AtomicUsize::new(0)),
        },
        ServerTransportTimeoutsV1::default(),
        metadata(),
        Some(observability.emitter()),
    ));
    let (mut client, server) =
        UnixStream::pair().expect("Unix stream fixture pair must be created");
    let handler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || transport.handle_connection(server))
    };

    send_and_half_close(&mut client, &request_frame(&request()));
    wait_for_dispatcher_entry(&gate);
    drop(client);
    release_dispatcher(&gate);

    assert!(matches!(
        handler.join().expect("handler must not panic"),
        Err(ServerConnectionErrorV1::ResponseWrite(_))
            | Err(ServerConnectionErrorV1::ResponseFlush(_))
    ));
    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    let events = sink
        .events
        .lock()
        .expect("observability events lock must not be poisoned")
        .clone();
    assert_eq!(
        events.first().map(String::as_str),
        Some("ts=42 operation=service_dispatch outcome=succeeded\n")
    );
    let counters = report.counters();
    match events.as_slice() {
        [_, response] => {
            assert_eq!(response, "ts=42 operation=response_write outcome=failed\n");
            assert_eq!(counters.fallback_dropped, 0);
        }
        [_] => assert_eq!(counters.fallback_dropped, 1),
        _ => panic!("response failure must be emitted or explicitly counted"),
    }
    assert_eq!(counters.primary_dropped, 0);
    assert_eq!(
        counters.written.saturating_add(counters.fallback_dropped),
        2
    );
}
#[test]
fn accept_loop_does_not_add_a_service_failure_for_handler_completion() {
    let fixture = SocketFixture::new();
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(CapturingObservabilitySink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedObservabilityClock));
    let emitter = observability.emitter();
    let transport = Arc::new(UnixServerTransportV1::with_metadata_and_observability(
        PeerUidVerifierV1::new(EXPECTED_UID, FixedPeerCredentialSourceV1::uid(EXPECTED_UID)),
        BlockingDispatcher {
            gate: Arc::clone(&gate),
            calls: Arc::clone(&calls),
            completed: Arc::new(AtomicUsize::new(0)),
        },
        ServerTransportTimeoutsV1::default(),
        metadata(),
        Some(emitter.clone()),
    ));
    let admission = ShutdownAdmissionV1::new();
    let accept_loop = BoundedAcceptLoopV1::new_with_observability(
        transport,
        admission.clone(),
        NonZeroUsize::new(1).expect("one is nonzero"),
        Some(emitter),
    );
    let listener = fixture
        .listener
        .try_clone()
        .expect("listener must clone for accept loop");
    let loop_thread = thread::spawn(move || accept_loop.run(&listener));

    let mut client = UnixStream::connect(&fixture.socket_path).expect("client must connect");
    send_and_half_close(&mut client, &request_frame(&request()));
    wait_for_dispatcher_entry(&gate);
    drop(client);
    release_dispatcher(&gate);
    admission.request_shutdown();

    assert!(
        loop_thread
            .join()
            .expect("accept loop must not panic")
            .is_ok()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    let events = sink
        .events
        .lock()
        .expect("observability events lock must not be poisoned");
    assert_eq!(
        events.first().map(String::as_str),
        Some("ts=42 operation=connection_accepted outcome=succeeded\n")
    );
    assert!(
        events
            .iter()
            .any(|event| event == "ts=42 operation=service_dispatch outcome=succeeded\n")
    );
    assert!(
        !events
            .iter()
            .any(|event| event == "ts=42 operation=transport_service_request outcome=failed\n")
    );
    let counters = report.counters();
    assert_eq!(
        counters.fallback_dropped, 0,
        "handler completion must not hide a failed service event in the fallback drop counter"
    );
    assert_eq!(counters.unflushed, 0);
    assert_eq!(counters.write_failures, 0);
    assert_eq!(counters.flush_failures, 0);
    assert_eq!(counters.clock_errors, 0);
    assert_eq!(counters.clock_panics, 0);
    assert_eq!(counters.sink_failures, 0);
}
#[test]
fn accept_loop_rejects_queued_connections_at_capacity_with_one_saturation_event() {
    let fixture = SocketFixture::new();
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(CapturingObservabilitySink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedObservabilityClock));
    let emitter = observability.emitter();
    let transport = Arc::new(UnixServerTransportV1::with_metadata_and_observability(
        PeerUidVerifierV1::new(EXPECTED_UID, FixedPeerCredentialSourceV1::uid(EXPECTED_UID)),
        BlockingDispatcher {
            gate: Arc::clone(&gate),
            calls: Arc::clone(&calls),
            completed: Arc::new(AtomicUsize::new(0)),
        },
        ServerTransportTimeoutsV1::default(),
        metadata(),
        Some(emitter.clone()),
    ));
    let admission = ShutdownAdmissionV1::new();
    let accept_loop = BoundedAcceptLoopV1::new_with_observability(
        transport,
        admission.clone(),
        NonZeroUsize::new(1).expect("one is nonzero"),
        Some(emitter),
    );
    let listener = fixture
        .listener
        .try_clone()
        .expect("listener must clone for accept loop");
    let loop_thread = thread::spawn(move || accept_loop.run(&listener));

    let mut first = UnixStream::connect(&fixture.socket_path).expect("first client must connect");
    send_and_half_close(&mut first, &request_frame(&request()));
    wait_for_dispatcher_entry(&gate);

    let mut rejected = UnixStream::connect(&fixture.socket_path)
        .expect("second client must queue while the first handler is admitted");
    rejected
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("rejected client read timeout must configure");
    assert_eq!(
        rejected
            .read(&mut [0_u8; 1])
            .expect("capacity-rejected client must close"),
        0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    admission.request_shutdown();
    release_dispatcher(&gate);
    match read_response(&mut first) {
        ResponseEnvelopeV1::Output(_) => {}
        ResponseEnvelopeV1::Error(error) => panic!("admitted request must finish: {error:?}"),
    }
    assert!(
        loop_thread
            .join()
            .expect("accept loop must not panic")
            .is_ok()
    );

    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    let counters = report.counters();
    assert_eq!(counters.accepted, 4);
    assert_eq!(counters.written, 4);
    assert_eq!(counters.primary_dropped, 0);
    assert_eq!(counters.fallback_dropped, 0);
    assert_eq!(counters.stopped_dropped, 0);
    assert_eq!(counters.degraded_dropped, 0);
    assert_eq!(counters.unflushed, 0);
    assert_eq!(counters.write_failures, 0);
    assert_eq!(counters.flush_failures, 0);
    assert_eq!(counters.clock_errors, 0);
    assert_eq!(counters.clock_panics, 0);
    assert_eq!(counters.sink_failures, 0);
    let events = sink
        .events
        .lock()
        .expect("observability events lock must not be poisoned");
    let inventory = events.iter().fold(BTreeMap::new(), |mut inventory, event| {
        *inventory.entry(event.as_str()).or_insert(0_usize) += 1;
        inventory
    });
    assert_eq!(
        inventory,
        BTreeMap::from([
            (
                "ts=42 operation=connection_accepted outcome=succeeded\n",
                2_usize,
            ),
            (
                "ts=42 operation=admission_saturation outcome=saturated\n",
                1_usize,
            ),
            (
                "ts=42 operation=service_dispatch outcome=succeeded\n",
                1_usize,
            ),
        ]),
        "capacity saturation emits only the closed expected inventory"
    );
}

#[test]
fn accept_loop_records_peer_rejection_without_service_failure() {
    let fixture = SocketFixture::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(CapturingObservabilitySink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedObservabilityClock));
    let emitter = observability.emitter();
    let transport = Arc::new(UnixServerTransportV1::with_metadata_and_observability(
        PeerUidVerifierV1::new(
            EXPECTED_UID,
            FixedPeerCredentialSourceV1::uid(EXPECTED_UID + 1),
        ),
        TestDispatcher::new(DispatcherOutcome::Success, Arc::clone(&calls)),
        ServerTransportTimeoutsV1::default(),
        metadata(),
        Some(emitter.clone()),
    ));
    let admission = ShutdownAdmissionV1::new();
    let accept_loop = BoundedAcceptLoopV1::new_with_observability(
        transport,
        admission.clone(),
        NonZeroUsize::new(1).expect("one is nonzero"),
        Some(emitter),
    );
    let listener = fixture
        .listener
        .try_clone()
        .expect("listener must clone for accept loop");
    let loop_thread = thread::spawn(move || accept_loop.run(&listener));

    let mut client = UnixStream::connect(&fixture.socket_path).expect("client must connect");
    assert_eq!(
        client
            .read(&mut [0_u8; 1])
            .expect("rejected client must close"),
        0
    );
    admission.request_shutdown();

    assert!(
        loop_thread
            .join()
            .expect("accept loop must not panic")
            .is_ok()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    let events = sink
        .events
        .lock()
        .expect("observability events lock must not be poisoned")
        .clone();
    let counters = report.counters();
    let actual_inventory = events.iter().fold(BTreeMap::new(), |mut inventory, event| {
        *inventory.entry(event.as_str()).or_insert(0_usize) += 1;
        inventory
    });
    let expected_inventory = BTreeMap::from([
        ("ts=42 operation=connection_accepted outcome=succeeded\n", 1),
        ("ts=42 operation=peer_admission outcome=rejected\n", 1),
    ]);
    assert_eq!(
        actual_inventory.get("ts=42 operation=peer_admission outcome=rejected\n"),
        Some(&1),
        "the legitimate fallback rejection must reach the sink"
    );
    assert!(
        actual_inventory.iter().all(|(event, count)| {
            expected_inventory
                .get(*event)
                .is_some_and(|expected| count <= expected)
        }),
        "peer rejection must not emit an unexpected service failure"
    );
    assert_eq!(
        counters.fallback_dropped, 0,
        "peer rejection's sole legitimate fallback event must not mask an unexpected fallback drop"
    );
    assert_eq!(counters.accepted, 2);
    assert_eq!(counters.written, events.len() as u64);
    assert_eq!(
        counters
            .written
            .saturating_add(counters.primary_dropped)
            .saturating_add(counters.fallback_dropped),
        2
    );
    assert_eq!(counters.unflushed, 0);
    assert_eq!(counters.write_failures, 0);
    assert_eq!(counters.flush_failures, 0);
    assert_eq!(counters.clock_errors, 0);
    assert_eq!(counters.clock_panics, 0);
    assert_eq!(counters.sink_failures, 0);
}
#[test]
fn shutdown_stops_new_admission_and_waits_for_an_admitted_handler_to_finish() {
    let fixture = SocketFixture::new();
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let observation_sink = Arc::new(CapturingObservabilitySink::default());
    let observability =
        ObservabilityV1::start(observation_sink.clone(), Arc::new(FixedObservabilityClock));
    let emitter = observability.emitter();
    let transport = Arc::new(UnixServerTransportV1::with_metadata_and_observability(
        PeerUidVerifierV1::new(EXPECTED_UID, FixedPeerCredentialSourceV1::uid(EXPECTED_UID)),
        BlockingDispatcher {
            gate: Arc::clone(&gate),
            calls: Arc::clone(&calls),
            completed: Arc::clone(&completed),
        },
        ServerTransportTimeoutsV1::default(),
        metadata(),
        Some(emitter.clone()),
    ));
    let admission = ShutdownAdmissionV1::new();
    let accept_loop = BoundedAcceptLoopV1::new_with_observability(
        Arc::clone(&transport),
        admission.clone(),
        NonZeroUsize::new(1).expect("one is nonzero"),
        Some(emitter),
    );
    let listener = fixture
        .listener
        .try_clone()
        .expect("listener must clone for accept loop");
    let loop_thread = thread::spawn(move || accept_loop.run(&listener));

    let mut first = UnixStream::connect(&fixture.socket_path).expect("first client must connect");
    send_and_half_close(&mut first, &request_frame(&request()));
    wait_for_dispatcher_entry(&gate);

    admission.request_shutdown();
    let second = UnixStream::connect(&fixture.socket_path)
        .expect("kernel may queue a connection while listener is draining");

    release_dispatcher(&gate);
    match read_response(&mut first) {
        ResponseEnvelopeV1::Output(_) => {}
        ResponseEnvelopeV1::Error(error) => panic!("admitted request must finish: {error:?}"),
    }
    assert!(
        loop_thread
            .join()
            .expect("accept loop must not panic")
            .is_ok()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(admission.in_flight(), 0);
    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    let events = observation_sink
        .events
        .lock()
        .expect("observability events lock must not be poisoned");
    assert!(
        events
            .iter()
            .any(|event| event.contains("operation=connection_accepted outcome=succeeded")),
        "the real accept boundary must emit accepted-connection evidence"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("operation=service_dispatch outcome=succeeded")),
        "the real dispatch boundary must emit service outcome evidence"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.contains("operation=admission_saturation outcome=saturated")),
        "shutdown closure is not admission saturation"
    );
    let counters = report.counters();
    assert_eq!(
        counters.fallback_dropped, 0,
        "shutdown closure must not hide saturation in the fallback drop counter"
    );
    assert_eq!(counters.unflushed, 0);
    assert_eq!(counters.write_failures, 0);
    assert_eq!(counters.flush_failures, 0);
    assert_eq!(counters.clock_errors, 0);
    assert_eq!(counters.clock_panics, 0);
    assert_eq!(counters.sink_failures, 0);
    drop(second);
}
#[test]
fn handler_spawn_failure_closes_admission_releases_ticket_and_drains_existing_handler() {
    let fixture = SocketFixture::new();
    let gate = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let transport = Arc::new(UnixServerTransportV1::with_metadata(
        PeerUidVerifierV1::new(EXPECTED_UID, FixedPeerCredentialSourceV1::uid(EXPECTED_UID)),
        BlockingDispatcher {
            gate: Arc::clone(&gate),
            calls: Arc::clone(&calls),
            completed: Arc::clone(&completed),
        },
        ServerTransportTimeoutsV1::default(),
        metadata(),
    ));
    let admission = ShutdownAdmissionV1::new();
    let spawner = Arc::new(FailSecondConnectionHandlerSpawner::default());
    let accept_loop = BoundedAcceptLoopV1::with_poll_interval_and_handler_spawner(
        Arc::clone(&transport),
        admission.clone(),
        NonZeroUsize::new(2).expect("two is nonzero"),
        Duration::from_millis(1),
        spawner.clone(),
    )
    .expect("positive accept poll interval is valid");
    let listener = fixture
        .listener
        .try_clone()
        .expect("listener must clone for accept loop");
    let loop_thread = thread::spawn(move || accept_loop.run(&listener));

    let mut first = UnixStream::connect(&fixture.socket_path).expect("first client must connect");
    send_and_half_close(&mut first, &request_frame(&request()));
    wait_for_dispatcher_entry(&gate);

    let mut second = UnixStream::connect(&fixture.socket_path).expect("second client must connect");
    second
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("second client read timeout must configure");
    wait_for_spawn_attempts(&spawner, 2);
    release_dispatcher(&gate);

    match read_response(&mut first) {
        ResponseEnvelopeV1::Output(_) => {}
        ResponseEnvelopeV1::Error(error) => panic!("admitted request must finish: {error:?}"),
    }
    assert_eq!(
        second
            .read(&mut [0_u8; 1])
            .expect("failed connection must close"),
        0
    );
    assert!(matches!(
        loop_thread.join().expect("accept loop must not panic"),
        Err(ServerAcceptLoopErrorV1::SpawnHandler(source))
            if source.kind() == io::ErrorKind::Other
    ));
    assert!(!admission.is_accepting());
    assert_eq!(admission.in_flight(), 0);
    assert_eq!(spawner.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

fn success_response(request: &RequestEnvelopeV1) -> ResponseEnvelopeV1 {
    let output = OutputEnvelopeV1::new(OutputEnvelopeInputV1 {
        request_id: request.request_id().clone(),
        command: request.command().clone(),
        generated_at: timestamp(),
        workspace: None,
        job: None,
        session: None,
        result: Map::new(),
        warnings: Vec::new(),
    })
    .expect("fixture output is valid");
    assert!(
        output.validate().is_ok(),
        "fixture output must satisfy the public response contract: {:?}",
        output.validate()
    );
    ResponseEnvelopeV1::Output(output)
}
fn invalid_response(request: &RequestEnvelopeV1) -> ResponseEnvelopeV1 {
    ResponseEnvelopeV1::Output(
        OutputEnvelopeV1::new(OutputEnvelopeInputV1 {
            request_id: fallback_request_id(),
            command: request.command().clone(),
            generated_at: timestamp(),
            workspace: None,
            job: None,
            session: None,
            result: Map::new(),
            warnings: Vec::new(),
        })
        .expect("fixture output is valid"),
    )
}

fn wait_for_dispatcher_entry(gate: &Arc<(Mutex<GateState>, Condvar)>) {
    let (lock, changed) = &**gate;
    let state = lock.lock().expect("test gate lock must not be poisoned");
    let (state, _timeout) = changed
        .wait_timeout_while(state, Duration::from_secs(10), |state| !state.entered)
        .expect("test gate lock must not be poisoned");
    assert!(state.entered, "dispatcher was not admitted before timeout");
}
fn wait_for_spawn_attempts(spawner: &FailSecondConnectionHandlerSpawner, expected: usize) {
    for _ in 0..10_000 {
        if spawner.attempts.load(Ordering::SeqCst) >= expected {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("handler spawner did not receive {expected} attempts before timeout");
}

fn release_dispatcher(gate: &Arc<(Mutex<GateState>, Condvar)>) {
    let (lock, changed) = &**gate;
    let mut state = lock.lock().expect("test gate lock must not be poisoned");
    state.release = true;
    changed.notify_all();
}
