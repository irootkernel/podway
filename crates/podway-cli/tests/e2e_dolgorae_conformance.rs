//! Dolgorae consumer conformance through the real Podway binaries.

#![forbid(unsafe_code)]

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use nix::unistd::geteuid;
use podway_cli::client::DaemonClientV1;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
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

fn daemon_binary() -> PathBuf {
    std::env::var_os("PODWAYD_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd"))
}

#[test]
fn aut_t_path_installs_explicit_sibling_and_path_daemons_from_a_sanitized_directory() {
    let fixture = ControlledPathFixtureV1::new();
    let source_cli = Path::new(env!("CARGO_BIN_EXE_podway"));
    let source_daemon = daemon_binary();

    let explicit_cli = fixture.root.join("explicit/cli/podway-real");
    let explicit_daemon = fixture.root.join("explicit/daemon/podwayd-real");
    let explicit_bin = fixture.root.join("explicit/bin");
    copy_executable(source_cli, &explicit_cli);
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
    copy_executable(source_cli, &sibling_cli);
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
    copy_executable(source_cli, &path_cli);
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
