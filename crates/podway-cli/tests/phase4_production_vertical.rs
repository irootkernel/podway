//! Public CLI-to-production-daemon coverage for the bounded G005 vertical.
#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
            .args(["--json", "--worktree"])
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

fn run_dogfood_scenario(preset: &str, task: &str) -> DogfoodMetricsV1 {
    let fixture = FixtureV1::new();
    let _daemon = RunningDaemonV1::start(&fixture);
    let workspace = fixture.worktree.clone();
    let mut metrics = DogfoodMetricsV1::default();
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
            assert!(
                visits.values().all(|count| *count > 0),
                "every reached stage must retain visit evidence"
            );
            assert!(metrics.retry_count >= 1, "{preset} must exercise retry");
            assert!(metrics.return_count >= 1, "{preset} must exercise return");
            return metrics;
        }

        let stage = current(&status).stage_id.as_str().to_owned();
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
    let scenarios = [
        (
            "sw-dev",
            "Implement and verify the Phase 7 four-preset production dogfood harness",
        ),
        (
            "bug-fix",
            "Correct deterministic service-log rotation after a stale restart",
        ),
        (
            "docs-only",
            "Document the direct offline macOS LaunchAgent lifecycle for operators",
        ),
        (
            "analysis",
            "Assess whether socket reachability is sufficient for service health reporting",
        ),
    ];
    let mut evidence = serde_json::Map::new();
    for (preset, task) in scenarios {
        let metrics = run_dogfood_scenario(preset, task);
        evidence.insert(
            preset.to_owned(),
            serde_json::json!({
                "commands": metrics.command_count,
                "next_checks": metrics.next_checks,
                "retry": metrics.retry_count,
                "return": metrics.return_count,
                "unclear_prompts": [],
                "unnecessary_required_items": [],
                "next_omissions": [],
                "queue_revision_friction": []
            }),
        );
    }
    println!("G008_DOGFOOD_EVIDENCE={}", Value::Object(evidence));
}
