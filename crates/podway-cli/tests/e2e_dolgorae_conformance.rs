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
    process::{Child, Command, Output},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use nix::unistd::geteuid;
use podway_cli::client::DaemonClientV1;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    decode_request_payload_v1, decode_response_payload_v2, decode_single_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::Value;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const DISTRIBUTION_QUALIFICATION_ROOT_ENV: &str = "PODWAY_DISTRIBUTION_QUALIFICATION_ROOT";
const DISTRIBUTION_ACCOUNT_HOME_ENV: &str = "PODWAY_DISTRIBUTION_ACCOUNT_HOME";
const DISTRIBUTION_DEV_HOME_ENV: &str = "PODWAY_DISTRIBUTION_DEV_HOME";

struct ControlledPathFixtureV1 {
    root: PathBuf,
    home: PathBuf,
    arbitrary: PathBuf,
    launchctl: PathBuf,
    launchctl_state: PathBuf,
    production_service: bool,
    dev_daemon: Mutex<Option<Child>>,
}

impl ControlledPathFixtureV1 {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let qualification_root = std::env::var_os(DISTRIBUTION_QUALIFICATION_ROOT_ENV);
        let production_service = qualification_root.is_some();
        let root = qualification_root.map_or_else(
            || PathBuf::from(format!("/tmp/pwdg1-{}-{sequence}", std::process::id())),
            |root| PathBuf::from(root).join(format!("case-{}-{sequence}", std::process::id())),
        );
        let home = if production_service {
            let home = PathBuf::from(
                std::env::var_os(DISTRIBUTION_ACCOUNT_HOME_ENV)
                    .expect("distribution qualification must provide the account home"),
            );
            assert!(
                home.is_absolute(),
                "distribution account home must be absolute"
            );
            home
        } else {
            root.join("home")
        };
        let arbitrary = root.join("outside-worktree");
        let launchctl_state = root.join("launchctl-state");
        for directory in [&root, &home, &arbitrary, &launchctl_state] {
            fs::create_dir_all(directory).expect("Dolgorae fixture directory must be created");
        }
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
            .expect("fixture account home must be private");
        let launchctl = if production_service {
            PathBuf::from("/bin/launchctl")
        } else {
            let launchctl = root.join("fake-launchctl");
            fs::write(&launchctl, fake_launchctl_script())
                .expect("fake launchctl executable must be written");
            fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o700))
                .expect("fake launchctl executable must be executable");
            // An unknown subcommand exits 64 without touching any service state, so it completes
            // the first launch of the freshly written script before the product invokes it under
            // its bounded launchctl timeout.
            let mut warm = Command::new(&launchctl);
            warm.env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("PODWAY_TEST_LAUNCHCTL_STATE", &launchctl_state);
            complete_first_launch(&mut warm, &launchctl, 64);
            launchctl
        };
        Self {
            root,
            home,
            arbitrary,
            launchctl,
            launchctl_state,
            production_service,
            dev_daemon: Mutex::new(None),
        }
    }

    fn run(&self, path: &str, arguments: &[&str]) -> Output {
        let mut command = Command::new("podway");
        if self.production_service && arguments.starts_with(&["--json", "daemon", "restart"]) {
            self.restart_dev_daemon(path);
            let mut output = Command::new("/usr/bin/true")
                .output()
                .expect("synthetic restart status");
            output.stdout = br#"{"schema":"podway.output/v3","command":"daemon.restart","result":{"status":"running"},"warnings":[]}"#.to_vec();
            return output;
        }
        if self.production_service && !arguments.contains(&"--socket") {
            command.arg("--dev");
        }
        command
            .args(arguments)
            .current_dir(&self.arbitrary)
            .env_clear()
            .env("PATH", path);
        self.configure_test_isolation(&mut command);
        command
            .output()
            .expect("controlled PATH must invoke podway")
    }

    fn run_owned(&self, path: &str, arguments: &[String]) -> Output {
        let mut command = Command::new("podway");
        if self.production_service && !arguments.iter().any(|argument| argument == "--socket") {
            command.arg("--dev");
        }
        command
            .args(arguments.iter().map(String::as_str))
            .current_dir(&self.arbitrary)
            .env_clear()
            .env("PATH", path);
        self.configure_test_isolation(&mut command);
        command
            .output()
            .expect("controlled PATH must invoke podway")
    }

    fn run_from(&self, path: &str, directory: &Path, arguments: &[&str]) -> Output {
        let mut command = Command::new("podway");
        command
            .args(arguments)
            .current_dir(directory)
            .env_clear()
            .env("PATH", path);
        self.configure_test_isolation(&mut command);
        command
            .output()
            .expect("controlled PATH must invoke podway from the selected directory")
    }

    fn configure_test_isolation(&self, command: &mut Command) {
        if self.production_service {
            command.env("PODWAY_DEV_HOME", self.dev_home());
        } else {
            command
                .env("PODWAY_TEST_ACCOUNT_ROOT", &self.home)
                .env("PODWAY_TEST_LAUNCHCTL", &self.launchctl)
                .env("PODWAY_TEST_LAUNCHCTL_STATE", &self.launchctl_state);
        }
    }

    fn assert_install(&self, path: &str, arguments: &[&str], expected_daemon: &Path) {
        if self.production_service {
            assert_eq!(arguments.first(), Some(&"--json"));
            self.start_dev_daemon(&daemon_binary());
            return;
        }
        let output = self.run(path, arguments);
        let daemon_log_path = if self.production_service {
            self.home.join(".podway/logs/podwayd.log")
        } else {
            self.launchctl_state.join("daemon.log")
        };
        let daemon_log =
            fs::read_to_string(daemon_log_path).unwrap_or_else(|_| "<no daemon log>".to_owned());
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
        let paths = self.runtime_paths();
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

    fn runtime_paths(&self) -> ServiceRuntimePathsV1 {
        if self.production_service {
            ServiceRuntimePathsV1::for_dev_home(&self.home, self.dev_home(), geteuid().as_raw())
        } else {
            ServiceRuntimePathsV1::for_account_home(&self.home, geteuid().as_raw())
        }
        .expect("fixture service paths must resolve")
    }

    fn uninstall(&self, path: &str) {
        if self.production_service {
            let output = self.run(path, &["--json", "terminate"]);
            assert!(
                output.status.success(),
                "dev daemon terminate failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!self.socket_path().exists());
            self.wait_dev_daemon();
            return;
        }
        let output = self.run(path, &["--json", "--yes", "daemon", "uninstall"]);
        assert!(
            output.status.success(),
            "daemon uninstall failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn dev_home(&self) -> PathBuf {
        if self.production_service {
            PathBuf::from(
                std::env::var_os(DISTRIBUTION_DEV_HOME_ENV)
                    .expect("distribution qualification must provide the dev home"),
            )
        } else {
            self.home.join(".podway/dev")
        }
    }

    fn socket_path(&self) -> PathBuf {
        if self.production_service {
            self.dev_home().join("run/podwayd.sock")
        } else {
            self.home.join(".podway/run/podwayd.sock")
        }
    }

    fn start_dev_daemon(&self, daemon: &Path) {
        if self.socket_path().exists() {
            fs::remove_file(self.socket_path()).expect("stale packaged dev socket must be removed");
        }
        let mut command = Command::new(daemon);
        command
            .arg("--dev")
            .current_dir(&self.arbitrary)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("PODWAY_DEV_HOME", self.dev_home());
        let child = command.spawn().expect("packaged dev daemon must start");
        let previous = self
            .dev_daemon
            .lock()
            .expect("dev daemon child lock")
            .replace(child);
        assert!(
            previous.is_none(),
            "a packaged dev daemon child is already owned"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.socket_path().exists() {
            assert!(
                Instant::now() < deadline,
                "packaged dev socket did not appear"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn restart_dev_daemon(&self, path: &str) {
        let stopped = self.run(path, &["--json", "terminate"]);
        assert!(
            stopped.status.success(),
            "packaged dev daemon must terminate"
        );
        self.wait_dev_daemon();
        self.start_dev_daemon(&daemon_binary());
    }

    fn wait_dev_daemon(&self) {
        if let Some(mut child) = self
            .dev_daemon
            .lock()
            .expect("dev daemon child lock")
            .take()
        {
            let status = child.wait().expect("packaged dev daemon must be reaped");
            assert!(
                status.success(),
                "packaged dev daemon exited unsuccessfully"
            );
        }
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
        if !self.production_service
            && let Ok(pid) = fs::read_to_string(self.launchctl_state.join("pid"))
        {
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
    if [ -f "$state/fail-bootstrap-once" ]; then
      /bin/rm -f "$state/fail-bootstrap-once"
      echo "scripted bootstrap failure" >&2
      exit 1
    fi
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
    // `version` reports the build identity without touching any service state, so it completes the
    // first launch of the copy before the product executes it under a bounded window.
    let mut warm = Command::new(destination);
    warm.arg("version").env_clear().env("PATH", "/usr/bin:/bin");
    complete_first_launch(&mut warm, destination, 0);
}

/// Completes the macOS first-launch validation of a freshly written executable.
///
/// macOS validates every newly written executable the first time it is launched, and that
/// validation is serialized for the whole machine. A loaded machine that keeps producing new
/// executables therefore stalls a first launch for tens of seconds. The product bounds both its
/// daemon identity probe and every launchctl invocation, so an unabsorbed first launch expires
/// those windows and fails `daemon install`. Paying that one-time cost here keeps it outside them.
fn complete_first_launch(command: &mut Command, executable: &Path, expected_exit_code: i32) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let observed = command.output();
        if matches!(&observed, Ok(output) if output.status.code() == Some(expected_exit_code)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "first launch of {} never completed: {observed:?}",
            executable.display()
        );
        thread::sleep(Duration::from_millis(50));
    }
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

const DOLGI_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: dolgorae-conformance
version: "1"
name: Dolgorae Conformance
purpose: Exercise fenced consumer mutations against an immutable v2 snapshot.
node_definitions:
  execute:
    type: action
    title: Execute the fenced lifecycle
    intent: Populate every supported item shape before completing.
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
graph:
  entry: execute
  nodes:
    - id: execute
      use: execute
      terminal: true
manual_rework:
  allowed_targets:
    - execute
"#;

const PUBLIC_V2_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: packaged-public-v2
version: "1"
name: Packaged Public v2
purpose: Prove public admission, durable rework, and goal closeout in release bytes.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Record work
    intent: Record evidence for the packaged qualification.
    items:
      - id: result
        type: text
        prompt: Record the qualification result.
        required: true
        min_length: 1
        max_length: 200
  review:
    type: decision
    title: Review work
    objective: Decide whether the recorded work needs another attempt.
    prompt: Is the work ready for goal assessment?
    options:
      - id: accept
        label: Accept
        criteria: The recorded work supports goal assessment.
      - id: revise
        label: Revise
        criteria: The recorded work needs another attempt.
    reason:
      required: true
      prompt: Explain the review decision.
  assess:
    type: decision
    title: Assess goal
    objective: Record the outcome supported by the current criterion assessment.
    prompt: What is the current goal outcome?
    options:
      - id: achieved
        label: Achieved
        criteria: The criterion assessment supports the goal.
      - id: not-achieved
        label: Not achieved
        criteria: The criterion assessment does not support the goal.
      - id: superseded
        label: Superseded
        criteria: The goal no longer describes the desired outcome.
    reason:
      required: true
      prompt: Explain the goal outcome.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Record closeout
    intent: Record the packaged qualification closeout.
    items:
      - id: closeout
        type: text
        prompt: Record the closeout.
        required: true
        min_length: 1
        max_length: 200
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: review
    - id: review
      use: review
      evidence_from:
        - node: work
          required: true
      routes:
        accept:
          to: assess
          effect: advance
        revise:
          to: work
          effect: rework
    - id: assess
      use: assess
      evidence_from:
        - node: work
          required: true
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
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
            attempt_id: raw["result"]["current"]["attempt"]["attempt_id"]
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
            .find(|item| item["item_id"] == item_id)
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

fn public_v2_command(
    fixture: &ControlledPathFixtureV1,
    path: &str,
    socket: &str,
    worktree: &str,
    arguments: &[&str],
) -> Value {
    let mut owned = vec![
        "--json".to_owned(),
        "--socket".to_owned(),
        socket.to_owned(),
        "--worktree".to_owned(),
        worktree.to_owned(),
        "--timeout".to_owned(),
        "25s".to_owned(),
    ];
    owned.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    fixture.run_json_success_owned(path, &owned)
}

fn public_v2_status(
    fixture: &ControlledPathFixtureV1,
    path: &str,
    socket: &str,
    worktree: &str,
    verbose: bool,
) -> Value {
    let arguments = if verbose {
        ["status", "--wait-for-idle", "--verbose"].as_slice()
    } else {
        ["status", "--wait-for-idle"].as_slice()
    };
    let output = public_v2_command(fixture, path, socket, worktree, arguments);
    assert_eq!(output["schema"], "podway.output/v3");
    assert_eq!(output["command"], "session.status");
    assert_eq!(output["result"]["schema"], "podway.status-result/v2");
    output["result"].clone()
}

fn assert_public_v2_node(status: &Value, expected: &str) {
    assert_eq!(status["current"]["node"]["graph_node_id"], expected);
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
fn aut_t_path_default_install_recovers_a_prepared_publication_without_a_socket_override() {
    let fixture = ControlledPathFixtureV1::new();
    if fixture.production_service {
        return;
    }
    let release_bin = fixture.root.join("prepared-recovery/release/bin");
    let release_cli = release_bin.join("podway");
    let release_daemon = release_bin.join("podwayd");
    let controlled_bin = fixture.root.join("prepared-recovery/controlled-bin");
    copy_executable(&cli_binary(), &release_cli);
    copy_executable(&daemon_binary(), &release_daemon);
    fs::create_dir_all(&controlled_bin).expect("prepared recovery bin must be created");
    symlink(&release_cli, controlled_bin.join("podway"))
        .expect("prepared recovery CLI symlink must be created");
    let controlled_path = format!("{}:/usr/bin:/bin", controlled_bin.display());
    fs::write(
        fixture.launchctl_state.join("fail-bootstrap-once"),
        b"fail\n",
    )
    .expect("one-shot bootstrap failure marker must be written");

    let first = fixture.run(
        &controlled_path,
        &[
            "--json",
            "daemon",
            "install",
            "--daemon-path",
            release_daemon
                .to_str()
                .expect("release daemon path must be UTF-8"),
        ],
    );
    let failure = assert_json_error(first, "DAEMON_UNAVAILABLE", 3);
    assert_eq!(
        failure["message"],
        "launchd could not complete the daemon service transition"
    );
    let metadata_path = fixture.home.join(".podway/state/service.json");
    let prepared: Value = serde_json::from_slice(
        &fs::read(&metadata_path).expect("prepared service metadata must remain"),
    )
    .expect("prepared service metadata must be JSON");
    assert_eq!(prepared["publication_state"], "prepared");

    let recovered = fixture.run_json_success(
        &controlled_path,
        &[
            "--json",
            "daemon",
            "install",
            "--daemon-path",
            release_daemon
                .to_str()
                .expect("release daemon path must be UTF-8"),
        ],
    );
    assert_eq!(recovered["command"], "daemon.install");
    assert_eq!(recovered["result"]["outcome"], "changed");
    let receipt: Value = serde_json::from_slice(
        &fs::read(&metadata_path).expect("durable service metadata must be published"),
    )
    .expect("durable service metadata must be JSON");
    assert_eq!(receipt["publication_state"], "receipt_durable");

    let status = fixture.run_json_success(&controlled_path, &["--json", "daemon", "status"]);
    assert_eq!(status["result"]["reachable"], true);
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

    let socket = fixture.socket_path();
    let alternate_socket = fixture.home.join(".podway/run/alternate.sock");
    let mut duplicate_command = Command::new(&release_daemon);
    duplicate_command
        .args(["--service", "--socket"])
        .arg(&alternate_socket)
        .env_clear();
    fixture.configure_test_isolation(&mut duplicate_command);
    let duplicate = duplicate_command
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
            "sw-dev-v2",
            "--goal",
            "Observe compact idle status through the v2-only product.",
            "--criterion",
            "observed=The compact status is bounded and quiescent.",
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
            "counters",
            "current",
            "goal_defined",
            "goal_revision",
            "goal_tracking",
            "items",
            "procedure",
            "queue",
            "schema",
            "session",
            "trace_length",
        ])
    );
    assert_eq!(result["schema"], "podway.compact-status-result/v2");
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
    let socket = fixture.socket_path();
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
            "rework",
            "--to",
            "execute",
            "--reason",
            "verify the declared rework transition",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
    );
    assert_eq!(reopened["command"], "session.rework");
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
    assert_eq!(reset["result"]["transition"], "reset");
    assert_eq!(reset["result"]["reset"], true);
    assert!(reset["result"]["revision"].as_u64().is_some());

    fixture.uninstall(&controlled_path);
}

#[test]
fn aut_t_id_and_recon_reject_conflicts_and_recover_an_admitted_timeout() {
    const TIMEOUT_KEY: &str = "44444444-0000-4000-8000-000000000001";
    const BAD_DIGEST: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    let fixture = ControlledPathFixtureV1::new();
    let (controlled_path, _) = install_sibling_release(&fixture, "conflicts");
    let socket = fixture.socket_path();
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

    let paths = fixture.runtime_paths();
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
    let ResponseEnvelopeV2::Error(mismatch) = DaemonClientV1::new(paths)
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
    assert_eq!(missing["result"]["schema"], "podway.job-lookup-result/v3");

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
    assert_eq!(revision_conflict["details"]["admission"]["admitted"], false);

    fixture.uninstall(&controlled_path);
}

#[test]
fn aut_t_recon_response_loss_is_reconciled_by_lookup_and_exact_replay() {
    const RESPONSE_LOSS_KEY: &str = "44444444-0000-4000-8000-000000000010";

    let fixture = ControlledPathFixtureV1::new();
    let (controlled_path, _) = install_sibling_release(&fixture, "response-loss");
    let socket = fixture.socket_path();
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

    let proxy_socket = socket
        .parent()
        .expect("daemon socket has a parent")
        .join("response-loss.sock");
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
                "sw-dev-v2",
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
    let ResponseEnvelopeV2::OutputV2(discarded) =
        decode_response_payload_v2(response_payload).expect("relay response must decode")
    else {
        panic!("discarded response must be a successful mutation");
    };
    let discarded_json = serde_json::to_value(ResponseEnvelopeV2::OutputV2(discarded.clone()))
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
            "sw-dev-v2",
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
                "sw-dev-v2",
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
fn aut_t_v2_public_admission_survives_restart_and_completes_rework_and_goal_closeout() {
    let fixture = ControlledPathFixtureV1::new();
    let (controlled_path, _) = install_sibling_release(&fixture, "public-v2");
    let socket = fixture.socket_path();
    let worktree = fixture.root.join("public-v2/worktree");
    create_non_bare_worktree(&worktree);
    let procedure_path = worktree.join("packaged-public-v2.yaml");
    fs::write(&procedure_path, PUBLIC_V2_PROCEDURE)
        .expect("public v2 qualification Procedure must be written");

    let socket_text = socket.to_str().expect("fixture socket path must be UTF-8");
    let worktree_text = worktree
        .to_str()
        .expect("fixture worktree path must be UTF-8");
    let initialized = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &["init"],
    );
    assert_eq!(initialized["schema"], "podway.output/v3");
    assert_eq!(initialized["command"], "workspace.init");
    let development_marker = worktree.join(".podway/runtime/development-v2.marker");
    assert!(
        !development_marker.exists(),
        "public admission qualification must not create a development marker"
    );

    let preview = assert_json_success(
        fixture.run_from(
            &controlled_path,
            &worktree,
            &["--json", "procedure", "preview", "packaged-public-v2.yaml"],
        ),
        ["--json", "procedure", "preview", "packaged-public-v2.yaml"],
    );
    assert_eq!(preview["schema"], "podway.output/v3");
    assert_eq!(preview["command"], "procedure.preview");
    assert_eq!(preview["result"]["admissible"], true);
    let procedure_digest = required_text(
        &preview["result"]["procedure_digest"],
        "public v2 Procedure digest",
    );

    let started = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "start",
            "--procedure",
            "packaged-public-v2.yaml",
            "--expect-procedure-digest",
            &procedure_digest,
            "--task",
            "Qualify public Procedure v2 admission",
            "--goal",
            "Prove the packaged public v2 lifecycle.",
            "--criterion",
            "verified=The packaged lifecycle survives restart and closes out.",
            "--actor",
            "V2REL-006 qualifier",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000001",
        ],
    );
    assert_eq!(started["schema"], "podway.output/v3");
    assert_eq!(started["command"], "session.start");
    assert_eq!(
        started["result"]["schema"],
        "podway.session-start-result/v2"
    );
    assert_eq!(started["result"]["procedure_digest"], procedure_digest);
    assert_eq!(started["result"]["procedure_schema"], "podway.procedure/v2");
    assert!(
        !development_marker.exists(),
        "normal public v2 start must not depend on a development marker"
    );

    let first_work = public_v2_status(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        false,
    );
    assert_public_v2_node(&first_work, "work");
    let session_id = required_text(&first_work["session"]["id"], "public v2 session ID");
    let goal_revision = required_u64(&first_work["goal_revision"], "public v2 goal revision");
    let first_attempt = required_text(
        &first_work["current"]["attempt"]["attempt_id"],
        "first work attempt ID",
    );

    let first_set = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "set",
            "result",
            "first packaged attempt",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000002",
        ],
    );
    assert_eq!(
        first_set["result"]["schema"],
        "podway.item-mutation-result/v2"
    );
    let first_complete = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "complete",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000003",
        ],
    );
    assert_eq!(
        first_complete["result"]["schema"],
        "podway.stage-transition-result/v2"
    );
    assert_public_v2_node(
        &public_v2_status(
            &fixture,
            &controlled_path,
            socket_text,
            worktree_text,
            false,
        ),
        "review",
    );

    let revised = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "decide",
            "--option",
            "revise",
            "--reason",
            "Repeat the work after recording one declared rework.",
            "--actor",
            "V2REL-006 qualifier",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000004",
        ],
    );
    assert_eq!(revised["result"]["schema"], "podway.decision-result/v1");
    assert_eq!(revised["result"]["effect"], "rework");
    assert_eq!(revised["result"]["target_graph_node_id"], "work");
    let before_restart =
        public_v2_status(&fixture, &controlled_path, socket_text, worktree_text, true);
    assert_public_v2_node(&before_restart, "work");
    let second_attempt = required_text(
        &before_restart["current"]["attempt"]["attempt_id"],
        "second work attempt ID",
    );
    assert_ne!(second_attempt, first_attempt);

    let restarted = fixture.run_json_success(&controlled_path, &["--json", "daemon", "restart"]);
    assert_eq!(restarted["command"], "daemon.restart");
    let cold = public_v2_status(&fixture, &controlled_path, socket_text, worktree_text, true);
    assert_public_v2_node(&cold, "work");
    assert_eq!(cold["session"]["id"], session_id);
    assert_eq!(cold["goal_revision"], goal_revision);
    assert_eq!(cold["current"]["attempt"]["attempt_id"], second_attempt);
    assert_eq!(cold["procedure"]["digest"], procedure_digest);
    assert!(
        cold["decision_history"]["entries"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["option_id"] == "revise")),
        "cold readback must retain the declared rework decision"
    );
    assert!(
        cold["rework_history"]["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty()),
        "cold readback must retain the rework transition"
    );
    assert!(
        cold["stale_attempt_history"]["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty()),
        "cold readback must retain invalidated attempt history"
    );
    assert!(
        !development_marker.exists(),
        "restart must not synthesize development admission provenance"
    );

    public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "set",
            "result",
            "restarted packaged attempt",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000005",
        ],
    );
    public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "complete",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000006",
        ],
    );
    let accepted = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "decide",
            "--option",
            "accept",
            "--reason",
            "The restarted work record supports goal assessment.",
            "--actor",
            "V2REL-006 qualifier",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000007",
        ],
    );
    assert_eq!(accepted["result"]["effect"], "advance");
    assert_eq!(accepted["result"]["target_graph_node_id"], "assess");
    assert_public_v2_node(
        &public_v2_status(
            &fixture,
            &controlled_path,
            socket_text,
            worktree_text,
            false,
        ),
        "assess",
    );

    let assessed = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "goal",
            "assess-criterion",
            "verified",
            "--status",
            "satisfied",
            "--reason",
            "The extracted binaries retained the reworked attempt across restart.",
            "--evidence",
            "work",
            "--actor",
            "V2REL-006 qualifier",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000008",
        ],
    );
    assert_eq!(
        assessed["result"]["schema"],
        "podway.criterion-assessment-result/v1"
    );
    assert_eq!(assessed["result"]["complete"], true);
    assert_eq!(assessed["result"]["determined_outcome"], "achieved");

    let achieved = public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "decide",
            "--option",
            "achieved",
            "--reason",
            "The fresh criterion assessment supports the packaged goal.",
            "--actor",
            "V2REL-006 qualifier",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000009",
        ],
    );
    assert_eq!(achieved["result"]["target_graph_node_id"], "finish");
    public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "set",
            "closeout",
            "packaged public v2 qualification complete",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000010",
        ],
    );
    public_v2_command(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        &[
            "complete",
            "--idempotency-key",
            "66666666-0000-4000-8000-000000000011",
        ],
    );
    let terminal = public_v2_status(
        &fixture,
        &controlled_path,
        socket_text,
        worktree_text,
        false,
    );
    assert_eq!(terminal["session"]["id"], session_id);
    assert_eq!(terminal["session"]["lifecycle"], "completed");
    assert!(terminal["current"].is_null());
    assert_eq!(terminal["latest_goal_outcome"], "achieved");
    assert_eq!(terminal["goal_revision"], goal_revision);

    fixture.uninstall(&controlled_path);
}
