use std::{
    fs,
    io::Write,
    num::NonZeroUsize,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use nix::unistd::geteuid;
use podway_core::{RuntimeModeV1, UnixMillis};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2, Rfc3339MillisV1,
    SliceRequestV1, decode_response_payload_v2, encode_request_payload_v1, read_frame_v1,
    write_frame_v1,
};
use podway_service::{
    LaunchctlOutputV1, LaunchctlRunnerV1, RuntimeResetErrorV1, RuntimeResetInspectorV1,
    RuntimeResetLockV1, RuntimeResetOperationV1, RuntimeResetPathV1, RuntimeResetPeerV1,
    RuntimeResetPlannerV1, RuntimeResetProcessV1, RuntimeResetSelectionV1, ServiceErrorV1,
    ServiceRuntimePathsV1,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::RuntimeResetControllerV1;
use crate::{
    endpoint::SingletonEndpointV1,
    peer::{NativePeerCredentialSourceV1, PeerUidVerifierV1},
    runtime_workspace::WorkspaceRuntimeManagerV1,
    server::{
        DaemonProcessIdentityV1, DaemonReadinessV1, DaemonRequestV1, RequestDispatcherV1,
        ServerConnectionErrorV1, ServerTransportTimeoutsV1, ShutdownAdmissionOutcomeV1,
        ShutdownAdmissionV1, UnixServerTransportV1,
    },
};

struct NoDispatcher;
impl RequestDispatcherV1 for NoDispatcher {
    fn dispatch(&self, _: &RequestEnvelopeV1, _: &SliceRequestV1) -> ResponseEnvelopeV2 {
        panic!("reset controls do not reach normal dispatch")
    }
    fn dispatch_daemon(&self, _: &RequestEnvelopeV1, _: &DaemonRequestV1) -> ResponseEnvelopeV2 {
        panic!("reserved normal requests do not reach dispatch")
    }
}

type Transport = UnixServerTransportV1<NativePeerCredentialSourceV1, NoDispatcher>;

struct Fixture {
    root: PathBuf,
    paths: ServiceRuntimePathsV1,
    transport: Arc<Transport>,
    _endpoint: crate::endpoint::SingletonEndpointGuardV1,
}

impl Fixture {
    fn new() -> Self {
        let root = Path::new("/private/tmp").join(format!("pw-rc-{}", Uuid::new_v4().simple()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let paths = ServiceRuntimePathsV1::for_account_home_mode(
            &root,
            RuntimeModeV1::development(),
            geteuid().as_raw(),
        )
        .unwrap();
        let endpoint = SingletonEndpointV1::acquire(&paths).unwrap();
        let manager =
            Arc::new(WorkspaceRuntimeManagerV1::with_isolated_maintenance_for_tests(&paths));
        assert!(manager.daemon_activity_counts(UnixMillis::new(1)).is_some());
        let identity = DaemonProcessIdentityV1::new(
            RequestIdV1::new(Uuid::new_v4().to_string()).unwrap(),
            std::process::id(),
            Rfc3339MillisV1::from_unix_millis(1_000).unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap(),
            paths.socket_path().as_path(),
            paths.socket_path().as_path(),
        )
        .unwrap()
        .with_runtime_mode(paths.mode().clone());
        let readiness = DaemonReadinessV1::new();
        readiness.mark_ready();
        let admission = ShutdownAdmissionV1::new();
        let reset =
            RuntimeResetControllerV1::new(&paths, identity.clone(), manager, admission, readiness)
                .unwrap();
        let transport = Arc::new(
            Transport::new(
                PeerUidVerifierV1::for_current_user(),
                NoDispatcher,
                ServerTransportTimeoutsV1::default(),
            )
            .with_process_identity(identity)
            .with_runtime_reset(reset),
        );
        Self {
            root,
            paths,
            transport,
            _endpoint: endpoint,
        }
    }

    fn external_managed() -> Self {
        let root = Path::new("/private/tmp").join(format!("pw-rc-{}", Uuid::new_v4().simple()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for directory in ["LaunchAgents", "state", "logs", "run"] {
            let path = root.join(directory);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let paths = ServiceRuntimePathsV1::from_directories(
            root.join("LaunchAgents"),
            root.join("state"),
            root.join("logs"),
            root.join("run"),
        )
        .unwrap();
        assert!(paths.ordinary_account_home().unwrap().is_none());
        let endpoint = SingletonEndpointV1::acquire(&paths).unwrap();
        let manager =
            Arc::new(WorkspaceRuntimeManagerV1::with_isolated_maintenance_for_tests(&paths));
        let identity = DaemonProcessIdentityV1::new(
            RequestIdV1::new(Uuid::new_v4().to_string()).unwrap(),
            std::process::id(),
            Rfc3339MillisV1::from_unix_millis(1_000).unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap(),
            paths.socket_path().as_path(),
            paths.socket_path().as_path(),
        )
        .unwrap()
        .with_runtime_mode(paths.mode().clone());
        let readiness = DaemonReadinessV1::new();
        readiness.mark_ready();
        let admission = ShutdownAdmissionV1::new();
        let reset =
            RuntimeResetControllerV1::new(&paths, identity.clone(), manager, admission, readiness)
                .unwrap();
        let transport = Arc::new(
            Transport::new(
                PeerUidVerifierV1::for_current_user(),
                NoDispatcher,
                ServerTransportTimeoutsV1::default(),
            )
            .with_process_identity(identity)
            .with_runtime_reset(reset),
        );
        Self {
            root,
            paths,
            transport,
            _endpoint: endpoint,
        }
    }

    fn controller(&self) -> &RuntimeResetControllerV1 {
        self.transport.runtime_reset.as_ref().unwrap()
    }

    fn request(
        &self,
        action: &str,
        reservation: Option<&str>,
        operation: Option<&RuntimeResetOperationV1>,
    ) -> RequestEnvelopeV1 {
        RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
            request_id: RequestIdV1::new(Uuid::new_v4().to_string()).unwrap(),
            client: ClientInfoV1::new("podway", env!("CARGO_PKG_VERSION"), std::process::id()).unwrap(),
            operation: OperationV1::Control, command: CommandNameV1::new("daemon.runtime_reset").unwrap(),
            workspace: None, idempotency_key: None, preconditions: PreconditionsV1::default(), options: RequestOptionsV1::new(false, 0).unwrap(),
            payload: serde_json::json!({
                "schema": "podway.runtime-reset-control-input/v1", "action": action,
                "mode": self.paths.mode(), "namespace_root": self.controller().root,
                "expected_process_id": self.controller().process.process_id(),
                "operation_id": operation.map(|operation| &operation.operation_id),
                "token_sha256": operation.map(|operation| &operation.token_sha256), "reservation_id": reservation,
            }).as_object().unwrap().clone(),
        }).unwrap()
    }

    fn connection(&self) -> (UnixStream, JoinHandle<Result<(), ServerConnectionErrorV1>>) {
        let (client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let ticket = match self
            .controller()
            .admission
            .try_admit(NonZeroUsize::new(16).unwrap())
            .unwrap()
        {
            ShutdownAdmissionOutcomeV1::Admitted(ticket) => ticket,
            _ => panic!("test connection is admitted"),
        };
        let transport = Arc::clone(&self.transport);
        let handle = thread::spawn(move || {
            let _ticket = ticket;
            transport.handle_connection(server)
        });
        (client, handle)
    }

    fn plan(&self) -> (Value, RuntimeResetOperationV1) {
        struct Inspector(RuntimeResetProcessV1);
        impl RuntimeResetInspectorV1 for Inspector {
            fn inspect(
                &mut self,
                _: &ServiceRuntimePathsV1,
            ) -> Result<RuntimeResetPeerV1, RuntimeResetErrorV1> {
                Ok(RuntimeResetPeerV1::Live {
                    process: self.0.clone(),
                    busy_reason: None,
                })
            }
        }
        struct Launchctl;
        impl LaunchctlRunnerV1 for Launchctl {
            fn run(&self, _: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
                panic!("named mode never uses launchctl")
            }
        }
        let identity = &self.controller().process;
        let mut planner = RuntimeResetPlannerV1::new(
            self.paths.ordinary_account_home().unwrap().unwrap(),
            Launchctl,
            Inspector(RuntimeResetProcessV1 {
                pid: identity.pid(),
                process_id: identity.process_id().as_str().to_owned(),
                executable: RuntimeResetPathV1::new(identity.executable_path()).unwrap(),
                started_at_ms: 1_000,
            }),
        );
        let plan = planner
            .plan(
                RuntimeResetSelectionV1::Mode {
                    mode: self.paths.mode().clone(),
                },
                UnixMillis::new(2_000),
            )
            .unwrap();
        let token = plan.plan_token.expect("live idle participant has a token");
        let bytes = podway_protocol::decode_base64url_unpadded_v1(token.split_once('.').unwrap().0)
            .unwrap();
        let operation = RuntimeResetOperationV1 {
            operation_id: Uuid::new_v4().to_string(),
            token_sha256: format!("sha256:{:x}", Sha256::digest(token.as_bytes())),
        };
        (serde_json::from_slice(&bytes).unwrap(), operation)
    }

    fn record(
        &self,
        plan: &Value,
        operation: &RuntimeResetOperationV1,
        reservation: &str,
        phase: &str,
    ) {
        let record = serde_json::json!({
            "schema": "podway.runtime-reset-record/v1", "operation_id": operation.operation_id,
            "token_sha256": operation.token_sha256, "phase": "in_progress", "plan": plan,
            "modes": [{"mode": self.paths.mode(), "phase": phase, "reservation_id": reservation, "entries": []}], "result": null,
        });
        let path = self.root.join(".podway/maintenance/runtime-reset.json");
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::File::open(path).unwrap().sync_all().unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn exchange(stream: &mut UnixStream, request: &RequestEnvelopeV1) -> ResponseEnvelopeV2 {
    write_frame_v1(stream, &encode_request_payload_v1(request).unwrap()).unwrap();
    decode_response_payload_v2(&read_frame_v1(stream).unwrap().expect("one response")).unwrap()
}

fn result(response: ResponseEnvelopeV2) -> Map<String, Value> {
    match response {
        ResponseEnvelopeV2::OutputV2(output) => output.result().clone(),
        other => panic!("expected reset result: {other:?}"),
    }
}

#[test]
fn idle_reservation_closes_admission_and_disconnect_reopens_only_before_commit() {
    let fixture = Fixture::new();
    let (plan, operation) = fixture.plan();
    let (mut stream, handler) = fixture.connection();
    let reserved = result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    assert_eq!(reserved["state"], "reserved");
    let id = reserved["reservation_id"].as_str().unwrap();
    assert!(!fixture.controller().normal_admission_open());
    assert!(
        fixture
            .controller()
            .manager
            .reserve_runtime_reset(UnixMillis::new(3_000))
            .is_err()
    );
    let (mut normal, normal_handler) = fixture.connection();
    let mut request = serde_json::to_value(fixture.request("inspect", None, None)).unwrap();
    request["command"] = serde_json::json!("session.begin");
    request["operation"] = serde_json::json!("mutate");
    request["workspace"] = serde_json::json!({ "root": fixture.root });
    request["idempotency_key"] = serde_json::json!(Uuid::new_v4().to_string());
    request["payload"] = serde_json::json!({});
    let request = serde_json::from_value(request).unwrap();
    write_frame_v1(&mut normal, &encode_request_payload_v1(&request).unwrap()).unwrap();
    normal.shutdown(std::net::Shutdown::Write).unwrap();
    let refusal =
        decode_response_payload_v2(&read_frame_v1(&mut normal).unwrap().unwrap()).unwrap();
    assert!(
        matches!(refusal, ResponseEnvelopeV2::Error(error) if error.code().as_str() == "DAEMON_SHUTTING_DOWN" && error.details()["admission"]["admitted"] == false)
    );
    normal_handler.join().unwrap().unwrap();
    let snapshot = result(exchange(
        &mut stream,
        &fixture.request("snapshot", Some(id), Some(&operation)),
    ));
    assert_eq!(snapshot["state"], "reserved");
    let release = result(exchange(
        &mut stream,
        &fixture.request("release", Some(id), Some(&operation)),
    ));
    assert_eq!(release["state"], "released");
    handler.join().unwrap().unwrap();
    assert!(fixture.controller().normal_admission_open());

    let (mut stream, handler) = fixture.connection();
    result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    drop(stream);
    handler.join().unwrap().unwrap();
    assert!(fixture.controller().normal_admission_open());

    let (mut stream, handler) = fixture.connection();
    let reserved = result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    let id = reserved["reservation_id"].as_str().unwrap();
    let home = fixture.paths.ordinary_account_home().unwrap().unwrap();
    let commit = RuntimeResetLockV1::acquire_commit(&home).unwrap();
    // This exchange must finish while the coordinator holds the commit interlock.
    assert_eq!(
        result(exchange(
            &mut stream,
            &fixture.request("snapshot", Some(id), Some(&operation))
        ))["state"],
        "reserved"
    );
    fixture.record(&plan, &operation, id, "pending");
    drop(stream);
    drop(commit);
    handler.join().unwrap().unwrap();
    assert!(!fixture.controller().normal_admission_open());

    let (mut stream, handler) = fixture.connection();
    let busy = result(exchange(
        &mut stream,
        &fixture.request("inspect", None, None),
    ));
    assert_eq!(busy["state"], "busy");
    assert_eq!(busy["reason"], "operation_in_progress");
    assert!(busy["reservation_id"].is_null());
    handler.join().unwrap().unwrap();

    let (mut stream, handler) = fixture.connection();
    let recovered = result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    assert_eq!(recovered["state"], "committed");
    assert_eq!(recovered["reservation_id"], id);
    assert_eq!(
        result(exchange(
            &mut stream,
            &fixture.request("release", Some(id), Some(&operation))
        ))["state"],
        "committed"
    );
    handler.join().unwrap().unwrap();
    assert!(!fixture.controller().normal_admission_open());

    let (mut stream, handler) = fixture.connection();
    result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    let refusal = exchange(
        &mut stream,
        &fixture.request("shutdown", Some(id), Some(&operation)),
    );
    assert!(
        matches!(refusal, ResponseEnvelopeV2::Error(error) if error.details()["reason"] == "missing_intent")
    );
    handler.join().unwrap().unwrap();
    assert!(fixture.controller().admission.is_accepting());
    fixture.record(&plan, &operation, id, "stop_intended");
    let (mut stream, handler) = fixture.connection();
    result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    assert_eq!(
        result(exchange(
            &mut stream,
            &fixture.request("shutdown", Some(id), Some(&operation))
        ))["state"],
        "shutting_down"
    );
    handler.join().unwrap().unwrap();
    assert!(!fixture.controller().admission.is_accepting());
}

#[test]
fn external_managed_controller_reports_unsupported_without_closing_admission() {
    let fixture = Fixture::external_managed();
    let (mut stream, handler) = fixture.connection();
    let unsupported = result(exchange(
        &mut stream,
        &fixture.request("inspect", None, None),
    ));
    assert_eq!(unsupported["state"], "unsupported");
    assert_eq!(unsupported["reason"], "managed_runtime");
    assert!(unsupported["reservation_id"].is_null());
    handler.join().unwrap().unwrap();
    assert!(fixture.controller().normal_admission_open());
}

#[test]
fn control_refuses_other_clients_recovery_stale_identity_and_pipelining() {
    let fixture = Fixture::new();
    let (_, operation) = fixture.plan();
    let (mut other, other_handler) = fixture.connection();
    let (mut stream, handler) = fixture.connection();
    let busy = result(exchange(
        &mut stream,
        &fixture.request("inspect", None, None),
    ));
    assert_eq!(busy["state"], "busy");
    assert_eq!(busy["reason"], "activity");
    handler.join().unwrap().unwrap();
    let (mut stream, handler) = fixture.connection();
    assert_eq!(
        result(exchange(
            &mut stream,
            &fixture.request("reserve", None, Some(&operation))
        ))["state"],
        "busy"
    );
    handler.join().unwrap().unwrap();
    assert!(fixture.controller().normal_admission_open());
    other.shutdown(std::net::Shutdown::Write).unwrap();
    assert!(read_frame_v1(&mut other).unwrap().is_some());
    other_handler.join().unwrap().unwrap();

    fixture.controller().readiness.begin_worktree_recovery(1);
    fixture
        .controller()
        .readiness
        .record_worktree_recovery(true);
    fixture.controller().readiness.mark_ready();
    let (mut stream, handler) = fixture.connection();
    let busy = result(exchange(
        &mut stream,
        &fixture.request("inspect", None, None),
    ));
    assert_eq!(busy["reason"], "recovery");
    handler.join().unwrap().unwrap();

    let fixture = Fixture::new();
    for (field, value) in [
        (
            "expected_process_id",
            serde_json::json!(Uuid::new_v4().to_string()),
        ),
        (
            "namespace_root",
            serde_json::json!(RuntimeResetPathV1::new(Path::new("/other")).unwrap()),
        ),
        ("mode", serde_json::json!("prod")),
    ] {
        let request = fixture.request("reserve", None, Some(&operation));
        let mut wire = serde_json::to_value(request).unwrap();
        wire["payload"][field] = value;
        let request = serde_json::from_value(wire).unwrap();
        let (mut stream, handler) = fixture.connection();
        assert!(
            matches!(exchange(&mut stream, &request), ResponseEnvelopeV2::Error(error) if error.details()["reason"] == "identity_changed")
        );
        handler.join().unwrap().unwrap();
        assert!(fixture.controller().normal_admission_open());
    }
    let (mut stream, handler) = fixture.connection();
    let frame = podway_protocol::encode_frame_v1(
        &encode_request_payload_v1(&fixture.request("reserve", None, Some(&operation))).unwrap(),
    )
    .unwrap();
    let mut pipelined = frame.clone();
    pipelined.extend(frame);
    stream.write_all(&pipelined).unwrap();
    let response =
        decode_response_payload_v2(&read_frame_v1(&mut stream).unwrap().unwrap()).unwrap();
    assert!(
        matches!(response, ResponseEnvelopeV2::Error(error) if error.code().as_str() == "REQUEST_INVALID")
    );
    handler.join().unwrap().unwrap();
    assert!(fixture.controller().normal_admission_open());
}

#[test]
fn control_exchange_has_eight_pairs_and_lost_reservations_cannot_be_reused() {
    let fixture = Fixture::new();
    let (_, operation) = fixture.plan();
    let (mut stream, handler) = fixture.connection();
    let reserved = result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    let id = reserved["reservation_id"].as_str().unwrap();
    for _ in 0..7 {
        assert_eq!(
            result(exchange(
                &mut stream,
                &fixture.request("snapshot", Some(id), Some(&operation))
            ))["state"],
            "reserved"
        );
    }
    handler.join().unwrap().unwrap();
    assert!(read_frame_v1(&mut stream).unwrap().is_none());
    assert!(fixture.controller().normal_admission_open());
    let (mut stream, handler) = fixture.connection();
    assert!(
        matches!(exchange(&mut stream, &fixture.request("snapshot", Some(id), Some(&operation))), ResponseEnvelopeV2::Error(error) if error.details()["reason"] == "reservation_lost")
    );
    handler.join().unwrap().unwrap();
}

#[test]
fn unreadable_commit_record_retains_admission_and_startup_fences() {
    let fixture = Fixture::new();
    let (_, operation) = fixture.plan();
    let (mut stream, handler) = fixture.connection();
    result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    let path = fixture.root.join(".podway/maintenance/runtime-reset.json");
    fs::write(&path, b"invalid committed record").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    drop(stream);
    handler.join().unwrap().unwrap();
    assert!(!fixture.controller().normal_admission_open());
    assert!(matches!(
        SingletonEndpointV1::acquire(&fixture.paths),
        Err(crate::endpoint::EndpointErrorV1::RuntimeReset(_))
    ));
}

#[test]
fn competing_start_checks_the_committed_record_after_the_topology_gate() {
    use std::sync::{Barrier, mpsc};
    let fixture = Fixture::new();
    let (plan, operation) = fixture.plan();
    let home = fixture.paths.ordinary_account_home().unwrap().unwrap();
    let gate = RuntimeResetLockV1::acquire_topology(&home).unwrap();
    let start = Arc::new(Barrier::new(2));
    let worker_start = Arc::clone(&start);
    let paths = fixture.paths.clone();
    let (sent, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        worker_start.wait();
        sent.send(SingletonEndpointV1::acquire(&paths)).unwrap();
    });
    start.wait();
    assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
    fixture.record(&plan, &operation, &Uuid::new_v4().to_string(), "pending");
    drop(gate);
    assert!(
        matches!(received.recv_timeout(Duration::from_secs(5)).unwrap(), Err(crate::endpoint::EndpointErrorV1::RuntimeReset(error)) if error.code() == "RUNTIME_RESET_IN_PROGRESS")
    );
    worker.join().unwrap();
}

#[test]
fn control_frame_inactivity_timeout_releases_an_uncommitted_reservation() {
    let fixture = Fixture::new();
    let (_, operation) = fixture.plan();
    let (mut stream, handler) = fixture.connection();
    stream
        .set_read_timeout(Some(Duration::from_secs(45)))
        .unwrap();
    let started = Instant::now();
    result(exchange(
        &mut stream,
        &fixture.request("reserve", None, Some(&operation)),
    ));
    assert!(!fixture.controller().normal_admission_open());
    let response = read_frame_v1(&mut stream).unwrap().unwrap();
    assert!(matches!(
        decode_response_payload_v2(&response).unwrap(),
        ResponseEnvelopeV2::Error(_)
    ));
    handler.join().unwrap().unwrap();
    assert!(started.elapsed() >= Duration::from_secs(29));
    assert!(fixture.controller().normal_admission_open());
}
