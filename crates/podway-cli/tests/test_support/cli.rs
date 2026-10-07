//! Owned real-CLI fixtures with bounded output and child cleanup.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::MetadataExt,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use podway_protocol::OutputEnvelopeV4;
use serde_json::Value;

pub(super) fn podway_binary() -> PathBuf {
    let Some(path) = std::env::var_os("PODWAY_TEST_CLI_BINARY") else {
        return PathBuf::from(env!("CARGO_BIN_EXE_podway"));
    };
    let path = PathBuf::from(path);
    assert!(
        path.is_absolute(),
        "the packaged CLI must be an absolute path"
    );
    assert!(fs::symlink_metadata(&path).unwrap().is_file());
    assert_eq!(path.canonicalize().unwrap(), path);
    let fixture_root = PathBuf::from(
        std::env::var_os("PODWAY_TEST_FIXTURE_ROOT")
            .expect("packaged fixtures require an owned root"),
    );
    assert!(fixture_root.is_absolute());
    assert_eq!(fixture_root.canonicalize().unwrap(), fixture_root);
    assert_eq!(std::env::temp_dir().canonicalize().unwrap(), fixture_root);
    path
}

pub(super) fn assert_packaged_fixture(path: &Path) {
    if std::env::var_os("PODWAY_TEST_CLI_BINARY").is_some() {
        let _ = podway_binary();
        let root = PathBuf::from(std::env::var_os("PODWAY_TEST_FIXTURE_ROOT").unwrap());
        let path = path.canonicalize().unwrap();
        assert!(path.starts_with(&root) && path != root);
    }
}

pub(super) struct Fixture(pub(super) PathBuf);
pub(super) type PodwaySnapshot = Vec<(PathBuf, Option<Vec<u8>>, u64, u64)>;

pub(super) struct GuardedChild(pub(super) Option<Child>, Option<nix::unistd::Pid>);

impl GuardedChild {
    pub(super) fn new(child: Child) -> Self {
        Self(Some(child), None)
    }
}

impl Drop for GuardedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            if let Some(group) = self.1 {
                let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("podway-direct-cli-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert_packaged_fixture(&root);
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        Self(root)
    }

    pub(super) fn output(&self, arguments: &[&str]) -> Output {
        let mut argv = vec!["--json"];
        argv.extend_from_slice(arguments);
        self.raw_output(&argv)
    }

    pub(super) fn raw_output(&self, arguments: &[&str]) -> Output {
        assert_packaged_fixture(&self.0);
        let mut command = Command::new(podway_binary());
        let route = arguments
            .iter()
            .copied()
            .find(|argument| !matches!(*argument, "--json" | "--quiet" | "--no-color"));
        if !matches!(
            route,
            Some("help" | "--help" | "-h" | "version" | "completions" | "procedure" | "preset")
        ) {
            command.arg("--worktree").arg(&self.0);
        }
        let child = command
            .args(arguments)
            .current_dir(&self.0)
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        bounded_output(GuardedChild::new(child), arguments)
    }

    pub(super) fn run(&self, arguments: &[&str]) -> Value {
        let output = self.output(arguments);
        assert_eq!(output.status.code(), Some(0), "{arguments:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{arguments:?}: {output:?}");
        serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
            .unwrap()
            .into_value()
    }

    pub(super) fn podway_snapshot(&self) -> PodwaySnapshot {
        let root = self.0.join(".podway");
        let mut pending = vec![root.clone()];
        let mut entries = Vec::new();
        while let Some(path) = pending.pop() {
            let metadata = fs::symlink_metadata(&path).unwrap();
            let bytes = if metadata.is_file() {
                Some(fs::read(&path).unwrap())
            } else if metadata.file_type().is_symlink() {
                Some(
                    fs::read_link(&path)
                        .unwrap()
                        .into_os_string()
                        .into_encoded_bytes(),
                )
            } else {
                assert!(metadata.is_dir());
                pending.extend(
                    fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
                None
            };
            entries.push((
                path.strip_prefix(&root).unwrap().to_path_buf(),
                bytes,
                metadata.dev(),
                metadata.ino(),
            ));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    pub(super) fn assert_podway_snapshot(&self, expected: &PodwaySnapshot) {
        let actual = self.podway_snapshot();
        assert_eq!(
            actual.iter().map(|entry| &entry.0).collect::<Vec<_>>(),
            expected.iter().map(|entry| &entry.0).collect::<Vec<_>>()
        );
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                actual.1 == expected.1,
                "bytes changed: {}",
                actual.0.display()
            );
            assert_eq!(
                (actual.2, actual.3),
                (expected.2, expected.3),
                "file identity changed: {}",
                actual.0.display()
            );
        }
    }

    pub(super) fn output_with_stdin(&self, arguments: &[&str], input: &[u8]) -> Output {
        assert_packaged_fixture(&self.0);
        let child = Command::new(podway_binary())
            .arg("--worktree")
            .arg(&self.0)
            .arg("--json")
            .args(arguments)
            .current_dir(&self.0)
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut guard = GuardedChild::new(child);
        let mut stdin = guard.0.as_mut().unwrap().stdin.take().unwrap();
        let input = input.to_owned();
        let writer = thread::spawn(move || stdin.write_all(&input));
        let output = bounded_output(guard, arguments);
        // A local decoder may reject before consuming all input.
        let written = writer.join().unwrap();
        assert!(written.is_ok() || written.unwrap_err().kind() == std::io::ErrorKind::BrokenPipe);
        output
    }

    pub(super) fn continue_in_pty(&self) -> String {
        assert_packaged_fixture(&self.0);
        let child = Command::new("/usr/bin/script")
            .args(["-q", "/dev/null"])
            .arg(podway_binary())
            .arg("--worktree")
            .arg(&self.0)
            .args([
                "start",
                "--preset",
                "small-change-v2",
                "--task",
                "successor",
            ])
            .process_group(0)
            .current_dir(&self.0)
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let group = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
        let mut guard = GuardedChild(Some(child), Some(group));
        guard
            .0
            .as_mut()
            .unwrap()
            .stdin
            .take()
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        let output = bounded_output(guard, &["interactive start"]);
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    pub(super) fn pause_at_maintenance_boundary(
        &self,
        arguments: &[&str],
        stage: &str,
    ) -> GuardedChild {
        assert_packaged_fixture(&self.0);
        let child = Command::new(podway_binary())
            .arg("--worktree")
            .arg(&self.0)
            .arg("--json")
            .args(arguments)
            .current_dir(&self.0)
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env(
                "PODWAY_TEST_DIRECT_MAINTENANCE_TOKEN",
                "podway-direct-maintenance-test-v1",
            )
            .env("PODWAY_TEST_DIRECT_MAINTENANCE_PAUSE", stage)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut guard = GuardedChild::new(child);
        let stderr = guard.0.as_mut().unwrap().stderr.take().unwrap();
        let (send, receive) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stderr.take(1024)).read_line(&mut line);
            let _ = send.send((result, line));
        });
        let (result, line) = receive
            .recv_timeout(Duration::from_secs(10))
            .expect("the real CLI must reach the selected durable boundary");
        result.unwrap();
        assert_eq!(line, format!("podway-test-maintenance-boundary:{stage}\n"));
        assert!(
            guard.0.as_mut().unwrap().try_wait().unwrap().is_none(),
            "the boundary witness must precede process termination"
        );
        reader.join().unwrap();
        guard
    }

    pub(super) fn kill_at_maintenance_boundary(&self, arguments: &[&str], stage: &str) {
        let mut guard = self.pause_at_maintenance_boundary(arguments, stage);
        guard.0.as_mut().unwrap().kill().unwrap();
        let output = guard.0.take().unwrap().wait_with_output().unwrap();
        assert_eq!(output.status.signal(), Some(9));
        assert!(
            output.stdout.is_empty(),
            "the interrupted mutation must not have delivered a success"
        );
    }
}

pub(super) fn bounded_output(mut guard: GuardedChild, arguments: &[&str]) -> Output {
    let stdout = guard.0.as_mut().unwrap().stdout.take().unwrap();
    let stderr = guard.0.as_mut().unwrap().stderr.take().unwrap();
    let stdout = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(1_048_578).read_to_end(&mut bytes).unwrap();
        bytes
    });
    let stderr = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(65_537).read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = guard.0.as_mut().unwrap().try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the real CLI exceeded its liveness bound: {arguments:?}"
        );
        thread::sleep(Duration::from_millis(5));
    };
    guard.0.take();
    Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
