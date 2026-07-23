//! Public CLI-to-production-daemon coverage for the bounded G005 vertical.
#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::{Pid, geteuid},
};
use podway_protocol::{
    ItemTypeResultV1, NextResultV1, OutputEnvelopeV1, ResponseEnvelopeV1, SessionLifecycleV1,
    StageStatusResultV1, StatusResultV1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct FixtureV1 {
    root: PathBuf,
    home: PathBuf,
    paths: ServiceRuntimePathsV1,
    worktree: PathBuf,
}

impl FixtureV1 {
    fn new() -> Self {
        let root = unique_private_directory("pw4v");
        let home = root.join("h");
        let worktree = root.join("w");
        fs::create_dir(&home).expect("fixture HOME must be created");
        make_private(&home);
        create_non_bare_worktree(&worktree);
        let registry_parent = home.join(".podway/state");
        fs::create_dir_all(&registry_parent).expect("fixture registry parent must be created");
        make_private(&registry_parent);

        let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw())
            .expect("short private fixture paths must be valid");
        Self {
            root,
            home,
            paths,
            worktree,
        }
    }

    fn run(&self, workspace: &Path, command: &str, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(["--json", "--socket"])
            .arg(self.paths.socket_path().as_path())
            .arg("--worktree")
            .arg(workspace)
            .arg(command)
            .args(arguments)
            .current_dir(&self.root)
            .env_remove("HOME")
            .env_remove("TMPDIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("the real podway binary must run")
    }
}

impl Drop for FixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn daemon_binary_for_test() -> PathBuf {
    let configured = std::env::var_os("PODWAYD_TEST_BINARY").map(PathBuf::from);
    let binary = configured
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd"));
    let binary = fs::canonicalize(&binary).unwrap_or_else(|error| {
        panic!(
            "the production vertical requires PODWAYD_TEST_BINARY or a sibling podwayd binary at {}: {error}",
            binary.display()
        )
    });
    let metadata = fs::metadata(&binary)
        .unwrap_or_else(|error| panic!("podwayd test binary metadata must be readable: {error}"));
    assert!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "podwayd test binary must be an executable file: {}",
        binary.display()
    );
    assert_daemon_binary_matches_build_receipt(&binary);
    binary
}

fn assert_daemon_binary_matches_build_receipt(binary: &Path) {
    let receipt_path = std::env::var_os("PODWAYD_BUILD_RECEIPT")
        .map(PathBuf::from)
        .expect("the production vertical requires PODWAYD_BUILD_RECEIPT naming its canonical build receipt");
    let receipt_path = fs::canonicalize(&receipt_path).unwrap_or_else(|error| {
        panic!(
            "PODWAYD_BUILD_RECEIPT must name a readable canonical receipt at {}: {error}",
            receipt_path.display()
        )
    });
    let receipt: Value = serde_json::from_slice(
        &fs::read(&receipt_path).expect("daemon build receipt must be readable"),
    )
    .unwrap_or_else(|error| panic!("daemon build receipt must be JSON: {error}"));
    let object = receipt
        .as_object()
        .expect("daemon build receipt must be a JSON object");
    assert_eq!(
        object.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["binary", "binary_sha256", "inputs", "schema", "toolchain"]),
        "daemon build receipt must have exactly the canonical fields"
    );
    assert_eq!(
        object.get("schema").and_then(Value::as_str),
        Some("podway.daemon-build-receipt/v1"),
        "daemon build receipt schema must be canonical"
    );
    let binary_path = binary.display().to_string();
    assert_eq!(
        object.get("binary").and_then(Value::as_str),
        Some(binary_path.as_str()),
        "daemon build receipt must bind the exact canonical daemon binary path"
    );
    let binary_digest = sha256_file(binary);
    assert_eq!(
        object.get("binary_sha256").and_then(Value::as_str),
        Some(binary_digest.as_str()),
        "daemon build receipt must bind the exact daemon binary bytes"
    );
    let recorded_inputs = object
        .get("inputs")
        .and_then(Value::as_object)
        .expect("daemon build receipt must include source input hashes");
    let recorded_inputs = recorded_inputs
        .iter()
        .map(|(path, digest)| {
            (
                path.clone(),
                digest
                    .as_str()
                    .unwrap_or_else(|| panic!("receipt input digest for {path} must be a string"))
                    .to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (path, digest) in daemon_source_input_hashes() {
        assert_eq!(
            recorded_inputs.get(&path),
            Some(&digest),
            "daemon build receipt must bind current daemon source input {path}"
        );
    }
    let toolchain = object
        .get("toolchain")
        .and_then(Value::as_object)
        .expect("daemon build receipt must include the exact build toolchain");
    assert_eq!(
        toolchain
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["cargo", "rustc"]),
        "daemon build receipt must bind exactly cargo and rustc"
    );
    for tool_id in ["cargo", "rustc"] {
        let tool = toolchain[tool_id]
            .as_object()
            .unwrap_or_else(|| panic!("{tool_id} receipt must be an object"));
        assert_eq!(
            tool.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["path", "sha256", "version"]),
            "{tool_id} receipt fields must be canonical"
        );
        let path = Path::new(
            tool["path"]
                .as_str()
                .unwrap_or_else(|| panic!("{tool_id} path must be a string")),
        );
        assert!(
            path.is_absolute() && path.is_file() && !path.is_symlink(),
            "{tool_id} path must name an absolute regular non-symlink file"
        );
        let expected_digest = sha256_file(path);
        assert_eq!(
            tool["sha256"].as_str(),
            Some(expected_digest.as_str()),
            "{tool_id} receipt must bind the executable bytes"
        );
        assert!(
            tool["version"]
                .as_str()
                .is_some_and(|version| !version.trim().is_empty()),
            "{tool_id} version must be non-empty"
        );
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("podway-cli manifest must be nested under the workspace root")
        .to_path_buf()
}

fn daemon_source_input_hashes() -> BTreeMap<String, String> {
    let workspace = workspace_root();
    let mut inputs = BTreeMap::new();
    for path in [
        workspace.join("Cargo.toml"),
        workspace.join("Cargo.lock"),
        workspace.join("presets"),
        workspace.join("spec"),
    ] {
        collect_daemon_source_input_hashes(&workspace, &path, &mut inputs);
    }
    for crate_name in [
        "podway-core",
        "podway-protocol",
        "podway-config",
        "podway-presets",
        "podway-store",
        "podway-git",
        "podway-service",
        "podway-daemon",
    ] {
        let crate_root = workspace.join("crates").join(crate_name);
        collect_daemon_source_input_hashes(&workspace, &crate_root.join("Cargo.toml"), &mut inputs);
        collect_daemon_source_input_hashes(&workspace, &crate_root.join("src"), &mut inputs);
    }
    inputs
}

fn collect_daemon_source_input_hashes(
    workspace: &Path,
    path: &Path,
    inputs: &mut BTreeMap<String, String>,
) {
    let metadata = fs::symlink_metadata(path).unwrap_or_else(|error| {
        panic!(
            "daemon source input {} must be readable: {error}",
            path.display()
        )
    });
    if metadata.file_type().is_symlink() {
        panic!(
            "daemon source input must not be a symlink: {}",
            path.display()
        );
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).unwrap_or_else(|error| {
            panic!(
                "daemon source directory {} must be readable: {error}",
                path.display()
            )
        }) {
            collect_daemon_source_input_hashes(
                workspace,
                &entry.expect("daemon source directory entry").path(),
                inputs,
            );
        }
    } else if metadata.is_file() && is_daemon_source_input(path) {
        let relative = path
            .strip_prefix(workspace)
            .expect("daemon source input must remain below the workspace")
            .to_string_lossy()
            .into_owned();
        assert!(
            inputs.insert(relative.clone(), sha256_file(path)).is_none(),
            "daemon source input list must not contain duplicates: {relative}"
        );
    }
}

fn is_daemon_source_input(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("rs" | "toml" | "yaml" | "yml" | "sql")
    ) || path.file_name().and_then(|name| name.to_str()) == Some("Cargo.lock")
}

fn sha256_file(path: &Path) -> String {
    let mut source = fs::File::open(path)
        .unwrap_or_else(|error| panic!("open {} for SHA-256: {error}", path.display()));
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("read {} for SHA-256: {error}", path.display()));
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    format!("{:x}", hasher.finalize())
}
fn same_size_mutation(bytes: &[u8]) -> Vec<u8> {
    let mut mutated = bytes.to_vec();
    let first = mutated
        .first_mut()
        .expect("artifact mutation coverage requires nonempty bytes");
    *first ^= 1;
    mutated
}
struct DaemonLogsV1 {
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

struct RunningDaemonV1 {
    child: Option<Child>,
    socket_path: PathBuf,
    logs: DaemonLogsV1,
    stdout_reader: Option<thread::JoinHandle<()>>,
    stderr_reader: Option<thread::JoinHandle<()>>,
    readiness: Duration,
}

impl RunningDaemonV1 {
    fn start(fixture: &FixtureV1) -> Self {
        let daemon_binary = daemon_binary_for_test();
        let socket_path = fixture.paths.socket_path().as_path().to_path_buf();
        let readiness_started = Instant::now();
        let mut child = Command::new(&daemon_binary)
            .args(["--service", "--socket"])
            .arg(&socket_path)
            .current_dir(&fixture.root)
            .env_clear()
            .env("PODWAY_TEST_ACCOUNT_ROOT", &fixture.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the real podwayd binary must start");
        let logs = DaemonLogsV1 {
            stdout: Arc::new(Mutex::new(Vec::new())),
            stderr: Arc::new(Mutex::new(Vec::new())),
        };
        let stdout_reader = drain_daemon_stream(
            child.stdout.take().expect("podwayd stdout must be piped"),
            Arc::clone(&logs.stdout),
        );
        let stderr_reader = drain_daemon_stream(
            child.stderr.take().expect("podwayd stderr must be piped"),
            Arc::clone(&logs.stderr),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if socket_path.exists() {
                break;
            }
            if let Some(status) = child
                .try_wait()
                .expect("podwayd process state must be observable")
            {
                let _ = child.wait();
                join_daemon_reader(stdout_reader);
                join_daemon_reader(stderr_reader);
                panic!(
                    "podwayd exited before binding ({status}): {}",
                    daemon_stderr(&logs)
                );
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                join_daemon_reader(stdout_reader);
                join_daemon_reader(stderr_reader);
                panic!(
                    "podwayd did not bind within ten seconds: {}",
                    daemon_stderr(&logs)
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut daemon = Self {
            child: Some(child),
            socket_path,
            logs,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            readiness: Duration::ZERO,
        };
        probe_daemon_readiness(fixture);
        daemon.readiness = readiness_started.elapsed();
        daemon
    }

    fn stop(mut self) {
        let mut child = self.child.take().expect("podwayd process must exist");
        kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM)
            .expect("SIGTERM must be delivered to podwayd");
        let status = child
            .wait()
            .expect("podwayd shutdown state must be readable");
        self.join_log_readers();
        assert!(
            status.success(),
            "podwayd must exit successfully after SIGTERM; stdout={} stderr={}",
            daemon_stdout(&self.logs),
            daemon_stderr(&self.logs)
        );
        assert!(
            !self.socket_path.exists(),
            "normal runtime shutdown must remove its owned socket"
        );
    }

    fn join_log_readers(&mut self) {
        if let Some(reader) = self.stdout_reader.take() {
            join_daemon_reader(reader);
        }
        if let Some(reader) = self.stderr_reader.take() {
            join_daemon_reader(reader);
        }
    }
    fn readiness_millis(&self) -> u128 {
        self.readiness.as_millis()
    }
}

impl Drop for RunningDaemonV1 {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.join_log_readers();
    }
}

fn drain_daemon_stream(
    mut stream: impl Read + Send + 'static,
    retained: Arc<Mutex<Vec<u8>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => return,
                Ok(count) => retained
                    .lock()
                    .expect("daemon log retention must not be poisoned")
                    .extend_from_slice(&buffer[..count]),
                Err(error) => panic!("daemon log stream must remain readable: {error}"),
            }
        }
    })
}

fn join_daemon_reader(reader: thread::JoinHandle<()>) {
    reader
        .join()
        .expect("daemon log reader must terminate cleanly");
}

fn daemon_stdout(logs: &DaemonLogsV1) -> String {
    String::from_utf8_lossy(
        &logs
            .stdout
            .lock()
            .expect("daemon stdout retention must not be poisoned"),
    )
    .into_owned()
}

fn daemon_stderr(logs: &DaemonLogsV1) -> String {
    String::from_utf8_lossy(
        &logs
            .stderr
            .lock()
            .expect("daemon stderr retention must not be poisoned"),
    )
    .into_owned()
}

fn probe_daemon_readiness(fixture: &FixtureV1) {
    let output = fixture.run(&fixture.worktree, "status", &[]);
    assert!(
        output.stderr.is_empty(),
        "the successful CLI transport readiness probe must not write stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response =
        serde_json::from_slice::<ResponseEnvelopeV1>(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "CLI readiness probe must receive a daemon protocol response: {error}; stdout={}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
    if let ResponseEnvelopeV1::Error(error) = response {
        assert_ne!(
            error.code().as_str(),
            "DAEMON_UNAVAILABLE",
            "readiness requires a daemon response, not a client-side transport failure"
        );
    }
}

fn unique_private_directory(prefix: &str) -> PathBuf {
    for _ in 0..1024 {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{sequence}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => {
                make_private(&path);
                return path;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("short private fixture root must be created: {error}"),
        }
    }
    panic!("short private fixture root names must not be exhausted");
}

fn make_private(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("fixture-owned directory must be private");
}

fn create_non_bare_worktree(path: &Path) {
    run_git(
        Command::new("git").arg("init").arg("--quiet").arg(path),
        "initialize worktree",
    );
    run_git(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("config")
            .arg("user.email")
            .arg("podway-phase4@example.invalid"),
        "configure fixture author email",
    );
    run_git(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("config")
            .arg("user.name")
            .arg("Podway Phase 4"),
        "configure fixture author name",
    );
    run_git(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("commit")
            .arg("--quiet")
            .arg("--allow-empty")
            .arg("-m")
            .arg("initial fixture commit"),
        "create initial fixture commit",
    );
    let workspace_runtime = path.join(".podway/runtime");
    fs::create_dir_all(&workspace_runtime)
        .expect("worktree-local runtime directory must be created");
    make_private(&workspace_runtime);
}

fn run_git(command: &mut Command, action: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("Git must be available to {action}: {error}"));
    assert!(
        output.status.success(),
        "Git must {action}; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn assert_output_shape(raw: &Value, command: &str) {
    let object = raw
        .as_object()
        .expect("CLI success must be one JSON object response");
    for key in [
        "schema",
        "request_id",
        "command",
        "generated_at",
        "result",
        "warnings",
    ] {
        assert!(
            object.contains_key(key),
            "{command} success response must contain {key}: {raw}"
        );
    }
    assert_eq!(
        object.get("schema"),
        Some(&Value::String("podway.output/v1".to_owned())),
        "success response must use the public output schema"
    );
    assert_eq!(
        object.get("command"),
        Some(&Value::String(command.to_owned())),
        "success response command must match the invoked CLI command"
    );
    assert!(
        object.keys().all(|key| {
            matches!(
                key.as_str(),
                "schema"
                    | "request_id"
                    | "command"
                    | "generated_at"
                    | "workspace"
                    | "job"
                    | "session"
                    | "result"
                    | "warnings"
            )
        }),
        "success response must expose only the documented output-envelope fields"
    );
}

fn assert_error_shape(raw: &Value, command: &str) {
    let object = raw
        .as_object()
        .expect("CLI failure must be one JSON object response");
    for key in [
        "schema",
        "request_id",
        "command",
        "generated_at",
        "code",
        "message",
        "retryable",
        "exit_code",
        "details",
    ] {
        assert!(
            object.contains_key(key),
            "error response must contain {key}"
        );
    }
    assert_eq!(
        object.get("schema"),
        Some(&Value::String("podway.error/v1".to_owned())),
        "error response must use the public error schema"
    );
    assert_eq!(
        object.get("command"),
        Some(&Value::String(command.to_owned())),
        "error response command must match the invoked CLI command"
    );
    assert!(
        object.keys().all(|key| {
            matches!(
                key.as_str(),
                "schema"
                    | "request_id"
                    | "command"
                    | "generated_at"
                    | "code"
                    | "message"
                    | "retryable"
                    | "exit_code"
                    | "workspace"
                    | "details"
            )
        }),
        "error response must expose only the documented error-envelope fields"
    );
}

fn wire_command(command: &str) -> &'static str {
    match command {
        "init" => "workspace.init",
        "start" => "session.start",
        "status" => "session.status",
        "next" => "session.next",
        "check" => "item.check",
        "set" => "item.set",
        "add" => "item.add",
        "attach" => "item.attach",
        "block" => "session.block",
        "unblock" => "session.unblock",
        "retry" => "session.retry",
        "return" => "session.return",
        "complete" => "session.complete",
        _ => panic!("test must invoke a bounded G005 CLI command"),
    }
}
fn cli_output(
    fixture: &FixtureV1,
    workspace: &Path,
    command: &str,
    arguments: &[&str],
) -> OutputEnvelopeV1 {
    let expected_wire_command = wire_command(command);
    let output = fixture.run(workspace, command, arguments);
    assert!(
        output.stderr.is_empty(),
        "successful CLI {command} must not write stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI {command} must emit exactly one JSON response: {error}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    });
    assert_output_shape(&raw, expected_wire_command);
    let response: ResponseEnvelopeV1 = serde_json::from_value(raw)
        .expect("public success response must satisfy the exact protocol schema");
    assert_eq!(
        output.status.code(),
        Some(0),
        "CLI {command} must exit zero for a successful public output; stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    match response {
        ResponseEnvelopeV1::Output(output) => output,
        ResponseEnvelopeV1::Error(error) => panic!(
            "CLI {command} returned unexpected {} error: {}",
            error.code().as_str(),
            error.message()
        ),
    }
}

fn cli_error(
    fixture: &FixtureV1,
    workspace: &Path,
    command: &str,
    arguments: &[&str],
    expected_code: &str,
    expected_exit_code: i32,
    expected_retryable: bool,
) {
    let expected_wire_command = wire_command(command);
    let output = fixture.run(workspace, command, arguments);
    let raw: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI {command} must emit its public JSON error: {error}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    });
    assert_error_shape(&raw, expected_wire_command);
    let response: ResponseEnvelopeV1 = serde_json::from_value(raw)
        .expect("public error response must satisfy the exact protocol schema");
    let ResponseEnvelopeV1::Error(error) = response else {
        panic!("CLI {command} unexpectedly returned a success envelope");
    };
    assert_eq!(error.code().as_str(), expected_code);
    assert_eq!(
        error.retryable(),
        expected_retryable,
        "{expected_code} retryability must remain stable"
    );
    assert_eq!(
        i32::from(error.exit_code().get()),
        expected_exit_code,
        "public error must declare the expected process exit code"
    );
    assert_eq!(
        output.status.code(),
        Some(expected_exit_code),
        "CLI {command} must return the exit code declared by its public error envelope; stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn public_status(fixture: &FixtureV1, workspace: &Path) -> (OutputEnvelopeV1, StatusResultV1) {
    let output = cli_output(fixture, workspace, "status", &[]);
    let status = StatusResultV1::from_result_map(output.result())
        .expect("session.status output must satisfy the documented status result schema");
    (output, status)
}

fn public_next(fixture: &FixtureV1, workspace: &Path) -> NextResultV1 {
    let output = cli_output(fixture, workspace, "next", &[]);
    NextResultV1::from_result_map(output.result())
        .expect("session.next output must satisfy the documented next result schema")
}

fn current(status: &StatusResultV1) -> &podway_protocol::CurrentAttemptResultV1 {
    status
        .current
        .as_ref()
        .expect("running session status must contain its current attempt")
}

fn item<'a>(status: &'a StatusResultV1, item_id: &str) -> &'a podway_protocol::StatusItemResultV1 {
    status
        .items
        .iter()
        .find(|item| item.id.as_str() == item_id)
        .unwrap_or_else(|| panic!("status must contain item {item_id}"))
}

fn assert_current_stage(fixture: &FixtureV1, workspace: &Path, expected_stage: &str) {
    let (_, status) = public_status(fixture, workspace);
    assert_eq!(current(&status).stage_id.as_str(), expected_stage);
}

fn complete_remaining_sw_dev(
    fixture: &FixtureV1,
    workspace: &Path,
    verification_artifact: &str,
    verification_content: &str,
) {
    cli_output(fixture, workspace, "check", &["relevant-code-inspected"]);
    cli_output(
        fixture,
        workspace,
        "set",
        &["current-behavior", "the production vertical is operating"],
    );
    cli_output(
        fixture,
        workspace,
        "add",
        &["affected-components", "production daemon runtime"],
    );
    cli_output(fixture, workspace, "complete", &[]);
    assert_current_stage(fixture, workspace, "plan");

    cli_output(
        fixture,
        workspace,
        "set",
        &["implementation-plan", "complete the ordinary preset stages"],
    );
    cli_output(fixture, workspace, "set", &["risk-level", "low"]);
    cli_output(
        fixture,
        workspace,
        "add",
        &["verification-plan", "run the focused vertical test"],
    );
    cli_output(fixture, workspace, "complete", &[]);
    assert_current_stage(fixture, workspace, "implement");

    cli_output(fixture, workspace, "check", &["implementation-complete"]);
    cli_output(
        fixture,
        workspace,
        "set",
        &[
            "implementation-summary",
            "the requested test path is complete",
        ],
    );
    cli_output(fixture, workspace, "complete", &[]);
    assert_current_stage(fixture, workspace, "verify");

    cli_output(fixture, workspace, "check", &["relevant-checks-passed"]);
    cli_output(
        fixture,
        workspace,
        "set",
        &["verification-note", "the public vertical completed"],
    );
    cli_output(
        fixture,
        workspace,
        "attach",
        &[
            "verification-reference",
            verification_artifact,
            "--media-type",
            "text/plain",
        ],
    );
    let (_, verification_status) = public_status(fixture, workspace);
    let artifact = item(&verification_status, "verification-reference")
        .value
        .as_object()
        .expect("public status must expose attached artifact metadata");
    assert_eq!(
        artifact.get("location_type").and_then(Value::as_str),
        Some("path")
    );
    assert_eq!(
        artifact.get("location").and_then(Value::as_str),
        Some(verification_artifact)
    );
    assert_eq!(
        artifact.get("media_type").and_then(Value::as_str),
        Some("text/plain")
    );
    assert_eq!(
        artifact.get("size_bytes").and_then(Value::as_u64),
        Some(verification_content.len() as u64)
    );
    let artifact_path = workspace.join(verification_artifact);
    let expected_digest = format!("sha256:{}", sha256_file(&artifact_path));
    assert_eq!(
        artifact.get("sha256_digest").and_then(Value::as_str),
        Some(expected_digest.as_str()),
        "public status must expose the exact SHA-256 digest of the attached artifact bytes"
    );

    let mutated = same_size_mutation(verification_content.as_bytes());
    fs::write(&artifact_path, &mutated)
        .expect("attached artifact must be changed for revalidation coverage");
    assert_eq!(
        fs::metadata(&artifact_path)
            .expect("mutated artifact metadata")
            .len(),
        verification_content.len() as u64,
        "artifact mutation coverage must preserve the original byte length"
    );
    cli_error(
        fixture,
        workspace,
        "complete",
        &[],
        "ARTIFACT_CHANGED",
        1,
        true,
    );
    assert_current_stage(fixture, workspace, "verify");
    fs::write(&artifact_path, verification_content)
        .expect("attached artifact must be restored before completion");
    cli_output(fixture, workspace, "complete", &[]);
    assert_current_stage(fixture, workspace, "review");

    cli_output(fixture, workspace, "check", &["review-complete"]);
    cli_output(fixture, workspace, "check", &["findings-resolved"]);
    cli_output(
        fixture,
        workspace,
        "set",
        &["review-note", "no unresolved findings remain"],
    );
    cli_output(fixture, workspace, "complete", &[]);
    assert_current_stage(fixture, workspace, "finish");

    cli_output(fixture, workspace, "check", &["task-result-ready"]);
    cli_output(
        fixture,
        workspace,
        "set",
        &["final-summary", "ordinary sw-dev flow completed"],
    );
    cli_output(fixture, workspace, "complete", &[]);
}

#[test]
#[ignore = "run with tools/run_g005_vertical.py so Cargo supplies a freshly built podwayd artifact"]
fn pac_001_public_init_on_a_valid_git_worktree_creates_no_task_session() {
    let fixture = FixtureV1::new();
    let daemon = RunningDaemonV1::start(&fixture);

    let initialized = cli_output(&fixture, &fixture.worktree, "init", &[]);
    assert!(
        initialized.session().is_none(),
        "PAC-001 init must succeed in a valid Git worktree without creating a task session"
    );
    assert!(
        initialized
            .workspace()
            .is_some_and(|workspace| workspace.root()
                == fs::canonicalize(&fixture.worktree)
                    .expect("fixture worktree must canonicalize")
                    .display()
                    .to_string()),
        "PAC-001 init must identify the initialized Git worktree"
    );

    daemon.stop();
}
#[test]
#[ignore = "run with tools/run_g005_vertical.py so Cargo supplies a freshly built podwayd artifact"]
fn pac_004_006_007_009_production_status_and_next_preserve_actionable_state_without_history() {
    let fixture = FixtureV1::new();
    let daemon = RunningDaemonV1::start(&fixture);
    let workspace = fixture.worktree.clone();

    cli_output(&fixture, &workspace, "init", &[]);
    cli_output(
        &fixture,
        &workspace,
        "start",
        &[
            "--preset",
            "sw-dev",
            "--task",
            "production status and next response boundary",
        ],
    );
    let (_, first_status) = public_status(&fixture, &workspace);
    let first_attempt_id = current(&first_status).attempt_id.clone();
    cli_output(
        &fixture,
        &workspace,
        "set",
        &["goal", "preserve the actionable state after retry"],
    );
    cli_output(
        &fixture,
        &workspace,
        "retry",
        &["--reason", "create a prior attempt that remains internal"],
    );

    let verbose_status_output = cli_output(&fixture, &workspace, "status", &["--verbose"]);
    let verbose_status = StatusResultV1::from_result_map(verbose_status_output.result())
        .expect("verbose status must satisfy the documented status result schema");
    let previous_attempts = verbose_status
        .previous_attempts
        .as_ref()
        .expect("verbose status must expose the retry-created prior attempt");
    assert_eq!(previous_attempts.len(), 1);
    assert_eq!(previous_attempts[0].attempt_id, first_attempt_id);
    assert_eq!(previous_attempts[0].attempt_number, 1);
    let (status_output, status) = public_status(&fixture, &workspace);
    let current_attempt = current(&status);
    assert_eq!(current_attempt.stage_id.as_str(), "understand");
    assert_eq!(current_attempt.attempt_number, 2);
    assert_ne!(current_attempt.attempt_id, first_attempt_id);
    assert_eq!(status.session.lifecycle, SessionLifecycleV1::Running);
    assert_eq!(item(&status, "goal").value, Value::Null);
    assert!(!item(&status, "goal").satisfied);
    assert!(status.blockers.is_empty());
    assert!(!status.queue.pending_mutations);
    assert_eq!(status.queue.queued_count, 0);
    assert!(
        status_output.result().get("previous_attempts").is_none(),
        "ordinary status must omit internal attempt history even after retry created one"
    );
    assert!(
        status.previous_attempts.is_none(),
        "the typed status boundary must preserve the intentional absence of history"
    );

    let next = public_next(&fixture, &workspace);
    let next_stage = next
        .stage
        .as_ref()
        .expect("next must identify the active stage");
    assert_eq!(next_stage.id.as_str(), "understand");
    assert_eq!(next_stage.attempt_id, current_attempt.attempt_id);
    assert_eq!(next_stage.attempt_number, 2);
    assert_eq!(
        next.missing_required_items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["goal", "acceptance-criteria"]
    );
    assert!(next.blockers.is_empty());
    assert!(!next.allowed_actions.complete);
    assert!(next.allowed_actions.retry);
    for (missing_id, command) in [("goal", "item.set"), ("acceptance-criteria", "item.add")] {
        assert!(next.suggestions.iter().any(|suggestion| {
            suggestion
                .item_id
                .as_ref()
                .is_some_and(|id| id.as_str() == missing_id)
                && suggestion.command == command
        }));
    }

    daemon.stop();
}
#[test]
#[ignore = "run with tools/run_g005_vertical.py so Cargo supplies a freshly built podwayd artifact"]
fn public_cli_production_vertical_covers_g005_lifecycle_recovery_replay_and_conflict() {
    let fixture = FixtureV1::new();
    let daemon = RunningDaemonV1::start(&fixture);
    let workspace = fixture.worktree.clone();
    let verification_artifact = "verification-report.txt";
    let verification_content = "public verification artifact\n";
    fs::write(workspace.join(verification_artifact), verification_content)
        .expect("worktree-local verification artifact must be created");

    let initialized = cli_output(&fixture, &workspace, "init", &[]);
    assert!(
        initialized.session().is_none(),
        "workspace.init must publicly succeed before a session exists"
    );
    assert!(
        initialized
            .job()
            .expect("workspace.init must expose its durable job")
            .finished_at()
            .is_some(),
        "workspace.init must wait for its terminal durable job"
    );

    let started = cli_output(
        &fixture,
        &workspace,
        "start",
        &[
            "--preset",
            "sw-dev",
            "--task",
            "public production vertical lifecycle",
        ],
    );
    assert!(
        started
            .job()
            .expect("session.start must expose its durable job")
            .finished_at()
            .is_some(),
        "session.start must wait for its terminal durable job"
    );

    let (initial_status_output, initial_status) = public_status(&fixture, &workspace);
    let workspace_uuid = initial_status_output
        .workspace()
        .expect("public status must identify the initialized workspace")
        .uuid()
        .clone();
    let initial_session_id = initial_status.session.id.clone();
    let initial_current = current(&initial_status).clone();
    assert_eq!(
        initial_current.stage_id.as_str(),
        "understand",
        "[FIRST-CORRECTNESS-ACTIVE-ATTEMPT] sw-dev must start at understand"
    );
    assert_eq!(
        initial_current.attempt_number, 1,
        "[FIRST-CORRECTNESS-ACTIVE-ATTEMPT] initial attempt must be attempt one"
    );
    assert_eq!(
        initial_status
            .stages
            .iter()
            .filter(|stage| stage.status == StageStatusResultV1::Current)
            .count(),
        1,
        "[FIRST-CORRECTNESS-ACTIVE-ATTEMPT] exactly one stage must expose the active attempt"
    );
    assert_eq!(
        initial_status
            .stages
            .iter()
            .find(|stage| stage.id.as_str() == "understand")
            .expect("sw-dev must include understand")
            .latest_attempt_number,
        1,
        "[FIRST-CORRECTNESS-ACTIVE-ATTEMPT] start must create exactly one initial attempt"
    );

    let initial_next = public_next(&fixture, &workspace);
    let next_stage = initial_next
        .stage
        .as_ref()
        .expect("public next response must identify the active stage");
    assert_eq!(next_stage.id.as_str(), "understand");
    assert_eq!(next_stage.attempt_id, initial_current.attempt_id);
    assert!(
        initial_next
            .missing_required_items
            .iter()
            .any(|item| item.id.as_str() == "goal"),
        "public next response must identify the missing goal"
    );
    assert!(
        initial_next
            .suggestions
            .iter()
            .any(|suggestion| suggestion.command == "item.set"),
        "public next response must suggest item.set"
    );

    cli_output(
        &fixture,
        &workspace,
        "set",
        &["goal", "discard this distinctive value on clean retry"],
    );
    let (_, seeded_status) = public_status(&fixture, &workspace);
    assert_eq!(
        item(&seeded_status, "goal").value,
        Value::String("discard this distinctive value on clean retry".to_owned())
    );
    cli_output(
        &fixture,
        &workspace,
        "retry",
        &["--reason", "restart the active attempt cleanly"],
    );
    let (_, retried_status) = public_status(&fixture, &workspace);
    let retried_current = current(&retried_status);
    assert_ne!(
        retried_current.attempt_id, initial_current.attempt_id,
        "[FIRST-CORRECTNESS-RETRY] retry must create a fresh attempt"
    );
    assert_eq!(
        retried_current.attempt_number, 2,
        "[FIRST-CORRECTNESS-RETRY] retry must advance the current stage attempt number"
    );
    assert_eq!(
        item(&retried_status, "goal").value,
        Value::Null,
        "[FIRST-CORRECTNESS-RETRY] clean retry must not copy prior item values"
    );

    let concurrent_attempt = retried_current.attempt_id.as_str().to_owned();
    let concurrent_revision = item(&retried_status, "goal").revision.get().to_string();
    let concurrent_left = cli_output(
        &fixture,
        &workspace,
        "set",
        &[
            "goal",
            "concurrent-left",
            "--if-attempt",
            &concurrent_attempt,
            "--if-item-revision",
            &concurrent_revision,
            "--idempotency-key",
            "public-concurrent-left",
        ],
    );
    assert!(
        concurrent_left
            .job()
            .expect("successful public CLI mutation must expose a durable job")
            .finished_at()
            .is_some(),
        "successful public CLI mutation must be terminal"
    );
    cli_error(
        &fixture,
        &workspace,
        "set",
        &[
            "goal",
            "concurrent-right",
            "--if-attempt",
            &concurrent_attempt,
            "--if-item-revision",
            &concurrent_revision,
            "--idempotency-key",
            "public-concurrent-right",
        ],
        "ITEM_REVISION_CONFLICT",
        4,
        true,
    );

    let (_, concurrent_status) = public_status(&fixture, &workspace);
    assert_eq!(
        item(&concurrent_status, "goal").value,
        Value::String("concurrent-left".to_owned()),
        "the authoritative public status must retain the successful explicit-precondition mutation"
    );
    let immutable_attempt = current(&concurrent_status).attempt_id.as_str().to_owned();
    let immutable_revision = item(&concurrent_status, "goal").revision.get().to_string();
    let immutable_arguments = [
        "goal",
        "idempotent terminal result must remain immutable",
        "--if-attempt",
        &immutable_attempt,
        "--if-item-revision",
        &immutable_revision,
        "--idempotency-key",
        "public-exact-replay",
    ];
    let immutable_first = cli_output(&fixture, &workspace, "set", &immutable_arguments);
    assert!(
        immutable_first
            .job()
            .expect("first exact public CLI mutation must expose a durable job")
            .finished_at()
            .is_some(),
        "first exact public CLI mutation must be terminal"
    );
    let immutable_before_transition = cli_output(&fixture, &workspace, "set", &immutable_arguments);
    assert_eq!(
        immutable_before_transition.command(),
        immutable_first.command(),
        "exact public replay before transition must retain the command"
    );
    assert_eq!(
        immutable_before_transition.job(),
        immutable_first.job(),
        "exact public replay before transition must retain the durable receipt"
    );
    assert_eq!(
        immutable_before_transition.session(),
        immutable_first.session(),
        "exact public replay before transition must retain the terminal session"
    );
    assert_eq!(
        immutable_before_transition.result(),
        immutable_first.result(),
        "exact public replay before transition must retain the terminal result"
    );
    assert_eq!(
        immutable_before_transition.warnings(),
        immutable_first.warnings(),
        "exact public replay before transition must retain warnings"
    );

    cli_output(
        &fixture,
        &workspace,
        "add",
        &[
            "acceptance-criteria",
            "exercise the real production dispatcher",
        ],
    );
    cli_output(&fixture, &workspace, "complete", &[]);
    assert_current_stage(&fixture, &workspace, "inspect");
    let immutable_after_transition = cli_output(&fixture, &workspace, "set", &immutable_arguments);
    assert_eq!(
        immutable_after_transition.command(),
        immutable_first.command(),
        "exact public replay after transition must retain the command"
    );
    assert_eq!(
        immutable_after_transition.job(),
        immutable_first.job(),
        "exact public replay after transition must retain the durable receipt"
    );
    assert_eq!(
        immutable_after_transition.session(),
        immutable_first.session(),
        "exact public replay after transition must retain the terminal session"
    );
    assert_eq!(
        immutable_after_transition.result(),
        immutable_first.result(),
        "exact public replay after transition must retain the terminal result"
    );
    assert_eq!(
        immutable_after_transition.warnings(),
        immutable_first.warnings(),
        "exact public replay after transition must retain warnings"
    );
    assert_current_stage(&fixture, &workspace, "inspect");

    cli_output(
        &fixture,
        &workspace,
        "return",
        &[
            "--to",
            "understand",
            "--reason",
            "revisit the completed stage",
        ],
    );
    let (_, returned_status) = public_status(&fixture, &workspace);
    let returned_current = current(&returned_status);
    assert_eq!(
        returned_current.stage_id.as_str(),
        "understand",
        "[FIRST-CORRECTNESS-RETURN] return must activate the requested destination stage"
    );
    assert_eq!(
        returned_current.attempt_number, 3,
        "[FIRST-CORRECTNESS-RETURN] return must start a fresh destination attempt"
    );
    assert_eq!(
        returned_status
            .stages
            .iter()
            .find(|stage| stage.id.as_str() == "inspect")
            .expect("sw-dev must include inspect")
            .status,
        StageStatusResultV1::Redo,
        "[FIRST-CORRECTNESS-RETURN] return must mark the abandoned source stage for redo"
    );
    assert_eq!(
        item(&returned_status, "goal").value,
        Value::Null,
        "[FIRST-CORRECTNESS-RETURN] return must start the destination with clean item values"
    );

    cli_error(
        &fixture,
        &workspace,
        "complete",
        &[],
        "REQUIRED_ITEMS_MISSING",
        1,
        false,
    );
    cli_output(
        &fixture,
        &workspace,
        "set",
        &["goal", "complete the returned understand stage"],
    );
    cli_output(
        &fixture,
        &workspace,
        "add",
        &[
            "acceptance-criteria",
            "returned stage has acceptance criteria",
        ],
    );
    cli_output(
        &fixture,
        &workspace,
        "block",
        &["--reason", "prove completion refuses open blockers"],
    );
    cli_error(
        &fixture,
        &workspace,
        "complete",
        &[],
        "BLOCKERS_PRESENT",
        1,
        false,
    );
    cli_output(&fixture, &workspace, "unblock", &["--all"]);
    cli_output(&fixture, &workspace, "complete", &[]);
    assert_current_stage(&fixture, &workspace, "inspect");

    complete_remaining_sw_dev(
        &fixture,
        &workspace,
        verification_artifact,
        verification_content,
    );
    let (completed_output, completed_status) = public_status(&fixture, &workspace);
    assert_eq!(
        completed_status.session.lifecycle,
        SessionLifecycleV1::Completed
    );
    assert!(
        completed_status.current.is_none(),
        "completed session must not expose an active attempt"
    );
    assert!(
        completed_status.session.completed_at.is_some(),
        "completed session must expose its completion timestamp"
    );

    let original_workspace = workspace.clone();
    let relocated_workspace = fixture.root.join("m");
    fs::rename(&original_workspace, &relocated_workspace)
        .expect("live non-bare worktree must move atomically");
    let (moved_output, moved_status) = public_status(&fixture, &relocated_workspace);
    assert_eq!(
        moved_output
            .workspace()
            .expect("moved status must identify its workspace")
            .uuid(),
        completed_output
            .workspace()
            .expect("completed status must identify its workspace")
            .uuid(),
        "moved worktree must preserve its durable workspace identity"
    );
    assert_eq!(
        moved_output
            .workspace()
            .expect("moved status must identify its workspace")
            .root(),
        fs::canonicalize(&relocated_workspace)
            .expect("moved worktree must canonicalize")
            .display()
            .to_string(),
        "moved status must route through the new canonical worktree path"
    );
    assert_eq!(
        moved_status.session.id, initial_session_id,
        "moved worktree must preserve its durable session identity"
    );
    assert_eq!(
        moved_status.session.lifecycle,
        SessionLifecycleV1::Completed
    );
    cli_error(
        &fixture,
        &original_workspace,
        "status",
        &[],
        "PATH_OUTSIDE_WORKTREE",
        5,
        false,
    );

    daemon.stop();
    let restarted = RunningDaemonV1::start(&fixture);
    let (restarted_output, restarted_status) = public_status(&fixture, &relocated_workspace);
    assert_eq!(
        restarted_output
            .workspace()
            .expect("restarted status must identify its workspace")
            .uuid(),
        &workspace_uuid,
        "restart with identical service paths must recover the moved workspace identity"
    );
    assert_eq!(restarted_status.session.id, initial_session_id);
    assert_eq!(
        restarted_status.session.lifecycle,
        SessionLifecycleV1::Completed
    );
    assert!(restarted_status.current.is_none());
    restarted.stop();
}

#[derive(Default)]
struct DogfoodMetricsV1 {
    command_count: u64,
    retry_count: u64,
    return_count: u64,
    next_checks: u64,
    stage_visits: Vec<String>,
    readiness_millis: u128,
}

fn dogfood_output(
    metrics: &mut DogfoodMetricsV1,
    fixture: &FixtureV1,
    workspace: &Path,
    command: &str,
    arguments: &[&str],
) -> OutputEnvelopeV1 {
    metrics.command_count += 1;
    cli_output(fixture, workspace, command, arguments)
}

fn dogfood_status(
    metrics: &mut DogfoodMetricsV1,
    fixture: &FixtureV1,
    workspace: &Path,
) -> StatusResultV1 {
    let output = dogfood_output(metrics, fixture, workspace, "status", &[]);
    StatusResultV1::from_result_map(output.result())
        .expect("dogfood status must satisfy the public schema")
}

fn dogfood_next(
    metrics: &mut DogfoodMetricsV1,
    fixture: &FixtureV1,
    workspace: &Path,
) -> NextResultV1 {
    metrics.next_checks += 1;
    let output = dogfood_output(metrics, fixture, workspace, "next", &[]);
    NextResultV1::from_result_map(output.result())
        .expect("dogfood next must satisfy the public schema")
}

fn fill_current_dogfood_stage(
    metrics: &mut DogfoodMetricsV1,
    fixture: &FixtureV1,
    workspace: &Path,
    preset: &str,
    status: &StatusResultV1,
) {
    for item in status
        .items
        .iter()
        .filter(|item| item.required && !item.satisfied)
    {
        let id = item.id.as_str();
        match item.item_type {
            ItemTypeResultV1::Confirm => {
                dogfood_output(metrics, fixture, workspace, "check", &[id]);
            }
            ItemTypeResultV1::Text => {
                let value = format!("{preset} dogfood evidence for {id}");
                dogfood_output(metrics, fixture, workspace, "set", &[id, &value]);
            }
            ItemTypeResultV1::List => {
                let first = format!("{preset} primary evidence for {id}");
                let second = format!("{preset} challenge evidence for {id}");
                dogfood_output(metrics, fixture, workspace, "add", &[id, &first]);
                dogfood_output(metrics, fixture, workspace, "add", &[id, &second]);
            }
            ItemTypeResultV1::Artifact => {
                let relative = format!("{preset}-{id}.txt");
                fs::write(
                    workspace.join(&relative),
                    format!("{preset} production dogfood artifact for {id}\n"),
                )
                .expect("dogfood artifact must be written inside the isolated worktree");
                dogfood_output(
                    metrics,
                    fixture,
                    workspace,
                    "attach",
                    &[id, &relative, "--media-type", "text/plain"],
                );
            }
            ItemTypeResultV1::Choice => {
                dogfood_output(metrics, fixture, workspace, "set", &[id, "low"]);
            }
            ItemTypeResultV1::Integer => {
                panic!("shipped preset {preset} unexpectedly requires integer item {id}")
            }
        }
    }
}

fn dogfood_transition(
    preset: &str,
    stage: &str,
    visit: u32,
) -> Option<(&'static str, Option<&'static str>)> {
    match (preset, stage, visit) {
        ("sw-dev", "verify", 1)
        | ("bug-fix", "verify", 1)
        | ("docs-only", "validate", 1)
        | ("analysis", "challenge", 1) => Some(("retry", None)),
        ("sw-dev", "review", 1) => Some(("return", Some("implement"))),
        ("bug-fix", "review", 1) => Some(("return", Some("fix"))),
        ("docs-only", "validate", 2) => Some(("return", Some("draft"))),
        ("analysis", "challenge", 2) => Some(("return", Some("collect-sources"))),
        _ => None,
    }
}

fn run_dogfood_scenario(
    preset: &str,
    task: &str,
    expected_stages: &[&str],
    expected_topology: &[&str],
) -> DogfoodMetricsV1 {
    let fixture = FixtureV1::new();
    let daemon = RunningDaemonV1::start(&fixture);
    let workspace = fixture.worktree.clone();
    let mut metrics = DogfoodMetricsV1 {
        readiness_millis: daemon.readiness_millis(),
        ..Default::default()
    };
    dogfood_output(&mut metrics, &fixture, &workspace, "init", &[]);
    dogfood_output(
        &mut metrics,
        &fixture,
        &workspace,
        "start",
        &["--preset", preset, "--task", task],
    );

    let mut visits = BTreeMap::<String, u32>::new();
    for _ in 0..40 {
        let status = dogfood_status(&mut metrics, &fixture, &workspace);
        if status.current.is_none() {
            assert_eq!(status.session.lifecycle, SessionLifecycleV1::Completed);
            let completed_next = dogfood_next(&mut metrics, &fixture, &workspace);
            assert!(completed_next.stage.is_none());
            assert!(completed_next.missing_required_items.is_empty());
            assert_eq!(
                visits.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                expected_stages.iter().copied().collect::<BTreeSet<_>>(),
                "{preset} must visit exactly its declared preset stages"
            );
            assert_eq!(
                metrics
                    .stage_visits
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected_topology,
                "{preset} must preserve the declared ordered stage topology and revisits"
            );
            assert!(
                metrics.readiness_millis <= 10_000,
                "{preset} readiness must remain bounded"
            );
            assert!(metrics.retry_count >= 1, "{preset} must exercise retry");
            assert!(metrics.return_count >= 1, "{preset} must exercise return");
            return metrics;
        }

        let stage = current(&status).stage_id.as_str().to_owned();
        metrics.stage_visits.push(stage.clone());
        let visit = visits.entry(stage.clone()).or_insert(0);
        *visit += 1;
        let visit = *visit;
        let expected_missing = status
            .items
            .iter()
            .filter(|item| item.required && !item.satisfied)
            .map(|item| item.id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let required_prompts = status
            .items
            .iter()
            .filter(|item| item.required)
            .map(|item| item.prompt.trim())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            required_prompts.len(),
            status.items.iter().filter(|item| item.required).count(),
            "{preset}/{stage} required prompts must be nonempty and unambiguous within the stage"
        );
        assert!(required_prompts.iter().all(|prompt| !prompt.is_empty()));
        let before_next = dogfood_next(&mut metrics, &fixture, &workspace);
        let reported_missing = before_next
            .missing_required_items
            .iter()
            .map(|item| item.id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            reported_missing, expected_missing,
            "{preset}/{stage} next must identify every unsatisfied required item"
        );
        for missing in &expected_missing {
            assert!(
                before_next.suggestions.iter().any(|suggestion| {
                    suggestion
                        .item_id
                        .as_ref()
                        .is_some_and(|item_id| item_id.as_str() == missing)
                }),
                "{preset}/{stage} next must suggest a command for {missing}"
            );
        }
        fill_current_dogfood_stage(&mut metrics, &fixture, &workspace, preset, &status);
        let ready = dogfood_status(&mut metrics, &fixture, &workspace);
        assert!(
            current(&ready).ready_to_complete,
            "{preset}/{stage} must become ready after public item commands"
        );
        let ready_next = dogfood_next(&mut metrics, &fixture, &workspace);
        assert!(ready_next.missing_required_items.is_empty());
        assert!(ready_next.allowed_actions.complete);
        assert!(
            ready_next
                .suggestions
                .iter()
                .any(|suggestion| suggestion.command == "session.complete"),
            "{preset}/{stage} next must suggest completion when ready"
        );

        match dogfood_transition(preset, &stage, visit) {
            Some(("retry", None)) => {
                metrics.retry_count += 1;
                dogfood_output(
                    &mut metrics,
                    &fixture,
                    &workspace,
                    "retry",
                    &["--reason", "phase7 dogfood retry for clarity"],
                );
            }
            Some(("return", Some(destination))) => {
                metrics.return_count += 1;
                dogfood_output(
                    &mut metrics,
                    &fixture,
                    &workspace,
                    "return",
                    &[
                        "--to",
                        destination,
                        "--reason",
                        "phase7 dogfood return after review",
                    ],
                );
            }
            None => {
                dogfood_output(&mut metrics, &fixture, &workspace, "complete", &[]);
            }
            transition => panic!("invalid dogfood transition {transition:?}"),
        }
    }
    panic!("{preset} dogfood exceeded the bounded transition budget")
}

#[test]
#[ignore = "run with tools/run_g008_dogfood.py so Cargo supplies a freshly built podwayd artifact"]
fn public_cli_dogfoods_all_four_presets_with_retry_return_and_next_evidence() {
    let scenarios: [(&str, &str, &[&str], &[&str]); 4] = [
        (
            "sw-dev",
            "Implement and verify the Phase 7 four-preset production dogfood harness",
            &[
                "understand",
                "inspect",
                "plan",
                "implement",
                "verify",
                "review",
                "finish",
            ],
            &[
                "understand",
                "inspect",
                "plan",
                "implement",
                "verify",
                "verify",
                "review",
                "implement",
                "verify",
                "review",
                "finish",
            ],
        ),
        (
            "bug-fix",
            "Correct deterministic service-log rotation after a stale restart",
            &[
                "reproduce",
                "diagnose",
                "regression",
                "fix",
                "verify",
                "review",
                "finish",
            ],
            &[
                "reproduce",
                "diagnose",
                "regression",
                "fix",
                "verify",
                "verify",
                "review",
                "fix",
                "verify",
                "review",
                "finish",
            ],
        ),
        (
            "docs-only",
            "Document the direct offline macOS LaunchAgent lifecycle for operators",
            &[
                "ground-sources",
                "define-audience",
                "outline",
                "draft",
                "validate",
                "review",
                "finish",
            ],
            &[
                "ground-sources",
                "define-audience",
                "outline",
                "draft",
                "validate",
                "validate",
                "draft",
                "validate",
                "review",
                "finish",
            ],
        ),
        (
            "analysis",
            "Assess whether socket reachability is sufficient for service health reporting",
            &[
                "define-question",
                "collect-sources",
                "analyze",
                "challenge",
                "synthesize",
                "finish",
            ],
            &[
                "define-question",
                "collect-sources",
                "analyze",
                "challenge",
                "challenge",
                "collect-sources",
                "analyze",
                "challenge",
                "synthesize",
                "finish",
            ],
        ),
    ];
    let mut evidence = serde_json::Map::new();
    for (preset, task, expected_stages, expected_topology) in scenarios {
        let metrics = run_dogfood_scenario(preset, task, expected_stages, expected_topology);
        evidence.insert(
            preset.to_owned(),
            serde_json::json!({
                "commands": metrics.command_count,
                "next_checks": metrics.next_checks,
                "readiness_millis": metrics.readiness_millis,
                "retry": metrics.retry_count,
                "return": metrics.return_count,
                "stage_topology": metrics.stage_visits,
            }),
        );
    }
    println!("G008_DOGFOOD_EVIDENCE={}", Value::Object(evidence));
}
