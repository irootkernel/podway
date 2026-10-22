//! Socket fixtures keep their bound Unix socket path below the `sun_path`
//! capacity under any inherited TMPDIR length or spelling.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::{Builder, TempDir};

const SUN_PATH_CAPACITY: usize = 104;
const LONG_TMPDIR_BYTES: usize = 120;
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);
const LONGEST_SOCKET_SUFFIX: &str = "/socket/.podway/config.yaml";
const PHASE3_SOCKET_TEST: &str = concat!(
    "int_phase3_resolver::",
    "artifacts_reject_final_and_intermediate_symlinks_and_preserve_non_utf8_bytes"
);
const PHASE4_SOCKET_TEST: &str = concat!(
    "int_phase4_init_layout::",
    "config_conflicting_node_kinds_fail_without_layout_mutation"
);

// `/tmp` canonicalizes to `/private/tmp` on macOS; the padded leaf name makes
// the canonical root reach the long fixture length.
fn plain_long_tmpdir() -> (TempDir, PathBuf) {
    let prefix = "g".repeat(LONG_TMPDIR_BYTES - "/private/tmp".len() - 1);
    let long = Builder::new()
        .prefix(&prefix)
        .tempdir_in("/tmp")
        .expect("long temporary directory");
    let root = fs::canonicalize(long.path()).expect("canonical long temporary root");
    assert!(
        root.as_os_str().len() >= LONG_TMPDIR_BYTES,
        "long TMPDIR fixture must stay long: {}",
        root.display()
    );
    (long, root)
}

// A long spelling ending in a symlink to a short directory: the literal path
// exceeds the socket capacity while its canonical form still fits.
fn aliased_short_tmpdir() -> (TempDir, TempDir, PathBuf) {
    let short = Builder::new()
        .tempdir_in("/tmp")
        .expect("short target directory");
    let short_root = fs::canonicalize(short.path()).expect("canonical short target");
    let parent = Builder::new()
        .tempdir_in("/tmp")
        .expect("alias parent directory");
    let name_length = LONG_TMPDIR_BYTES - parent.path().as_os_str().len() - 1;
    let alias = parent.path().join("l".repeat(name_length));
    symlink(&short_root, &alias).expect("long temporary-directory alias");
    assert!(
        alias.as_os_str().len() >= LONG_TMPDIR_BYTES,
        "alias TMPDIR fixture must stay long: {}",
        alias.display()
    );
    let canonical = fs::canonicalize(&alias).expect("canonical alias target");
    assert!(
        canonical.as_os_str().len() + LONGEST_SOCKET_SUFFIX.len() < SUN_PATH_CAPACITY,
        "alias canonical form must stay short: {}",
        canonical.display()
    );
    (short, parent, alias)
}

fn exact_socket_child(case: &str, test: &str, tmpdir: &Path) -> (String, String, ExitStatus) {
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("TMPDIR", tmpdir)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("socket fixture child process");
    let limit = Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < limit => thread::sleep(Duration::from_millis(10)),
            state => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("socket fixture child failed to finish: {state:?}");
            }
        }
    };
    (case.to_string(), test.to_string(), status)
}

#[test]
fn socket_fixtures_pass_with_a_long_tmpdir() {
    let mut results = Vec::new();
    {
        let (long, root) = plain_long_tmpdir();
        results.push(exact_socket_child(
            "plain long TMPDIR",
            PHASE3_SOCKET_TEST,
            &root,
        ));
        results.push(exact_socket_child(
            "plain long TMPDIR",
            PHASE4_SOCKET_TEST,
            &root,
        ));
        // The long root is removed only after its children are reaped.
        drop(long);
    }
    {
        let (short, parent, alias) = aliased_short_tmpdir();
        results.push(exact_socket_child(
            "symlinked short TMPDIR",
            PHASE3_SOCKET_TEST,
            &alias,
        ));
        results.push(exact_socket_child(
            "symlinked short TMPDIR",
            PHASE4_SOCKET_TEST,
            &alias,
        ));
        // The alias symlink and both directories are removed only after the
        // children are reaped; removal deletes the symlink itself.
        drop(parent);
        drop(short);
    }
    let failed: Vec<&(String, String, ExitStatus)> = results
        .iter()
        .filter(|(_, _, status)| !status.success())
        .collect();
    assert!(
        failed.is_empty(),
        "socket fixture children failed: {failed:?}"
    );
}
