//! Real Git, SQLite, and Unix-socket coverage for the production daemon runtime.

#![forbid(unsafe_code)]

use crate::support_phase4_workspace;

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io,
    net::Shutdown,
    num::NonZeroUsize,
    os::unix::{
        ffi::OsStrExt,
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use podway_core::UnixMillis;
use podway_daemon::{
    dispatch::{DispatchFailureKindV1, WorkspaceRuntimeV1},
    observability::{
        ClockErrorV1, ClockV1, LogSinkV1, ObservabilityFinalizationV1, ObservabilityV1,
    },
    production::{NativeProductionClockV1, ProductionWorkspaceRuntimeV1, compose_dispatcher_v1},
    runtime::{
        ProductionDaemonRuntimeConfigV1, ProductionDaemonRuntimeV1, WorkspaceRecoveryEntryV1,
        WorkspaceRecoveryUnavailableReasonV1,
    },
    runtime_workspace::{WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1},
    server::RequestDispatcherV1,
};
use podway_git::{GitResolverContractV1, NativeGitResolverV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, DAEMON_COMMAND_NAMES_V1,
    IdempotencyKeyV1 as ProtocolIdempotencyKeyV1, JobStateV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV1,
    Rfc3339MillisV1, SliceRequestV1, WorkspaceContextV1, WorktreeSelectorWireV1,
    decode_response_payload_v1, encode_request_payload_v1, read_single_frame_v1, write_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{SqliteStoreOptionsV1, SqliteStoreV1, ValidatedWorkspaceRootV1, WorkerIdV1};
use serde_json::Value;
use support_phase4_workspace::{
    TemporaryDirectoryV1, copy_tree, git_worktrees, non_utf8_child_path, read_file,
    selector as git_selector,
};
#[derive(Default)]
struct CapturingObservabilitySinkV1 {
    events: Mutex<Vec<String>>,
}

impl LogSinkV1 for CapturingObservabilitySinkV1 {
    fn write_event(&self, event: &str) -> io::Result<()> {
        self.events
            .lock()
            .expect("observability event lock must not be poisoned")
            .push(event.to_owned());
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

struct FixedObservabilityClockV1;

impl ClockV1 for FixedObservabilityClockV1 {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1> {
        Ok(42)
    }
}

struct RuntimePathsFixtureV1 {
    paths: ServiceRuntimePathsV1,
    runtime_directory: PathBuf,
}

impl RuntimePathsFixtureV1 {
    fn paths(&self) -> &ServiceRuntimePathsV1 {
        &self.paths
    }
}

impl Drop for RuntimePathsFixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.runtime_directory);
    }
}

fn runtime_paths(root: &Path) -> RuntimePathsFixtureV1 {
    let application_support = root.join("Application Support/Podway");
    fs::create_dir_all(&application_support).expect("registry parent must be created");
    fs::set_permissions(&application_support, fs::Permissions::from_mode(0o700))
        .expect("registry parent must be private");
    let mut hasher = DefaultHasher::new();
    root.as_os_str().as_bytes().hash(&mut hasher);
    let runtime_directory = std::env::temp_dir().join(format!("pdr-{:016x}", hasher.finish()));
    let _ = fs::remove_dir_all(&runtime_directory);
    let paths = ServiceRuntimePathsV1::from_directories(
        root.join("LaunchAgents"),
        application_support,
        root.join("Logs/Podway"),
        &runtime_directory,
    )
    .expect("service paths must be valid");
    RuntimePathsFixtureV1 {
        paths,
        runtime_directory,
    }
}

fn make_workspace_runtime_private(root: &Path) {
    fs::set_permissions(
        root.join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("workspace runtime directory must be private");
}

fn configuration(worker: &str) -> ProductionDaemonRuntimeConfigV1 {
    ProductionDaemonRuntimeConfigV1::new(
        WorkerIdV1::new(worker).expect("worker ID must be valid"),
        NonZeroUsize::new(4).expect("four is nonzero"),
        Default::default(),
    )
}
fn observation() -> WorkspaceRuntimeObservationV1 {
    WorkspaceRuntimeObservationV1::new(
        UnixMillis::new(1_700_000_000_123),
        Rfc3339MillisV1::new("2026-07-15T12:34:56.789Z")
            .expect("fixture registry timestamp must be valid"),
    )
}

fn selector(path: &Path) -> WorktreeSelectorWireV1 {
    use std::os::unix::ffi::OsStrExt;

    let canonical = fs::canonicalize(path).expect("fixture worktree must be canonical");
    WorktreeSelectorWireV1::new(
        canonical.as_os_str().as_bytes(),
        canonical.display().to_string(),
        None,
    )
    .expect("fixture selector must be valid")
}

fn request(
    request_number: u64,
    command: &str,
    operation: OperationV1,
    selector: &WorktreeSelectorWireV1,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    request_with_payload(
        request_number,
        command,
        operation,
        selector,
        serde_json::json!({"selector": selector})
            .as_object()
            .expect("fixture payload must be an object")
            .clone(),
    )
}

fn request_with_payload(
    request_number: u64,
    command: &str,
    operation: OperationV1,
    selector: &WorktreeSelectorWireV1,
    payload: serde_json::Map<String, serde_json::Value>,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    request_with_configuration(
        request_number,
        command,
        operation,
        selector,
        payload,
        PreconditionsV1::default(),
        RequestOptionsV1::new(
            false,
            if operation == OperationV1::Query {
                0
            } else {
                5_000
            },
        )
        .expect("fixture request options must be valid"),
    )
}

fn request_with_configuration(
    request_number: u64,
    command: &str,
    operation: OperationV1,
    selector: &WorktreeSelectorWireV1,
    payload: serde_json::Map<String, serde_json::Value>,
    preconditions: PreconditionsV1,
    options: RequestOptionsV1,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .expect("fixture request ID must be valid"),
        client: ClientInfoV1::new("daemon-runtime-test", "1", 1)
            .expect("fixture client must be valid"),
        operation,
        command: CommandNameV1::new(command).expect("fixture command must be valid"),
        workspace: Some(
            WorkspaceContextV1::new(selector.display(), None)
                .expect("fixture workspace context must be valid"),
        ),
        idempotency_key: (operation != OperationV1::Query).then(|| {
            ProtocolIdempotencyKeyV1::new(format!("daemon-runtime-{request_number}"))
                .expect("fixture idempotency key must be valid")
        }),
        preconditions,
        options,
        payload,
    })
    .expect("fixture request must be valid");
    let slice = SliceRequestV1::from_envelope(&envelope).expect("fixture request must be routed");
    (envelope, slice)
}
fn raw_request(
    request_number: u64,
    command: &str,
    operation: OperationV1,
    selector: &WorktreeSelectorWireV1,
) -> RequestEnvelopeV1 {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .expect("fixture request ID must be valid"),
        client: ClientInfoV1::new("daemon-runtime-test", "1", 1)
            .expect("fixture client must be valid"),
        operation,
        command: CommandNameV1::new(command).expect("fixture command must be valid"),
        workspace: Some(
            WorkspaceContextV1::new(selector.display(), None)
                .expect("fixture workspace context must be valid"),
        ),
        idempotency_key: (operation != OperationV1::Query).then(|| {
            ProtocolIdempotencyKeyV1::new(format!("daemon-runtime-{request_number}"))
                .expect("fixture idempotency key must be valid")
        }),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 5_000)
            .expect("fixture request options must be valid"),
        payload: serde_json::json!({"selector": selector})
            .as_object()
            .expect("fixture raw payload must be an object")
            .clone(),
    })
    .expect("fixture raw request must be valid")
}
fn send_request(socket_path: &Path, request: &RequestEnvelopeV1) -> ResponseEnvelopeV1 {
    let payload = encode_request_payload_v1(request).expect("production request must encode");
    let mut client = UnixStream::connect(socket_path)
        .expect("client must connect to the bound production daemon");
    write_frame_v1(&mut client, &payload).expect("client must write the production request frame");
    client
        .shutdown(Shutdown::Write)
        .expect("client must half-close the production request");
    let response_payload = read_single_frame_v1(&mut client)
        .expect("production daemon must return one response frame")
        .expect("production daemon must return a response frame");
    decode_response_payload_v1(&response_payload)
        .expect("production response must satisfy the protocol")
}

struct PassiveObservabilityFlowV1 {
    bootstrap: ResponseEnvelopeV1,
    session_start: ResponseEnvelopeV1,
    persisted_session: ResponseEnvelopeV1,
}

fn passive_observability_flow(
    fixture: &support_phase4_workspace::GitWorktreeFixtureV1,
    observability: Option<podway_daemon::observability::ObservabilityEmitterV1>,
) -> PassiveObservabilityFlowV1 {
    make_workspace_runtime_private(fixture.main());
    let paths = runtime_paths(fixture.temporary_path());
    let workspace_selector = selector(fixture.main());
    let runtime = ProductionDaemonRuntimeV1::bind_with_observability(
        paths.paths(),
        SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid"),
        configuration("observability-equivalence-worker"),
        observability,
    )
    .expect("production daemon must bind for the observability equivalence flow");
    let socket_path = runtime.socket_path().to_path_buf();
    let shutdown = runtime.shutdown_handle();
    let server = thread::spawn(move || runtime.run());

    let bootstrap = send_request(
        &socket_path,
        &request(
            100,
            "workspace.init",
            OperationV1::Bootstrap,
            &workspace_selector,
        )
        .0,
    );
    let session_start = send_request(
        &socket_path,
        &request_with_payload(
            101,
            "session.start",
            OperationV1::Mutate,
            &workspace_selector,
            serde_json::json!({
                "selector": workspace_selector,
                "preset": "sw-dev",
                "task_title": "Observability equivalence"
            })
            .as_object()
            .expect("session start payload must be an object")
            .clone(),
        )
        .0,
    );
    let persisted_session = send_request(
        &socket_path,
        &request(
            102,
            "session.status",
            OperationV1::Query,
            &workspace_selector,
        )
        .0,
    );

    shutdown.request_shutdown();
    server
        .join()
        .expect("production daemon thread must not panic")
        .expect("production daemon shutdown must succeed");
    assert!(
        fixture
            .main()
            .join(".podway/runtime/state.sqlite3")
            .is_file(),
        "the compared session status must come from a daemon-owned persisted Store"
    );
    for (operation, response) in [
        ("workspace.init", &bootstrap),
        ("session.start", &session_start),
        ("session.status", &persisted_session),
    ] {
        let ResponseEnvelopeV1::Output(output) = response else {
            panic!("{operation} must succeed in the production observability flow");
        };
        assert_eq!(
            output
                .workspace()
                .expect("{operation} must report its persisted workspace")
                .root(),
            fs::canonicalize(fixture.main())
                .expect("fixture workspace root must canonicalize")
                .to_str()
                .expect("fixture workspace root must be UTF-8"),
            "{operation} must report the Store workspace root it used"
        );
    }
    PassiveObservabilityFlowV1 {
        bootstrap,
        session_start,
        persisted_session,
    }
}

fn normalize_identity_value(value: &str, identities: &mut BTreeMap<String, usize>) -> String {
    let next_identity = identities.len() + 1;
    format!(
        "<identity-{}>",
        identities.entry(value.to_owned()).or_insert(next_identity)
    )
}

fn is_identity_field(field: &str) -> bool {
    field == "id" || field == "uuid" || field.ends_with("_id") || field.ends_with("_uuid")
}

fn normalize_semantic_value(value: &Value, identities: &mut BTreeMap<String, usize>) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| normalize_semantic_value(value, identities))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(field, value)| {
                    let value = match value {
                        Value::String(value) if is_identity_field(field) => {
                            Value::String(normalize_identity_value(value, identities))
                        }
                        Value::String(_) | Value::Number(_)
                            if field.ends_with("_at")
                                || field.ends_with("_at_ms")
                                || field == "generated_at" =>
                        {
                            Value::String("<timestamp>".to_owned())
                        }
                        _ => normalize_semantic_value(value, identities),
                    };
                    (field.clone(), value)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn normalize_result(
    result: &serde_json::Map<String, Value>,
    identities: &mut BTreeMap<String, usize>,
) -> Value {
    normalize_semantic_value(&Value::Object(result.clone()), identities)
}

fn assert_equivalent_public_output(
    disabled: &ResponseEnvelopeV1,
    enabled: &ResponseEnvelopeV1,
    operation: &str,
    disabled_identities: &mut BTreeMap<String, usize>,
    enabled_identities: &mut BTreeMap<String, usize>,
) {
    let (ResponseEnvelopeV1::Output(disabled), ResponseEnvelopeV1::Output(enabled)) =
        (disabled, enabled)
    else {
        panic!(
            "{operation} must have the same successful public outcome with observability enabled"
        );
    };
    assert_eq!(
        disabled.command(),
        enabled.command(),
        "{operation} command outcome must not change when observability is enabled"
    );
    assert_eq!(
        normalize_result(disabled.result(), disabled_identities),
        normalize_result(enabled.result(), enabled_identities),
        "{operation} response result semantics must not change when observability is enabled"
    );
    assert_eq!(
        disabled.warnings(),
        enabled.warnings(),
        "{operation} response warnings must not change when observability is enabled"
    );
    assert_eq!(
        disabled.workspace().map(|workspace| {
            (
                normalize_identity_value(workspace.uuid().as_str(), disabled_identities),
                workspace.latest_workspace_sequence(),
            )
        }),
        enabled.workspace().map(|workspace| {
            (
                normalize_identity_value(workspace.uuid().as_str(), enabled_identities),
                workspace.latest_workspace_sequence(),
            )
        }),
        "{operation} persisted workspace state must not change when observability is enabled"
    );
    assert_eq!(
        disabled.job().map(|job| {
            (
                normalize_identity_value(job.id().as_str(), disabled_identities),
                job.sequence(),
                job.state(),
                job.claimed_at().is_some(),
                job.finished_at().is_some(),
            )
        }),
        enabled.job().map(|job| {
            (
                normalize_identity_value(job.id().as_str(), enabled_identities),
                job.sequence(),
                job.state(),
                job.claimed_at().is_some(),
                job.finished_at().is_some(),
            )
        }),
        "{operation} persisted job state must not change when observability is enabled"
    );
    assert_eq!(
        disabled.session().map(|session| {
            (
                normalize_identity_value(session.id().as_str(), disabled_identities),
                session.title().to_owned(),
                session.lifecycle(),
                session.revision_before(),
                session.revision_after(),
            )
        }),
        enabled.session().map(|session| {
            (
                normalize_identity_value(session.id().as_str(), enabled_identities),
                session.title().to_owned(),
                session.lifecycle(),
                session.revision_before(),
                session.revision_after(),
            )
        }),
        "{operation} persisted session state must not change when observability is enabled"
    );
}

#[test]
fn pac017_enabled_observability_is_passive_for_production_requests_and_store_state() {
    let disabled_fixture = git_worktrees();
    let enabled_fixture = git_worktrees();
    let disabled = passive_observability_flow(&disabled_fixture, None);

    let sink = Arc::new(CapturingObservabilitySinkV1::default());
    let observability = ObservabilityV1::start(sink, Arc::new(FixedObservabilityClockV1));
    let enabled = passive_observability_flow(&enabled_fixture, Some(observability.emitter()));
    assert_eq!(
        observability.shutdown().finalization(),
        ObservabilityFinalizationV1::Completed
    );

    let mut disabled_identities = BTreeMap::new();
    let mut enabled_identities = BTreeMap::new();
    assert_equivalent_public_output(
        &disabled.bootstrap,
        &enabled.bootstrap,
        "workspace.init",
        &mut disabled_identities,
        &mut enabled_identities,
    );
    assert_equivalent_public_output(
        &disabled.session_start,
        &enabled.session_start,
        "session.start",
        &mut disabled_identities,
        &mut enabled_identities,
    );
    assert_equivalent_public_output(
        &disabled.persisted_session,
        &enabled.persisted_session,
        "session.status",
        &mut disabled_identities,
        &mut enabled_identities,
    );
}

#[test]
fn pac017_daemon_is_the_sole_normal_store_writer() {
    let fixture = git_worktrees();
    make_workspace_runtime_private(fixture.main());
    let paths = runtime_paths(fixture.temporary_path());
    let options = SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid");
    let workspace_selector = selector(fixture.main());

    let sink = Arc::new(CapturingObservabilitySinkV1::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedObservabilityClockV1));
    let runtime = ProductionDaemonRuntimeV1::bind_with_observability(
        paths.paths(),
        options,
        configuration("recovery-worker"),
        Some(observability.emitter()),
    )
    .expect("startup recovery must bind the endpoint and recover the workspace");
    assert_eq!(
        runtime.recovery_report().recovered_workspace_count(),
        0,
        "only the daemon-owned runtime may create the workspace Store"
    );

    let socket_path = runtime.socket_path().to_path_buf();
    let shutdown = runtime.shutdown_handle();
    let server = thread::spawn(move || runtime.run());
    let direct_request_inventory = ["workspace.init", "session.status", "session.start"];
    let initialize_workspace = request(
        1,
        direct_request_inventory[0],
        OperationV1::Bootstrap,
        &workspace_selector,
    );
    let payload = encode_request_payload_v1(&initialize_workspace.0)
        .expect("workspace init request must encode");
    let mut client =
        UnixStream::connect(&socket_path).expect("client must connect to the bound daemon");
    write_frame_v1(&mut client, &payload).expect("client must write framed workspace init request");
    client
        .shutdown(Shutdown::Write)
        .expect("client must half-close workspace init request");
    let response_payload = read_single_frame_v1(&mut client)
        .expect("daemon must return one workspace init response frame")
        .expect("daemon must return a workspace init response frame");
    assert!(matches!(
        decode_response_payload_v1(&response_payload)
            .expect("workspace init response must satisfy the public protocol"),
        ResponseEnvelopeV1::Output(_)
    ));
    let status = request(
        2,
        direct_request_inventory[1],
        OperationV1::Query,
        &workspace_selector,
    );
    let payload = encode_request_payload_v1(&status.0).expect("status request must encode");
    let mut client =
        UnixStream::connect(&socket_path).expect("client must connect to bound daemon");
    write_frame_v1(&mut client, &payload).expect("client must write framed status request");
    client
        .shutdown(Shutdown::Write)
        .expect("client must half-close status request");
    let response_payload = read_single_frame_v1(&mut client)
        .expect("daemon must return one framed response")
        .expect("daemon must return a response frame");
    let response = decode_response_payload_v1(&response_payload)
        .expect("daemon response must satisfy the public protocol");
    match response {
        ResponseEnvelopeV1::Error(error) => assert_eq!(error.code().as_str(), "SESSION_NOT_FOUND"),
        ResponseEnvelopeV1::Output(_) => {
            panic!("uninitialized session status must be a typed error")
        }
    }

    let initialize = request_with_payload(
        3,
        direct_request_inventory[2],
        OperationV1::Mutate,
        &workspace_selector,
        serde_json::json!({
            "selector": workspace_selector,
            "preset": "sw-dev",
            "task_title": "Observability production trace"
        })
        .as_object()
        .expect("session start payload must be an object")
        .clone(),
    );
    let payload = encode_request_payload_v1(&initialize.0).expect("init request must encode");
    let mut client = UnixStream::connect(&socket_path)
        .expect("client must connect for workspace initialization");
    write_frame_v1(&mut client, &payload).expect("client must write framed init request");
    client
        .shutdown(Shutdown::Write)
        .expect("client must half-close init request");
    let response_payload = read_single_frame_v1(&mut client)
        .expect("daemon must return one init response frame")
        .expect("daemon must return an init response frame");
    let response = decode_response_payload_v1(&response_payload)
        .expect("init response must satisfy the public protocol");
    let ResponseEnvelopeV1::Output(output) = response else {
        panic!("a normal public mutation must cross the daemon boundary and be admitted");
    };
    assert_eq!(output.command().as_str(), "session.start");
    assert!(
        output.job().is_some(),
        "the daemon, rather than a caller-controlled transcript, must report the admitted mutation job",
    );

    let normal_public_writers = DAEMON_COMMAND_NAMES_V1
        .iter()
        .copied()
        .filter(|route| {
            !matches!(
                *route,
                "workspace.doctor"
                    | "workspace.show"
                    | "session.status"
                    | "session.next"
                    | "job.list"
                    | "job.lookup"
                    | "job.status"
                    | "job.wait"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        normal_public_writers,
        [
            "workspace.init",
            "workspace.repair",
            "session.start",
            "session.start_replace",
            "session.complete",
            "session.skip",
            "session.retry",
            "session.return",
            "session.block",
            "session.unblock",
            "session.cancel",
            "session.reopen",
            "session.reset",
            "workspace.reset_all",
            "item.check",
            "item.uncheck",
            "item.set",
            "item.add",
            "item.remove",
            "item.attach",
            "item.clear",
            "job.cancel",
        ],
        "the protocol-owned daemon route inventory must be the complete normal writable surface"
    );
    let normal_writer_service_request_inventory = ["workspace.init"];
    assert!(
        normal_writer_service_request_inventory
            .iter()
            .all(|route| normal_public_writers.contains(route)),
        "the service-dispatched inventory must be drawn from the exercised normal-writer routes"
    );
    let normal_writer_transport_rejection_inventory = normal_public_writers
        .iter()
        .copied()
        .filter(|route| !normal_writer_service_request_inventory.contains(route))
        .collect::<Vec<_>>();
    assert_eq!(
        normal_writer_service_request_inventory.len()
            + normal_writer_transport_rejection_inventory.len(),
        normal_public_writers.len(),
        "every normal-writer probe must be classified as either service-dispatched or transport-rejected"
    );
    let expected_connection_attempts = direct_request_inventory.len() + normal_public_writers.len();
    let expected_transport_rejections = normal_writer_transport_rejection_inventory.len();
    for (offset, route) in normal_public_writers.iter().enumerate() {
        let operation = match *route {
            "workspace.init" | "workspace.reset_all" => OperationV1::Bootstrap,
            "workspace.repair" | "job.cancel" => OperationV1::Control,
            _ => OperationV1::Mutate,
        };
        let probe = raw_request(10 + offset as u64, route, operation, &workspace_selector);
        let payload =
            encode_request_payload_v1(&probe).expect("normal-writer probe must encode for IPC");
        let mut client = UnixStream::connect(&socket_path)
            .expect("client must connect for every normal-writer IPC probe");
        write_frame_v1(&mut client, &payload)
            .expect("every normal writer must cross the framed daemon IPC boundary");
        client
            .shutdown(Shutdown::Write)
            .expect("client must half-close every normal-writer IPC probe");
        let response_payload = read_single_frame_v1(&mut client)
            .expect("daemon must answer every normal-writer IPC probe")
            .expect("daemon must return a normal-writer response frame");
        assert!(
            matches!(
                decode_response_payload_v1(&response_payload)
                    .expect("normal-writer response must satisfy the public protocol"),
                ResponseEnvelopeV1::Output(_) | ResponseEnvelopeV1::Error(_)
            ),
            "{route} must be handled by the production daemon authority"
        );
    }
    assert!(
        fixture
            .main()
            .join(".podway/runtime/state.sqlite3")
            .is_file(),
        "the daemon-created Store is owned below the workspace runtime directory"
    );

    shutdown.request_shutdown();
    assert!(
        server
            .join()
            .expect("daemon thread must not panic")
            .expect("daemon shutdown must succeed")
            .recovered_workspace_count()
            == 0
    );
    assert!(
        !socket_path.exists(),
        "owned socket must be removed at shutdown"
    );

    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    let events = sink
        .events
        .lock()
        .expect("observability event lock must not be poisoned");
    let actual_inventory = events.iter().fold(BTreeMap::new(), |mut inventory, event| {
        *inventory.entry(event.as_str()).or_insert(0_usize) += 1;
        inventory
    });
    let expected_inventory = BTreeMap::from([
        (
            "ts=42 operation=connection_accepted outcome=succeeded\n",
            expected_connection_attempts,
        ),
        ("ts=42 operation=daemon_start outcome=succeeded\n", 1),
        ("ts=42 operation=daemon_stop outcome=succeeded\n", 1),
        ("ts=42 operation=integrity_check outcome=succeeded\n", 1),
        ("ts=42 operation=job_admission outcome=succeeded\n", 2),
        ("ts=42 operation=artifact_move outcome=succeeded\n", 3),
        ("ts=42 operation=job_claim outcome=succeeded\n", 2),
        ("ts=42 operation=job_claim outcome=rejected\n", 2),
        ("ts=42 operation=job_wait outcome=succeeded\n", 2),
        ("ts=42 operation=job_terminal outcome=succeeded\n", 2),
        ("ts=42 operation=scheduler_created outcome=succeeded\n", 1),
        ("ts=42 operation=service_dispatch outcome=rejected\n", 2),
        ("ts=42 operation=service_dispatch outcome=succeeded\n", 2),
        (
            "ts=42 operation=transport_service_request outcome=rejected\n",
            expected_transport_rejections,
        ),
    ]);
    for (event, actual_count) in &actual_inventory {
        assert!(
            expected_inventory
                .get(event)
                .is_some_and(|expected_count| actual_count <= expected_count),
            "the production trace contains an unexpected operation, outcome, or excess event: {event}"
        );
    }
    let expected_total = expected_inventory.values().sum::<usize>();
    let actual_total = actual_inventory.values().sum::<usize>();
    let counters = report.counters();
    let accounted_drops = counters
        .primary_dropped
        .saturating_add(counters.fallback_dropped);
    assert_eq!(
        expected_total.saturating_sub(actual_total),
        usize::try_from(accounted_drops).expect("bounded event count fits usize"),
        "every omitted event in the closed inventory must be explicitly counted as a non-blocking observability drop"
    );
    assert_eq!(
        counters.accepted.saturating_add(accounted_drops),
        u64::try_from(expected_total).expect("bounded event count fits u64"),
        "emitted and explicitly dropped events must exactly cover the closed inventory"
    );
    assert_eq!(
        counters.written,
        u64::try_from(actual_total).expect("bounded event count fits u64"),
        "every accepted event must be written to the sink"
    );
    assert_eq!(
        counters.written, counters.accepted,
        "successful fixture writes must exactly cover accepted events"
    );
    assert_eq!(counters.stopped_dropped, 0);
    assert_eq!(counters.degraded_dropped, 0);
    assert_eq!(counters.unflushed, 0);
    assert_eq!(counters.queued, 0);
    assert_eq!(counters.writing, 0);
    assert_eq!(counters.flushing, 0);
    assert_eq!(counters.write_failures, 0);
    assert_eq!(counters.flush_failures, 0);
    assert_eq!(counters.clock_errors, 0);
    assert_eq!(counters.clock_panics, 0);
    assert_eq!(counters.sink_failures, 0);
    assert!(!counters.counters_saturated);
}

#[test]
fn readonly_resolution_reuses_only_the_exact_active_context_without_registry_refresh() {
    let fixture = git_worktrees();
    make_workspace_runtime_private(fixture.main());
    let paths = runtime_paths(fixture.temporary_path());
    let manager = WorkspaceRuntimeManagerV1::new(
        paths.paths(),
        SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid"),
    );
    let scheduler = manager
        .bootstrap(git_selector(fixture.main()), observation())
        .expect("workspace bootstrap must succeed");
    let context = scheduler.context_snapshot();
    let workspace_id = context.binding().identity().workspace_uuid().clone();
    let before = manager
        .registry()
        .lookup(&workspace_id)
        .expect("registry must be readable")
        .expect("bootstrap must publish registry metadata");
    let resolution = manager
        .resolve_existing_readonly(git_selector(fixture.main()), Some(&workspace_id))
        .expect("read-only resolution must validate the existing workspace");
    let active = resolution
        .active_scheduler()
        .expect("the exact active context must be reusable");
    assert!(Arc::ptr_eq(active, &scheduler));
    assert_eq!(resolution.binding(), context.binding());
    let after = manager
        .registry()
        .lookup(&workspace_id)
        .expect("registry must remain readable")
        .expect("read-only resolution must retain registry metadata");
    assert_eq!(after.last_known_root(), before.last_known_root());
    assert_eq!(after.last_seen_at(), before.last_seen_at());
}
#[test]
fn shutdown_leaves_a_replacement_socket_untouched() {
    let temporary = TemporaryDirectoryV1::new("podway-daemon-runtime-endpoint");
    let paths = runtime_paths(temporary.path());
    let runtime = ProductionDaemonRuntimeV1::bind(
        paths.paths(),
        SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid"),
        configuration("replacement-worker"),
    )
    .expect("daemon must bind an empty registry");
    let socket_path = runtime.socket_path().to_path_buf();
    fs::remove_file(&socket_path).expect("test must replace the owned socket path");
    let replacement = UnixListener::bind(&socket_path).expect("replacement socket must bind");

    let shutdown = runtime.shutdown_handle();
    shutdown.request_shutdown();
    runtime.run().expect("runtime must stop cleanly");
    assert!(
        socket_path.exists(),
        "endpoint shutdown must not unlink a replacement socket"
    );
    drop(replacement);
    fs::remove_file(socket_path).expect("test replacement socket must be removed");
}

#[test]
fn active_move_rebind_recovers_an_exact_store_registry_split_idempotently() {
    let fixture = git_worktrees();
    make_workspace_runtime_private(fixture.main());
    let paths = runtime_paths(fixture.temporary_path());
    let manager = WorkspaceRuntimeManagerV1::new(
        paths.paths(),
        SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid"),
    );
    let initial = manager
        .bootstrap(git_selector(fixture.main()), observation())
        .expect("main worktree must bootstrap");
    let initial_context = initial.context_snapshot();
    let workspace_uuid = initial_context
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let initial_key = initial.key().clone();
    let initial_generation = initial.generation();
    let original_root = initial_context.workspace_root().to_path_buf();
    let relocated = fixture.temporary_path().join("relocated-main");
    fs::rename(fixture.main(), &relocated).expect("real Git worktree must move atomically");
    let canonical_relocated =
        fs::canonicalize(&relocated).expect("relocated worktree must canonicalize");
    let relocated_root = ValidatedWorkspaceRootV1::from_path(&canonical_relocated)
        .expect("canonical relocated root must be Store-valid");
    let relocated_database = relocated.join(".podway/runtime/state.sqlite3");

    // Simulate the durable half of a prior move activation that completed before its metadata
    // publication. The retry below must only repair metadata from the validated Store/Git state.
    let partial_store = SqliteStoreV1::open(
        &relocated_database,
        &relocated_root,
        initial_context.binding().identity().clone(),
        initial_context.store_options().clone(),
        observation().store_now(),
    )
    .expect("Store root update must establish the deterministic partial move state");
    drop(partial_store);
    let binding_after_partial = SqliteStoreV1::inspect_workspace_binding(
        &relocated_database,
        initial_context.store_options(),
    )
    .expect("partial Store binding remains inspectable")
    .expect("partial Store binding remains present");
    assert_eq!(binding_after_partial.last_validated_root(), &relocated_root);
    assert_eq!(
        manager
            .registry()
            .lookup(&workspace_uuid)
            .expect("registry remains readable")
            .expect("bootstrap metadata remains registered")
            .last_known_root()
            .to_path_buf(),
        original_root,
        "the deterministic partial state leaves only registry metadata at the prior root"
    );

    let rebound = manager
        .resolve_existing(
            git_selector(&relocated),
            Some(&workspace_uuid),
            observation(),
        )
        .expect("validated move retry must reconcile only the stale registry metadata");
    let rebound_context = rebound.context_snapshot();
    assert!(Arc::ptr_eq(&initial, &rebound));
    assert_eq!(rebound.key(), &initial_key);
    assert_eq!(rebound.generation(), initial_generation);
    assert_eq!(
        rebound_context.workspace_root().to_path_buf(),
        canonical_relocated
    );
    assert_eq!(
        manager
            .registry()
            .lookup(&workspace_uuid)
            .expect("reconciled registry remains readable")
            .expect("reconciled metadata remains registered")
            .last_known_root()
            .to_path_buf(),
        canonical_relocated
    );

    let retried = manager
        .resolve_existing(
            git_selector(&relocated),
            Some(&workspace_uuid),
            observation(),
        )
        .expect("the recovered stationary binding must remain idempotently available");
    assert!(Arc::ptr_eq(&rebound, &retried));
    assert_eq!(retried.generation(), initial_generation);
}
#[test]
fn pac037_deleted_registered_worktree_is_unavailable_without_adoption() {
    let fixture = git_worktrees();
    let direct = NativeGitResolverV1::new()
        .resolve(git_selector(fixture.main()))
        .expect("fixture main worktree must resolve natively");
    assert_eq!(
        direct
            .roots()
            .worktree_root()
            .decode_path_bytes()
            .expect("native root bytes must decode"),
        fs::canonicalize(fixture.main())
            .expect("fixture main path must canonicalize")
            .as_os_str()
            .as_bytes()
    );
    assert!(
        non_utf8_child_path(fixture.temporary_path())
            .as_os_str()
            .as_bytes()
            .ends_with(&[0xff]),
        "lossless fixture must retain the non-UTF-8 path byte"
    );
    make_workspace_runtime_private(fixture.main());
    make_workspace_runtime_private(fixture.linked());
    let linked_selector = selector(fixture.linked());
    let paths = runtime_paths(fixture.temporary_path());
    let options = SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid");
    let manager = Arc::new(WorkspaceRuntimeManagerV1::new(
        paths.paths(),
        options.clone(),
    ));
    let workspace_runtime = ProductionWorkspaceRuntimeV1::new(
        Arc::clone(&manager),
        Arc::new(NativeProductionClockV1::default()),
    );
    let main = workspace_runtime
        .resolve_bootstrap(&selector(fixture.main()))
        .expect("main worktree must bootstrap");
    let linked = workspace_runtime
        .resolve_bootstrap(&linked_selector)
        .expect("linked worktree must bootstrap");
    assert!(
        read_file(&fixture.main().join(".podway/config.yaml")).starts_with(b"schema:"),
        "bootstrap must publish the admitted workspace configuration"
    );
    let copied = fixture.temporary_path().join("copied-linked-worktree");
    copy_tree(fixture.linked(), &copied);
    let copied_error = match workspace_runtime.resolve_existing(&selector(&copied)) {
        Ok(_) => panic!("a copied worktree must not adopt the registered durable identity"),
        Err(error) => error,
    };
    assert_eq!(
        copied_error.kind(),
        DispatchFailureKindV1::DaemonUnavailable,
        "a copied linked worktree with stale Git administration must remain unavailable rather than adopting the durable identity: {copied_error:?}"
    );
    let main_uuid = main
        .scheduler()
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let linked_context = linked.scheduler().context_snapshot();
    let linked_identity = linked_context.binding().identity().clone();
    let linked_uuid = linked_identity.workspace_uuid().clone();
    let linked_database = linked_context.database_path().to_path_buf();
    let linked_runtime_directory = linked_context.runtime_directory_path().to_path_buf();
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("deleted-worktree-seed-worker").expect("fixture worker ID must be valid"),
    );
    let (seed_task_envelope, seed_task) = request_with_payload(
        37,
        "session.start",
        OperationV1::Mutate,
        &linked_selector,
        serde_json::json!({
            "selector": linked_selector.clone(),
            "preset": "sw-dev",
            "task_title": "PAC-037 deleted worktree seed",
        })
        .as_object()
        .expect("seed task payload must be an object")
        .clone(),
    );
    let seeded_response = dispatcher.dispatch(&seed_task_envelope, &seed_task);
    let ResponseEnvelopeV1::Output(seeded_output) = seeded_response else {
        panic!("the registered worktree must admit a task/session through the runtime API");
    };
    assert!(
        seeded_output.job().is_some(),
        "the admitted task must have a durable runtime job before deletion",
    );
    let seeded_session = seeded_output
        .session()
        .expect("the seeded task must project its durable session identity");
    let seeded_session_id = seeded_session.id().clone();
    let seeded_session_revision = seeded_session.revision_after();
    let (detached_envelope, detached_request) = request_with_configuration(
        38,
        "session.reset",
        OperationV1::Mutate,
        &linked_selector,
        serde_json::json!({
            "selector": linked_selector.clone(),
            "confirmed": true,
        })
        .as_object()
        .expect("detached reset payload must be an object")
        .clone(),
        PreconditionsV1::new(
            Some(seeded_session_id),
            Some(seeded_session_revision),
            None,
            None,
            None,
            None,
        )
        .expect("reset identity precondition must be valid"),
        RequestOptionsV1::new(true, 5_000).expect("detached request options must be valid"),
    );
    let detached_response = dispatcher.dispatch(&detached_envelope, &detached_request);
    let ResponseEnvelopeV1::Output(detached_output) = detached_response else {
        panic!("a distinct detached mutation must be durably admitted before deletion");
    };
    let detached_job = detached_output
        .job()
        .expect("a detached mutation must expose its exact durable job")
        .clone();
    assert_eq!(
        detached_job.state(),
        JobStateV1::Queued,
        "the detached job must remain exactly queued at the deletion boundary"
    );
    let detached_job_id = detached_job.id().clone();
    assert!(
        linked_database.exists(),
        "the linked worktree must own local SQLite state before deletion"
    );
    assert!(
        linked_runtime_directory.join("state.sqlite3").exists(),
        "the linked runtime sidecar directory must exist before deletion"
    );
    drop(linked_context);
    drop(linked);
    drop(main);
    drop(workspace_runtime);
    drop(manager);
    fs::remove_dir_all(fixture.linked()).expect("linked worktree must be deleted before recovery");

    let runtime =
        ProductionDaemonRuntimeV1::bind(paths.paths(), options, configuration("isolation-worker"))
            .expect("a deleted registry entry must not block recovery of the valid worktree");
    assert_eq!(runtime.recovery_report().recovered_workspace_count(), 1);
    assert_eq!(runtime.recovery_report().unavailable_workspace_count(), 1);
    assert!(runtime.recovery_report().workspaces().iter().any(|entry| {
        matches!(
            entry,
            WorkspaceRecoveryEntryV1::Recovered(report)
                if report.workspace_uuid() == &main_uuid
        )
    }));
    assert!(runtime.recovery_report().workspaces().iter().any(|entry| {
        matches!(
            entry,
            WorkspaceRecoveryEntryV1::Unavailable(report)
                if report.workspace_uuid() == &linked_uuid
                    && report.reason() == WorkspaceRecoveryUnavailableReasonV1::WorktreeGone
        )
    }));
    assert!(
        !linked_database.exists()
            && !linked_database.with_extension("sqlite3-wal").exists()
            && !linked_database.with_extension("sqlite3-shm").exists()
            && !linked_runtime_directory.exists(),
        "deleting the registered worktree must remove its local SQLite database and sidecars"
    );
    let socket_path = runtime.socket_path().to_path_buf();
    let shutdown = runtime.shutdown_handle();
    let server = thread::spawn(move || runtime.run());
    for (request_number, command, payload) in [
        (
            40,
            "job.list",
            serde_json::json!({"selector": linked_selector.clone()}),
        ),
        (
            41,
            "job.status",
            serde_json::json!({
                "selector": linked_selector.clone(),
                "job_id": detached_job_id.as_str(),
            }),
        ),
        (
            42,
            "job.wait",
            serde_json::json!({
                "selector": linked_selector.clone(),
                "job_id": detached_job_id.as_str(),
            }),
        ),
    ] {
        let request = request_with_payload(
            request_number,
            command,
            OperationV1::Query,
            &linked_selector,
            payload
                .as_object()
                .expect("job-specific public payload must be an object")
                .clone(),
        );
        let payload = encode_request_payload_v1(&request.0)
            .expect("job-specific request must encode for framed IPC");
        let mut client = UnixStream::connect(&socket_path)
            .expect("client must connect for a job-specific unavailability probe");
        write_frame_v1(&mut client, &payload)
            .expect("client must write framed job-specific request");
        client
            .shutdown(Shutdown::Write)
            .expect("client must half-close job-specific request");
        let response_payload = read_single_frame_v1(&mut client)
            .expect("daemon must return one framed job-specific rejection")
            .expect("daemon must return a job-specific rejection frame");
        let response = decode_response_payload_v1(&response_payload)
            .expect("job-specific rejection must satisfy the public protocol");
        let ResponseEnvelopeV1::Error(error) = response else {
            panic!("{command} must not expose the deleted worktree's detached job");
        };
        assert_eq!(
            error.code().as_str(),
            "DAEMON_UNAVAILABLE",
            "{command} must make the exact deleted-worktree job unavailable"
        );
    }
    for (request_number, rejected_selector, expected_code) in [
        (38, linked_selector.clone(), "DAEMON_UNAVAILABLE"),
        (39, selector(&copied), "DAEMON_UNAVAILABLE"),
    ] {
        let status = request(
            request_number,
            "session.status",
            OperationV1::Query,
            &rejected_selector,
        );
        let payload = encode_request_payload_v1(&status.0).expect("status request must encode");
        let mut client =
            UnixStream::connect(&socket_path).expect("client must connect to the running daemon");
        write_frame_v1(&mut client, &payload).expect("client must write framed status request");
        client
            .shutdown(Shutdown::Write)
            .expect("client must half-close status request");
        let response_payload = read_single_frame_v1(&mut client)
            .expect("daemon must return one framed rejection")
            .expect("daemon must return a rejection frame");
        let response = decode_response_payload_v1(&response_payload)
            .expect("daemon rejection must satisfy the public protocol");
        let ResponseEnvelopeV1::Error(error) = response else {
            panic!("unavailable or copied worktrees must not expose task/job state");
        };
        assert_eq!(error.code().as_str(), expected_code);
    }
    for (request_number, rejected_selector, expected_code) in [
        (43, linked_selector, "DAEMON_UNAVAILABLE"),
        (44, selector(&copied), "DAEMON_UNAVAILABLE"),
    ] {
        let start = request_with_payload(
            request_number,
            "session.start",
            OperationV1::Mutate,
            &rejected_selector,
            serde_json::json!({
                "selector": rejected_selector,
                "preset": "sw-dev",
                "task_title": "must not execute after identity loss",
            })
            .as_object()
            .expect("rejection mutation payload must be an object")
            .clone(),
        );
        let payload = encode_request_payload_v1(&start.0)
            .expect("rejection mutation must encode for framed IPC");
        let mut client = UnixStream::connect(&socket_path)
            .expect("client must connect for a deleted-identity mutation probe");
        write_frame_v1(&mut client, &payload)
            .expect("client must write framed deleted-identity mutation probe");
        client
            .shutdown(Shutdown::Write)
            .expect("client must half-close deleted-identity mutation probe");
        let response_payload = read_single_frame_v1(&mut client)
            .expect("daemon must return one deleted-identity mutation rejection")
            .expect("daemon must return a deleted-identity mutation rejection frame");
        let response = decode_response_payload_v1(&response_payload)
            .expect("deleted-identity mutation rejection must satisfy the public protocol");
        let ResponseEnvelopeV1::Error(error) = response else {
            panic!("deleted or copied identities must not admit, claim, or execute a mutation");
        };
        assert_eq!(error.code().as_str(), expected_code);
    }
    assert!(
        !linked_database.exists() && !linked_runtime_directory.exists(),
        "rejected deleted-identity mutations must not recreate Store state or side effects"
    );

    shutdown.request_shutdown();
    server
        .join()
        .expect("daemon thread must not panic")
        .expect("runtime cleanup must succeed");
}
