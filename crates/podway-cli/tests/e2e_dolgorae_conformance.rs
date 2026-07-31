//! Dolgorae consumer conformance through the real Podway binaries.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use nix::unistd::geteuid;
use podway_cli::client::DaemonClientV1;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV1,
    decode_request_payload_v1, decode_response_payload_v1, decode_single_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::Value;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ControlledPathFixtureV1 {
    root: PathBuf,
    home: PathBuf,
    arbitrary: PathBuf,
    launchctl: PathBuf,
    launchctl_state: PathBuf,
}

impl ControlledPathFixtureV1 {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(format!("/tmp/pwdg1-{}-{sequence}", std::process::id()));
        let home = root.join("home");
        let arbitrary = root.join("outside-worktree");
        let launchctl_state = root.join("launchctl-state");
        for directory in [&home, &arbitrary, &launchctl_state] {
            fs::create_dir_all(directory).expect("Dolgorae fixture directory must be created");
        }
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
            .expect("fixture account home must be private");
        let launchctl = root.join("fake-launchctl");
        fs::write(&launchctl, fake_launchctl_script())
            .expect("fake launchctl executable must be written");
        fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o700))
            .expect("fake launchctl executable must be executable");
        Self {
            root,
            home,
            arbitrary,
            launchctl,
            launchctl_state,
        }
    }

    fn run(&self, path: &str, arguments: &[&str]) -> Output {
        Command::new("podway")
            .args(arguments)
            .current_dir(&self.arbitrary)
            .env_clear()
            .env("PATH", path)
            .env("PODWAY_TEST_ACCOUNT_ROOT", &self.home)
            .env("PODWAY_TEST_LAUNCHCTL", &self.launchctl)
            .env("PODWAY_TEST_LAUNCHCTL_STATE", &self.launchctl_state)
            .output()
            .expect("controlled PATH must invoke podway")
    }

    fn run_owned(&self, path: &str, arguments: &[String]) -> Output {
        Command::new("podway")
            .args(arguments.iter().map(String::as_str))
            .current_dir(&self.arbitrary)
            .env_clear()
            .env("PATH", path)
            .env("PODWAY_TEST_ACCOUNT_ROOT", &self.home)
            .env("PODWAY_TEST_LAUNCHCTL", &self.launchctl)
            .env("PODWAY_TEST_LAUNCHCTL_STATE", &self.launchctl_state)
            .output()
            .expect("controlled PATH must invoke podway")
    }

    fn assert_install(&self, path: &str, arguments: &[&str], expected_daemon: &Path) {
        let output = self.run(path, arguments);
        let daemon_log = fs::read_to_string(self.launchctl_state.join("daemon.log"))
            .unwrap_or_else(|_| "<no daemon log>".to_owned());
        let status_diagnostic = if output.status.success() {
            "<not needed>".to_owned()
        } else {
            self.live_daemon_diagnostic()
        };
        assert!(
            output.status.success(),
            "daemon install failed: stdout={} stderr={} daemon_log={daemon_log} status={status_diagnostic}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let envelope: Value =
            serde_json::from_slice(&output.stdout).expect("install output must be JSON");
        assert_eq!(envelope["command"], "daemon.install");

        let expected_daemon =
            fs::canonicalize(expected_daemon).expect("selected daemon path must canonicalize");
        let metadata_path = self.home.join(".podway/state/service.json");
        let metadata: Value = serde_json::from_slice(
            &fs::read(&metadata_path).expect("service metadata must be published"),
        )
        .expect("service metadata must be JSON");
        assert_eq!(
            metadata["daemon_binary"],
            expected_daemon.display().to_string()
        );

        let plist_path = self
            .home
            .join("Library/LaunchAgents/dev.podway.podwayd.plist");
        assert!(plist_path.is_absolute());
        let plist = fs::read_to_string(&plist_path).expect("LaunchAgent plist must be published");
        assert!(
            plist.contains(&format!("<string>{}</string>", expected_daemon.display())),
            "plist must execute the canonical selected daemon"
        );
        assert!(!plist.contains("<string>podwayd</string>"));
    }

    fn live_daemon_diagnostic(&self) -> String {
        let paths = ServiceRuntimePathsV1::for_account_home(&self.home, geteuid().as_raw())
            .expect("diagnostic service paths must resolve");
        let request = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
            request_id: RequestIdV1::new("123e4567-e89b-42d3-a456-426614174000")
                .expect("diagnostic request ID"),
            client: ClientInfoV1::new(
                "podway-dolgi-test",
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
            )
            .expect("diagnostic client"),
            command: CommandNameV1::new("daemon.status").expect("diagnostic command"),
            operation: OperationV1::Control,
            workspace: None,
            idempotency_key: None,
            preconditions: PreconditionsV1::default(),
            options: RequestOptionsV1::new(false, 0).expect("diagnostic options"),
            payload: serde_json::Map::new(),
        })
        .expect("diagnostic request");
        format!("{:?}", DaemonClientV1::new(paths).daemon_status(&request))
    }

    fn uninstall(&self, path: &str) {
        let output = self.run(path, &["--json", "--yes", "daemon", "uninstall"]);
        assert!(
            output.status.success(),
            "daemon uninstall failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_json_success(&self, path: &str, arguments: &[&str]) -> Value {
        let output = self.run(path, arguments);
        assert!(
            output.status.success(),
            "Podway command failed: args={arguments:?} stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).expect("successful Podway output must be JSON")
    }

    fn run_json_success_owned(&self, path: &str, arguments: &[String]) -> Value {
        let output = self.run_owned(path, arguments);
        assert_json_success(output, arguments.iter().map(String::as_str))
    }
}

impl Drop for ControlledPathFixtureV1 {
    fn drop(&mut self) {
        if let Ok(pid) = fs::read_to_string(self.launchctl_state.join("pid")) {
            let _ = Command::new("/bin/kill").arg(pid.trim()).status();
        }
        if std::env::var_os("PODWAY_KEEP_DOLGI_FIXTURE").is_some() {
            eprintln!("preserving Dolgorae fixture at {}", self.root.display());
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fake_launchctl_script() -> &'static [u8] {
    br##"#!/bin/sh
set -eu
state=${PODWAY_TEST_LAUNCHCTL_STATE:?}
pid_file="$state/pid"
target="gui/$(/usr/bin/id -u)/dev.podway.podwayd"
case "${1:-}" in
  print)
    if [ -f "$pid_file" ] && /bin/kill -0 "$(/bin/cat "$pid_file")" 2>/dev/null; then
      echo "$target = {"
      echo "    pid = $(/bin/cat "$pid_file")"
      echo "}"
      exit 0
    fi
    echo "Bad request." >&2
    echo "Could not find service \"dev.podway.podwayd\" in domain for user gui: $(/usr/bin/id -u)" >&2
    exit 113
    ;;
  bootstrap)
    plist=${3:?}
    daemon=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' "$plist")
    socket=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:3' "$plist")
    daemon_log="$state/daemon.log"
    /usr/bin/python3 - "$daemon" "$socket" "$pid_file" "$daemon_log" <<'PY'
import subprocess
import sys

daemon, socket, pid_file, daemon_log = sys.argv[1:]
with open(daemon_log, "ab", buffering=0) as log:
    child = subprocess.Popen(
        [daemon, "--service", "--socket", socket],
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        close_fds=True,
    )
with open(pid_file, "w", encoding="ascii") as destination:
    destination.write(str(child.pid))
PY
    /bin/sleep 0.1
    if ! /bin/kill -0 "$(/bin/cat "$pid_file")" 2>/dev/null; then
      /bin/cat "$daemon_log" >&2
      exit 1
    fi
    exit 0
    ;;
  bootout)
    if [ -f "$pid_file" ]; then
      pid=$(/bin/cat "$pid_file")
      /bin/kill -TERM "$pid" 2>/dev/null || true
      i=0
      while /bin/kill -0 "$pid" 2>/dev/null && [ "$i" -lt 200 ]; do
        /bin/sleep 0.01
        i=$((i + 1))
      done
      /bin/rm -f "$pid_file"
    fi
    exit 0
    ;;
esac
exit 64
"##
}

fn copy_executable(source: &Path, destination: &Path) {
    fs::create_dir_all(
        destination
            .parent()
            .expect("copied executable must have a parent"),
    )
    .expect("executable parent must be created");
    fs::copy(source, destination).expect("product executable must be copied");
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))
        .expect("copied product executable must be executable");
}

fn assert_json_success<'a>(output: Output, arguments: impl IntoIterator<Item = &'a str>) -> Value {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    assert!(
        output.status.success(),
        "Podway command failed: args={arguments:?} stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("successful Podway output must be JSON")
}

fn assert_json_error(output: Output, code: &str, exit_code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "unexpected failure: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let error: Value =
        serde_json::from_slice(&output.stdout).expect("failed Podway output must be JSON");
    assert_eq!(error["schema"], "podway.error/v1");
    assert_eq!(error["code"], code);
    assert_eq!(error["exit_code"], exit_code);
    error
}

fn install_sibling_release(fixture: &ControlledPathFixtureV1, label: &str) -> (String, PathBuf) {
    let release_bin = fixture.root.join(label).join("release/bin");
    let release_cli = release_bin.join("podway");
    let release_daemon = release_bin.join("podwayd");
    let controlled_bin = fixture.root.join(label).join("controlled-bin");
    copy_executable(&cli_binary(), &release_cli);
    copy_executable(&daemon_binary(), &release_daemon);
    fs::create_dir_all(&controlled_bin).expect("controlled bin must be created");
    symlink(&release_cli, controlled_bin.join("podway"))
        .expect("controlled CLI symlink must be created");
    let controlled_path = format!("{}:/usr/bin:/bin", controlled_bin.display());
    fixture.assert_install(
        &controlled_path,
        &["--json", "daemon", "install"],
        &release_daemon,
    );
    (controlled_path, release_daemon)
}

fn create_non_bare_worktree(path: &Path) {
    run_git(
        Command::new("/usr/bin/git")
            .arg("init")
            .arg("--quiet")
            .arg(path),
        "initialize the Dolgorae worktree",
    );
    run_git(
        Command::new("/usr/bin/git").arg("-C").arg(path).args([
            "config",
            "user.email",
            "dolgorae@example.invalid",
        ]),
        "configure the Dolgorae fixture email",
    );
    run_git(
        Command::new("/usr/bin/git").arg("-C").arg(path).args([
            "config",
            "user.name",
            "Dolgorae Conformance",
        ]),
        "configure the Dolgorae fixture author",
    );
    run_git(
        Command::new("/usr/bin/git").arg("-C").arg(path).args([
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "initial",
        ]),
        "create the Dolgorae fixture commit",
    );
}

fn run_git(command: &mut Command, action: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("Git must be available to {action}: {error}"));
    assert!(
        output.status.success(),
        "Git must {action}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn daemon_binary() -> PathBuf {
    std::env::var_os("PODWAYD_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd"))
}

fn cli_binary() -> PathBuf {
    std::env::var_os("PODWAY_TEST_CLI_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_podway")))
}

const DOLGI_PROCEDURE: &str = r#"schema: podway.procedure/v1
id: dolgorae-conformance
version: "1"
name: Dolgorae Conformance
description: Exercise fenced consumer mutations against an immutable snapshot.
stages:
  - id: execute
    title: Execute the fenced lifecycle
    instructions:
      - Populate every supported item shape before completing.
    items:
      - id: confirmed
        type: confirm
        prompt: Confirm the lifecycle.
        required: true
      - id: note
        type: text
        prompt: Record the lifecycle note.
        required: true
        min_length: 1
      - id: risk
        type: choice
        prompt: Select the risk.
        required: true
        choices: [low, high]
      - id: count
        type: integer
        prompt: Record the count.
        required: true
        minimum: 1
        maximum: 10
      - id: checks
        type: list
        prompt: List completed checks.
        required: true
        min_items: 1
        max_items: 10
      - id: evidence
        type: artifact
        prompt: Attach lifecycle evidence.
        required: true
rework:
  allow_return_to: any_previous
"#;

#[derive(Clone, Debug)]
struct FencedStatusV1 {
    raw: Value,
    workspace_id: String,
    session_id: String,
    session_revision: String,
    attempt_id: Option<String>,
}

impl FencedStatusV1 {
    fn from_compact(raw: Value) -> Self {
        Self {
            workspace_id: required_text(&raw["workspace"]["uuid"], "workspace UUID"),
            session_id: required_text(&raw["result"]["session"]["id"], "session ID"),
            session_revision: required_u64(
                &raw["result"]["session"]["revision"],
                "session revision",
            )
            .to_string(),
            attempt_id: raw["result"]["current"]["attempt_id"]
                .as_str()
                .map(str::to_owned),
            raw,
        }
    }

    fn item_revision(&self, item_id: &str) -> String {
        let item = self.raw["result"]["items"]
            .as_array()
            .expect("compact items must be an array")
            .iter()
            .find(|item| item["id"] == item_id)
            .unwrap_or_else(|| panic!("compact status must contain item {item_id}"));
        required_u64(&item["revision"], "item revision").to_string()
    }
}

fn required_text(value: &Value, label: &str) -> String {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{label} must be non-empty text"))
        .to_owned()
}

fn required_u64(value: &Value, label: &str) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("{label} must be an unsigned integer"))
}

fn compact_status(
    fixture: &ControlledPathFixtureV1,
    path: &str,
    socket: &str,
    worktree: &str,
    guard: Option<(&str, &str)>,
) -> FencedStatusV1 {
    let mut arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        socket.to_owned(),
        "--worktree".to_owned(),
        worktree.to_owned(),
        "--timeout".to_owned(),
        "25s".to_owned(),
    ];
    if let Some((workspace_id, session_id)) = guard {
        arguments.extend([
            "--if-workspace-uuid".to_owned(),
            workspace_id.to_owned(),
            "--if-session-id".to_owned(),
            session_id.to_owned(),
        ]);
    }
    arguments.extend([
        "status".to_owned(),
        "--wait-for-idle".to_owned(),
        "--compact".to_owned(),
    ]);
    FencedStatusV1::from_compact(fixture.run_json_success_owned(path, &arguments))
}

fn fenced_item_mutation(
    fixture: &ControlledPathFixtureV1,
    path: &str,
    socket: &str,
    worktree: &str,
    status: &FencedStatusV1,
    mutation: (&str, &str, &[&str]),
) -> Value {
    let (item_id, idempotency_key, command) = mutation;
    let attempt_id = status
        .attempt_id
        .as_deref()
        .expect("running status must contain an attempt");
    let mut arguments = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        socket.to_owned(),
        "--worktree".to_owned(),
        worktree.to_owned(),
        "--timeout".to_owned(),
        "25s".to_owned(),
        "--idempotency-key".to_owned(),
        idempotency_key.to_owned(),
        "--if-workspace-uuid".to_owned(),
        status.workspace_id.clone(),
        "--if-session-id".to_owned(),
        status.session_id.clone(),
        "--if-attempt".to_owned(),
        attempt_id.to_owned(),
        "--if-item-revision".to_owned(),
        status.item_revision(item_id),
    ];
    arguments.extend(command.iter().map(|argument| (*argument).to_owned()));
    fixture.run_json_success_owned(path, &arguments)
}

fn terminal_lookup(
    fixture: &ControlledPathFixtureV1,
    path: &str,
    socket: &str,
    worktree: &str,
    idempotency_key: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let lookup = fixture.run_json_success(
            path,
            &[
                "--json",
                "--socket",
                socket,
                "--worktree",
                worktree,
                "--timeout",
                "25s",
                "job",
                "lookup",
                "--idempotency-key",
                idempotency_key,
            ],
        );
        if lookup["result"]["found"] == true
            && matches!(
                lookup["result"]["job"]["state"].as_str(),
                Some("succeeded" | "failed" | "cancelled")
            )
        {
            return lookup;
        }
        assert!(
            Instant::now() < deadline,
            "job lookup did not become terminal"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn aut_t_path_installs_explicit_sibling_and_path_daemons_from_a_sanitized_directory() {
    let fixture = ControlledPathFixtureV1::new();
    let source_cli = cli_binary();
    let source_daemon = daemon_binary();

    let explicit_cli = fixture.root.join("explicit/cli/podway-real");
    let explicit_daemon = fixture.root.join("explicit/daemon/podwayd-real");
    let explicit_bin = fixture.root.join("explicit/bin");
    copy_executable(&source_cli, &explicit_cli);
    copy_executable(&source_daemon, &explicit_daemon);
    fs::create_dir_all(&explicit_bin).expect("explicit controlled bin must be created");
    symlink(&explicit_cli, explicit_bin.join("podway"))
        .expect("controlled PATH CLI symlink must be created");
    let explicit_path = format!("{}:/usr/bin:/bin", explicit_bin.display());
    fixture.assert_install(
        &explicit_path,
        &[
            "--json",
            "daemon",
            "install",
            "--daemon-path",
            explicit_daemon
                .to_str()
                .expect("fixture daemon path must be UTF-8"),
        ],
        &explicit_daemon,
    );
    fixture.uninstall(&explicit_path);

    let sibling_directory = fixture.root.join("sibling/release/bin");
    let sibling_cli = sibling_directory.join("podway");
    let sibling_daemon = sibling_directory.join("podwayd");
    let sibling_bin = fixture.root.join("sibling/path");
    copy_executable(&source_cli, &sibling_cli);
    copy_executable(&source_daemon, &sibling_daemon);
    fs::create_dir_all(&sibling_bin).expect("sibling controlled bin must be created");
    symlink(&sibling_cli, sibling_bin.join("podway")).expect("sibling CLI symlink must be created");
    let sibling_path = format!("{}:/usr/bin:/bin", sibling_bin.display());
    fixture.assert_install(
        &sibling_path,
        &["--json", "daemon", "install"],
        &sibling_daemon,
    );
    fixture.uninstall(&sibling_path);

    let path_cli = fixture.root.join("path/cli-only/podway");
    let path_daemon = fixture.root.join("path/daemon-bin/podwayd");
    let path_bin = fixture.root.join("path/controlled-bin");
    copy_executable(&source_cli, &path_cli);
    copy_executable(&source_daemon, &path_daemon);
    fs::create_dir_all(&path_bin).expect("PATH controlled bin must be created");
    symlink(&path_cli, path_bin.join("podway")).expect("PATH CLI symlink must be created");
    let controlled_path = format!(
        "{}:{}:/usr/bin:/bin",
        path_bin.display(),
        path_daemon
            .parent()
            .expect("PATH daemon must have a parent")
            .display()
    );
    fixture.assert_install(
        &controlled_path,
        &["--json", "daemon", "install"],
        &path_daemon,
    );
    fixture.uninstall(&controlled_path);
}

#[test]
fn aut_t_obs_installed_service_returns_compact_quiescent_status_on_the_explicit_socket() {
    let fixture = ControlledPathFixtureV1::new();
    let release_bin = fixture.root.join("observation/release/bin");
    let release_cli = release_bin.join("podway");
    let release_daemon = release_bin.join("podwayd");
    let controlled_bin = fixture.root.join("observation/controlled-bin");
    copy_executable(&cli_binary(), &release_cli);
    copy_executable(&daemon_binary(), &release_daemon);
    fs::create_dir_all(&controlled_bin).expect("observation controlled bin must be created");
    symlink(&release_cli, controlled_bin.join("podway"))
        .expect("observation CLI symlink must be created");
    let controlled_path = format!("{}:/usr/bin:/bin", controlled_bin.display());
    fixture.assert_install(
        &controlled_path,
        &["--json", "daemon", "install"],
        &release_daemon,
    );

    let socket = fixture.home.join(".podway/run/podwayd.sock");
    let alternate_socket = fixture.home.join(".podway/run/alternate.sock");
    let duplicate = Command::new(&release_daemon)
        .args(["--service", "--socket"])
        .arg(&alternate_socket)
        .env_clear()
        .env("PODWAY_TEST_ACCOUNT_ROOT", &fixture.home)
        .output()
        .expect("duplicate daemon probe must execute");
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("cannot acquire daemon endpoint"),
        "duplicate daemon stderr={}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    assert!(!alternate_socket.exists());

    let worktree = fixture.root.join("observation/worktree");
    create_non_bare_worktree(&worktree);
    let socket_text = socket.to_str().expect("fixture socket path must be UTF-8");
    let worktree_text = worktree
        .to_str()
        .expect("fixture worktree path must be UTF-8");
    let init = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "init",
        ],
    );
    assert_eq!(init["command"], "workspace.init");
    assert_eq!(init["result"]["initialized"], true);

    let start = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "--idempotency-key",
            "11111111-1111-4111-8111-111111111111",
            "start",
            "--preset",
            "analysis",
            "--task",
            "Observe compact idle status",
        ],
    );
    assert_eq!(start["command"], "session.start");

    let compact = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "status",
            "--wait-for-idle",
            "--compact",
        ],
    );
    assert_eq!(compact["command"], "session.status");
    let result = compact["result"]
        .as_object()
        .expect("compact status result must be an object");
    assert_eq!(
        result.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "blockers",
            "current",
            "items",
            "procedure",
            "queue",
            "schema",
            "session",
        ])
    );
    assert_eq!(result["schema"], "podway.compact-status-result/v1");
    assert_eq!(result["queue"]["pending_mutations"], false);
    assert_eq!(result["queue"]["queued_count"], 0);
    assert!(result["queue"]["running_job_id"].is_null());
    assert_eq!(
        result["queue"]["latest_workspace_sequence"],
        compact["workspace"]["latest_workspace_sequence"]
    );
    assert!(
        result["queue"]["latest_workspace_sequence"]
            .as_u64()
            .is_some_and(|sequence| sequence >= 1)
    );
    let encoded = serde_json::to_vec(&compact).expect("compact status must serialize");
    assert!(encoded.len() < 262_144);
    let encoded_text = String::from_utf8(encoded).expect("compact status must be UTF-8");
    for forbidden in [
        "\"instructions\"",
        "\"prompt\"",
        "\"task\"",
        "\"title\"",
        "\"value\"",
    ] {
        assert!(!encoded_text.contains(forbidden));
    }

    fixture.uninstall(&controlled_path);
}

#[test]
fn aut_t_id_custom_procedure_survives_restart_and_completes_the_fenced_lifecycle() {
    let fixture = ControlledPathFixtureV1::new();
    let (controlled_path, _) = install_sibling_release(&fixture, "lifecycle");
    let socket = fixture.home.join(".podway/run/podwayd.sock");
    let worktree = fixture.root.join("lifecycle/worktree");
    create_non_bare_worktree(&worktree);
    let procedure_path = worktree.join("dolgorae-procedure.yaml");
    let artifact_path = worktree.join("evidence.txt");
    fs::write(&procedure_path, DOLGI_PROCEDURE).expect("custom Procedure must be written");
    fs::write(&artifact_path, "Dolgorae lifecycle evidence\n")
        .expect("lifecycle evidence must be written");

    let socket_text = socket.to_str().expect("fixture socket path must be UTF-8");
    let worktree_text = worktree
        .to_str()
        .expect("fixture worktree path must be UTF-8");
    let procedure_text = procedure_path
        .to_str()
        .expect("fixture Procedure path must be UTF-8");
    let initialized = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "init",
        ],
    );
    let workspace_id = required_text(&initialized["workspace"]["uuid"], "workspace UUID");

    let validated = fixture.run_json_success(
        &controlled_path,
        &["--json", "procedure", "validate", procedure_text],
    );
    let procedure_digest = required_text(&validated["result"]["digest"], "Procedure digest");
    let started = fixture.run_json_success_owned(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "--idempotency-key",
            "33333333-0000-4000-8000-000000000001",
            "--if-workspace-uuid",
            &workspace_id,
            "start",
            "--procedure",
            "dolgorae-procedure.yaml",
            "--expect-procedure-digest",
            &procedure_digest,
            "--task",
            "Exercise the fenced Dolgorae lifecycle",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
    );
    assert_eq!(started["command"], "session.start");
    assert_eq!(started["result"]["procedure_digest"], procedure_digest);

    let initial = compact_status(&fixture, &controlled_path, socket_text, worktree_text, None);
    assert_eq!(initial.workspace_id, workspace_id);
    assert_eq!(
        initial.raw["result"]["procedure"]["digest"],
        procedure_digest
    );

    fs::remove_file(&procedure_path).expect("Procedure source must be removable after start");
    let restarted = fixture.run_json_success(&controlled_path, &["--json", "daemon", "restart"]);
    assert_eq!(restarted["command"], "daemon.restart");
    let recovered = compact_status(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        Some((&initial.workspace_id, &initial.session_id)),
    );
    assert_eq!(recovered.session_id, initial.session_id);
    assert_eq!(
        recovered.raw["result"]["procedure"]["digest"],
        procedure_digest
    );

    let mutations: [(&str, &str, &[&str]); 6] = [
        (
            "confirmed",
            "33333333-0000-4000-8000-000000000002",
            &["check", "confirmed"],
        ),
        (
            "note",
            "33333333-0000-4000-8000-000000000003",
            &["set", "note", "snapshot survived restart"],
        ),
        (
            "risk",
            "33333333-0000-4000-8000-000000000004",
            &["set", "risk", "low"],
        ),
        (
            "count",
            "33333333-0000-4000-8000-000000000005",
            &["set", "count", "3"],
        ),
        (
            "checks",
            "33333333-0000-4000-8000-000000000006",
            &["add", "checks", "identity fences"],
        ),
        (
            "evidence",
            "33333333-0000-4000-8000-000000000007",
            &[
                "attach",
                "evidence",
                "evidence.txt",
                "--media-type",
                "text/plain",
            ],
        ),
    ];
    for (item_id, idempotency_key, command) in mutations {
        let status = compact_status(
            &fixture,
            &controlled_path,
            socket_text,
            worktree_text,
            Some((&recovered.workspace_id, &recovered.session_id)),
        );
        let output = fenced_item_mutation(
            &fixture,
            &controlled_path,
            socket_text,
            worktree_text,
            &status,
            (item_id, idempotency_key, command),
        );
        assert!(output["job"]["finished_at"].is_string());
    }

    let ready = compact_status(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        Some((&recovered.workspace_id, &recovered.session_id)),
    );
    let completed = fixture.run_json_success_owned(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "--idempotency-key",
            "33333333-0000-4000-8000-000000000008",
            "--if-workspace-uuid",
            &ready.workspace_id,
            "--if-session-id",
            &ready.session_id,
            "--if-session-revision",
            &ready.session_revision,
            "--if-attempt",
            ready.attempt_id.as_deref().expect("ready attempt"),
            "complete",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
    );
    assert_eq!(completed["command"], "session.complete");

    let terminal = compact_status(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        Some((&ready.workspace_id, &ready.session_id)),
    );
    assert_eq!(terminal.raw["result"]["session"]["lifecycle"], "completed");
    assert!(terminal.raw["result"]["current"].is_null());
    assert_eq!(
        terminal.raw["result"]["procedure"]["digest"],
        procedure_digest
    );

    let reopened = fixture.run_json_success_owned(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "--idempotency-key",
            "33333333-0000-4000-8000-000000000009",
            "--if-workspace-uuid",
            &terminal.workspace_id,
            "--if-session-id",
            &terminal.session_id,
            "--if-session-revision",
            &terminal.session_revision,
            "reopen",
            "--to",
            "execute",
            "--reason",
            "verify the reopen transition",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
    );
    assert_eq!(reopened["command"], "session.reopen");
    let reopened_status = compact_status(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        Some((&terminal.workspace_id, &terminal.session_id)),
    );
    assert_eq!(
        reopened_status.raw["result"]["session"]["lifecycle"],
        "running"
    );
    assert!(reopened_status.attempt_id.is_some());

    fs::write(&procedure_path, DOLGI_PROCEDURE).expect("Procedure source must be restored");
    let replacement = fixture.run_json_success_owned(
        &controlled_path,
        &[
            "--json",
            "--yes",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "--idempotency-key",
            "33333333-0000-4000-8000-000000000010",
            "--if-workspace-uuid",
            &reopened_status.workspace_id,
            "--if-session-id",
            &reopened_status.session_id,
            "--if-session-revision",
            &reopened_status.session_revision,
            "start",
            "--replace",
            "--procedure",
            "dolgorae-procedure.yaml",
            "--expect-procedure-digest",
            &procedure_digest,
            "--task",
            "Replace the reopened Dolgorae session",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
    );
    assert_eq!(replacement["command"], "session.start_replace");
    assert_eq!(replacement["result"]["procedure_digest"], procedure_digest);
    let replacement_status =
        compact_status(&fixture, &controlled_path, socket_text, worktree_text, None);
    assert_ne!(replacement_status.session_id, reopened_status.session_id);
    let stale_session = assert_json_error(
        fixture.run_owned(
            &controlled_path,
            &[
                "--json",
                "--socket",
                socket_text,
                "--worktree",
                worktree_text,
                "--timeout",
                "25s",
                "--if-workspace-uuid",
                &replacement_status.workspace_id,
                "--if-session-id",
                &reopened_status.session_id,
                "status",
                "--wait-for-idle",
                "--compact",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        ),
        "SESSION_ID_MISMATCH",
        4,
    );
    assert_eq!(
        stale_session["details"]["expected_session_id"],
        reopened_status.session_id
    );
    assert_eq!(
        stale_session["details"]["actual_session_id"],
        replacement_status.session_id
    );
    assert_eq!(stale_session["details"]["admission"]["admitted"], false);

    let reset = fixture.run_json_success_owned(
        &controlled_path,
        &[
            "--json",
            "--yes",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "--idempotency-key",
            "33333333-0000-4000-8000-000000000011",
            "--if-workspace-uuid",
            &replacement_status.workspace_id,
            "--if-session-id",
            &replacement_status.session_id,
            "--if-session-revision",
            &replacement_status.session_revision,
            "reset",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
    );
    assert_eq!(reset["command"], "session.reset");
    assert_eq!(reset["result"]["changed"], true);
    assert_eq!(reset["result"]["revision_after"], 0);

    fixture.uninstall(&controlled_path);
}

#[test]
fn aut_t_id_and_recon_reject_conflicts_and_recover_an_admitted_timeout() {
    const TIMEOUT_KEY: &str = "44444444-0000-4000-8000-000000000001";
    const BAD_DIGEST: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    let fixture = ControlledPathFixtureV1::new();
    let (controlled_path, _) = install_sibling_release(&fixture, "conflicts");
    let socket = fixture.home.join(".podway/run/podwayd.sock");
    let worktree = fixture.root.join("conflicts/worktree");
    create_non_bare_worktree(&worktree);
    let procedure_path = worktree.join("dolgorae-procedure.yaml");
    fs::write(&procedure_path, DOLGI_PROCEDURE).expect("conflict Procedure must be written");
    let socket_text = socket.to_str().expect("fixture socket path must be UTF-8");
    let worktree_text = worktree
        .to_str()
        .expect("fixture worktree path must be UTF-8");
    let procedure_text = procedure_path
        .to_str()
        .expect("fixture Procedure path must be UTF-8");

    let paths = ServiceRuntimePathsV1::for_account_home(&fixture.home, geteuid().as_raw())
        .expect("mismatch service paths must resolve");
    let mismatch_request = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("44444444-0000-4000-8000-000000000097")
            .expect("mismatch request ID"),
        client: ClientInfoV1::new_with_contract_identity(
            "dolgorae-mismatch",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            "podway",
            BAD_DIGEST,
        )
        .expect("mismatch client identity"),
        command: CommandNameV1::new("daemon.status").expect("mismatch status command"),
        operation: OperationV1::Control,
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).expect("mismatch request options"),
        payload: serde_json::Map::new(),
    })
    .expect("mismatch request");
    let ResponseEnvelopeV1::Error(mismatch) = DaemonClientV1::new(paths)
        .daemon_status(&mismatch_request)
        .expect("mismatch daemon exchange must complete")
    else {
        panic!("mismatched client identity must be rejected");
    };
    assert_eq!(mismatch.code().as_str(), "DAEMON_CONTRACT_MISMATCH");
    assert_eq!(mismatch.exit_code().get(), 3);
    assert!(!mismatch.retryable());
    assert_eq!(
        mismatch.details()["actual"]["contract_manifest_digest"],
        BAD_DIGEST
    );
    assert_eq!(mismatch.details()["admission"]["admitted"], false);

    let initialized = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "init",
        ],
    );
    let workspace_id = required_text(&initialized["workspace"]["uuid"], "workspace UUID");
    let validated = fixture.run_json_success(
        &controlled_path,
        &["--json", "procedure", "validate", procedure_text],
    );
    let procedure_digest = required_text(&validated["result"]["digest"], "Procedure digest");

    let missing = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "job",
            "lookup",
            "--idempotency-key",
            "44444444-0000-4000-8000-000000000000",
        ],
    );
    assert_eq!(missing["result"]["found"], false);
    assert_eq!(missing["result"]["schema"], "podway.job-lookup-result/v1");

    let digest_mismatch = assert_json_error(
        fixture.run(
            &controlled_path,
            &[
                "--json",
                "--socket",
                socket_text,
                "--worktree",
                worktree_text,
                "--if-workspace-uuid",
                &workspace_id,
                "start",
                "--procedure",
                "dolgorae-procedure.yaml",
                "--expect-procedure-digest",
                BAD_DIGEST,
                "--task",
                "Reject a stale Procedure digest",
            ],
        ),
        "PROCEDURE_DIGEST_MISMATCH",
        4,
    );
    assert_eq!(digest_mismatch["retryable"], false);
    assert_eq!(
        digest_mismatch["details"]["actual_procedure_digest"],
        procedure_digest
    );
    assert_eq!(digest_mismatch["details"]["admission"]["admitted"], false);

    let timeout = assert_json_error(
        fixture.run(
            &controlled_path,
            &[
                "--json",
                "--socket",
                socket_text,
                "--worktree",
                worktree_text,
                "--timeout",
                "0ms",
                "--idempotency-key",
                TIMEOUT_KEY,
                "--if-workspace-uuid",
                &workspace_id,
                "start",
                "--procedure",
                "dolgorae-procedure.yaml",
                "--expect-procedure-digest",
                &procedure_digest,
                "--task",
                "Recover an admitted timeout",
            ],
        ),
        "JOB_WAIT_TIMEOUT",
        4,
    );
    assert_eq!(timeout["retryable"], true);
    assert_eq!(timeout["details"]["admission"]["admitted"], true);
    assert!(timeout["details"]["admission"]["job_id"].is_string());
    let timeout_lookup = terminal_lookup(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        TIMEOUT_KEY,
    );
    assert_eq!(timeout_lookup["result"]["job"]["state"], "succeeded");
    assert_eq!(
        timeout_lookup["result"]["job"]["id"],
        timeout["details"]["admission"]["job_id"]
    );
    assert_eq!(
        timeout_lookup["result"]["job"]["terminal_response"]["command"],
        "session.start"
    );

    let current = compact_status(&fixture, &controlled_path, socket_text, worktree_text, None);
    let wrong_workspace = "44444444-0000-4000-8000-000000000099";
    let workspace_conflict = assert_json_error(
        fixture.run(
            &controlled_path,
            &[
                "--json",
                "--socket",
                socket_text,
                "--worktree",
                worktree_text,
                "--if-workspace-uuid",
                wrong_workspace,
                "status",
                "--wait-for-idle",
                "--compact",
            ],
        ),
        "WORKSPACE_UUID_MISMATCH",
        4,
    );
    assert_eq!(
        workspace_conflict["details"]["actual_workspace_uuid"],
        current.workspace_id
    );
    assert_eq!(
        workspace_conflict["details"]["admission"]["admitted"],
        false
    );

    let wrong_session = "44444444-0000-4000-8000-000000000098";
    let session_conflict = assert_json_error(
        fixture.run(
            &controlled_path,
            &[
                "--json",
                "--socket",
                socket_text,
                "--worktree",
                worktree_text,
                "--if-workspace-uuid",
                &current.workspace_id,
                "--if-session-id",
                wrong_session,
                "status",
                "--wait-for-idle",
                "--compact",
            ],
        ),
        "SESSION_ID_MISMATCH",
        4,
    );
    assert_eq!(
        session_conflict["details"]["actual_session_id"],
        current.session_id
    );
    assert_eq!(session_conflict["details"]["admission"]["admitted"], false);

    let stale_revision = current.session_revision.clone();
    fenced_item_mutation(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &current,
        (
            "confirmed",
            "44444444-0000-4000-8000-000000000002",
            &["check", "confirmed"],
        ),
    );
    let advanced = compact_status(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        Some((&current.workspace_id, &current.session_id)),
    );
    assert_ne!(advanced.session_revision, stale_revision);
    let revision_conflict = assert_json_error(
        fixture.run_owned(
            &controlled_path,
            &[
                "--json",
                "--yes",
                "--socket",
                socket_text,
                "--worktree",
                worktree_text,
                "--timeout",
                "25s",
                "--idempotency-key",
                "44444444-0000-4000-8000-000000000003",
                "--if-workspace-uuid",
                &advanced.workspace_id,
                "--if-session-id",
                &advanced.session_id,
                "--if-session-revision",
                &stale_revision,
                "start",
                "--replace",
                "--procedure",
                "dolgorae-procedure.yaml",
                "--expect-procedure-digest",
                &procedure_digest,
                "--task",
                "Reject a stale replacement revision",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        ),
        "SESSION_REVISION_CONFLICT",
        4,
    );
    assert_eq!(revision_conflict["details"]["admission"]["admitted"], true);

    fixture.uninstall(&controlled_path);
}

#[test]
fn aut_t_recon_response_loss_is_reconciled_by_lookup_and_exact_replay() {
    const RESPONSE_LOSS_KEY: &str = "44444444-0000-4000-8000-000000000010";

    let fixture = ControlledPathFixtureV1::new();
    let (controlled_path, _) = install_sibling_release(&fixture, "response-loss");
    let socket = fixture.home.join(".podway/run/podwayd.sock");
    let worktree = fixture.root.join("response-loss/worktree");
    create_non_bare_worktree(&worktree);
    let socket_text = socket.to_str().expect("fixture socket path must be UTF-8");
    let worktree_text = worktree
        .to_str()
        .expect("fixture worktree path must be UTF-8");
    fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "init",
        ],
    );

    let proxy_socket = fixture.home.join(".podway/run/response-loss.sock");
    let listener = UnixListener::bind(&proxy_socket).expect("response-loss relay must bind");
    fs::set_permissions(&proxy_socket, fs::Permissions::from_mode(0o600))
        .expect("response-loss relay socket must be private");
    listener
        .set_nonblocking(true)
        .expect("response-loss relay must become nonblocking");
    let daemon_socket = socket.clone();
    let relay = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut downstream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "response-loss relay accept timed out",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        };
        downstream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut request_wire = Vec::new();
        downstream.read_to_end(&mut request_wire)?;
        let mut upstream = UnixStream::connect(daemon_socket)?;
        upstream.set_read_timeout(Some(Duration::from_secs(10)))?;
        upstream.set_write_timeout(Some(Duration::from_secs(10)))?;
        upstream.write_all(&request_wire)?;
        upstream.shutdown(Shutdown::Write)?;
        let mut response_wire = Vec::new();
        upstream.read_to_end(&mut response_wire)?;
        drop(downstream);
        Ok::<_, std::io::Error>((request_wire, response_wire))
    });

    let proxy_text = proxy_socket
        .to_str()
        .expect("fixture proxy socket path must be UTF-8");
    let lost = assert_json_error(
        fixture.run(
            &controlled_path,
            &[
                "--json",
                "--socket",
                proxy_text,
                "--worktree",
                worktree_text,
                "--timeout",
                "25s",
                "--idempotency-key",
                RESPONSE_LOSS_KEY,
                "start",
                "--preset",
                "analysis",
                "--task",
                "Recover the discarded daemon response",
            ],
        ),
        "MUTATION_OUTCOME_UNKNOWN",
        4,
    );
    assert_eq!(lost["retryable"], true);
    assert_eq!(lost["details"]["outcome"], "unknown");
    assert_eq!(lost["details"]["idempotency_key"], RESPONSE_LOSS_KEY);
    assert_eq!(lost["details"]["reconcile"]["command"], "job.lookup");

    let (request_wire, response_wire) = relay
        .join()
        .expect("response-loss relay must not panic")
        .expect("response-loss relay must finish both exchanges");
    let request_payload =
        decode_single_frame_v1(&request_wire).expect("relay request must contain one frame");
    let request = decode_request_payload_v1(request_payload).expect("relay request must decode");
    assert_eq!(request.operation(), OperationV1::Mutate);
    assert_eq!(request.command().as_str(), "session.start");
    assert_eq!(
        request
            .idempotency_key()
            .expect("relay mutation must have an idempotency key")
            .as_str(),
        RESPONSE_LOSS_KEY
    );
    assert_eq!(lost["request_id"], request.request_id().as_str());

    let response_payload =
        decode_single_frame_v1(&response_wire).expect("relay response must contain one frame");
    let ResponseEnvelopeV1::Output(discarded) =
        decode_response_payload_v1(response_payload).expect("relay response must decode")
    else {
        panic!("discarded response must be a successful mutation");
    };
    let discarded_json = serde_json::to_value(ResponseEnvelopeV1::Output(discarded.clone()))
        .expect("discarded response must serialize");
    let lookup = terminal_lookup(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        RESPONSE_LOSS_KEY,
    );
    assert_eq!(lookup["result"]["job"]["command"], "session.start");
    assert_eq!(lookup["result"]["job"]["state"], "succeeded");
    assert!(
        lookup["result"]["job"]["request_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    assert_eq!(lookup["result"]["job"]["terminal_response"], discarded_json);

    let replayed = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "--socket",
            socket_text,
            "--worktree",
            worktree_text,
            "--timeout",
            "25s",
            "--idempotency-key",
            RESPONSE_LOSS_KEY,
            "start",
            "--preset",
            "analysis",
            "--task",
            "Recover the discarded daemon response",
        ],
    );
    assert_eq!(replayed["job"], discarded_json["job"]);
    assert_eq!(replayed["session"], discarded_json["session"]);
    assert_eq!(replayed["result"], discarded_json["result"]);

    let reused = assert_json_error(
        fixture.run(
            &controlled_path,
            &[
                "--json",
                "--socket",
                socket_text,
                "--worktree",
                worktree_text,
                "--timeout",
                "25s",
                "--idempotency-key",
                RESPONSE_LOSS_KEY,
                "start",
                "--preset",
                "analysis",
                "--task",
                "A different canonical request",
            ],
        ),
        "IDEMPOTENCY_KEY_REUSED",
        2,
    );
    assert_eq!(reused["retryable"], false);

    fixture.uninstall(&controlled_path);
}

#[test]
fn aut_t_dist_extracted_native_archive_runs_the_complete_dolgorae_suite() {
    const CHILD_MARKER: &str = "PODWAY_DOLGI_DIST_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        return;
    }

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = PathBuf::from(format!("/tmp/pwdgd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let output_directory = root.join("dist");
    let package = Command::new("/usr/bin/python3")
        .arg(repository.join("tools/release_archive.py"))
        .args(["package", "--allow-dirty", "--podway"])
        .arg(cli_binary())
        .arg("--podwayd")
        .arg(daemon_binary())
        .arg("--output-dir")
        .arg(&output_directory)
        .current_dir(&repository)
        .output()
        .expect("native archive builder must execute");
    assert!(
        package.status.success(),
        "native archive build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&package.stdout),
        String::from_utf8_lossy(&package.stderr)
    );
    let receipt: Value =
        serde_json::from_slice(&package.stdout).expect("archive receipt must be JSON");
    assert_eq!(receipt["ok"], true);
    let archive = PathBuf::from(required_text(&receipt["archive"], "archive path"));
    let provenance: Value = serde_json::from_slice(
        &fs::read(PathBuf::from(required_text(
            &receipt["provenance"],
            "provenance path",
        )))
        .expect("archive provenance must be readable"),
    )
    .expect("archive provenance must be JSON");
    assert_eq!(provenance["target"], "aarch64-apple-darwin");
    assert_eq!(provenance["archive"]["sha256"], receipt["archive_sha256"]);

    let extraction = root.join("extracted");
    fs::create_dir_all(&extraction).expect("archive extraction directory must be created");
    let extracted = Command::new("/usr/bin/tar")
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(&extraction)
        .output()
        .expect("native archive must extract");
    assert!(
        extracted.status.success(),
        "native archive extraction failed: {}",
        String::from_utf8_lossy(&extracted.stderr)
    );
    let archive_root = extraction.join("podway-0.1.0-aarch64-apple-darwin");
    let packaged_cli = archive_root.join("bin/podway");
    let packaged_daemon = archive_root.join("bin/podwayd");
    for binary in [&packaged_cli, &packaged_daemon] {
        let metadata = fs::symlink_metadata(binary).expect("packaged binary must exist");
        assert!(metadata.file_type().is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_ne!(metadata.permissions().mode() & 0o111, 0);
    }

    let cli_identity = assert_json_success(
        Command::new(&packaged_cli)
            .args(["--json", "version"])
            .output()
            .expect("packaged CLI identity probe must execute"),
        ["--json", "version"],
    );
    let daemon_probe = Command::new(&packaged_daemon)
        .args(["--json", "version"])
        .output()
        .expect("packaged daemon identity probe must execute");
    assert!(daemon_probe.status.success());
    assert!(daemon_probe.stderr.is_empty());
    let daemon_identity: Value = serde_json::from_slice(&daemon_probe.stdout)
        .expect("packaged daemon identity must be JSON");
    for field in [
        "build_identity",
        "contract_manifest_digest",
        "source_commit",
        "target",
        "version",
    ] {
        assert_eq!(cli_identity["result"][field], daemon_identity[field]);
    }
    assert_eq!(cli_identity["result"]["target"], "aarch64-apple-darwin");
    assert_eq!(
        cli_identity["result"]["contract_manifest_digest"],
        provenance["contract_manifest_digest"]
    );
    assert_eq!(
        cli_identity["result"]["source_commit"],
        provenance["source_commit"]
    );

    let child = Command::new(std::env::current_exe().expect("E2E test binary path"))
        .arg("e2e_dolgorae_conformance::aut_t_")
        .args(["--nocapture", "--test-threads=1"])
        .env(CHILD_MARKER, "1")
        .env("PODWAY_TEST_CLI_BINARY", &packaged_cli)
        .env("PODWAYD_TEST_BINARY", &packaged_daemon)
        .output()
        .expect("packaged Dolgorae child suite must execute");
    assert!(
        child.status.success(),
        "packaged Dolgorae suite failed: stdout={} stderr={}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    let child_stdout = String::from_utf8(child.stdout).expect("child output must be UTF-8");
    assert!(
        child_stdout.contains("6 passed"),
        "child output={child_stdout}"
    );

    fs::remove_dir_all(root).expect("distribution fixture cleanup must succeed");
}
