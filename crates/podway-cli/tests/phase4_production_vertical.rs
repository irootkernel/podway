//! Public CLI-to-production-daemon coverage for the bounded G005 vertical.
#![forbid(unsafe_code)]

use std::{
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::{Pid, geteuid},
};
use podway_cli::client::DaemonClientV1;
use podway_core::WorkspaceId;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, NextResultV1, OperationV1, OutputEnvelopeV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV1, SessionLifecycleV1, StageStatusResultV1, StatusResultV1,
    WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::{Map, Value};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct FixtureV1 {
    root: PathBuf,
    home: PathBuf,
    temporary: PathBuf,
    paths: ServiceRuntimePathsV1,
    worktree: PathBuf,
}

impl FixtureV1 {
    fn new() -> Self {
        let root = unique_private_directory("pw4v");
        let home = root.join("h");
        let temporary = root.join("t");
        let worktree = root.join("w");
        fs::create_dir(&home).expect("fixture HOME must be created");
        fs::create_dir(&temporary).expect("fixture TMPDIR must be created");
        make_private(&home);
        make_private(&temporary);
        create_non_bare_worktree(&worktree);
        let registry_parent = home.join("Library/Application Support/Podway");
        fs::create_dir_all(&registry_parent).expect("fixture registry parent must be created");
        make_private(&registry_parent);

        let paths = ServiceRuntimePathsV1::for_user(&home, &temporary, geteuid().as_raw())
            .expect("short private fixture paths must be valid");
        Self {
            root,
            home,
            temporary,
            paths,
            worktree,
        }
    }

    fn run(&self, workspace: &Path, command: &str, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(["--json", "--workspace"])
            .arg(workspace)
            .arg(command)
            .args(arguments)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.temporary)
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
    assert_daemon_binary_is_current(&binary);
    binary
}

fn assert_daemon_binary_is_current(binary: &Path) {
    let modified = fs::metadata(binary)
        .and_then(|metadata| metadata.modified())
        .unwrap_or_else(|error| {
            panic!("podwayd test binary modification time must be readable: {error}")
        });
    let newest_source = newest_daemon_source_modification();
    assert!(
        modified >= newest_source,
        "podwayd test binary is older than a current daemon input; rebuild it and pass its path via PODWAYD_TEST_BINARY (binary: {}, modified: {modified:?}, source: {newest_source:?})",
        binary.display()
    );
}

fn newest_daemon_source_modification() -> SystemTime {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("podway-cli manifest must be nested under the workspace root");
    let mut newest = UNIX_EPOCH;
    for path in [
        workspace.join("Cargo.toml"),
        workspace.join("Cargo.lock"),
        workspace.join("presets"),
        workspace.join("spec"),
    ] {
        record_newest_modification(&path, &mut newest);
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
        record_newest_modification(&crate_root.join("Cargo.toml"), &mut newest);
        record_newest_modification(&crate_root.join("src"), &mut newest);
    }
    newest
}

fn record_newest_modification(path: &Path, newest: &mut SystemTime) {
    let metadata = fs::metadata(path).unwrap_or_else(|error| {
        panic!(
            "daemon source input {} must be readable: {error}",
            path.display()
        )
    });
    if metadata.is_dir() {
        for entry in fs::read_dir(path).unwrap_or_else(|error| {
            panic!(
                "daemon source directory {} must be readable: {error}",
                path.display()
            )
        }) {
            let entry = entry.unwrap_or_else(|error| {
                panic!(
                    "daemon source directory entry {} must be readable: {error}",
                    path.display()
                )
            });
            record_newest_modification(&entry.path(), newest);
        }
    } else if metadata.is_file() && is_daemon_source_input(path) {
        let modified = metadata.modified().unwrap_or_else(|error| {
            panic!(
                "daemon source input modification time must be readable for {}: {error}",
                path.display()
            )
        });
        if modified > *newest {
            *newest = modified;
        }
    }
}
fn is_daemon_source_input(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("rs" | "toml" | "yaml" | "yml" | "sql")
    ) || path.file_name().and_then(|name| name.to_str()) == Some("Cargo.lock")
}
struct RunningDaemonV1 {
    child: Option<Child>,
    socket_path: PathBuf,
}

impl RunningDaemonV1 {
    fn start(fixture: &FixtureV1) -> Self {
        let daemon_binary = daemon_binary_for_test();
        let socket_path = fixture.paths.socket_path().as_path().to_path_buf();
        let mut child = Command::new(&daemon_binary)
            .current_dir(&fixture.root)
            .env("HOME", &fixture.home)
            .env("TMPDIR", &fixture.temporary)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the real podwayd binary must start");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if socket_path.exists() {
                break;
            }
            if let Some(status) = child
                .try_wait()
                .expect("podwayd process state must be observable")
            {
                let output = child
                    .wait_with_output()
                    .expect("terminated podwayd output must be readable");
                panic!(
                    "podwayd exited before binding ({status}): {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let output = child
                    .wait_with_output()
                    .expect("timed-out podwayd output must be readable");
                panic!(
                    "podwayd did not bind within ten seconds: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
        Self {
            child: Some(child),
            socket_path,
        }
    }

    fn stop(mut self) {
        let child = self.child.take().expect("podwayd process must exist");
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
            !self.socket_path.exists(),
            "normal runtime shutdown must remove its owned socket"
        );
    }
}

impl Drop for RunningDaemonV1 {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn unique_private_directory(prefix: &str) -> PathBuf {
    for _ in 0..1024 {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path =
            PathBuf::from("/tmp").join(format!("{prefix}-{}-{sequence}", std::process::id()));
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
            "success response must contain {key}"
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
        "start" => "preset.start",
        "status" => "session.status",
        "next" => "session.next",
        "check" => "item.check",
        "set" => "item.set",
        "add" => "item.add",
        "attach" => "item.attach_path",
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

fn direct_item_set(
    request_number: u64,
    workspace_path: &Path,
    workspace_uuid: WorkspaceId,
    status: &StatusResultV1,
    item_id: &str,
    value: &str,
    idempotency_key: &str,
) -> RequestEnvelopeV1 {
    let canonical = fs::canonicalize(workspace_path).expect("direct request worktree must exist");
    let selector = WorktreeSelectorWireV1::new(
        canonical.as_os_str().as_bytes(),
        canonical.display().to_string(),
        Some(workspace_uuid.clone()),
    )
    .expect("direct request selector must be valid");
    let active = current(status);
    let preconditions = PreconditionsV1::new(
        None,
        None,
        Some(active.attempt_id.clone()),
        Some(item(status, item_id).revision),
        None,
        None,
    )
    .expect("same-item mutation preconditions must be valid");
    let mut payload = Map::new();
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(&selector).expect("direct request selector must encode"),
    );
    payload.insert("item_id".to_owned(), Value::String(item_id.to_owned()));
    payload.insert("value".to_owned(), Value::String(value.to_owned()));
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .expect("direct request ID must be valid"),
        client: ClientInfoV1::new("phase4-production-vertical", "1", 1)
            .expect("direct request client metadata must be valid"),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("item.set").expect("direct item.set command must be valid"),
        workspace: Some(
            WorkspaceContextV1::new(canonical.display().to_string(), Some(workspace_uuid))
                .expect("direct workspace context must be valid"),
        ),
        idempotency_key: Some(
            IdempotencyKeyV1::new(idempotency_key).expect("direct idempotency key must be valid"),
        ),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000)
            .expect("direct request wait options must be valid"),
        payload,
    })
    .expect("direct item.set request must satisfy the public protocol")
}

fn output_response<'a>(response: &'a ResponseEnvelopeV1, action: &str) -> &'a OutputEnvelopeV1 {
    match response {
        ResponseEnvelopeV1::Output(output) => output,
        ResponseEnvelopeV1::Error(error) => panic!(
            "{action} returned unexpected {} error: {}",
            error.code().as_str(),
            error.message()
        ),
    }
}

fn assert_same_terminal_result(
    replay: &ResponseEnvelopeV1,
    original: &ResponseEnvelopeV1,
    context: &str,
) {
    let replay = output_response(replay, context);
    let original = output_response(original, context);
    assert_eq!(replay.command(), original.command(), "{context}: command");
    assert_eq!(replay.job(), original.job(), "{context}: durable job");
    assert_eq!(
        replay.session(),
        original.session(),
        "{context}: terminal session"
    );
    assert_eq!(
        replay.result(),
        original.result(),
        "{context}: terminal result"
    );
    assert_eq!(
        replay.warnings(),
        original.warnings(),
        "{context}: terminal warnings"
    );
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
    let digest = artifact
        .get("sha256_digest")
        .and_then(Value::as_str)
        .expect("public status must expose the stored artifact digest");
    assert!(
        digest.len() == "sha256:".len() + 64
            && digest
                .strip_prefix("sha256:")
                .is_some_and(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "public status must expose the stored SHA-256 artifact digest"
    );

    let artifact_path = workspace.join(verification_artifact);
    fs::write(
        &artifact_path,
        "the artifact changed after public attachment\n",
    )
    .expect("attached artifact must be changed for revalidation coverage");
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
            .expect("preset.start must expose its durable job")
            .finished_at()
            .is_some(),
        "preset.start must wait for its terminal durable job"
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

    let client = DaemonClientV1::new(fixture.paths.clone());
    let concurrent_left = direct_item_set(
        101,
        &workspace,
        workspace_uuid.clone(),
        &retried_status,
        "goal",
        "concurrent-left",
        "public-concurrent-left",
    );
    let concurrent_right = direct_item_set(
        102,
        &workspace,
        workspace_uuid.clone(),
        &retried_status,
        "goal",
        "concurrent-right",
        "public-concurrent-right",
    );
    assert_eq!(
        concurrent_left.preconditions(),
        concurrent_right.preconditions(),
        "concurrent public IPC requests must start from identical same-item preconditions"
    );
    let barrier = Arc::new(Barrier::new(3));
    let left_barrier = Arc::clone(&barrier);
    let left_client = client.clone();
    let left = thread::spawn(move || {
        left_barrier.wait();
        left_client
            .request(&concurrent_left)
            .expect("left public IPC request must receive one framed response")
    });
    let right_barrier = Arc::clone(&barrier);
    let right_client = client.clone();
    let right = thread::spawn(move || {
        right_barrier.wait();
        right_client
            .request(&concurrent_right)
            .expect("right public IPC request must receive one framed response")
    });
    barrier.wait();
    let left = left
        .join()
        .expect("left public IPC client thread must not panic");
    let right = right
        .join()
        .expect("right public IPC client thread must not panic");
    let mut successful = 0;
    let mut conflicts = 0;
    for response in [left, right] {
        match response {
            ResponseEnvelopeV1::Output(output) => {
                successful += 1;
                assert!(
                    output
                        .job()
                        .expect("successful concurrent mutation must expose a durable job")
                        .finished_at()
                        .is_some(),
                    "successful concurrent mutation must be terminal"
                );
            }
            ResponseEnvelopeV1::Error(error) => {
                conflicts += 1;
                assert_eq!(error.code().as_str(), "ITEM_REVISION_CONFLICT");
                assert!(error.retryable(), "revision conflict must remain retryable");
                assert_eq!(error.exit_code().get(), 4);
            }
        }
    }
    assert_eq!(successful, 1, "exactly one same-item mutation must succeed");
    assert_eq!(
        conflicts, 1,
        "exactly one same-item mutation must return the stable revision conflict"
    );

    let (_, concurrent_status) = public_status(&fixture, &workspace);
    assert!(
        matches!(
            &item(&concurrent_status, "goal").value,
            Value::String(value) if value == "concurrent-left" || value == "concurrent-right"
        ),
        "the authoritative public status must retain exactly one concurrent winner"
    );
    let immutable_request = direct_item_set(
        103,
        &workspace,
        workspace_uuid.clone(),
        &concurrent_status,
        "goal",
        "idempotent terminal result must remain immutable",
        "public-exact-replay",
    );
    let immutable_first = client
        .request(&immutable_request)
        .expect("first exact framed mutation must receive a response");
    assert!(
        output_response(&immutable_first, "first exact framed mutation")
            .job()
            .expect("first exact framed mutation must expose a durable job")
            .finished_at()
            .is_some(),
        "first exact framed mutation must be terminal"
    );
    let immutable_before_transition = client
        .request(&immutable_request)
        .expect("replayed framed mutation before transition must receive a response");
    assert_same_terminal_result(
        &immutable_before_transition,
        &immutable_first,
        "exact replay before transition",
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
    let immutable_after_transition = client
        .request(&immutable_request)
        .expect("replayed framed mutation after transition must receive a response");
    assert_same_terminal_result(
        &immutable_after_transition,
        &immutable_first,
        "exact replay after transition without reapplication",
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
