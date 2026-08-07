//! CLI process-boundary integration contracts using controlled collaborators.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

use nix::unistd::geteuid;
use podway_protocol::{
    OperationV1, RequestEnvelopeV1, ResponseEnvelopeV1, decode_request_payload_v1,
    decode_single_frame_v1, encode_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;

use serde_json::Value;

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(arguments)
        .env(
            "PODWAY_TEST_ACCOUNT_ROOT",
            format!("/tmp/podway-cli-phase5-{}", std::process::id()),
        )
        .env_remove("HOME")
        .env_remove("TMPDIR")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("podway binary must run")
}
fn registered_command_catalog_route_availability() -> BTreeMap<String, String> {
    let catalog = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/specifications/command-catalog.yaml"),
    )
    .expect("the frozen production command catalog must be readable");
    let mut routes = BTreeMap::new();
    let mut lines = catalog.lines();
    while let Some(line) = lines.next() {
        let Some(route) = line.strip_prefix("- name: ") else {
            continue;
        };
        let availability = lines
            .next()
            .and_then(|line| line.strip_prefix("  availability: "))
            .expect("every registered command must declare availability after its name");
        assert!(
            matches!(availability, "executable" | "reserved_contract"),
            "command availability must use the closed contract enum",
        );
        assert!(
            routes
                .insert(route.to_owned(), availability.to_owned())
                .is_none(),
            "the frozen command catalog must not repeat routes",
        );
    }
    assert_eq!(
        routes.len(),
        59,
        "the registered command catalog must contain the 50 executable routes and 9 reserved v2 routes"
    );
    routes
}

fn registered_command_route_contract_availability() -> BTreeMap<String, String> {
    let contract: Value =
        serde_json::from_str(include_str!("../../../contracts/command-routes.json"))
            .expect("the command route contract must be valid JSON");
    contract["routes"]
        .as_array()
        .expect("the command route contract must contain routes")
        .iter()
        .map(|route| {
            (
                route["command"]
                    .as_str()
                    .expect("route command must be a string")
                    .to_owned(),
                route["availability"]
                    .as_str()
                    .expect("route availability must be a string")
                    .to_owned(),
            )
        })
        .collect()
}

fn one_json(output: &Output) -> Value {
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).lines().count(),
        1,
        "JSON mode must emit exactly one stdout object: {output:?}"
    );
    serde_json::from_slice(&output.stdout).expect("stdout must be JSON")
}
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique_fixture_path(label: &str) -> PathBuf {
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}-{}-{sequence}", std::process::id()))
}

fn unique_short_fixture_path() -> PathBuf {
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("pw5-{}-{sequence}", std::process::id()))
}

struct CompletionScript {
    path: PathBuf,
}

impl CompletionScript {
    fn generated(shell: &str) -> Self {
        let output = run(&["completions", shell]);
        assert!(
            output.status.success(),
            "completion generation for {shell} failed: {output:?}"
        );
        let path = unique_fixture_path("podway-completion-script");
        fs::write(&path, output.stdout).expect("completion script fixture must be writable");
        Self { path }
    }
}

impl Drop for CompletionScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn shell_available(shell: &str) -> bool {
    Command::new(shell).arg("--version").output().is_ok()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn generated_dynamic_candidates(
    shell: &str,
    script: &CompletionScript,
    fixture: &DynamicCompletionFixture,
    bin: &Path,
    current_dir: &Path,
    words: &[String],
) -> (Vec<String>, Vec<u8>) {
    let mut effective_words = words.to_vec();
    let has_socket = effective_words
        .iter()
        .any(|word| word == "--socket" || word.starts_with("--socket="));
    if fixture.socket_path.exists() && !has_socket {
        effective_words.splice(
            1..1,
            [
                "--socket".to_owned(),
                fixture.socket_path.display().to_string(),
            ],
        );
    }
    let rendered_words = effective_words
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var_os("PATH")
            .expect("test PATH must be configured")
            .to_string_lossy()
    );
    let output = match shell {
        "bash" => {
            let program = format!(
                "source \"$1\"\nCOMP_WORDS=({rendered_words})\nCOMP_CWORD={}\n_podway\nprintf '%s\\n' \"${{COMPREPLY[@]}}\"\n",
                effective_words.len() - 1
            );
            Command::new("bash")
                .args(["-c", &program, "bash"])
                .arg(&script.path)
                .current_dir(current_dir)
                .env("PATH", &path)
                .env_remove("HOME")
                .env_remove("TMPDIR")
                .env_remove("XDG_CONFIG_HOME")
                .output()
                .expect("bash must run")
        }
        "zsh" => {
            let program = format!(
                "autoload -Uz compinit\ncompinit -D -i\nsource \"$1\"\nwords=({rendered_words})\nCURRENT={}\nroute=$(_podway_route)\n_podway_candidates \"$route\"\n",
                effective_words.len()
            );
            Command::new("zsh")
                .args(["-fc", &program, "zsh"])
                .arg(&script.path)
                .current_dir(current_dir)
                .env("PATH", &path)
                .env_remove("HOME")
                .env_remove("TMPDIR")
                .env_remove("XDG_CONFIG_HOME")
                .output()
                .expect("zsh must run")
        }
        "fish" => {
            let command_line = format!(
                "{} ",
                effective_words
                    .iter()
                    .filter(|word| !word.is_empty())
                    .map(|word| shell_quote(word))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            Command::new("fish")
                .args(["-c", "source $argv[1]; complete -C \"$argv[2]\""])
                .arg(&script.path)
                .arg(command_line)
                .current_dir(current_dir)
                .env("PATH", &path)
                .env_remove("HOME")
                .env_remove("TMPDIR")
                .env_remove("XDG_CONFIG_HOME")
                .output()
                .expect("fish must run")
        }
        _ => panic!("unsupported completion shell {shell}"),
    };
    assert!(
        output.status.success(),
        "{shell} dynamic completion execution failed: {output:?}"
    );
    let stderr = output.stderr;
    let lines = String::from_utf8(output.stdout)
        .expect("{shell} dynamic completion output must be UTF-8")
        .lines()
        .map(|line| {
            if shell == "fish" {
                line.split_once('\t')
                    .map_or(line, |(candidate, _)| candidate)
                    .to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect();
    (lines, stderr)
}

struct DynamicCompletionFixture {
    root: PathBuf,
    socket_path: PathBuf,
    dev_home: PathBuf,
}

impl DynamicCompletionFixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("pdc-{}-{sequence}", std::process::id()));
        let home = root.join("home");
        fs::create_dir_all(&home).expect("fixture home must be created");
        let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw())
            .expect("fixture paths must be valid");
        fs::create_dir_all(paths.runtime_directory().as_path())
            .expect("fixture daemon runtime directory must be created");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("fixture root must be private");
        fs::set_permissions(
            paths.runtime_directory().as_path(),
            fs::Permissions::from_mode(0o700),
        )
        .expect("fixture runtime directory must be private");
        Self {
            dev_home: root.join("dev"),
            root,
            socket_path: paths.socket_path().as_path().to_path_buf(),
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let arguments = self.arguments_with_explicit_endpoint(arguments);
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(&arguments)
            .env("PODWAY_DEV_HOME", &self.dev_home)
            .env_remove("HOME")
            .env_remove("TMPDIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("podway binary must run")
    }
    fn run_in(&self, directory: &Path, arguments: &[String]) -> Output {
        let arguments = self.arguments_with_explicit_endpoint(
            &arguments.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(&arguments)
            .current_dir(directory)
            .env("PODWAY_DEV_HOME", &self.dev_home)
            .env_remove("HOME")
            .env_remove("TMPDIR")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("podway binary must run")
    }

    fn arguments_with_explicit_endpoint(&self, arguments: &[&str]) -> Vec<String> {
        let mut resolved = Vec::with_capacity(arguments.len() + 2);
        if self.socket_path.exists()
            && command_accepts_explicit_socket(arguments)
            && !arguments
                .iter()
                .any(|argument| *argument == "--socket" || argument.starts_with("--socket="))
        {
            resolved.push("--socket".to_owned());
            resolved.push(self.socket_path.display().to_string());
        }
        resolved.extend(arguments.iter().map(|argument| (*argument).to_owned()));
        resolved
    }

    fn install_cli_on_path(&self) -> PathBuf {
        let bin = self.root.join("bin");
        fs::create_dir(&bin).expect("completion fixture bin directory must be created");
        symlink(env!("CARGO_BIN_EXE_podway"), bin.join("podway"))
            .expect("completion fixture podway link must be created");
        bin
    }
}

fn command_accepts_explicit_socket(arguments: &[&str]) -> bool {
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        match *argument {
            "--json" | "--no-color" | "--quiet" | "--yes" => index += 1,
            "--worktree" | "--timeout" | "--daemon-path" => index += 2,
            argument if argument.starts_with('-') => index += 1,
            "version" | "completions" | "help" | "preset" | "procedure" => return false,
            "daemon" => return arguments.get(index + 1) == Some(&"install"),
            _ => return true,
        }
    }
    false
}

impl Drop for DynamicCompletionFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct DynamicCompletionServer {
    handle: JoinHandle<io::Result<(String, OperationV1)>>,
}

impl DynamicCompletionServer {
    fn start(fixture: &DynamicCompletionFixture, result: Value) -> Self {
        let listener = UnixListener::bind(&fixture.socket_path)
            .expect("fake daemon socket must bind at the service-owned path");
        fs::set_permissions(&fixture.socket_path, fs::Permissions::from_mode(0o600))
            .expect("fake daemon socket must be private");
        let handle = thread::spawn(move || {
            let (mut connection, _) = listener.accept()?;
            let mut wire = Vec::new();
            connection.read_to_end(&mut wire)?;
            let request = decode_request_payload_v1(
                decode_single_frame_v1(&wire)
                    .map_err(|error| io::Error::other(error.to_string()))?,
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
            let response = serde_json::json!({
                "schema": "podway.output/v1",
                "request_id": request.request_id().as_str(),
                "command": request.command().as_str(),
                "generated_at": "2026-07-16T12:34:56.789Z",
                "result": result,
                "warnings": [],
            });
            let frame = encode_frame_v1(
                &serde_json::to_vec(&response).expect("dynamic completion response must serialize"),
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
            connection.write_all(&frame)?;
            connection.shutdown(Shutdown::Write)?;
            Ok((request.command().as_str().to_owned(), request.operation()))
        });
        Self { handle }
    }

    fn finish(self) -> (String, OperationV1) {
        self.handle
            .join()
            .expect("fake daemon thread must not panic")
            .expect("fake daemon I/O must succeed")
    }
}

fn authoritative_status_result(items: Value) -> Value {
    authoritative_status_result_with_blockers(items, serde_json::json!([]))
}

fn authoritative_status_result_with_blockers(items: Value, blockers: Value) -> Value {
    serde_json::json!({
        "schema": "podway.status-result/v1",
        "task": {
            "title": "Completion fixture",
            "procedure": {
                "id": "fixture",
                "version": "1",
                "name": "Fixture",
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        },
        "session": {
            "id": "123e4567-e89b-42d3-a456-426614174004",
            "lifecycle": "running",
            "revision": 1,
            "created_at": "2026-07-16T12:34:56.789Z",
            "completed_at": null,
            "cancelled_at": null
        },
        "current": {
            "stage_id": "implement",
            "stage_index": 0,
            "title": "Implement",
            "attempt_id": "123e4567-e89b-42d3-a456-426614174002",
            "attempt_number": 1,
            "blocked": false,
            "ready_to_complete": false
        },
        "stages": [{
            "id": "implement",
            "index": 0,
            "title": "Implement",
            "status": "current",
            "latest_attempt_number": 1
        }],
        "items": items,
        "blockers": blockers,
        "queue": {
            "pending_mutations": false,
            "queued_count": 0,
            "running_job_id": null,
            "latest_workspace_sequence": 1
        }
    })
}

fn authoritative_compact_status_result() -> Value {
    serde_json::json!({
        "schema": "podway.compact-status-result/v1",
        "procedure": {
            "id": "fixture",
            "version": "1",
            "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        },
        "session": {
            "id": "123e4567-e89b-42d3-a456-426614174004",
            "lifecycle": "running",
            "revision": 1
        },
        "current": {
            "stage_id": "implement",
            "attempt_id": "123e4567-e89b-42d3-a456-426614174002",
            "attempt_number": 1,
            "ready_to_complete": false
        },
        "items": [{
            "id": "decision",
            "type": "confirm",
            "required": true,
            "satisfied": false,
            "revision": 0
        }],
        "blockers": [{
            "id": "123e4567-e89b-42d3-a456-426614174006",
            "attempt_id": "123e4567-e89b-42d3-a456-426614174002",
            "state": "open"
        }],
        "queue": {
            "pending_mutations": false,
            "queued_count": 0,
            "running_job_id": null,
            "latest_workspace_sequence": 1
        }
    })
}

fn authoritative_next_result() -> Value {
    serde_json::json!({
        "schema": "podway.next-result/v1",
        "stage": {
            "id": "implement",
            "title": "Implement",
            "attempt_id": "123e4567-e89b-42d3-a456-426614174002",
            "attempt_number": 1,
            "instructions": []
        },
        "missing_required_items": [],
        "blockers": [],
        "allowed_actions": {
            "complete": false,
            "skip": false,
            "retry": false,
            "return_to": ["plan", "intake"],
            "cancel": true
        },
        "next_stage_after_completion": null,
        "suggestions": []
    })
}

fn authoritative_job_list_result() -> Value {
    serde_json::json!({
        "jobs": [{
            "id": "123e4567-e89b-42d3-a456-426614174005",
            "sequence": 7,
            "state": "queued",
            "submitted_at": "2026-07-16T12:34:56.789Z",
            "finished_at": null
        }]
    })
}
const RECORDING_WORKSPACE_ID: &str = "123e4567-e89b-42d3-a456-426614174001";

#[derive(Clone)]
enum RecordingReply {
    Output(Value),
    Error,
}

struct RecordingDaemon {
    handle: JoinHandle<io::Result<Vec<Value>>>,
}

impl RecordingDaemon {
    fn start(fixture: &DynamicCompletionFixture, replies: Vec<RecordingReply>) -> Self {
        Self::start_at(fixture.socket_path.clone(), replies)
    }

    fn start_at(socket_path: PathBuf, replies: Vec<RecordingReply>) -> Self {
        if socket_path.exists() {
            fs::remove_file(&socket_path)
                .expect("previous recording daemon socket must be removed");
        }
        let listener = UnixListener::bind(socket_path)
            .expect("recording daemon socket must bind at the service-owned path");
        fs::set_permissions(
            listener
                .local_addr()
                .expect("recording daemon socket address must be readable")
                .as_pathname()
                .expect("recording daemon socket must be named"),
            fs::Permissions::from_mode(0o600),
        )
        .expect("recording daemon socket must be private");
        let handle = thread::spawn(move || {
            let mut requests = Vec::with_capacity(replies.len());
            for reply in replies {
                let (mut connection, _) = listener.accept()?;
                let mut wire = Vec::new();
                connection.read_to_end(&mut wire)?;
                let request = decode_request_payload_v1(
                    decode_single_frame_v1(&wire)
                        .map_err(|error| io::Error::other(error.to_string()))?,
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                requests.push(
                    serde_json::to_value(&request)
                        .expect("recording daemon request must serialize"),
                );
                let response = recording_response(&request, reply);
                let frame = encode_frame_v1(
                    &serde_json::to_vec(&response)
                        .expect("recording daemon response must serialize"),
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                connection.write_all(&frame)?;
            }
            Ok(requests)
        });
        Self { handle }
    }

    fn finish(self) -> Vec<Value> {
        self.handle
            .join()
            .expect("recording daemon thread must not panic")
            .expect("recording daemon I/O must succeed")
    }
}

fn recording_response(request: &RequestEnvelopeV1, reply: RecordingReply) -> Value {
    match reply {
        RecordingReply::Output(result) => {
            let admitted = result.get("admission").is_some();
            let mut envelope = serde_json::json!({
                "schema": "podway.output/v1",
                "request_id": request.request_id().as_str(),
                "command": request.command().as_str(),
                "generated_at": "2026-07-16T12:34:56.789Z",
                "workspace": {
                    "uuid": RECORDING_WORKSPACE_ID,
                    "root": request.workspace().expect("recorded request must select a workspace").root(),
                    "latest_workspace_sequence": 1
                },
                "result": result,
                "warnings": []
            });
            if admitted {
                envelope["job"] = serde_json::json!({
                    "id": RECORDING_JOB_ID,
                    "sequence": 1,
                    "state": "succeeded",
                    "submitted_at": "2026-07-16T12:34:56.789Z",
                    "claimed_at": "2026-07-16T12:34:56.789Z",
                    "finished_at": "2026-07-16T12:34:56.789Z"
                });
            }
            envelope
        }
        RecordingReply::Error => serde_json::json!({
            "schema": "podway.error/v1",
            "request_id": request.request_id().as_str(),
            "command": request.command().as_str(),
            "generated_at": "2026-07-16T12:34:56.789Z",
            "code": "WORKSPACE_NOT_INITIALIZED",
            "message": "recording daemon rejection",
            "retryable": false,
            "exit_code": 5,
            "details": {}
        }),
    }
}

fn recording_success_result(command: &str) -> Value {
    let admission = serde_json::json!({
        "admitted": true,
        "job_id": RECORDING_JOB_ID,
        "workspace_sequence": 1
    });
    match command {
        "session.status" => authoritative_status_result(serde_json::json!([{
            "id": "item",
            "type": "text",
            "prompt": "Record an item",
            "required": false,
            "satisfied": false,
            "revision": 1,
            "value": null
        }])),
        "session.next" => authoritative_next_result(),
        "job.list" => authoritative_job_list_result(),
        "job.lookup" => serde_json::json!({
            "schema": "podway.job-lookup-result/v1",
            "found": false
        }),
        "workspace.init" => serde_json::json!({
            "schema": "podway.workspace-init-result/v1",
            "initialized": true,
            "revision": 0,
            "admission": admission
        }),
        "workspace.doctor" => serde_json::json!({"deep": true, "healthy": true}),
        "workspace.show" => serde_json::json!({"workspace": "recorded"}),
        "workspace.repair" => serde_json::json!({"repaired": true}),
        "session.start" | "session.start_replace" => serde_json::json!({
            "schema": "podway.session-start-result/v1",
            "changed": true,
            "revision_before": 1,
            "revision_after": 2,
            "procedure_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "admission": admission
        }),
        "session.complete" | "session.skip" | "session.retry" | "session.return"
        | "session.block" | "session.unblock" | "session.cancel" | "session.reopen" => {
            serde_json::json!({
                "schema": "podway.stage-transition-result/v1",
                "changed": true,
                "revision_before": 1,
                "revision_after": 2,
                "admission": admission
            })
        }
        "session.reset" => serde_json::json!({
            "schema": "podway.stage-transition-result/v1",
            "reset": true,
            "revision": 2,
            "admission": admission
        }),
        "item.check" | "item.uncheck" | "item.set" | "item.add" | "item.remove" | "item.attach"
        | "item.clear" => serde_json::json!({
            "schema": "podway.item-mutation-result/v1",
            "changed": true,
            "item_id": "item",
            "revision_before": 1,
            "revision_after": 2,
            "admission": admission
        }),
        "workspace.reset_all" | "job.cancel" => {
            serde_json::json!({"accepted_route": command})
        }
        "job.status" | "job.wait" => serde_json::json!({
            "schema": "podway.job-result/v1",
            "job": null
        }),
        unknown => panic!("no recorded success envelope exists for daemon route {unknown}"),
    }
}
fn dynamic_completion_result(kind: &str) -> Value {
    match kind {
        "items" => authoritative_status_result(serde_json::json!([{
            "id": "decision",
            "type": "text",
            "prompt": "Record the decision",
            "required": true,
            "satisfied": false,
            "revision": 1,
            "value": null
        }])),
        "blockers" => authoritative_status_result_with_blockers(
            serde_json::json!([]),
            serde_json::json!([{
                "id": RECORDING_BLOCKER_ID,
                "attempt_id": RECORDING_ATTEMPT_ID,
                "reason": "recorded blocker"
            }]),
        ),
        "returns" => authoritative_next_result(),
        "jobs" => authoritative_job_list_result(),
        _ => panic!("unknown dynamic completion kind {kind}"),
    }
}
const RECORDING_SESSION_ID: &str = "123e4567-e89b-42d3-a456-426614174004";
const RECORDING_ATTEMPT_ID: &str = "123e4567-e89b-42d3-a456-426614174002";
const RECORDING_JOB_ID: &str = "123e4567-e89b-42d3-a456-426614174003";
const RECORDING_BLOCKER_ID: &str = "123e4567-e89b-42d3-a456-426614174006";
const EXPLICIT_WORKSPACE_ID: &str = "123e4567-e89b-42d3-a456-426614174007";
const EXPLICIT_SESSION_ID: &str = "123e4567-e89b-42d3-a456-426614174008";

#[derive(Clone, Copy, Debug)]
enum PayloadValue {
    Bool(bool),
    Text(&'static str),
}

#[derive(Clone, Copy, Debug)]
enum PreconditionExpectation {
    None,
    SessionAttempt,
    SessionIdentity,
    SessionRevision,
    Item,
    QueuedJob,
}

#[derive(Clone, Copy)]
struct DaemonContract {
    route: &'static str,
    arguments: &'static [&'static str],
    operation: OperationV1,
    payload: &'static [(&'static str, PayloadValue)],
    preconditions: PreconditionExpectation,
    status_probe: bool,
    detachable: bool,
}

const DAEMON_CONTRACTS: &[DaemonContract] = &[
    DaemonContract {
        route: "workspace.init",
        arguments: &["init", "--repair"],
        operation: OperationV1::Bootstrap,
        payload: &[("repair", PayloadValue::Bool(true))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: true,
    },
    DaemonContract {
        route: "workspace.doctor",
        arguments: &["doctor", "--deep"],
        operation: OperationV1::Query,
        payload: &[("deep", PayloadValue::Bool(true))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "workspace.show",
        arguments: &["workspace", "show"],
        operation: OperationV1::Query,
        payload: &[],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "workspace.repair",
        arguments: &["workspace", "repair"],
        operation: OperationV1::Control,
        payload: &[],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "session.start",
        arguments: &["start", "--preset", "sw-dev", "--task", "recording task"],
        operation: OperationV1::Mutate,
        payload: &[
            ("preset", PayloadValue::Text("sw-dev")),
            ("task_title", PayloadValue::Text("recording task")),
        ],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: true,
    },
    DaemonContract {
        route: "session.start_replace",
        arguments: &[
            "start",
            "--preset",
            "sw-dev",
            "--task",
            "recording replacement",
            "--replace",
            "--yes",
        ],
        operation: OperationV1::Mutate,
        payload: &[
            ("preset", PayloadValue::Text("sw-dev")),
            ("task_title", PayloadValue::Text("recording replacement")),
            ("confirmed", PayloadValue::Bool(true)),
        ],
        preconditions: PreconditionExpectation::SessionIdentity,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.status",
        arguments: &["status", "--verbose"],
        operation: OperationV1::Query,
        payload: &[("verbose", PayloadValue::Bool(true))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "session.next",
        arguments: &["next", "--after-job", RECORDING_JOB_ID],
        operation: OperationV1::Query,
        payload: &[("after_job_id", PayloadValue::Text(RECORDING_JOB_ID))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "session.complete",
        arguments: &["complete"],
        operation: OperationV1::Mutate,
        payload: &[],
        preconditions: PreconditionExpectation::SessionAttempt,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.skip",
        arguments: &["skip", "--reason", "recorded skip"],
        operation: OperationV1::Mutate,
        payload: &[("reason", PayloadValue::Text("recorded skip"))],
        preconditions: PreconditionExpectation::SessionAttempt,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.retry",
        arguments: &["retry", "--reason", "recorded retry"],
        operation: OperationV1::Mutate,
        payload: &[("reason", PayloadValue::Text("recorded retry"))],
        preconditions: PreconditionExpectation::SessionAttempt,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.return",
        arguments: &["return", "--to", "plan", "--reason", "recorded return"],
        operation: OperationV1::Mutate,
        payload: &[
            ("destination_stage_id", PayloadValue::Text("plan")),
            ("reason", PayloadValue::Text("recorded return")),
        ],
        preconditions: PreconditionExpectation::SessionAttempt,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.block",
        arguments: &["block", "--reason", "recorded blocker"],
        operation: OperationV1::Mutate,
        payload: &[("reason", PayloadValue::Text("recorded blocker"))],
        preconditions: PreconditionExpectation::SessionAttempt,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.unblock",
        arguments: &["unblock", RECORDING_BLOCKER_ID],
        operation: OperationV1::Mutate,
        payload: &[
            ("all", PayloadValue::Bool(false)),
            ("blocker_id", PayloadValue::Text(RECORDING_BLOCKER_ID)),
        ],
        preconditions: PreconditionExpectation::SessionAttempt,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.cancel",
        arguments: &["cancel", "--reason", "recorded cancellation"],
        operation: OperationV1::Mutate,
        payload: &[("reason", PayloadValue::Text("recorded cancellation"))],
        preconditions: PreconditionExpectation::SessionAttempt,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.reopen",
        arguments: &["reopen", "--to", "plan", "--reason", "recorded reopen"],
        operation: OperationV1::Mutate,
        payload: &[
            ("destination_stage_id", PayloadValue::Text("plan")),
            ("reason", PayloadValue::Text("recorded reopen")),
        ],
        preconditions: PreconditionExpectation::SessionRevision,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "session.reset",
        arguments: &["reset", "--yes"],
        operation: OperationV1::Mutate,
        payload: &[("confirmed", PayloadValue::Bool(true))],
        preconditions: PreconditionExpectation::SessionIdentity,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "workspace.reset_all",
        arguments: &["reset", "--all", "--force", "--yes"],
        operation: OperationV1::Bootstrap,
        payload: &[
            ("confirmed", PayloadValue::Bool(true)),
            (
                "expected_workspace_uuid",
                PayloadValue::Text(RECORDING_WORKSPACE_ID),
            ),
        ],
        preconditions: PreconditionExpectation::None,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "item.check",
        arguments: &["check", "item"],
        operation: OperationV1::Mutate,
        payload: &[("item_id", PayloadValue::Text("item"))],
        preconditions: PreconditionExpectation::Item,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "item.uncheck",
        arguments: &["uncheck", "item"],
        operation: OperationV1::Mutate,
        payload: &[("item_id", PayloadValue::Text("item"))],
        preconditions: PreconditionExpectation::Item,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "item.set",
        arguments: &["set", "item", "recorded value"],
        operation: OperationV1::Mutate,
        payload: &[
            ("item_id", PayloadValue::Text("item")),
            ("value", PayloadValue::Text("recorded value")),
        ],
        preconditions: PreconditionExpectation::Item,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "item.add",
        arguments: &["add", "item", "recorded value"],
        operation: OperationV1::Mutate,
        payload: &[
            ("item_id", PayloadValue::Text("item")),
            ("value", PayloadValue::Text("recorded value")),
        ],
        preconditions: PreconditionExpectation::Item,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "item.remove",
        arguments: &["remove", "item", "recorded value", "--ignore-missing"],
        operation: OperationV1::Mutate,
        payload: &[
            ("ignore_missing", PayloadValue::Bool(true)),
            ("item_id", PayloadValue::Text("item")),
            ("value", PayloadValue::Text("recorded value")),
        ],
        preconditions: PreconditionExpectation::Item,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "item.attach",
        arguments: &[
            "attach",
            "item",
            "recorded.txt",
            "--media-type",
            "text/plain",
        ],
        operation: OperationV1::Mutate,
        payload: &[
            ("item_id", PayloadValue::Text("item")),
            ("media_type", PayloadValue::Text("text/plain")),
            ("path", PayloadValue::Text("recorded.txt")),
        ],
        preconditions: PreconditionExpectation::Item,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "item.clear",
        arguments: &["clear", "item"],
        operation: OperationV1::Mutate,
        payload: &[("item_id", PayloadValue::Text("item"))],
        preconditions: PreconditionExpectation::Item,
        status_probe: true,
        detachable: true,
    },
    DaemonContract {
        route: "job.list",
        arguments: &["job", "list", "--state", "queued"],
        operation: OperationV1::Query,
        payload: &[("state", PayloadValue::Text("queued"))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "job.lookup",
        arguments: &["job", "lookup", "--idempotency-key", "recording-key"],
        operation: OperationV1::Query,
        payload: &[("idempotency_key", PayloadValue::Text("recording-key"))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "job.status",
        arguments: &["job", "status", RECORDING_JOB_ID],
        operation: OperationV1::Query,
        payload: &[("job_id", PayloadValue::Text(RECORDING_JOB_ID))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "job.wait",
        arguments: &["job", "wait", RECORDING_JOB_ID],
        operation: OperationV1::Query,
        payload: &[("job_id", PayloadValue::Text(RECORDING_JOB_ID))],
        preconditions: PreconditionExpectation::None,
        status_probe: false,
        detachable: false,
    },
    DaemonContract {
        route: "job.cancel",
        arguments: &["job", "cancel", RECORDING_JOB_ID],
        operation: OperationV1::Control,
        payload: &[("job_id", PayloadValue::Text(RECORDING_JOB_ID))],
        preconditions: PreconditionExpectation::QueuedJob,
        status_probe: false,
        detachable: false,
    },
];
/// The smallest legal Procedure v2 document, already in canonical authoring form.
///
/// Held here rather than under `tests/fixtures/` because that tree is a manifest-tracked contract
/// surface; an executable-route success fixture belongs to the test, not to the frozen catalog.
const MINIMAL_PROCEDURE_V2_YAML: &str = r#"schema: podway.procedure/v2
id: minimal
version: "1"
name: Minimal
purpose: The smallest legal Procedure v2 document.
node_definitions:
  work:
    type: action
    title: Work
    intent: Do the work.
graph:
  entry: only
  nodes:
    - id: only
      use: work
      terminal: true
"#;

#[derive(Clone, Copy)]
struct RouteSurface {
    route: &'static str,
    parser: &'static [&'static str],
    flags: &'static [&'static str],
    values: &'static [&'static str],
    help_tokens: &'static [&'static str],
    dynamic: Option<&'static str>,
}

const DISPLAY_FLAGS: &[&str] = &["--json", "--no-color", "--quiet"];
const DAEMON_READ_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
];
const SESSION_MUTATION_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-session-revision",
    "--if-attempt",
];
const ITEM_MUTATION_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-attempt",
    "--if-item-revision",
];
const START_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-session-revision",
    "--preset",
    "--procedure",
    "--expect-procedure-digest",
    "--task",
    "--replace",
    "--dry-run",
    "--yes",
];
const RESET_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-session-revision",
    "--all",
    "--force",
    "--dry-run",
    "--yes",
];
const STATUS_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--if-workspace-uuid",
    "--if-session-id",
    "--verbose",
    "--wait-for-idle",
    "--compact",
    "--after-job",
];
const NEXT_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--if-workspace-uuid",
    "--if-session-id",
    "--wait-for-idle",
    "--after-job",
];
const SKIP_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-session-revision",
    "--if-attempt",
    "--reason",
];
const RETURN_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-session-revision",
    "--if-attempt",
    "--to",
    "--reason",
    "--dry-run",
];
const UNBLOCK_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-session-revision",
    "--if-attempt",
    "--all",
];
const REOPEN_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-session-revision",
    "--to",
    "--reason",
    "--dry-run",
];
const SET_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-attempt",
    "--if-item-revision",
    "--stdin",
];
const REMOVE_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-attempt",
    "--if-item-revision",
    "--ignore-missing",
];
const ATTACH_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--socket",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-workspace-uuid",
    "--if-session-id",
    "--if-attempt",
    "--if-item-revision",
    "--reference",
    "--digest",
    "--size",
    "--media-type",
];

const ROUTE_SURFACES: &[RouteSurface] = &[
    RouteSurface {
        route: "help",
        parser: &["help"],
        flags: DISPLAY_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "version",
        parser: &["version"],
        flags: &["--json", "--no-color", "--quiet", "--identity"],
        values: &[],
        help_tokens: &["--identity"],
        dynamic: None,
    },
    RouteSurface {
        route: "completions",
        parser: &["completions", "bash"],
        flags: DISPLAY_FLAGS,
        values: &["bash", "zsh", "fish"],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "procedure.validate",
        parser: &["procedure", "validate", "missing.yaml"],
        flags: &["--json", "--no-color", "--quiet", "--warnings-as-errors"],
        values: &[],
        help_tokens: &["--warnings-as-errors"],
        dynamic: None,
    },
    RouteSurface {
        route: "procedure.show",
        parser: &["procedure", "show", "missing.yaml"],
        flags: &["--json", "--no-color", "--quiet", "--canonical"],
        values: &[],
        help_tokens: &["--canonical"],
        dynamic: None,
    },
    RouteSurface {
        route: "procedure.format",
        parser: &["procedure", "format", "missing.yaml"],
        flags: &["--json", "--no-color", "--quiet", "--check", "--write"],
        values: &[],
        help_tokens: &["--check", "--write"],
        dynamic: None,
    },
    RouteSurface {
        route: "procedure.lint",
        parser: &["procedure", "lint", "missing.yaml"],
        flags: &["--json", "--no-color", "--quiet", "--warnings-as-errors"],
        values: &[],
        help_tokens: &["--warnings-as-errors"],
        dynamic: None,
    },
    RouteSurface {
        route: "procedure.check",
        parser: &["procedure", "check", "missing.yaml"],
        flags: &["--json", "--no-color", "--quiet", "--warnings-as-errors"],
        values: &[],
        help_tokens: &["--warnings-as-errors"],
        dynamic: None,
    },
    RouteSurface {
        route: "procedure.scaffold",
        parser: &["procedure", "scaffold"],
        flags: &["--json", "--no-color", "--quiet", "--template"],
        values: &["minimal"],
        help_tokens: &["--template"],
        dynamic: None,
    },
    RouteSurface {
        route: "preset.list",
        parser: &["preset", "list"],
        flags: DISPLAY_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "preset.show",
        parser: &["preset", "show", "sw-dev"],
        flags: DISPLAY_FLAGS,
        values: &["analysis", "bug-fix", "docs-only", "sw-dev"],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "preset.explain",
        parser: &["preset", "explain", "sw-dev"],
        flags: DISPLAY_FLAGS,
        values: &["analysis", "bug-fix", "docs-only", "sw-dev"],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.install",
        parser: &["daemon", "install"],
        flags: &[
            "--json",
            "--no-color",
            "--quiet",
            "--socket",
            "--daemon-path",
        ],
        values: &[],
        help_tokens: &["--daemon-path"],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.uninstall",
        parser: &["daemon", "uninstall", "--yes"],
        flags: &["--json", "--no-color", "--quiet", "--yes", "--purge-logs"],
        values: &[],
        help_tokens: &["--yes", "--purge-logs"],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.start",
        parser: &["daemon", "start"],
        flags: DISPLAY_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.stop",
        parser: &["daemon", "stop"],
        flags: DISPLAY_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.restart",
        parser: &["daemon", "restart"],
        flags: DISPLAY_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.status",
        parser: &["daemon", "status"],
        flags: DISPLAY_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.terminate",
        parser: &["terminate", "--dev"],
        flags: &["--json", "--dev", "--timeout", "--no-color", "--quiet"],
        values: &[],
        help_tokens: &["--dev"],
        dynamic: None,
    },
    RouteSurface {
        route: "daemon.logs",
        parser: &["daemon", "logs", "--lines", "10"],
        flags: &["--json", "--no-color", "--quiet", "--follow", "--lines"],
        values: &[],
        help_tokens: &["--follow", "--lines"],
        dynamic: None,
    },
    RouteSurface {
        route: "workspace.init",
        parser: &["init"],
        flags: &[
            "--json",
            "--worktree",
            "--timeout",
            "--socket",
            "--no-color",
            "--quiet",
            "--idempotency-key",
            "--detach",
            "--repair",
        ],
        values: &[],
        help_tokens: &["--repair"],
        dynamic: None,
    },
    RouteSurface {
        route: "workspace.doctor",
        parser: &["doctor", "--deep"],
        flags: &[
            "--json",
            "--worktree",
            "--timeout",
            "--socket",
            "--no-color",
            "--quiet",
            "--deep",
        ],
        values: &[],
        help_tokens: &["--deep"],
        dynamic: None,
    },
    RouteSurface {
        route: "workspace.show",
        parser: &["workspace", "show"],
        flags: DAEMON_READ_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "workspace.repair",
        parser: &["workspace", "repair"],
        flags: DAEMON_READ_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: None,
    },
    RouteSurface {
        route: "session.start",
        parser: &["start", "--preset", "sw-dev", "--task", "task"],
        flags: START_SURFACE_FLAGS,
        values: &["analysis", "bug-fix", "docs-only", "sw-dev"],
        help_tokens: &[
            "--preset",
            "--procedure",
            "--expect-procedure-digest",
            "--task",
            "--if-workspace-uuid",
            "--dry-run",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.start_replace",
        parser: &[
            "start",
            "--preset",
            "sw-dev",
            "--task",
            "task",
            "--replace",
            "--yes",
        ],
        flags: START_SURFACE_FLAGS,
        values: &["analysis", "bug-fix", "docs-only", "sw-dev"],
        help_tokens: &[
            "--replace",
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--expect-procedure-digest",
            "--yes",
            "--dry-run",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.status",
        parser: &["status", "--verbose"],
        flags: STATUS_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--verbose",
            "--wait-for-idle",
            "--compact",
            "--after-job",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.next",
        parser: &["next", "--wait-for-idle"],
        flags: NEXT_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--wait-for-idle",
            "--after-job",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.complete",
        parser: &["complete"],
        flags: SESSION_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--if-attempt",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.skip",
        parser: &["skip", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--if-attempt",
            "--reason",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.retry",
        parser: &["retry", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--if-attempt",
            "--reason",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.return",
        parser: &[
            "return",
            "--to",
            "implement",
            "--reason",
            "reason",
            "--dry-run",
        ],
        flags: RETURN_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--if-attempt",
            "--to",
            "--reason",
            "--dry-run",
        ],
        dynamic: Some("returns"),
    },
    RouteSurface {
        route: "session.block",
        parser: &["block", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--if-attempt",
            "--reason",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.unblock",
        parser: &["unblock", "--all"],
        flags: UNBLOCK_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--if-attempt",
            "--all",
        ],
        dynamic: Some("blockers"),
    },
    RouteSurface {
        route: "session.cancel",
        parser: &["cancel", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--if-attempt",
            "--reason",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "session.reopen",
        parser: &[
            "reopen",
            "--to",
            "implement",
            "--reason",
            "reason",
            "--dry-run",
        ],
        flags: REOPEN_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--to",
            "--reason",
            "--dry-run",
        ],
        dynamic: Some("returns"),
    },
    RouteSurface {
        route: "session.reset",
        parser: &["reset", "--yes"],
        flags: RESET_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-session-revision",
            "--dry-run",
            "--yes",
        ],
        dynamic: None,
    },
    RouteSurface {
        route: "workspace.reset_all",
        parser: &["reset", "--all", "--force", "--yes"],
        flags: RESET_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--if-workspace-uuid", "--all", "--force", "--yes"],
        dynamic: None,
    },
    RouteSurface {
        route: "item.check",
        parser: &["check", "item"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-attempt",
            "--if-item-revision",
        ],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.uncheck",
        parser: &["uncheck", "item"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-attempt",
            "--if-item-revision",
        ],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.set",
        parser: &["set", "item", "value"],
        flags: SET_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-attempt",
            "--if-item-revision",
            "--stdin",
        ],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.add",
        parser: &["add", "item", "value"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-attempt",
            "--if-item-revision",
        ],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.remove",
        parser: &["remove", "item", "value", "--ignore-missing"],
        flags: REMOVE_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-attempt",
            "--if-item-revision",
            "--ignore-missing",
        ],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.attach",
        parser: &["attach", "item", "path.txt", "--media-type", "text/plain"],
        flags: ATTACH_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-attempt",
            "--if-item-revision",
            "--reference",
            "--digest",
            "--size",
            "[--media-type <type>]",
        ],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.clear",
        parser: &["clear", "item"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[
            "--if-workspace-uuid",
            "--if-session-id",
            "--if-attempt",
            "--if-item-revision",
        ],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "job.list",
        parser: &["job", "list", "--state", "queued"],
        flags: &[
            "--json",
            "--worktree",
            "--timeout",
            "--socket",
            "--no-color",
            "--quiet",
            "--state",
        ],
        values: &["queued", "running", "succeeded", "failed", "cancelled"],
        help_tokens: &["--state"],
        dynamic: None,
    },
    RouteSurface {
        route: "job.lookup",
        parser: &["job", "lookup", "--idempotency-key", "recording-key"],
        flags: &[
            "--json",
            "--worktree",
            "--timeout",
            "--socket",
            "--no-color",
            "--quiet",
            "--idempotency-key",
        ],
        values: &[],
        help_tokens: &["--idempotency-key"],
        dynamic: None,
    },
    RouteSurface {
        route: "job.status",
        parser: &["job", "status", RECORDING_JOB_ID],
        flags: DAEMON_READ_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: Some("jobs"),
    },
    RouteSurface {
        route: "job.wait",
        parser: &["job", "wait", RECORDING_JOB_ID],
        flags: DAEMON_READ_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: Some("jobs"),
    },
    RouteSurface {
        route: "job.cancel",
        parser: &["job", "cancel", RECORDING_JOB_ID],
        flags: DAEMON_READ_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: Some("jobs"),
    },
];
fn payload_value(value: PayloadValue) -> Value {
    match value {
        PayloadValue::Bool(value) => Value::Bool(value),
        PayloadValue::Text(value) => Value::String(value.to_owned()),
    }
}

fn expected_preconditions(expectation: PreconditionExpectation) -> Value {
    match expectation {
        PreconditionExpectation::None => serde_json::json!({}),
        PreconditionExpectation::SessionAttempt => serde_json::json!({
            "session_id": RECORDING_SESSION_ID,
            "session_revision": 1,
            "attempt_id": RECORDING_ATTEMPT_ID,
        }),
        PreconditionExpectation::SessionIdentity => serde_json::json!({
            "session_id": RECORDING_SESSION_ID,
            "session_revision": 1,
        }),
        PreconditionExpectation::SessionRevision => serde_json::json!({
            "session_id": RECORDING_SESSION_ID,
            "session_revision": 1,
        }),
        PreconditionExpectation::Item => serde_json::json!({
            "session_id": RECORDING_SESSION_ID,
            "attempt_id": RECORDING_ATTEMPT_ID,
            "item_revision": 1,
        }),
        PreconditionExpectation::QueuedJob => serde_json::json!({
            "job_state": "queued",
        }),
    }
}

fn contract_arguments(
    contract: DaemonContract,
    worktree: &Path,
    json: bool,
    detach: bool,
) -> Vec<String> {
    let mut arguments = Vec::new();
    if json {
        arguments.push("--json".to_owned());
    }
    arguments.extend([
        "--worktree".to_owned(),
        worktree.display().to_string(),
        "--timeout".to_owned(),
        "17ms".to_owned(),
    ]);
    if detach {
        arguments.push("--detach".to_owned());
    }
    arguments.extend(
        contract
            .arguments
            .iter()
            .map(|argument| (*argument).to_owned()),
    );
    arguments
}

fn recording_replies(contract: DaemonContract, error: bool) -> Vec<RecordingReply> {
    let mut replies = Vec::new();
    if contract.status_probe {
        replies.push(RecordingReply::Output(recording_success_result(
            "session.status",
        )));
    }
    replies.push(if error {
        RecordingReply::Error
    } else {
        RecordingReply::Output(recording_success_result(contract.route))
    });
    replies
}

fn execute_recorded_contract(
    fixture: &DynamicCompletionFixture,
    contract: DaemonContract,
    json: bool,
    detach: bool,
    error: bool,
) -> (Output, Vec<Value>) {
    let server = RecordingDaemon::start(fixture, recording_replies(contract, error));
    let arguments = contract_arguments(contract, &fixture.root, json, detach);
    let output = fixture.run_in(&fixture.root, &arguments);
    (output, server.finish())
}

fn request_projection(request: &Value) -> Value {
    serde_json::json!({
        "operation": request["operation"],
        "command": request["command"],
        "workspace": request["workspace"],
        "preconditions": request.get("preconditions").cloned().unwrap_or_else(|| serde_json::json!({})),
        "options": request["options"],
        "payload": request["payload"],
    })
}

fn canonical_fixture_path(path: &Path) -> String {
    fs::canonicalize(path)
        .expect("fixture path must canonicalize")
        .display()
        .to_string()
}

fn assert_recorded_contract(
    requests: &[Value],
    contract: DaemonContract,
    worktree: &Path,
    detached: bool,
) -> Value {
    assert_eq!(
        requests.len(),
        if contract.status_probe { 2 } else { 1 },
        "{} must use exactly its declared status preflight and final request",
        contract.route
    );
    if contract.status_probe {
        let probe = &requests[0];
        assert_eq!(probe["command"], "session.status");
        assert_eq!(probe["operation"], "query");
        assert_eq!(probe["options"]["detach"], false);
        assert_eq!(probe["options"]["wait_timeout_ms"], 17);
        assert_eq!(
            probe
                .get("preconditions")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
            serde_json::json!({}),
        );
    }

    let request = requests
        .last()
        .expect("recording daemon must receive the final route request");
    assert_eq!(request["command"], contract.route);
    assert_eq!(
        request["operation"],
        serde_json::to_value(contract.operation)
            .expect("operation must have a stable wire representation")
    );
    assert_eq!(request["options"]["detach"], detached);
    assert_eq!(request["options"]["wait_timeout_ms"], 17);
    assert_eq!(
        request
            .get("preconditions")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        expected_preconditions(contract.preconditions),
        "{} must derive the documented optimistic preconditions",
        contract.route
    );

    let expected_workspace = contract
        .status_probe
        .then_some(Value::String(RECORDING_WORKSPACE_ID.to_owned()));
    assert_eq!(
        request["workspace"].get("expected_uuid").cloned(),
        expected_workspace,
        "{} must bind mutations and reset-all to the status workspace identity",
        contract.route
    );
    let expected_root = canonical_fixture_path(worktree);
    assert_eq!(
        request["workspace"]["root"], expected_root,
        "{} must select the requested workspace",
        contract.route
    );

    let mut payload = request["payload"]
        .as_object()
        .expect("recorded payload must be an object")
        .clone();
    let selector = payload
        .remove("selector")
        .expect("every daemon request must include the worktree selector");
    assert_eq!(selector["display"], canonical_fixture_path(worktree));
    let expected_selector_uuid = if contract.status_probe {
        Value::String(RECORDING_WORKSPACE_ID.to_owned())
    } else {
        Value::Null
    };
    assert_eq!(selector["expected_uuid"], expected_selector_uuid);
    let expected_payload = contract
        .payload
        .iter()
        .map(|(key, value)| ((*key).to_owned(), payload_value(*value)))
        .collect();
    assert_eq!(
        payload, expected_payload,
        "{} payload must have no undocumented or omitted fields",
        contract.route
    );

    if contract.detachable {
        assert!(
            request.get("idempotency_key").is_some(),
            "{} must carry an idempotency key",
            contract.route
        );
    } else {
        assert!(
            request.get("idempotency_key").is_none(),
            "{} must not add an idempotency key",
            contract.route
        );
    }

    request_projection(request)
}

#[test]
fn explicit_workspace_identity_guards_preflight_and_overrides_observed_identity() {
    let fixture = DynamicCompletionFixture::new();
    let server = RecordingDaemon::start(
        &fixture,
        vec![
            RecordingReply::Output(recording_success_result("session.status")),
            RecordingReply::Output(recording_success_result("session.complete")),
        ],
    );
    let arguments = [
        "--json",
        "--worktree",
        fixture.root.to_str().expect("fixture root must be UTF-8"),
        "--if-workspace-uuid",
        EXPLICIT_WORKSPACE_ID,
        "--if-session-id",
        EXPLICIT_SESSION_ID,
        "complete",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let output = fixture.run_in(&fixture.root, &arguments);
    assert!(
        output.status.success(),
        "identity-guarded complete failed: {output:?}"
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request["workspace"]["expected_uuid"], EXPLICIT_WORKSPACE_ID);
    }
    assert_eq!(requests[0]["command"], "session.status");
    assert_eq!(
        requests[0]["preconditions"]["session_id"],
        EXPLICIT_SESSION_ID
    );
    assert_eq!(requests[1]["command"], "session.complete");
    assert_eq!(
        requests[1]["preconditions"]["session_id"],
        EXPLICIT_SESSION_ID
    );
    assert_eq!(requests[1]["preconditions"]["session_revision"], 1);
    assert_eq!(
        requests[1]["preconditions"]["attempt_id"],
        RECORDING_ATTEMPT_ID
    );
}

#[test]
fn session_identity_flag_is_accepted_on_guarded_reads() {
    let fixture = DynamicCompletionFixture::new();
    let server = RecordingDaemon::start(
        &fixture,
        vec![RecordingReply::Output(recording_success_result(
            "session.status",
        ))],
    );
    let arguments = [
        "--json",
        "--worktree",
        fixture.root.to_str().expect("fixture root must be UTF-8"),
        "--if-session-id",
        EXPLICIT_SESSION_ID,
        "status",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let output = fixture.run_in(&fixture.root, &arguments);
    assert!(output.status.success(), "guarded status failed: {output:?}");
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["preconditions"]["session_id"],
        EXPLICIT_SESSION_ID
    );
}
fn status_json_semantic_projection(response: &Value) -> Value {
    let result = &response["result"];
    serde_json::json!({
        "task": result["task"]["title"],
        "session": {
            "id": result["session"]["id"],
            "lifecycle": result["session"]["lifecycle"],
            "revision": result["session"]["revision"],
        },
        "current": {
            "stage_id": result["current"]["stage_id"],
            "title": result["current"]["title"],
            "attempt_id": result["current"]["attempt_id"],
            "attempt_number": result["current"]["attempt_number"],
            "blocked": result["current"]["blocked"],
            "ready_to_complete": result["current"]["ready_to_complete"],
        },
        "item": {
            "id": result["items"][0]["id"],
            "type": result["items"][0]["type"],
            "required": result["items"][0]["required"],
            "satisfied": result["items"][0]["satisfied"],
            "revision": result["items"][0]["revision"],
            "prompt": result["items"][0]["prompt"],
            "value": result["items"][0]["value"],
        },
        "blocker": {
            "id": result["blockers"][0]["id"],
            "attempt_id": result["blockers"][0]["attempt_id"],
            "reason": result["blockers"][0]["reason"],
        },
        "queue": result["queue"],
    })
}

fn status_text_semantic_projection(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("text status output must be UTF-8");
    let line = |prefix| {
        text.lines()
            .find_map(|line| line.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("text status must render {prefix:?}"))
    };

    let task = line("task: ").to_owned();

    let session = line("session: ");
    let mut session_parts = session.split_whitespace();
    let session_id = session_parts.next().expect("session ID");
    let lifecycle = session_parts
        .next()
        .expect("session lifecycle")
        .to_ascii_lowercase();
    let session_revision = session_parts
        .next()
        .and_then(|part| part.strip_prefix("revision="))
        .expect("session revision")
        .parse::<u64>()
        .expect("numeric session revision");

    let current = line("current: ");
    let (current_head, current_rest) = current.split_once(" attempt=").expect("current attempt");
    let (stage_id, title) = current_head.split_once(' ').expect("current stage title");
    let (attempt_number, current_rest) = current_rest.split_once(" id=").expect("attempt ID");
    let (attempt_id, current_rest) = current_rest.split_once(" blocked=").expect("blocked state");
    let (blocked, ready_to_complete) = current_rest
        .split_once(" ready_to_complete=")
        .expect("completion state");

    let item = line("item: ");
    let (item_head, item_rest) = item.split_once(" required=").expect("item required state");
    let (item_id, item_type) = item_head.split_once(' ').expect("item type");
    let (required, item_rest) = item_rest
        .split_once(" satisfied=")
        .expect("item satisfied state");
    let (satisfied, item_rest) = item_rest.split_once(" revision=").expect("item revision");
    let (item_revision, item_rest) = item_rest.split_once(" prompt=").expect("item prompt");
    let (prompt, value) = item_rest.split_once(" value=").expect("item value");
    let blocker = line("blocker: ");
    let (blocker_id, blocker_rest) = blocker.split_once(" attempt=").expect("blocker attempt");
    let (blocker_attempt_id, blocker_reason) =
        blocker_rest.split_once(" reason=").expect("blocker reason");

    let queue = line("queue: ");
    let mut queue_values = serde_json::Map::new();
    for field in queue.split_whitespace() {
        let (key, value) = field.split_once('=').expect("queue key=value");
        queue_values.insert(
            key.to_owned(),
            match key {
                "pending_mutations" => Value::Bool(value.parse().expect("boolean pending state")),
                "queued_count" | "latest_workspace_sequence" => {
                    Value::from(value.parse::<u64>().expect("numeric queue state"))
                }
                "running_job_id" if value == "-" => Value::Null,
                "running_job_id" => Value::String(value.to_owned()),
                _ => panic!("unexpected queue field {key}"),
            },
        );
    }

    serde_json::json!({
        "task": task,
        "session": {
            "id": session_id,
            "lifecycle": lifecycle,
            "revision": session_revision,
        },
        "current": {
            "stage_id": stage_id,
            "title": title,
            "attempt_id": attempt_id,
            "attempt_number": attempt_number.parse::<u64>().expect("numeric attempt number"),
            "blocked": blocked.parse::<bool>().expect("boolean blocked state"),
            "ready_to_complete": ready_to_complete.parse::<bool>().expect("boolean completion state"),
        },
        "item": {
            "id": item_id,
            "type": item_type.to_ascii_lowercase(),
            "required": required.parse::<bool>().expect("boolean required state"),
            "satisfied": satisfied.parse::<bool>().expect("boolean satisfied state"),
            "revision": item_revision.parse::<u64>().expect("numeric item revision"),
            "prompt": prompt,
            "value": serde_json::from_str::<Value>(value).expect("JSON item value"),
        },
        "blocker": {
            "id": blocker_id,
            "attempt_id": blocker_attempt_id,
            "reason": blocker_reason,
        },
        "queue": queue_values,
    })
}

#[test]
fn pac_048_recording_daemon_contract_table_validates_successful_versioned_json_output_for_every_route()
 {
    let registered_routes = registered_command_catalog_route_availability();
    assert_eq!(
        registered_routes,
        registered_command_route_contract_availability(),
        "the command catalog and command route contract must agree on availability",
    );
    let executable_routes = registered_routes
        .iter()
        .filter_map(|(route, availability)| {
            (availability == "executable").then_some(route.as_str())
        })
        .collect::<BTreeSet<_>>();
    let executable_surfaces = ROUTE_SURFACES
        .iter()
        .map(|surface| surface.route)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        executable_routes, executable_surfaces,
        "availability must identify exactly the current 50-route CLI surface",
    );
    let reserved_v2_routes = registered_routes
        .iter()
        .filter_map(|(route, availability)| {
            (availability == "reserved_contract").then_some(route.as_str())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        reserved_v2_routes,
        BTreeSet::from([
            "goal.assess_criterion",
            "goal.define",
            "goal.revise",
            "procedure.convert",
            "procedure.graph",
            "procedure.preview",
            "procedure.vet",
            "session.decide",
            "session.rework",
        ]),
        "a registered v2 route stays absent from the executable CLI until its owning task lands",
    );
    assert_eq!(DAEMON_CONTRACTS.len(), 30);
    for contract in DAEMON_CONTRACTS {
        assert!(
            executable_routes.contains(&contract.route),
            "{} must be a frozen public command rather than an untracked daemon entry point",
            contract.route,
        );
    }
    const NORMAL_PUBLIC_WRITERS: &[&str] = &[
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
    ];
    assert_eq!(
        DAEMON_CONTRACTS
            .iter()
            .filter(|contract| contract.operation != OperationV1::Query)
            .map(|contract| contract.route)
            .collect::<Vec<_>>(),
        NORMAL_PUBLIC_WRITERS,
        "the CLI-to-daemon recording boundary must derive every normal public writer exactly once",
    );

    let fixture = DynamicCompletionFixture::new();
    for contract in DAEMON_CONTRACTS {
        let (json_success, json_requests) =
            execute_recorded_contract(&fixture, *contract, true, false, false);
        assert!(
            json_success.status.success(),
            "{} JSON success failed: {json_success:?}",
            contract.route
        );
        assert!(json_success.stderr.is_empty());
        let json_response = one_json(&json_success);
        assert_eq!(json_response["schema"], "podway.output/v1");
        assert_eq!(json_response["command"], contract.route);
        assert_eq!(
            json_response["result"],
            recording_success_result(contract.route),
            "{} must render the recorded typed result in its versioned JSON output",
            contract.route
        );
        let typed_json_response: ResponseEnvelopeV1 =
            serde_json::from_value(json_response.clone()).expect("JSON success must be typed");
        assert!(matches!(typed_json_response, ResponseEnvelopeV1::Output(_)));
        let json_projection =
            assert_recorded_contract(&json_requests, *contract, &fixture.root, false);

        let (text_success, text_requests) =
            execute_recorded_contract(&fixture, *contract, false, false, false);
        assert!(
            text_success.status.success(),
            "{} text success failed: {text_success:?}",
            contract.route
        );
        assert!(!text_success.stdout.is_empty());
        assert!(text_success.stderr.is_empty());
        let text_projection =
            assert_recorded_contract(&text_requests, *contract, &fixture.root, false);
        assert_eq!(json_projection, text_projection);

        let (json_error, json_error_requests) =
            execute_recorded_contract(&fixture, *contract, true, false, true);
        assert_eq!(json_error.status.code(), Some(5));
        assert!(json_error.stderr.is_empty());
        let json_error_response: ResponseEnvelopeV1 =
            serde_json::from_slice(&json_error.stdout).expect("JSON error must be typed");
        let ResponseEnvelopeV1::Error(json_error_response) = json_error_response else {
            panic!("recording daemon error must remain an error envelope");
        };
        assert_eq!(
            json_error_response.code().as_str(),
            "WORKSPACE_NOT_INITIALIZED"
        );
        assert_recorded_contract(&json_error_requests, *contract, &fixture.root, false);

        let (text_error, text_error_requests) =
            execute_recorded_contract(&fixture, *contract, false, false, true);
        assert_eq!(text_error.status.code(), Some(5));
        assert!(text_error.stdout.is_empty());
        assert_eq!(
            String::from_utf8(text_error.stderr).expect("text error must be UTF-8"),
            "error: WORKSPACE_NOT_INITIALIZED: recording daemon rejection\n",
        );
        assert_recorded_contract(&text_error_requests, *contract, &fixture.root, false);

        if contract.detachable {
            let (detached_success, detached_requests) =
                execute_recorded_contract(&fixture, *contract, true, true, false);
            assert!(detached_success.status.success());
            assert_recorded_contract(&detached_requests, *contract, &fixture.root, true);
        }
    }
    let procedure_path = fixture.root.join("causal-procedure.yaml");
    fs::write(
        &procedure_path,
        r#"schema: podway.procedure/v1
id: causal-fixture
version: "1"
name: Causal fixture
stages:
  - id: implement
    title: Implement
    instructions: []
    items: []
rework:
  allow_return_to: any_previous
"#,
    )
    .expect("offline procedure fixture must be writable");
    let v2_procedure_path = fixture.root.join("causal-procedure-v2.yaml");
    fs::write(&v2_procedure_path, MINIMAL_PROCEDURE_V2_YAML)
        .expect("offline Procedure v2 fixture must be writable");
    let local_successes = [
        ("help", vec!["--json".to_owned(), "help".to_owned()]),
        (
            "version",
            vec![
                "--json".to_owned(),
                "version".to_owned(),
                "--identity".to_owned(),
            ],
        ),
        (
            "completions",
            vec![
                "--json".to_owned(),
                "completions".to_owned(),
                "bash".to_owned(),
            ],
        ),
        (
            "procedure.validate",
            vec![
                "--json".to_owned(),
                "procedure".to_owned(),
                "validate".to_owned(),
                procedure_path.display().to_string(),
            ],
        ),
        (
            "procedure.show",
            vec![
                "--json".to_owned(),
                "procedure".to_owned(),
                "show".to_owned(),
                procedure_path.display().to_string(),
            ],
        ),
        (
            "procedure.format",
            vec![
                "--json".to_owned(),
                "procedure".to_owned(),
                "format".to_owned(),
                v2_procedure_path.display().to_string(),
            ],
        ),
        (
            "procedure.lint",
            vec![
                "--json".to_owned(),
                "procedure".to_owned(),
                "lint".to_owned(),
                v2_procedure_path.display().to_string(),
            ],
        ),
        (
            "procedure.check",
            vec![
                "--json".to_owned(),
                "procedure".to_owned(),
                "check".to_owned(),
                v2_procedure_path.display().to_string(),
            ],
        ),
        // Scaffold takes no file: it is the one local authoring route whose success fixture is
        // the invocation itself.
        (
            "procedure.scaffold",
            vec![
                "--json".to_owned(),
                "procedure".to_owned(),
                "scaffold".to_owned(),
            ],
        ),
        (
            "preset.list",
            vec!["--json".to_owned(), "preset".to_owned(), "list".to_owned()],
        ),
        (
            "preset.show",
            vec![
                "--json".to_owned(),
                "preset".to_owned(),
                "show".to_owned(),
                "sw-dev".to_owned(),
            ],
        ),
        (
            "preset.explain",
            vec![
                "--json".to_owned(),
                "preset".to_owned(),
                "explain".to_owned(),
                "sw-dev".to_owned(),
            ],
        ),
    ];
    // Local routes whose success is the v2 envelope rather than the v1 typed decoder; extend this
    // as later tasks land more v2-only static commands.
    const V2_ENVELOPE_ROUTES: &[&str] = &[
        "procedure.format",
        "procedure.lint",
        "procedure.check",
        "procedure.scaffold",
    ];
    for (route, arguments) in &local_successes {
        let output = fixture.run_in(&fixture.root, arguments);
        assert!(
            output.status.success(),
            "{route} must execute its concrete fixture successfully: {output:?}"
        );
        let envelope = one_json(&output);
        // Route identity, not envelope content, decides which decoder applies: a v1 route that
        // regressed into a v2-shaped envelope must fail the strict typed decode below rather than
        // silently pass through this branch, and a v2 route is asserted to carry exactly the v2
        // schema rather than merely branching on whatever schema it happened to emit.
        if V2_ENVELOPE_ROUTES.contains(route) {
            assert_eq!(
                envelope["schema"], "podway.output/v2",
                "{route} must carry the v2 success envelope: {envelope}"
            );
            assert_eq!(envelope["command"], *route);
            assert!(
                envelope["result"]["schema"]
                    .as_str()
                    .is_some_and(|schema| schema.starts_with("podway.procedure-")
                        && schema.ends_with("-result/v1")),
                "{route} must carry a registered v2 result family: {envelope}"
            );
            assert!(
                envelope["warnings"].is_array(),
                "{route} must always serialize the required v2 warnings array"
            );
            continue;
        }
        let response: ResponseEnvelopeV1 =
            serde_json::from_value(envelope).expect("local success must be typed JSON");
        let ResponseEnvelopeV1::Output(response) = response else {
            panic!("{route} must emit an output envelope");
        };
        assert_eq!(response.command().as_str(), *route);
    }

    let service_routes = [
        ("daemon.install", vec!["--json", "daemon", "install"]),
        (
            "daemon.uninstall",
            vec!["--json", "daemon", "uninstall", "--yes"],
        ),
        ("daemon.start", vec!["--json", "daemon", "start"]),
        ("daemon.stop", vec!["--json", "daemon", "stop"]),
        ("daemon.restart", vec!["--json", "daemon", "restart"]),
        ("daemon.status", vec!["--json", "daemon", "status"]),
        ("daemon.terminate", vec!["--json", "--dev", "terminate"]),
        (
            "daemon.logs",
            vec!["--json", "daemon", "logs", "--lines", "1"],
        ),
    ];
    for (route, arguments) in &service_routes {
        let output = fixture.run(arguments);
        let response: ResponseEnvelopeV1 =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!("{route} must produce a typed JSON service proof: {error}; {output:?}")
            });
        match response {
            ResponseEnvelopeV1::Output(response) => {
                assert!(
                    output.status.success(),
                    "{route} output must exit successfully"
                );
                assert_eq!(response.command().as_str(), *route);
            }
            ResponseEnvelopeV1::Error(response) => {
                assert!(
                    !output.status.success(),
                    "{route} error must fail the process"
                );
                assert_eq!(response.command().as_str(), *route);
                assert!(
                    !response.code().as_str().is_empty(),
                    "{route} typed service failure must retain a public error code"
                );
            }
        }
    }

    let executed_routes = DAEMON_CONTRACTS
        .iter()
        .map(|contract| contract.route)
        .chain(local_successes.iter().map(|(route, _)| *route))
        .chain(service_routes.iter().map(|(route, _)| *route))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        executed_routes,
        executable_routes.into_iter().collect::<BTreeSet<_>>(),
        "PAC-048 must execute an executable success fixture or a typed mandatory service proof for all 50 current executable routes"
    );
}

#[test]
fn mcont004_compact_status_is_idle_only_and_preserves_the_closed_wire_projection() {
    for arguments in [
        &["--json", "status", "--compact"][..],
        &[
            "--json",
            "status",
            "--wait-for-idle",
            "--compact",
            "--verbose",
        ][..],
        &[
            "--json",
            "status",
            "--compact",
            "--after-job",
            RECORDING_JOB_ID,
        ][..],
    ] {
        assert_eq!(run(arguments).status.code(), Some(2));
    }

    let fixture = DynamicCompletionFixture::new();
    let daemon = RecordingDaemon::start(
        &fixture,
        vec![
            RecordingReply::Output(authoritative_compact_status_result()),
            RecordingReply::Output(authoritative_compact_status_result()),
        ],
    );
    let worktree = fixture.root.to_string_lossy();
    let json_output = fixture.run(&[
        "--json",
        "--worktree",
        worktree.as_ref(),
        "status",
        "--wait-for-idle",
        "--compact",
    ]);
    assert!(json_output.status.success(), "{json_output:?}");
    assert!(json_output.stdout.len() <= 262_144);
    let json = one_json(&json_output);
    assert_eq!(json["result"]["schema"], "podway.compact-status-result/v1");
    for omitted in ["task", "stages", "previous_attempts"] {
        assert!(json["result"].get(omitted).is_none(), "{omitted}");
    }
    assert!(json["result"]["items"][0].get("prompt").is_none());
    assert!(json["result"]["items"][0].get("value").is_none());
    assert!(json["result"]["blockers"][0].get("reason").is_none());

    let text_output = fixture.run(&[
        "--worktree",
        worktree.as_ref(),
        "status",
        "--wait-for-idle",
        "--compact",
    ]);
    assert!(text_output.status.success(), "{text_output:?}");
    let text = String::from_utf8(text_output.stdout).unwrap();
    assert!(text.contains("procedure: fixture version=1"));
    assert!(text.contains("item: decision"));
    assert!(!text.contains("prompt="));
    assert!(!text.contains("value="));
    assert!(!text.contains("reason="));

    let requests = daemon.finish();
    for request in requests {
        assert_eq!(request["command"], "session.status");
        assert_eq!(request["payload"]["wait_for_idle"], true);
        assert_eq!(request["payload"]["compact"], true);
        assert!(request["payload"].get("verbose").is_none());
        assert!(request["payload"].get("after_job_id").is_none());
    }
}

#[test]
fn pac_050_status_text_and_json_render_the_same_typed_state_semantics() {
    let fixture = DynamicCompletionFixture::new();
    let contract = DAEMON_CONTRACTS
        .iter()
        .copied()
        .find(|contract| contract.route == "session.status")
        .expect("status route must be recorded");
    let mut status = recording_success_result("session.status");
    status["blockers"] = serde_json::json!([{
        "id": RECORDING_BLOCKER_ID,
        "attempt_id": RECORDING_ATTEMPT_ID,
        "reason": "waiting for a durable dependency"
    }]);
    status["queue"] = serde_json::json!({
        "pending_mutations": true,
        "queued_count": 2,
        "running_job_id": RECORDING_JOB_ID,
        "latest_workspace_sequence": 9
    });

    let json_server =
        RecordingDaemon::start(&fixture, vec![RecordingReply::Output(status.clone())]);
    let json_arguments = contract_arguments(contract, &fixture.root, true, false);
    let json_output = fixture.run_in(&fixture.root, &json_arguments);
    let json_requests = json_server.finish();
    assert!(
        json_output.status.success(),
        "JSON status failed: {json_output:?}"
    );
    let json_response = one_json(&json_output);
    assert_eq!(json_response["schema"], "podway.output/v1");
    let json_projection = status_json_semantic_projection(&json_response);
    assert_recorded_contract(&json_requests, contract, &fixture.root, false);

    let text_server = RecordingDaemon::start(&fixture, vec![RecordingReply::Output(status)]);
    let text_arguments = contract_arguments(contract, &fixture.root, false, false);
    let text_output = fixture.run_in(&fixture.root, &text_arguments);
    let text_requests = text_server.finish();
    assert!(
        text_output.status.success(),
        "text status failed: {text_output:?}"
    );
    assert!(text_output.stderr.is_empty());
    let text_projection = status_text_semantic_projection(&text_output.stdout);
    assert_recorded_contract(&text_requests, contract, &fixture.root, false);

    assert_eq!(
        text_projection, json_projection,
        "text and JSON must preserve non-empty blockers and pending/running queue semantics",
    );
}

#[test]
fn all_public_route_grammars_parse_to_a_single_structured_outcome() {
    let routes: &[(&str, &[&str])] = &[
        ("help", &["help"]),
        ("version", &["version"]),
        ("completions", &["completions", "bash"]),
        (
            "procedure.validate",
            &["procedure", "validate", "missing.yaml"],
        ),
        ("procedure.show", &["procedure", "show", "missing.yaml"]),
        ("procedure.format", &["procedure", "format", "missing.yaml"]),
        ("procedure.lint", &["procedure", "lint", "missing.yaml"]),
        ("procedure.check", &["procedure", "check", "missing.yaml"]),
        ("procedure.scaffold", &["procedure", "scaffold"]),
        ("preset.list", &["preset", "list"]),
        ("preset.show", &["preset", "show", "sw-dev"]),
        ("preset.explain", &["preset", "explain", "sw-dev"]),
        (
            "daemon.install",
            &[
                "daemon",
                "install",
                "--daemon-path",
                "/definitely/missing/podwayd",
            ],
        ),
        (
            "daemon.uninstall",
            &[
                "daemon",
                "uninstall",
                "--yes",
                "--worktree",
                "/tmp/podway-grammar",
            ],
        ),
        (
            "daemon.start",
            &["daemon", "start", "--worktree", "/tmp/podway-grammar"],
        ),
        (
            "daemon.stop",
            &["daemon", "stop", "--worktree", "/tmp/podway-grammar"],
        ),
        (
            "daemon.restart",
            &["daemon", "restart", "--worktree", "/tmp/podway-grammar"],
        ),
        (
            "daemon.status",
            &["daemon", "status", "--worktree", "/tmp/podway-grammar"],
        ),
        (
            "daemon.terminate",
            &["--dev", "terminate", "--worktree", "/tmp/podway-grammar"],
        ),
        (
            "daemon.logs",
            &[
                "daemon",
                "logs",
                "--lines",
                "10",
                "--worktree",
                "/tmp/podway-grammar",
            ],
        ),
        ("workspace.init", &["init"]),
        ("workspace.doctor", &["doctor", "--deep"]),
        ("workspace.show", &["workspace", "show"]),
        ("workspace.repair", &["workspace", "repair"]),
        (
            "session.start",
            &["start", "--preset", "sw-dev", "--task", "task"],
        ),
        (
            "session.start_replace",
            &[
                "start",
                "--preset",
                "sw-dev",
                "--task",
                "task",
                "--replace",
                "--yes",
            ],
        ),
        ("session.status", &["status", "--verbose"]),
        ("session.next", &["next", "--wait-for-idle"]),
        ("session.complete", &["complete"]),
        ("session.skip", &["skip", "--reason", "reason"]),
        ("session.retry", &["retry", "--reason", "reason"]),
        (
            "session.return",
            &[
                "return",
                "--to",
                "implement",
                "--reason",
                "reason",
                "--dry-run",
            ],
        ),
        ("session.block", &["block", "--reason", "reason"]),
        ("session.unblock", &["unblock", "--all"]),
        ("session.cancel", &["cancel", "--reason", "reason"]),
        (
            "session.reopen",
            &[
                "reopen",
                "--to",
                "implement",
                "--reason",
                "reason",
                "--dry-run",
            ],
        ),
        ("session.reset", &["reset", "--yes"]),
        (
            "workspace.reset_all",
            &["reset", "--all", "--force", "--yes"],
        ),
        ("item.check", &["check", "item"]),
        ("item.uncheck", &["uncheck", "item"]),
        ("item.set", &["set", "item", "value"]),
        ("item.add", &["add", "item", "value"]),
        (
            "item.remove",
            &["remove", "item", "value", "--ignore-missing"],
        ),
        (
            "item.attach",
            &["attach", "item", "path.txt", "--media-type", "text/plain"],
        ),
        ("item.clear", &["clear", "item"]),
        ("job.list", &["job", "list", "--state", "queued"]),
        (
            "job.lookup",
            &["job", "lookup", "--idempotency-key", "recording-key"],
        ),
        (
            "job.status",
            &["job", "status", "123e4567-e89b-42d3-a456-426614174003"],
        ),
        (
            "job.wait",
            &["job", "wait", "123e4567-e89b-42d3-a456-426614174003"],
        ),
        (
            "job.cancel",
            &["job", "cancel", "123e4567-e89b-42d3-a456-426614174003"],
        ),
    ];
    assert_eq!(routes.len(), 50);

    for (route, arguments) in routes {
        let mut argv = vec!["--json"];
        argv.extend_from_slice(arguments);
        let output = run(&argv);
        let response = one_json(&output);
        if *route == "version" {
            assert!(
                output.status.success(),
                "version must work offline: {output:?}"
            );
            assert_eq!(
                response,
                serde_json::json!({
                    "name": "podway",
                    "version": format!("v{}", env!("CARGO_PKG_VERSION")),
                })
            );
            continue;
        }
        assert_eq!(
            response["command"], *route,
            "{route} must retain its canonical command in every public envelope"
        );
        match *route {
            "help" | "completions" | "preset.list" | "preset.show" | "preset.explain" => {
                assert!(
                    output.status.success(),
                    "{route} must work offline: {output:?}"
                );
                assert_eq!(response["schema"], "podway.output/v1");
            }
            "procedure.validate" | "procedure.show" | "procedure.format" | "procedure.lint"
            | "procedure.check" => {
                assert_eq!(output.status.code(), Some(1));
                assert_eq!(response["schema"], "podway.error/v1");
                assert_eq!(response["code"], "PROCEDURE_NOT_FOUND");
                assert_eq!(response["retryable"], false);
                assert_eq!(response["exit_code"], 1);
            }
            // The only local authoring route that reads no file, so its grammar row is a
            // success row: there is no missing path for it to fail on.
            "procedure.scaffold" => {
                assert!(
                    output.status.success(),
                    "{route} must work offline: {output:?}"
                );
                assert_eq!(response["schema"], "podway.output/v2");
                assert_eq!(response["result"]["operation"], "scaffold");
            }
            "daemon.uninstall" | "daemon.start" | "daemon.stop" | "daemon.restart"
            | "daemon.status" | "daemon.terminate" | "daemon.logs" => {
                assert_eq!(output.status.code(), Some(2), "{route}: {output:?}");
                assert_eq!(response["schema"], "podway.error/v1");
                assert_eq!(response["code"], "REQUEST_INVALID");
                assert_eq!(response["retryable"], false);
                assert_eq!(response["exit_code"], 2);
            }
            "daemon.install" => {
                assert_eq!(output.status.code(), Some(3), "{route}: {output:?}");
                assert_eq!(response["schema"], "podway.error/v1");
                assert_eq!(response["code"], "DAEMON_UNAVAILABLE");
                assert_eq!(response["retryable"], true);
                assert_eq!(response["exit_code"], 3);
            }
            route if route.starts_with("daemon.") => {
                assert!(
                    output.status.success(),
                    "{route} must complete locally when the service is absent: {output:?}"
                );
                assert_eq!(response["schema"], "podway.output/v1");
            }
            _ => {
                assert_eq!(output.status.code(), Some(3), "{route}: {output:?}");
                assert_eq!(response["schema"], "podway.error/v1");
                assert_eq!(response["code"], "DAEMON_UNAVAILABLE");
                assert_eq!(response["retryable"], true);
                assert_eq!(response["exit_code"], 3);
            }
        }
    }
}
fn shell_route_candidates(script: &str, route: &str, prefix: &str) -> Vec<String> {
    let marker = format!("    \"{route}\")\n");
    let block = script
        .split_once(&marker)
        .map(|(_, remainder)| remainder)
        .and_then(|remainder| remainder.split_once("      ;;\n").map(|(block, _)| block))
        .unwrap_or_else(|| panic!("completion script must contain the {route} route"));
    block
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(|candidates| {
            candidates
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn fish_route_flags(script: &str, route: &str) -> Vec<String> {
    let route_predicate = format!("__podway_route_is \"{route}\"");
    script
        .lines()
        .filter(|line| line.contains(&route_predicate))
        .filter_map(|line| {
            line.split_once(" -l ")
                .and_then(|(_, suffix)| suffix.split_whitespace().next())
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn completion_route(surface: &RouteSurface) -> String {
    match surface.parser.first().copied() {
        Some("procedure" | "preset" | "daemon" | "workspace" | "job") => {
            surface.parser[..2].join(" ")
        }
        Some(route) => route.to_owned(),
        None => panic!("every public route must have a parser form"),
    }
}

#[test]
fn public_route_surface_table_keeps_parser_help_and_completion_in_lockstep() {
    assert_eq!(ROUTE_SURFACES.len(), 50);
    let bash = run(&["completions", "bash"]);
    let zsh = run(&["completions", "zsh"]);
    let fish = run(&["completions", "fish"]);
    assert!(bash.status.success() && zsh.status.success() && fish.status.success());
    let bash = String::from_utf8(bash.stdout).expect("bash completion must be UTF-8");
    let zsh = String::from_utf8(zsh.stdout).expect("zsh completion must be UTF-8");
    let fish = String::from_utf8(fish.stdout).expect("fish completion must be UTF-8");

    for surface in ROUTE_SURFACES {
        // The public contract disables Clap's `--help` flag. The parser must reject it before
        // normal lifecycle execution reaches command dispatch.
        let mut parser_arguments = surface.parser.to_vec();
        parser_arguments.push("--help");
        let parser_output = run(&parser_arguments);
        assert_eq!(
            parser_output.status.code(),
            Some(2),
            "{} clap parser help must short-circuit dispatch: {parser_output:?}",
            surface.route
        );

        let help_output = run(&["--json", "help", surface.route]);
        assert!(
            help_output.status.success(),
            "{} help must work offline",
            surface.route
        );
        let help = one_json(&help_output);
        let text = help["result"]["text"]
            .as_str()
            .expect("help response must contain text");
        assert!(
            text.contains("Usage:"),
            "{} help must contain usage",
            surface.route
        );
        for token in surface.help_tokens {
            assert!(
                text.contains(token),
                "{} help must document {token}",
                surface.route
            );
        }

        let expected_candidates = surface
            .flags
            .iter()
            .chain(surface.values)
            .map(|candidate| (*candidate).to_owned())
            .collect::<Vec<_>>();
        let completion_route = completion_route(surface);
        assert_eq!(
            shell_route_candidates(&bash, &completion_route, "      printf '%s\\n' "),
            expected_candidates,
            "{} bash completion candidates drifted from the public table",
            surface.route
        );
        assert_eq!(
            shell_route_candidates(&zsh, &completion_route, "      print -rl -- "),
            expected_candidates,
            "{} zsh completion candidates drifted from the public table",
            surface.route
        );
        assert_eq!(
            fish_route_flags(&fish, &completion_route),
            surface
                .flags
                .iter()
                .map(|flag| flag.trim_start_matches("--").to_owned())
                .collect::<Vec<_>>(),
            "{} fish completion flags drifted from the public table",
            surface.route
        );

        let expected_values = surface.values.join(" ");
        let fish_values = format!(
            "complete -c podway -n '__podway_route_is \"{}\"' -a '{}'",
            completion_route, expected_values
        );
        assert_eq!(
            fish.lines().any(|line| line == fish_values),
            !surface.values.is_empty(),
            "{} fish completion values drifted from the public table",
            surface.route
        );
        let dynamic = format!("_podway_dynamic {}", surface.dynamic.unwrap_or_default());
        assert_eq!(
            bash.split_once(&format!("    \"{completion_route}\")\n"))
                .and_then(|(_, remainder)| remainder.split_once("      ;;\n"))
                .is_some_and(|(block, _)| block.contains(&dynamic)),
            surface.dynamic.is_some(),
            "{} bash dynamic completion drifted from the public table",
            surface.route
        );
    }
}

#[test]
fn every_public_route_has_offline_sot_syntax_and_an_example() {
    let routes = [
        "help",
        "version",
        "completions",
        "procedure.validate",
        "procedure.show",
        "procedure.format",
        "procedure.lint",
        "procedure.check",
        "procedure.scaffold",
        "preset.list",
        "preset.show",
        "preset.explain",
        "daemon.install",
        "daemon.uninstall",
        "daemon.start",
        "daemon.stop",
        "daemon.restart",
        "daemon.status",
        "daemon.terminate",
        "daemon.logs",
        "workspace.init",
        "workspace.doctor",
        "workspace.show",
        "workspace.repair",
        "session.start",
        "session.start_replace",
        "session.status",
        "session.next",
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
        "job.list",
        "job.lookup",
        "job.status",
        "job.wait",
        "job.cancel",
    ];
    assert_eq!(routes.len(), 50);

    for route in routes {
        let output = run(&["--json", "help", route]);
        assert!(
            output.status.success(),
            "offline help failed for {route}: {output:?}"
        );
        let response = one_json(&output);
        assert_eq!(response["schema"], "podway.output/v1");
        assert_eq!(response["command"], "help");
        let text = response["result"]["text"]
            .as_str()
            .expect("help result must include text");
        assert!(text.contains("Usage:"), "{route} help must include syntax");
        assert!(
            text.contains("Example:") || text.contains("Examples:"),
            "{route} help must include an example"
        );
    }
}

#[test]
fn start_replace_dry_run_requires_the_readonly_daemon_preview() {
    let output = run(&[
        "--json",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "preview",
        "--replace",
        "--dry-run",
        "--if-session-revision",
        "1",
    ]);
    assert_eq!(output.status.code(), Some(3));
    let response = one_json(&output);
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["command"], "session.start_replace");
    assert_eq!(response["code"], "DAEMON_UNAVAILABLE");
    assert_eq!(response["retryable"], true);
    assert_eq!(response["exit_code"], 3);
}

#[test]
fn start_dry_run_with_explicit_workspace_requires_the_readonly_daemon_preview() {
    let output = run(&[
        "--json",
        "start",
        "--preset",
        "sw-dev",
        "--task",
        "preview",
        "--dry-run",
        "--if-workspace-uuid",
        EXPLICIT_WORKSPACE_ID,
    ]);
    assert_eq!(output.status.code(), Some(3));
    let response = one_json(&output);
    assert_eq!(response["schema"], "podway.error/v1");
    assert_eq!(response["command"], "session.start");
    assert_eq!(response["code"], "DAEMON_UNAVAILABLE");
    assert_eq!(response["retryable"], true);
    assert_eq!(response["exit_code"], 3);
}
#[test]
fn invalid_applicability_and_confirmation_are_usage_json_errors() {
    for arguments in [
        &["--json", "version", "--worktree", "."][..],
        &["--json", "status", "--if-workspace-uuid", "not-a-uuid"][..],
        &[
            "--json",
            "doctor",
            "--if-workspace-uuid",
            EXPLICIT_WORKSPACE_ID,
        ][..],
        &["--json", "job", "lookup"][..],
        &["--json", "job", "lookup", "--idempotency-key", ""][..],
        &[
            "--json",
            "start",
            "--preset",
            "sw-dev",
            "--task",
            "task",
            "--if-session-id",
            EXPLICIT_SESSION_ID,
        ][..],
        &[
            "--json",
            "reset",
            "--all",
            "--force",
            "--yes",
            "--if-session-id",
            EXPLICIT_SESSION_ID,
        ][..],
        &["--json", "status", "--detach"][..],
        &[
            "--json",
            "set",
            "item",
            "value",
            "--if-item-revision",
            "1",
            "--if-session-revision",
            "2",
        ][..],
        &["--json", "reset", "--all", "--yes"][..],
        &[
            "--json",
            "start",
            "--preset",
            "sw-dev",
            "--task",
            "task",
            "--replace",
        ][..],
        &[
            "--json",
            "attach",
            "item",
            "path.txt",
            "--reference",
            "build:1",
            "--digest",
            "sha256:00",
            "--size",
            "1",
            "--media-type",
            "text/plain",
        ][..],
    ] {
        let output = run(arguments);
        assert_eq!(
            output.status.code(),
            Some(2),
            "unexpected status: {output:?}"
        );
        let error = one_json(&output);
        assert_eq!(error["schema"], "podway.error/v1");
        assert_eq!(error["exit_code"], 2);
    }
}

#[test]
fn static_commands_and_all_completion_targets_do_not_need_a_daemon() {
    let version = run(&["--json", "version"]);
    assert!(
        version.status.success(),
        "version command failed: {version:?}"
    );
    assert_eq!(
        one_json(&version),
        serde_json::json!({
            "name": "podway",
            "version": format!("v{}", env!("CARGO_PKG_VERSION")),
        })
    );

    for arguments in [
        &["--json", "preset", "list"][..],
        &["--json", "preset", "show", "sw-dev"][..],
        &["--json", "preset", "explain", "sw-dev"][..],
        &["--json", "completions", "bash"][..],
        &["--json", "completions", "zsh"][..],
        &["--json", "completions", "fish"][..],
        &["--json", "__complete", "items"][..],
    ] {
        let output = run(arguments);
        assert!(output.status.success(), "static command failed: {output:?}");
        let response = one_json(&output);
        assert_eq!(response["schema"], "podway.output/v1");
    }

    for shell in ["bash", "zsh", "fish"] {
        let output = run(&["completions", shell]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).expect("completion is UTF-8");
        assert!(text.contains("__complete"));
        assert!(text.contains("sw-dev"));
        assert!(text.contains("--worktree") || text.contains("-l worktree"));
        assert!(text.contains("validate") && text.contains("show"));
        assert!(text.contains("install") && text.contains("uninstall"));
        assert!(
            text.contains("(__podway_dynamic items)")
                || text.contains("$(_podway_dynamic items)")
                || text.contains("_podway_dynamic items")
        );
    }
}
#[test]
fn generated_dynamic_completion_forwards_the_selected_worktree_in_every_shell() {
    let fixture = DynamicCompletionFixture::new();
    let default_worktree = fixture.root.join("default-worktree");
    let explicit_worktree = fixture.root.join("explicit worktree");
    fs::create_dir(&default_worktree).expect("default completion worktree must be created");
    fs::create_dir(&explicit_worktree).expect("explicit completion worktree must be created");
    let bin = fixture.install_cli_on_path();
    let dynamic_routes: &[(&str, &[&str], &str)] = &[
        ("items", &["check"], "decision"),
        ("blockers", &["unblock"], RECORDING_BLOCKER_ID),
        ("returns", &["return"], "plan"),
        (
            "jobs",
            &["job", "status"],
            "123e4567-e89b-42d3-a456-426614174005",
        ),
    ];

    for shell in ["bash", "zsh", "fish"] {
        assert!(
            shell_available(shell),
            "{shell} is a required completion-test prerequisite"
        );
        let script = CompletionScript::generated(shell);
        for (kind, route_words, expected_candidate) in dynamic_routes {
            for form in 0..3 {
                let selected_worktree = if form == 0 {
                    &default_worktree
                } else {
                    &explicit_worktree
                };
                let mut words = vec!["podway".to_owned()];
                match form {
                    0 => {}
                    1 => words.extend([
                        "--worktree".to_owned(),
                        explicit_worktree.display().to_string(),
                    ]),
                    2 => words.push(format!("--worktree={}", explicit_worktree.display())),
                    _ => unreachable!("completion worktree form must be known"),
                }
                words.extend(route_words.iter().map(|word| (*word).to_owned()));
                words.push(String::new());

                let server = RecordingDaemon::start(
                    &fixture,
                    vec![RecordingReply::Output(dynamic_completion_result(kind))],
                );
                let (candidates, stderr) = generated_dynamic_candidates(
                    shell,
                    &script,
                    &fixture,
                    &bin,
                    selected_worktree,
                    &words,
                );
                assert!(
                    stderr.is_empty(),
                    "{shell} {kind} completion must not write diagnostics: {}",
                    String::from_utf8_lossy(&stderr)
                );
                assert!(
                    candidates.contains(&(*expected_candidate).to_owned()),
                    "{shell} {kind} completion must include its authoritative candidate"
                );
                let requests = server.finish();
                assert_eq!(requests.len(), 1);
                let request = &requests[0];
                assert_eq!(
                    request["command"],
                    match *kind {
                        "items" | "blockers" => "session.status",
                        "returns" => "session.next",
                        "jobs" => "job.list",
                        _ => unreachable!("dynamic completion kind must be known"),
                    }
                );
                assert_eq!(request["operation"], "query");
                assert_eq!(
                    request["workspace"]["root"],
                    canonical_fixture_path(selected_worktree),
                    "{shell} {kind} completion form {form} queried the wrong workspace"
                );
            }
        }

        let explicit_socket = fixture.root.join(format!("completion-{shell}.sock"));
        let explicit = RecordingDaemon::start_at(
            explicit_socket.clone(),
            vec![RecordingReply::Output(dynamic_completion_result("items"))],
        );
        let words = vec![
            "podway".to_owned(),
            "--socket".to_owned(),
            explicit_socket.display().to_string(),
            "check".to_owned(),
            String::new(),
        ];
        let (candidates, stderr) =
            generated_dynamic_candidates(shell, &script, &fixture, &bin, &default_worktree, &words);
        assert!(stderr.is_empty());
        assert!(candidates.contains(&"decision".to_owned()));
        assert_eq!(explicit.finish().len(), 1);

        if fixture.socket_path.exists() {
            fs::remove_file(&fixture.socket_path)
                .expect("recording socket must be removed for unavailable completion");
        }
        let unavailable_words = vec!["podway".to_owned(), "check".to_owned(), String::new()];
        let (unavailable, unavailable_stderr) = generated_dynamic_candidates(
            shell,
            &script,
            &fixture,
            &bin,
            &default_worktree,
            &unavailable_words,
        );
        assert!(unavailable_stderr.is_empty());
        assert!(
            !unavailable.contains(&"decision".to_owned()),
            "{shell} unavailable dynamic completion must silently omit daemon candidates"
        );

        let malformed = RecordingDaemon::start(
            &fixture,
            vec![RecordingReply::Output(serde_json::json!({}))],
        );
        let (malformed_candidates, malformed_stderr) = generated_dynamic_candidates(
            shell,
            &script,
            &fixture,
            &bin,
            &default_worktree,
            &unavailable_words,
        );
        assert!(malformed_stderr.is_empty());
        assert!(
            !malformed_candidates.contains(&"decision".to_owned()),
            "{shell} malformed dynamic completion must silently omit candidates"
        );
        assert_eq!(malformed.finish().len(), 1);
    }
}
#[test]
fn generated_route_scanners_skip_worktree_values_in_every_shell() {
    let fixture = DynamicCompletionFixture::new();
    let command_named_worktree = fixture.root.join("check");
    let spaced_worktree = fixture.root.join("worktree with spaces");
    fs::create_dir(&command_named_worktree)
        .expect("command-named completion worktree must be created");
    fs::create_dir(&spaced_worktree).expect("spaced completion worktree must be created");
    let bin = fixture.install_cli_on_path();

    for shell in ["bash", "zsh", "fish"] {
        assert!(
            shell_available(shell),
            "{shell} is a required completion-test prerequisite"
        );
        let script = CompletionScript::generated(shell);
        for (worktree_arguments, expected_worktree) in [
            (
                vec!["--worktree".to_owned(), "check".to_owned()],
                &command_named_worktree,
            ),
            (vec!["--worktree=check".to_owned()], &command_named_worktree),
            (
                vec!["--worktree".to_owned(), "worktree with spaces".to_owned()],
                &spaced_worktree,
            ),
            (
                vec!["--worktree=worktree with spaces".to_owned()],
                &spaced_worktree,
            ),
        ] {
            let mut words = vec!["podway".to_owned()];
            words.extend(worktree_arguments);
            words.extend(["job".to_owned(), "status".to_owned(), String::new()]);

            let server = RecordingDaemon::start(
                &fixture,
                vec![RecordingReply::Output(dynamic_completion_result("jobs"))],
            );
            let (candidates, stderr) =
                generated_dynamic_candidates(shell, &script, &fixture, &bin, &fixture.root, &words);
            assert!(
                stderr.is_empty(),
                "{shell} completion must not write diagnostics: {}",
                String::from_utf8_lossy(&stderr)
            );
            assert!(
                candidates.contains(&"123e4567-e89b-42d3-a456-426614174005".to_owned()),
                "{shell} must retain the nested job status completion route"
            );

            let requests = server.finish();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["command"], "job.list");
            assert_eq!(
                requests[0]["workspace"]["root"],
                canonical_fixture_path(expected_worktree),
                "{shell} forwarded the wrong worktree"
            );
        }
    }
}

#[test]
fn dynamic_completion_extracts_ids_from_the_authoritative_typed_status_result() {
    let fixture = DynamicCompletionFixture::new();
    let server = DynamicCompletionServer::start(
        &fixture,
        authoritative_status_result(serde_json::json!([{
            "id": "decision",
            "type": "text",
            "prompt": "Record the decision",
            "required": true,
            "satisfied": false,
            "revision": 0,
            "value": null
        }])),
    );
    let worktree = fixture
        .root
        .to_str()
        .expect("fixture worktree must be UTF-8");
    let output = fixture.run(&["--worktree", worktree, "__complete", "items"]);

    assert!(
        output.status.success(),
        "dynamic completion failed: {output:?}"
    );
    assert_eq!(output.stdout, b"decision\n");
    assert!(output.stderr.is_empty());
    assert_eq!(
        server.finish(),
        ("session.status".to_owned(), OperationV1::Query)
    );
}

#[test]
fn dynamic_completion_uses_typed_next_and_job_list_sources() {
    let returns_fixture = DynamicCompletionFixture::new();
    let returns_server =
        DynamicCompletionServer::start(&returns_fixture, authoritative_next_result());
    let returns_worktree = returns_fixture
        .root
        .to_str()
        .expect("fixture worktree must be UTF-8");
    let returns = returns_fixture.run(&["--worktree", returns_worktree, "__complete", "returns"]);

    assert!(
        returns.status.success(),
        "return completion failed: {returns:?}"
    );
    assert_eq!(returns.stdout, b"plan\nintake\n");
    assert!(returns.stderr.is_empty());
    assert_eq!(
        returns_server.finish(),
        ("session.next".to_owned(), OperationV1::Query)
    );

    let jobs_fixture = DynamicCompletionFixture::new();
    let jobs_server =
        DynamicCompletionServer::start(&jobs_fixture, authoritative_job_list_result());
    let jobs_worktree = jobs_fixture
        .root
        .to_str()
        .expect("fixture worktree must be UTF-8");
    let jobs = jobs_fixture.run(&["--worktree", jobs_worktree, "__complete", "jobs"]);

    assert!(jobs.status.success(), "job completion failed: {jobs:?}");
    assert_eq!(jobs.stdout, b"123e4567-e89b-42d3-a456-426614174005\n");
    assert!(jobs.stderr.is_empty());
    assert_eq!(
        jobs_server.finish(),
        ("job.list".to_owned(), OperationV1::Query)
    );
}

#[test]
fn dynamic_completion_rejects_untyped_top_level_candidate_arrays_silently() {
    let fixture = DynamicCompletionFixture::new();
    let server = DynamicCompletionServer::start(
        &fixture,
        authoritative_status_result(serde_json::json!(["not-an-authoritative-item"])),
    );
    let worktree = fixture
        .root
        .to_str()
        .expect("fixture worktree must be UTF-8");
    let output = fixture.run(&["--worktree", worktree, "__complete", "items"]);

    assert!(
        output.status.success(),
        "dynamic completion failed: {output:?}"
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(
        server.finish(),
        ("session.status".to_owned(), OperationV1::Query)
    );
}

#[test]
fn hidden_dynamic_completion_silently_degrades_without_a_daemon() {
    let unique =
        std::env::temp_dir().join(format!("podway-completion-missing-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(["__complete", "items"])
        .env("HOME", &unique)
        .env("TMPDIR", &unique)
        .output()
        .expect("podway binary must run");
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unavailable dynamic completion must not emit diagnostics"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn daemon_install_rejects_non_native_executables_before_launchctl() {
    let root = unique_fixture_path("podway-invalid-daemon");
    fs::create_dir_all(&root).expect("invalid daemon fixture directory");
    let home = unique_short_fixture_path();
    fs::create_dir_all(&home).expect("invalid daemon HOME fixture directory");
    let temporary = unique_short_fixture_path();

    let matching_version_script = format!(
        "#!/bin/sh\nprintf 'podwayd {}\\n'\n",
        env!("CARGO_PKG_VERSION")
    );
    for (name, bytes) in [
        ("matching-version-script", matching_version_script.as_bytes()),
        ("truncated-macho", b"\xcf\xfa\xed\xfe".as_slice()),
        (
            "x86_64-macho",
            b"\xcf\xfa\xed\xfe\x07\x00\x00\x01\x03\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00".as_slice(),
        ),
        ("fat-macho", b"\xca\xfe\xba\xbe\x00\x00\x00\x00".as_slice()),
    ] {
        let binary = root.join(name);
        fs::write(&binary, bytes).expect("invalid daemon fixture");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("invalid daemon fixture mode");

        let binary_argument = binary.to_string_lossy().into_owned();
        let output = Command::new(env!("CARGO_BIN_EXE_podway"))
            .args([
                "--json",
                "daemon",
                "install",
                "--daemon-path",
                &binary_argument,
            ])
            .env("HOME", &home)
            .env("TMPDIR", &temporary)
            .output()
            .expect("podway binary must run");
        let response = one_json(&output);
        assert_eq!(output.status.code(), Some(3), "{name}: {output:?}");
        assert_eq!(
            response["code"],
            "DAEMON_VERSION_INCOMPATIBLE",
            "{name}: {response}"
        );
        assert_eq!(response["retryable"], false, "{name}: {response}");
        assert!(
            !home.join("Library/LaunchAgents").exists(),
            "{name} must not publish a service"
        );
    }

    let _ = fs::remove_dir_all(temporary);
    fs::remove_dir_all(home).expect("remove invalid daemon HOME fixture");
    fs::remove_dir_all(root).expect("remove invalid daemon fixture");
}
#[test]
fn pac_003_help_states_the_same_user_local_socket_trust_boundary() {
    let output = run(&["--json", "help"]);
    assert!(output.status.success(), "overview help failed: {output:?}");
    let response = one_json(&output);
    let text = response["result"]["text"]
        .as_str()
        .expect("overview help must include text");
    for required_text in [
        "Podway trusts same-user processes connecting through its local socket.",
        "It provides no authentication or workspace access key.",
        "It does not protect against malicious same-user processes.",
    ] {
        assert!(
            text.contains(required_text),
            "overview help must state the required trust-boundary text: {required_text}"
        );
    }
    let release_notes = include_str!("../../../RELEASE_NOTES.md");
    assert!(release_notes.contains("Podway is a same-user local tool."));
    assert!(release_notes.contains("does not provide a multi-user access-control boundary"));
}
#[test]
fn pac_067_public_surface_has_no_authentication_or_workspace_access_key() {
    const FORBIDDEN_FIELDS: &[&str] = &[
        "authentication",
        "authorization",
        "access_key",
        "workspace_access_key",
        "token",
        "password",
    ];

    fn assert_no_forbidden_fields(value: &Value) {
        match value {
            Value::Object(fields) => {
                for (field, nested) in fields {
                    assert!(
                        !FORBIDDEN_FIELDS.contains(&field.as_str()),
                        "public request unexpectedly exposes credential field {field}"
                    );
                    assert_no_forbidden_fields(nested);
                }
            }
            Value::Array(values) => {
                for value in values {
                    assert_no_forbidden_fields(value);
                }
            }
            _ => {}
        }
    }

    let overview = one_json(&run(&["--json", "help"]));
    let help_text = overview["result"]["text"]
        .as_str()
        .expect("overview help must include text");
    assert!(help_text.contains("It provides no authentication or workspace access key."));

    for shell in ["bash", "zsh", "fish"] {
        let completions = run(&["completions", shell]);
        assert!(
            completions.status.success(),
            "{shell} completion generation failed"
        );
        let text = String::from_utf8(completions.stdout).expect("completion output must be UTF-8");
        for forbidden in [
            "--auth",
            "--authentication",
            "--authorization",
            "--access-key",
            "--workspace-access-key",
            "--token",
            "--password",
        ] {
            assert!(
                !text.contains(forbidden),
                "{shell} completions unexpectedly expose {forbidden}"
            );
        }
    }

    let fixture = DynamicCompletionFixture::new();
    for contract in DAEMON_CONTRACTS {
        let (output, requests) = execute_recorded_contract(&fixture, *contract, true, false, false);
        assert!(
            output.status.success(),
            "{} request capture failed: {output:?}",
            contract.route
        );
        for request in requests {
            assert_no_forbidden_fields(&request);
        }
    }
}
