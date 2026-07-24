//! Process-level proof that `podwayd` binds the production endpoint and shuts down cleanly.

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
use podway_protocol::{
    ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV1,
    decode_response_payload_v1, encode_request_payload_v1, read_single_frame_v1, write_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;
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

fn status_request() -> RequestEnvelopeV1 {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(Uuid::new_v4().to_string()).expect("request UUID"),
        client: ClientInfoV1::new("podway-e2e", env!("CARGO_PKG_VERSION"), std::process::id())
            .expect("client identity"),
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

fn query_status(socket: &Path) -> Value {
    let request = status_request();
    let payload = encode_request_payload_v1(&request).expect("status request must encode");
    let mut stream = UnixStream::connect(socket).expect("daemon status socket must connect");
    write_frame_v1(&mut stream, &payload).expect("status request must write");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("request write side must close");
    let payload = read_single_frame_v1(&mut stream)
        .expect("status response frame must read")
        .expect("status response must exist");
    match decode_response_payload_v1(&payload).expect("status response must decode") {
        ResponseEnvelopeV1::Output(output) => output.result().clone().into(),
        ResponseEnvelopeV1::Error(error) => panic!("daemon status failed: {:?}", error.code()),
    }
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

    let first = query_status(socket);
    thread::sleep(Duration::from_millis(2));
    let second = query_status(socket);
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
}

#[test]
fn podwayd_service_and_version_modes_are_explicit() {
    let version = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .arg("--version")
        .output()
        .expect("podwayd version process must run");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).expect("version output is UTF-8"),
        format!("podwayd {}\n", env!("CARGO_PKG_VERSION"))
    );

    let json_version = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .args(["--json", "version"])
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
    let identity: Value =
        serde_json::from_slice(&json_version.stdout).expect("podwayd JSON version is valid");
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

    let invalid = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .arg("--unknown")
        .output()
        .expect("podwayd invalid-argument process must run");
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("usage: podwayd"));

    let relative_socket = Command::new(env!("CARGO_BIN_EXE_podwayd"))
        .args(["--service", "--socket", "relative.sock"])
        .env_remove("HOME")
        .env_remove("TMPDIR")
        .output()
        .expect("podwayd explicit socket validation process must run");
    assert!(!relative_socket.status.success());
    assert!(String::from_utf8_lossy(&relative_socket.stderr).contains("must be absolute"));
}
