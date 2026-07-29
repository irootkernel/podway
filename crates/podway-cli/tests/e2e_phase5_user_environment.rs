#![forbid(unsafe_code)]

use std::{
    fs,
    io::Write,
    os::unix::fs::symlink,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fixture_root(label: &str) -> std::path::PathBuf {
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("podway-{label}-{}-{sequence}", std::process::id()));
    fs::create_dir(&root).expect("user-environment fixture must be created");
    root
}

#[test]
fn aut_t_path_invokes_cli_symlink_from_sanitized_arbitrary_directory() {
    let root = fixture_root("path-e2e");
    let bin = root.join("bin");
    let arbitrary = root.join("outside-worktree");
    fs::create_dir(&bin).expect("controlled bin directory must be created");
    fs::create_dir(&arbitrary).expect("arbitrary working directory must be created");
    symlink(env!("CARGO_BIN_EXE_podway"), bin.join("podway"))
        .expect("controlled PATH entry must be a CLI symlink");

    let output = Command::new("podway")
        .args(["--json", "version"])
        .current_dir(&arbitrary)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .expect("controlled PATH must resolve podway");
    assert!(
        output.status.success(),
        "PATH version probe failed: {output:?}"
    );
    assert!(output.stderr.is_empty());
    let envelope: Value =
        serde_json::from_slice(&output.stdout).expect("version output must be JSON");
    assert_eq!(envelope["result"]["version"], env!("CARGO_PKG_VERSION"));

    fs::remove_dir_all(root).expect("user-environment fixture must be removed");
}

#[test]
fn generated_shell_grammars_execute_with_nested_route_context() {
    for shell in ["bash", "zsh", "fish"] {
        let available = Command::new(shell)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(
            available,
            "{shell} is a required completion-test prerequisite"
        );

        let generated = Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(["completions", shell])
            .output()
            .expect("real CLI must generate a completion script");
        assert!(
            generated.status.success(),
            "{shell} completion generation failed"
        );
        assert!(generated.stderr.is_empty());

        let mut parser = Command::new(shell)
            .arg("-n")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("real shell must start");
        parser
            .stdin
            .take()
            .expect("shell stdin must be piped")
            .write_all(&generated.stdout)
            .expect("completion script must reach the shell");
        let parsed = parser
            .wait_with_output()
            .expect("shell result must be readable");
        assert!(
            parsed.status.success(),
            "{shell} rejected its generated completion script: {}",
            String::from_utf8_lossy(&parsed.stderr)
        );
    }
}
