use std::{
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
    let isolated = unique_fixture_path("podway-cli-phase5-no-daemon");
    let output = Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(arguments)
        .env("HOME", &isolated)
        .env("TMPDIR", &isolated)
        .output()
        .expect("podway binary must run");
    if arguments
        .windows(2)
        .any(|window| window == ["daemon", "install"])
    {
        let _ = Command::new("launchctl")
            .args([
                "bootout",
                &format!("gui/{}/dev.podway.podwayd", geteuid().as_raw()),
            ])
            .output();
    }
    let _ = fs::remove_dir_all(isolated);
    output
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

fn shell_lines(output: Output, shell: &str) -> Vec<String> {
    assert!(
        output.status.success(),
        "{shell} completion execution failed: {output:?}"
    );
    String::from_utf8(output.stdout)
        .expect("{shell} completion output must be UTF-8")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

fn bash_candidates(script: &CompletionScript, words: &[&str]) -> Vec<String> {
    let word_count = words.len();
    let words = words
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    let program = format!(
        "source \"$1\"\nCOMP_WORDS=({words})\nCOMP_CWORD={}\n_podway\nprintf '%s\\n' \"${{COMPREPLY[@]}}\"\n",
        word_count - 1
    );
    shell_lines(
        Command::new("bash")
            .args(["-c", &program, "bash"])
            .arg(&script.path)
            .output()
            .expect("bash must run"),
        "bash",
    )
}

fn zsh_candidates(script: &CompletionScript, words: &[&str]) -> Vec<String> {
    let word_count = words.len();
    let words = words
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    let program = format!(
        "autoload -Uz compinit\ncompinit -D -i\nsource \"$1\"\nwords=({words})\nCURRENT={word_count}\nroute=$(_podway_route)\n_podway_candidates \"$route\"\n",
    );
    shell_lines(
        Command::new("zsh")
            .args(["-fc", &program, "zsh"])
            .arg(&script.path)
            .output()
            .expect("zsh must run"),
        "zsh",
    )
}

fn fish_candidates(script: &CompletionScript, command_line: &str) -> Vec<String> {
    shell_lines(
        Command::new("fish")
            .args(["-c", "source $argv[1]; complete -C \"$argv[2]\""])
            .arg(&script.path)
            .arg(command_line)
            .output()
            .expect("fish must run"),
        "fish",
    )
    .into_iter()
    .map(|line| {
        line.split_once('\t')
            .map_or(line.as_str(), |(candidate, _)| candidate)
            .to_owned()
    })
    .collect()
}
fn generated_dynamic_candidates(
    shell: &str,
    script: &CompletionScript,
    fixture: &DynamicCompletionFixture,
    bin: &Path,
    current_dir: &Path,
    words: &[String],
) -> (Vec<String>, Vec<u8>) {
    let rendered_words = words
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
                words.len() - 1
            );
            Command::new("bash")
                .args(["-c", &program, "bash"])
                .arg(&script.path)
                .current_dir(current_dir)
                .env("PATH", &path)
                .env("HOME", &fixture.home)
                .env("TMPDIR", &fixture.temporary)
                .output()
                .expect("bash must run")
        }
        "zsh" => {
            let program = format!(
                "autoload -Uz compinit\ncompinit -D -i\nsource \"$1\"\nwords=({rendered_words})\nCURRENT={}\nroute=$(_podway_route)\n_podway_candidates \"$route\"\n",
                words.len()
            );
            Command::new("zsh")
                .args(["-fc", &program, "zsh"])
                .arg(&script.path)
                .current_dir(current_dir)
                .env("PATH", &path)
                .env("HOME", &fixture.home)
                .env("TMPDIR", &fixture.temporary)
                .output()
                .expect("zsh must run")
        }
        "fish" => {
            let command_line = format!(
                "{} ",
                words
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
                .env("HOME", &fixture.home)
                .env("TMPDIR", &fixture.temporary)
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
    home: PathBuf,
    temporary: PathBuf,
    socket_path: PathBuf,
}

impl DynamicCompletionFixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from("/tmp").join(format!("pdc-{}-{sequence}", std::process::id()));
        let home = root.join("home");
        let temporary = root.join("temporary");
        fs::create_dir_all(&home).expect("fixture home must be created");
        fs::create_dir_all(&temporary).expect("fixture temporary directory must be created");
        let paths = ServiceRuntimePathsV1::for_user(&home, &temporary, geteuid().as_raw())
            .expect("fixture paths must be valid");
        fs::create_dir(paths.runtime_directory().as_path())
            .expect("fixture daemon runtime directory must be created");
        Self {
            root,
            home,
            temporary,
            socket_path: paths.socket_path().as_path().to_path_buf(),
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(arguments)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.temporary)
            .output()
            .expect("podway binary must run")
    }
    fn run_in(&self, directory: &Path, arguments: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(arguments)
            .current_dir(directory)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.temporary)
            .output()
            .expect("podway binary must run")
    }

    fn install_cli_on_path(&self) -> PathBuf {
        let bin = self.root.join("bin");
        fs::create_dir(&bin).expect("completion fixture bin directory must be created");
        symlink(env!("CARGO_BIN_EXE_podway"), bin.join("podway"))
            .expect("completion fixture podway link must be created");
        bin
    }
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

fn authoritative_next_result() -> Value {
    serde_json::json!({
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
        if fixture.socket_path.exists() {
            fs::remove_file(&fixture.socket_path)
                .expect("previous recording daemon socket must be removed");
        }
        let listener = UnixListener::bind(&fixture.socket_path)
            .expect("recording daemon socket must bind at the service-owned path");
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
        RecordingReply::Output(result) => serde_json::json!({
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
        }),
        RecordingReply::Error => serde_json::json!({
            "schema": "podway.error/v1",
            "request_id": request.request_id().as_str(),
            "command": request.command().as_str(),
            "generated_at": "2026-07-16T12:34:56.789Z",
            "code": "TEST_ERROR",
            "message": "recording daemon rejection",
            "retryable": false,
            "exit_code": 1,
            "details": {}
        }),
    }
}

fn recording_success_result(command: &str) -> Value {
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
        _ => serde_json::json!({ "accepted": true }),
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
const DAEMON_READ_FLAGS: &[&str] = &["--json", "--worktree", "--timeout", "--no-color", "--quiet"];
const SESSION_MUTATION_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-session-revision",
    "--if-attempt",
];
const ITEM_MUTATION_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-attempt",
    "--if-item-revision",
];
const START_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-session-revision",
    "--preset",
    "--procedure",
    "--task",
    "--replace",
    "--dry-run",
    "--yes",
];
const RESET_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
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
    "--no-color",
    "--quiet",
    "--verbose",
    "--wait-for-idle",
    "--after-job",
];
const NEXT_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--wait-for-idle",
    "--after-job",
];
const SKIP_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-session-revision",
    "--if-attempt",
    "--reason",
];
const RETURN_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
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
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-session-revision",
    "--if-attempt",
    "--all",
];
const REOPEN_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-session-revision",
    "--to",
    "--reason",
    "--dry-run",
];
const SET_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-attempt",
    "--if-item-revision",
    "--stdin",
];
const REMOVE_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
    "--if-attempt",
    "--if-item-revision",
    "--ignore-missing",
];
const ATTACH_SURFACE_FLAGS: &[&str] = &[
    "--json",
    "--worktree",
    "--timeout",
    "--no-color",
    "--quiet",
    "--idempotency-key",
    "--detach",
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
        flags: DISPLAY_FLAGS,
        values: &[],
        help_tokens: &[],
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
        flags: &["--json", "--no-color", "--quiet", "--daemon-path"],
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
        help_tokens: &["--preset", "--procedure", "--task", "--dry-run"],
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
        help_tokens: &["--replace", "--yes", "--dry-run"],
        dynamic: None,
    },
    RouteSurface {
        route: "session.status",
        parser: &["status", "--verbose"],
        flags: STATUS_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--verbose", "--wait-for-idle", "--after-job"],
        dynamic: None,
    },
    RouteSurface {
        route: "session.next",
        parser: &["next", "--wait-for-idle"],
        flags: NEXT_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--wait-for-idle", "--after-job"],
        dynamic: None,
    },
    RouteSurface {
        route: "session.complete",
        parser: &["complete"],
        flags: SESSION_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--if-session-revision", "--if-attempt"],
        dynamic: None,
    },
    RouteSurface {
        route: "session.skip",
        parser: &["skip", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--reason"],
        dynamic: None,
    },
    RouteSurface {
        route: "session.retry",
        parser: &["retry", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--reason"],
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
        help_tokens: &["--to", "--reason", "--dry-run"],
        dynamic: Some("returns"),
    },
    RouteSurface {
        route: "session.block",
        parser: &["block", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--reason"],
        dynamic: None,
    },
    RouteSurface {
        route: "session.unblock",
        parser: &["unblock", "--all"],
        flags: UNBLOCK_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--all"],
        dynamic: Some("blockers"),
    },
    RouteSurface {
        route: "session.cancel",
        parser: &["cancel", "--reason", "reason"],
        flags: SKIP_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--reason"],
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
        help_tokens: &["--to", "--reason", "--dry-run"],
        dynamic: Some("returns"),
    },
    RouteSurface {
        route: "session.reset",
        parser: &["reset", "--yes"],
        flags: RESET_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--dry-run", "--yes"],
        dynamic: None,
    },
    RouteSurface {
        route: "workspace.reset_all",
        parser: &["reset", "--all", "--force", "--yes"],
        flags: RESET_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--all", "--force", "--yes"],
        dynamic: None,
    },
    RouteSurface {
        route: "item.check",
        parser: &["check", "item"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.uncheck",
        parser: &["uncheck", "item"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.set",
        parser: &["set", "item", "value"],
        flags: SET_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--stdin"],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.add",
        parser: &["add", "item", "value"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.remove",
        parser: &["remove", "item", "value", "--ignore-missing"],
        flags: REMOVE_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--ignore-missing"],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.attach",
        parser: &["attach", "item", "path.txt", "--media-type", "text/plain"],
        flags: ATTACH_SURFACE_FLAGS,
        values: &[],
        help_tokens: &["--reference", "--digest", "--size", "[--media-type <type>]"],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "item.clear",
        parser: &["clear", "item"],
        flags: ITEM_MUTATION_SURFACE_FLAGS,
        values: &[],
        help_tokens: &[],
        dynamic: Some("items"),
    },
    RouteSurface {
        route: "job.list",
        parser: &["job", "list", "--state", "queued"],
        flags: &[
            "--json",
            "--worktree",
            "--timeout",
            "--no-color",
            "--quiet",
            "--state",
        ],
        values: &["queued", "running", "succeeded", "failed", "cancelled"],
        help_tokens: &["--state"],
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
            "session_revision": 1,
            "attempt_id": RECORDING_ATTEMPT_ID,
        }),
        PreconditionExpectation::SessionIdentity => serde_json::json!({
            "session_id": RECORDING_SESSION_ID,
            "session_revision": 1,
        }),
        PreconditionExpectation::SessionRevision => serde_json::json!({
            "session_revision": 1,
        }),
        PreconditionExpectation::Item => serde_json::json!({
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
fn recording_daemon_contract_table_covers_every_phase5_daemon_route() {
    assert_eq!(DAEMON_CONTRACTS.len(), 29);
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
        let json_response: ResponseEnvelopeV1 =
            serde_json::from_slice(&json_success.stdout).expect("JSON success must be typed");
        assert!(matches!(json_response, ResponseEnvelopeV1::Output(_)));
        let json_projection =
            assert_recorded_contract(&json_requests, *contract, &fixture.root, false);

        let (text_success, text_requests) =
            execute_recorded_contract(&fixture, *contract, false, false, false);
        assert!(
            text_success.status.success(),
            "{} text success failed: {text_success:?}",
            contract.route
        );
        assert!(
            !text_success.stdout.is_empty(),
            "{} text success must render a human result",
            contract.route
        );
        assert!(
            text_success.stderr.is_empty(),
            "{} text success must not emit diagnostics",
            contract.route
        );
        let text_projection =
            assert_recorded_contract(&text_requests, *contract, &fixture.root, false);
        assert_eq!(
            json_projection, text_projection,
            "{} text and JSON modes must send the identical daemon contract",
            contract.route
        );

        let (json_error, json_error_requests) =
            execute_recorded_contract(&fixture, *contract, true, false, true);
        assert_eq!(json_error.status.code(), Some(1));
        assert!(json_error.stderr.is_empty());
        let json_error_response: ResponseEnvelopeV1 =
            serde_json::from_slice(&json_error.stdout).expect("JSON error must be typed");
        let ResponseEnvelopeV1::Error(json_error_response) = json_error_response else {
            panic!("recording daemon error must remain an error envelope");
        };
        assert_eq!(json_error_response.code().as_str(), "TEST_ERROR");
        assert_recorded_contract(&json_error_requests, *contract, &fixture.root, false);

        let (text_error, text_error_requests) =
            execute_recorded_contract(&fixture, *contract, false, false, true);
        assert_eq!(text_error.status.code(), Some(1));
        assert!(
            text_error.stdout.is_empty(),
            "{} text errors must reserve stdout for successful results",
            contract.route
        );
        assert!(
            String::from_utf8_lossy(&text_error.stderr).contains("TEST_ERROR"),
            "{} text error must render the typed daemon error on stderr",
            contract.route
        );
        assert_recorded_contract(&text_error_requests, *contract, &fixture.root, false);

        if contract.detachable {
            let (detached_success, detached_requests) =
                execute_recorded_contract(&fixture, *contract, true, true, false);
            assert!(
                detached_success.status.success(),
                "{} detached request failed: {detached_success:?}",
                contract.route
            );
            assert_recorded_contract(&detached_requests, *contract, &fixture.root, true);
        }
    }
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
        ("preset.list", &["preset", "list"]),
        ("preset.show", &["preset", "show", "sw-dev"]),
        ("preset.explain", &["preset", "explain", "sw-dev"]),
        ("daemon.install", &["daemon", "install"]),
        ("daemon.uninstall", &["daemon", "uninstall", "--yes"]),
        ("daemon.start", &["daemon", "start"]),
        ("daemon.stop", &["daemon", "stop"]),
        ("daemon.restart", &["daemon", "restart"]),
        ("daemon.status", &["daemon", "status"]),
        ("daemon.logs", &["daemon", "logs", "--lines", "10"]),
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
    assert_eq!(routes.len(), 44);

    for (route, arguments) in routes {
        let mut argv = vec!["--json"];
        argv.extend_from_slice(arguments);
        let output = run(&argv);
        let response = one_json(&output);
        assert_eq!(
            response["command"], *route,
            "{route} must retain its canonical command in every public envelope"
        );
        match *route {
            "help" | "version" | "completions" | "preset.list" | "preset.show"
            | "preset.explain" => {
                assert!(
                    output.status.success(),
                    "{route} must work offline: {output:?}"
                );
                assert_eq!(response["schema"], "podway.output/v1");
            }
            "procedure.validate" | "procedure.show" => {
                assert_eq!(output.status.code(), Some(1));
                assert_eq!(response["schema"], "podway.error/v1");
                assert_eq!(response["code"], "PROCEDURE_NOT_FOUND");
                assert_eq!(response["retryable"], false);
                assert_eq!(response["exit_code"], 1);
            }
            "daemon.install" => {
                assert_eq!(output.status.code(), Some(3), "{route}: {output:?}");
                assert_eq!(response["schema"], "podway.error/v1");
                assert_eq!(response["code"], "DAEMON_UNAVAILABLE");
                assert_eq!(response["retryable"], true);
                assert_eq!(response["exit_code"], 3);
            }
            "daemon.logs" => {
                assert_eq!(output.status.code(), Some(3), "{route}: {output:?}");
                assert_eq!(response["schema"], "podway.error/v1");
                assert_eq!(response["code"], "DAEMON_NOT_INSTALLED");
                assert_eq!(response["retryable"], false);
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
    assert_eq!(ROUTE_SURFACES.len(), 44);
    let bash = run(&["completions", "bash"]);
    let zsh = run(&["completions", "zsh"]);
    let fish = run(&["completions", "fish"]);
    assert!(bash.status.success() && zsh.status.success() && fish.status.success());
    let bash = String::from_utf8(bash.stdout).expect("bash completion must be UTF-8");
    let zsh = String::from_utf8(zsh.stdout).expect("zsh completion must be UTF-8");
    let fish = String::from_utf8(fish.stdout).expect("fish completion must be UTF-8");

    for surface in ROUTE_SURFACES {
        let mut parser_arguments = vec!["--json"];
        parser_arguments.extend_from_slice(surface.parser);
        let parser_output = run(&parser_arguments);
        assert_ne!(
            parser_output.status.code(),
            Some(2),
            "{} parser form must remain accepted: {parser_output:?}",
            surface.route
        );
        assert_eq!(
            one_json(&parser_output)["command"],
            surface.route,
            "{} parser form must retain its canonical route",
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
        "preset.list",
        "preset.show",
        "preset.explain",
        "daemon.install",
        "daemon.uninstall",
        "daemon.start",
        "daemon.stop",
        "daemon.restart",
        "daemon.status",
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
        "job.status",
        "job.wait",
        "job.cancel",
    ];
    assert_eq!(routes.len(), 44);

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
fn invalid_applicability_and_confirmation_are_usage_json_errors() {
    for arguments in [
        &["--json", "version", "--worktree", "."][..],
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
    for arguments in [
        &["--json", "version"][..],
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
fn generated_shell_grammars_execute_with_nested_route_context_when_available() {
    if shell_available("bash") {
        let script = CompletionScript::generated("bash");
        let procedure = bash_candidates(&script, &["podway", "procedure", ""]);
        assert_eq!(procedure, ["validate".to_owned(), "show".to_owned()]);
        let install = bash_candidates(&script, &["podway", "daemon", "install", "--"]);
        assert!(install.contains(&"--daemon-path".to_owned()));
        assert!(!install.contains(&"--preset".to_owned()));
        assert!(!install.contains(&"--follow".to_owned()));
    }

    if shell_available("zsh") {
        let script = CompletionScript::generated("zsh");
        let install = zsh_candidates(&script, &["podway", "daemon", "install", ""]);
        assert!(install.contains(&"--daemon-path".to_owned()));
        assert!(!install.contains(&"--preset".to_owned()));
        assert!(!install.contains(&"--follow".to_owned()));
    }

    if shell_available("fish") {
        let script = CompletionScript::generated("fish");
        let procedure = fish_candidates(&script, "podway procedure ");
        assert!(procedure.contains(&"validate".to_owned()));
        assert!(procedure.contains(&"show".to_owned()));
        assert!(!procedure.contains(&"--canonical".to_owned()));
        let install = fish_candidates(&script, "podway daemon install --");
        assert!(install.contains(&"--daemon-path".to_owned()));
        assert!(!install.contains(&"--preset".to_owned()));
        assert!(!install.contains(&"--follow".to_owned()));
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
        if !shell_available(shell) {
            continue;
        }
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
                    "{shell} {kind} completion must not write diagnostics"
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
fn daemon_status_routes_locally_without_daemon_ipc() {
    let root = std::env::temp_dir().join(format!(
        "pws-{}-{}",
        std::process::id(),
        FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let home = root.join("home");
    let temporary = root.join("temporary");
    fs::create_dir_all(&home).expect("service-route home must be created");
    fs::create_dir_all(&temporary).expect("service-route temporary directory must be created");
    let paths = ServiceRuntimePathsV1::for_user(&home, &temporary, geteuid().as_raw())
        .expect("service-route paths must be valid");
    fs::create_dir_all(paths.runtime_directory().as_path())
        .expect("service-route runtime directory must be created");
    let _listener = UnixListener::bind(paths.socket_path().as_path())
        .expect("service-route daemon socket must be bindable");

    let output = Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(["--json", "daemon", "status"])
        .env("HOME", &home)
        .env("TMPDIR", &temporary)
        .output()
        .expect("podway binary must run");
    let response = one_json(&output);

    assert!(output.status.success(), "daemon status failed: {output:?}");
    assert_eq!(response["schema"], "podway.output/v1");
    assert_eq!(response["command"], "daemon.status");
    assert_eq!(response["result"]["status"], "not_installed");

    fs::remove_dir_all(root).expect("service-route fixture must be removed");
}

#[test]
fn hidden_dynamic_completion_silently_degrades_without_a_daemon() {
    let unique = format!("/tmp/podway-completion-missing-{}", std::process::id());
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
fn daemon_install_rejects_an_incompatible_binary_before_launchctl() {
    let root = unique_fixture_path("podway-incompatible-daemon");
    fs::create_dir_all(&root).expect("incompatible daemon fixture directory");
    let binary = root.join("podwayd");
    fs::write(&binary, b"#!/bin/sh\nprintf 'podwayd 9.0.0\\n'\n")
        .expect("incompatible daemon fixture");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
        .expect("incompatible daemon fixture mode");

    let binary_argument = binary.to_string_lossy().into_owned();
    let output = run(&[
        "--json",
        "daemon",
        "install",
        "--daemon-path",
        &binary_argument,
    ]);
    let response = one_json(&output);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(response["code"], "DAEMON_VERSION_INCOMPATIBLE");
    assert_eq!(response["retryable"], false);
    assert!(!root.join("Library/LaunchAgents").exists());

    fs::remove_dir_all(root).expect("remove incompatible daemon fixture");
}
