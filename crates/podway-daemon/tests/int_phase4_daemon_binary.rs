//! Component-level proof that `podwayd` binds the production endpoint and shuts down cleanly.

#![forbid(unsafe_code)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::{Pid, geteuid},
};
use podway_core::RuntimeModeV1;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    decode_response_payload_v2, encode_request_payload_v1, read_single_frame_v1, write_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::ValidatedWorkspaceRootV1;
use serde_json::{Map, Value};
use uuid::Uuid;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

struct ProcessFixtureV1 {
    root: PathBuf,
    home: PathBuf,
    paths: ServiceRuntimePathsV1,
}

impl ProcessFixtureV1 {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(format!("/tmp/pdb-{}-{sequence}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("h");
        let state_directory = home.join(".podway/state");
        fs::create_dir_all(&state_directory).expect("state directory fixture must exist");
        make_private(&home);
        make_private(home.join(".podway").as_path());
        make_private(&state_directory);
        let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw())
            .expect("short fixture service paths must be valid");
        Self { root, home, paths }
    }

    fn spawn(&self) -> Child {
        Command::new(env!("CARGO_BIN_EXE_podwayd"))
            .args(["--service", "--socket"])
            .arg(self.paths.socket_path().as_path())
            .env_clear()
            .env("PODWAY_TEST_ACCOUNT_ROOT", &self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("podwayd process must start")
    }

    fn mode_paths(&self, mode: &str) -> ServiceRuntimePathsV1 {
        ServiceRuntimePathsV1::for_account_home_mode(
            &self.home,
            RuntimeModeV1::new(mode).unwrap(),
            geteuid().as_raw(),
        )
        .unwrap()
    }

    fn spawn_mode(&self, mode: &str) -> Child {
        Command::new(env!("CARGO_BIN_EXE_podwayd"))
            .args(["--mode", mode])
            .env_clear()
            .env("PODWAY_TEST_ACCOUNT_ROOT", &self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("named podwayd process must start")
    }
}

impl Drop for ProcessFixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn make_private(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("fixture directory must be private");
}

fn status_request(client: ClientInfoV1) -> RequestEnvelopeV1 {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(Uuid::new_v4().to_string()).expect("request UUID"),
        client,
        operation: OperationV1::Control,
        command: CommandNameV1::new("daemon.status").expect("status command"),
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).expect("status options"),
        payload: Map::new(),
    })
    .expect("status request")
}

fn exchange_status(socket: &Path, request: &RequestEnvelopeV1) -> ResponseEnvelopeV2 {
    let payload = encode_request_payload_v1(request).expect("status request must encode");
    let mut stream = UnixStream::connect(socket).expect("daemon status socket must connect");
    write_frame_v1(&mut stream, &payload).expect("status request must write");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("request write side must close");
    let payload = read_single_frame_v1(&mut stream)
        .expect("status response frame must read")
        .expect("status response must exist");
    decode_response_payload_v2(&payload).expect("status response must decode")
}

fn query_status(socket: &Path) -> Value {
    let request = status_request(
        ClientInfoV1::new("podway-e2e", env!("CARGO_PKG_VERSION"), std::process::id())
            .expect("client identity"),
    );
    match exchange_status(socket, &request) {
        ResponseEnvelopeV2::OutputV2(output) => output.result().clone().into(),
        ResponseEnvelopeV2::Error(error) => panic!("daemon status failed: {:?}", error.code()),
    }
}

fn query_ready_status(socket: &Path) -> Value {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let status = query_status(socket);
        if status["readiness_state"] == "ready" {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not reach ready state: {status}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn bootstrap_event(stderr: &[u8]) -> Value {
    let stderr = String::from_utf8_lossy(stderr);
    let lines = stderr.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "bootstrap stderr must contain one JSONL record"
    );
    serde_json::from_str(lines[0]).expect("bootstrap stderr must be valid JSON")
}

fn bootstrap_file_event(paths: &ServiceRuntimePathsV1) -> Value {
    let log = fs::read_to_string(paths.bootstrap_log_path().as_path())
        .expect("daemon-managed bootstrap log must be readable");
    let lines = log.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "bootstrap file must contain one JSONL record"
    );
    serde_json::from_str(lines[0]).expect("bootstrap file must contain valid JSON")
}

#[test]
fn podwayd_sigterm_drains_and_removes_its_owned_socket() {
    let fixture = ProcessFixtureV1::new();
    let mut child = fixture.spawn();
    let socket = fixture.paths.socket_path().as_path();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !socket.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("podwayd status must be observable") {
            let output = child
                .wait_with_output()
                .expect("terminated podwayd output must be readable");
            panic!(
                "podwayd exited before binding ({status}): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "podwayd must bind its production socket");

    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM)
        .expect("SIGTERM must be delivered to podwayd");
    let output = child
        .wait_with_output()
        .expect("podwayd shutdown output must be readable");
    assert!(
        output.status.success(),
        "podwayd must exit successfully after SIGTERM: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !socket.exists(),
        "graceful shutdown must remove only the socket owned by this process"
    );
    assert!(output.stderr.is_empty());
    let startup = bootstrap_file_event(&fixture.paths);
    assert_eq!(startup["schema"], "podway.daemon-bootstrap-log/v1");
    assert_eq!(startup["operation"], "daemon_bootstrap");
    assert_eq!(startup["outcome"], "succeeded");
    assert!(startup["ts"].as_str().is_some());
    assert!(startup["daemon_id"].as_str().is_some());
    assert_eq!(startup["seq"], 0);
    assert!(startup["session_id"].is_null());
}

#[test]
fn podwayd_reports_stable_live_process_identity() {
    let fixture = ProcessFixtureV1::new();
    let child = fixture.spawn();
    let socket = fixture.paths.socket_path().as_path();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "podwayd must bind before identity probing");

    let stale_request = status_request(
        ClientInfoV1::new_with_contract_identity(
            "podway-e2e",
            "0.0.0-stale",
            std::process::id(),
            "podway",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("stale client identity"),
    );
    let ResponseEnvelopeV2::Error(mismatch) = exchange_status(socket, &stale_request) else {
        panic!("a stale contract must not receive fabricated daemon status");
    };
    assert_eq!(mismatch.code().as_str(), "DAEMON_CONTRACT_MISMATCH");
    assert_eq!(mismatch.exit_code().get(), 3);
    assert!(!mismatch.retryable());
    assert_eq!(mismatch.details()["admission"]["admitted"], false);

    let first = query_ready_status(socket);
    thread::sleep(Duration::from_millis(2));
    let second = query_status(socket);
    assert_eq!(first["schema"], "podway.daemon-status-result/v3");
    assert_eq!(first["mode"], "prod");
    assert_eq!(first["in_flight_client_count"], Value::Null);
    assert_eq!(first["maintenance_operation_count"], 0);
    assert_eq!(first["readiness_stage"], "ready");
    assert_eq!(second["readiness_state"], "ready");
    assert_eq!(
        first["daemon_version"],
        podway_protocol::build_identity_v1().version()
    );
    assert_eq!(first["pid"], child.id());
    assert_eq!(first["pid"], second["pid"]);
    assert_eq!(first["process_id"], second["process_id"]);
    assert!(Uuid::parse_str(first["process_id"].as_str().expect("process UUID")).is_ok());
    assert_eq!(first["started_at"], second["started_at"]);
    assert!(second["uptime_ms"].as_u64() >= first["uptime_ms"].as_u64());
    assert_eq!(
        first["configured_socket_path"],
        socket.display().to_string()
    );
    assert_eq!(first["effective_socket_path"], socket.display().to_string());
    assert_eq!(
        first["executable_path"],
        fs::canonicalize(env!("CARGO_BIN_EXE_podwayd"))
            .expect("daemon executable must canonicalize")
            .display()
            .to_string()
    );
    assert_eq!(
        first["contract_manifest_digest"],
        podway_protocol::build_identity_v1().contract_manifest_digest()
    );

    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).expect("stop podwayd");
    let output = child.wait_with_output().expect("read podwayd shutdown");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!socket.exists(), "the first daemon must release its socket");

    let replacement = fixture.spawn();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let restarted = query_ready_status(socket);
    assert_ne!(restarted["process_id"], first["process_id"]);
    assert_ne!(restarted["started_at"], first["started_at"]);
    assert_eq!(
        restarted["contract_manifest_digest"],
        first["contract_manifest_digest"]
    );
    assert_eq!(
        restarted["configured_socket_path"],
        first["configured_socket_path"]
    );
    kill(Pid::from_raw(replacement.id() as i32), Signal::SIGTERM).expect("stop replacement");
    let output = replacement
        .wait_with_output()
        .expect("read replacement shutdown");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn production_and_named_mode_own_disjoint_complete_namespaces() {
    let fixture = ProcessFixtureV1::new();
    let named_paths = fixture.mode_paths("demo");
    let production_socket = fixture.paths.socket_path().as_path();
    let named_socket = named_paths.socket_path().as_path();
    assert_ne!(
        fixture.paths.global_lock_path(),
        named_paths.global_lock_path()
    );
    assert_ne!(
        fixture.paths.workspace_registry_path(),
        named_paths.workspace_registry_path()
    );
    assert_ne!(fixture.paths.log_path(), named_paths.log_path());
    assert_ne!(
        fixture.paths.metadata_index_path(),
        named_paths.metadata_index_path()
    );
    assert_ne!(fixture.paths.recovery_path(), named_paths.recovery_path());

    let production = fixture.spawn();
    let named = fixture.spawn_mode("demo");
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while (!production_socket.exists() || !named_socket.exists()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(production_socket.exists());
    assert!(named_socket.exists());
    assert_eq!(query_ready_status(production_socket)["mode"], "prod");
    assert_eq!(query_ready_status(named_socket)["mode"], "demo");

    for child in [production, named] {
        kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).unwrap();
        assert!(child.wait_with_output().unwrap().status.success());
    }
    assert!(!production_socket.exists());
    assert!(!named_socket.exists());
}

#[test]
fn fatal_registry_recovery_fails_closed_and_removes_the_owned_socket() {
    let fixture = ProcessFixtureV1::new();
    let registry = fixture.paths.workspace_registry_path().as_path();
    fs::write(registry, b"not-json").expect("invalid registry fixture must be written");
    fs::set_permissions(registry, fs::Permissions::from_mode(0o600))
        .expect("registry fixture must be private");

    let child = fixture.spawn();
    let output = child
        .wait_with_output()
        .expect("failed podwayd output must be readable");
    assert!(
        !output.status.success(),
        "fatal registry recovery must fail"
    );
    assert!(
        !fixture.paths.socket_path().as_path().exists(),
        "fatal recovery must remove only the endpoint owned by the failed daemon"
    );
    let log = fs::read_to_string(fixture.paths.bootstrap_log_path().as_path())
        .expect("failed bootstrap log must be readable");
    let bootstrap: Value = serde_json::from_str(
        log.lines()
            .last()
            .expect("failed bootstrap log must contain a terminal record"),
    )
    .expect("failed bootstrap terminal record must be valid JSON");
    assert_eq!(bootstrap["outcome"], "failed");
}

#[test]
fn startup_prunes_one_conclusively_missing_registry_generation() {
    let fixture = ProcessFixtureV1::new();
    let missing_root = fixture.root.join("missing-worktree");
    let encoded_root = ValidatedWorkspaceRootV1::from_unix_bytes(
        std::os::unix::ffi::OsStrExt::as_bytes(missing_root.as_os_str()).to_vec(),
    )
    .expect("absolute fixture root must encode");
    let registry = fixture.paths.workspace_registry_path().as_path();
    fs::write(
        registry,
        format!(
            concat!(
                "{{\"schema\":\"podway.registry/v1\",\"workspaces\":[{{",
                "\"last_known_root\":\"{}\",",
                "\"last_seen_at\":\"2026-08-28T00:00:00.000Z\",",
                "\"workspace_uuid\":\"00000000-0000-0000-0000-000000000091\"",
                "}}]}}"
            ),
            encoded_root.as_encoded()
        ),
    )
    .expect("stale registry fixture must be written");
    fs::set_permissions(registry, fs::Permissions::from_mode(0o600))
        .expect("registry fixture must be private");

    let mut child = fixture.spawn();
    let socket = fixture.paths.socket_path().as_path();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !socket.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("podwayd status must be observable") {
            let output = child
                .wait_with_output()
                .expect("terminated podwayd output must be readable");
            panic!(
                "podwayd exited before stale cleanup ({status}): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        socket.exists(),
        "podwayd must bind before readiness probing"
    );
    let status = query_ready_status(socket);
    assert_eq!(status["worktree_recovery"]["total"], 1);
    assert_eq!(status["worktree_recovery"]["completed"], 1);
    assert_eq!(status["worktree_recovery"]["failed"], 0);
    assert_eq!(
        fs::read_to_string(registry).expect("pruned registry must remain readable"),
        "{\"schema\":\"podway.registry/v1\",\"workspaces\":[]}"
    );
    let log = fs::read_to_string(fixture.paths.log_path().as_path())
        .expect("daemon event log must be readable");
    let prune = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event must be JSON"))
        .find(|event| event["operation"] == "registry_entry_prune")
        .expect("startup cleanup must emit its closed operation");
    assert_eq!(prune["outcome"], "succeeded");
    assert_eq!(
        prune["workspace_uuid"],
        "00000000-0000-0000-0000-000000000091"
    );
    assert!(prune.get("root").is_none());

    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).expect("stop podwayd");
    let output = child.wait_with_output().expect("read podwayd shutdown");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn podwayd_service_and_version_modes_are_explicit() {
    let version = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .arg("version")
        .output()
        .expect("podwayd version process must run");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).expect("version output is UTF-8"),
        format!("podwayd {}\n", env!("CARGO_PKG_VERSION"))
    );

    let json_version = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .args(["version", "--json"])
        .env_clear()
        .output()
        .expect("podwayd JSON version process must run");
    assert!(json_version.status.success());
    assert!(json_version.stderr.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&json_version.stdout)
            .lines()
            .count(),
        1
    );
    let summary: Value =
        serde_json::from_slice(&json_version.stdout).expect("podwayd JSON version is valid");
    assert_eq!(
        summary,
        serde_json::json!({
            "name": "podwayd",
            "version": format!("v{}", env!("CARGO_PKG_VERSION")),
        })
    );

    let json_identity = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .args(["version", "--json", "--identity"])
        .env_clear()
        .output()
        .expect("podwayd JSON identity process must run");
    assert!(json_identity.status.success());
    assert!(json_identity.stderr.is_empty());
    let envelope: Value =
        serde_json::from_slice(&json_identity.stdout).expect("podwayd JSON identity is valid");
    let _: ResponseEnvelopeV2 = serde_json::from_slice(&json_identity.stdout)
        .expect("podwayd JSON identity satisfies the typed public protocol");
    assert_eq!(envelope["schema"], "podway.output/v3");
    assert_eq!(envelope["command"], "version");
    let identity = &envelope["result"];
    assert_eq!(identity["schema"], "podway.version-result/v1");
    assert_eq!(identity["product"], "podway");
    assert_eq!(identity["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        identity["contract_manifest_schema"],
        "podway.contract-manifest/v1"
    );
    assert_eq!(
        identity["supported_ipc_ids"],
        serde_json::json!(["podway.ipc/v1"])
    );

    let run_invalid = || {
        Command::new(env!("CARGO_BIN_EXE_podwayd"))
            .arg("--unknown")
            .output()
            .expect("podwayd invalid-argument process must run")
    };
    let invalid = run_invalid();
    let invalid_again = run_invalid();
    assert!(!invalid.status.success());
    assert!(!invalid_again.status.success());
    assert_eq!(invalid.stderr, invalid_again.stderr);
    let fatal = bootstrap_event(&invalid.stderr);
    assert_eq!(fatal["schema"], "podway.daemon-bootstrap-log/v1");
    assert_eq!(fatal["operation"], "daemon_bootstrap");
    assert_eq!(fatal["outcome"], "failed");
    assert!(fatal["ts"].is_null());
    assert!(fatal["daemon_id"].is_null());
    assert_eq!(fatal["seq"], 0);
    assert!(fatal["message"].is_null());

    let relative_socket = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .args(["--service", "--socket", "relative.sock"])
        .env_remove("HOME")
        .env_remove("TMPDIR")
        .output()
        .expect("podwayd explicit socket validation process must run");
    assert!(!relative_socket.status.success());
    assert!(bootstrap_event(&relative_socket.stderr)["message"].is_null());
    assert!(!String::from_utf8_lossy(&relative_socket.stderr).contains("relative.sock"));
}
