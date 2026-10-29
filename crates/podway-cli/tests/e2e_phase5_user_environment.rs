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
    assert_eq!(
        envelope,
        serde_json::json!({
            "name": "podway",
            "version": format!("v{}", env!("CARGO_PKG_VERSION")),
        })
    );

    let output = Command::new("podway")
        .arg("version")
        .current_dir(&arbitrary)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .expect("controlled PATH must resolve podway");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("version output must be UTF-8"),
        format!("podway v{}\n", env!("CARGO_PKG_VERSION"))
    );

    let output = Command::new("podway")
        .args(["version", "--json", "--identity"])
        .current_dir(&arbitrary)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .expect("controlled PATH must resolve podway");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let envelope: Value =
        serde_json::from_slice(&output.stdout).expect("version identity must be JSON");
    assert_eq!(envelope["schema"], "podway.output/v4");
    assert_eq!(envelope["command"], "version");
    assert_eq!(envelope["result"]["schema"], "podway.version-result/v2");
    assert_eq!(
        envelope["result"]["contract_manifest_schema"],
        "podway.contract-manifest/v2"
    );
    assert!(envelope["result"].get("supported_ipc_ids").is_none());
    assert_eq!(envelope["result"].as_object().unwrap().len(), 8);
    podway_protocol::OutputEnvelopeV4::from_value(envelope.clone())
        .expect("version identity satisfies the complete direct output contract");
    assert_eq!(
        envelope["result"]["version"],
        format!("v{}", env!("CARGO_PKG_VERSION"))
    );

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

#[test]
fn calls001_help_completion_executes_contextually_without_workspace_queries() {
    let root = fixture_root("help-completion");
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let sentinel = bin.join("podway");
    let query_marker = root.join("workspace-query");
    fs::write(
        &sentinel,
        "#!/bin/sh\nprintf query >> \"$PODWAY_COMPLETION_QUERY_MARKER\"\nprintf '%s\\n' calls-preserved-item\nexit 1\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o755)).unwrap();
    for (topic, selector) in [("reset", "--all"), ("start", "--on-existing")] {
        let help = Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(["help", "--", topic, selector])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(help.status.success(), "{topic} {selector}: {help:?}");
        assert!(help.stderr.is_empty());
    }
    for shell in ["bash", "zsh", "fish"] {
        let generated = Command::new(env!("CARGO_BIN_EXE_podway"))
            .args(["completions", shell])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(generated.status.success());
        let script = String::from_utf8(generated.stdout).unwrap();
        for (words, required, excluded) in [
            (
                "help ",
                vec!["overview", "workflow", "goal", "session.observe", "daemon"],
                vec!["__complete"],
            ),
            ("help go", vec!["goal"], vec!["__complete"]),
            (
                "help goal ",
                vec!["assess-criterion", "define", "revise"],
                vec!["__complete"],
            ),
            (
                "help session.ob",
                vec!["session.observe"],
                vec!["__complete"],
            ),
            (
                "--contract-generation 2 help --quiet goal ",
                vec!["assess-criterion"],
                vec!["__complete"],
            ),
            (
                "help --contract-generation=2 goal ",
                vec!["assess-criterion"],
                vec!["__complete"],
            ),
            (
                "--contract-generation goal help ",
                vec!["overview", "goal"],
                vec!["__complete"],
            ),
            (
                "--contract-generation = check help ",
                vec!["overview", "goal"],
                vec!["__complete"],
            ),
            (
                "help --contract-generation go",
                vec![],
                vec!["goal", "overview"],
            ),
            (
                "help --contract-generation ",
                vec![],
                vec!["goal", "overview"],
            ),
            ("help goal --contract-generation re", vec![], vec!["revise"]),
            (
                "help --contract-generation=go",
                vec![],
                vec!["goal", "overview"],
            ),
            (
                "help --contract-generation 2 ",
                vec!["goal", "overview"],
                vec!["__complete"],
            ),
            (
                "help goal --contract-generation 2 ",
                vec!["revise"],
                vec!["__complete"],
            ),
            (
                "help --contract-generation = go",
                vec![],
                vec!["goal", "overview"],
            ),
            (
                "help --contract-generation = 2 goal ",
                vec!["assess-criterion"],
                vec!["__complete"],
            ),
            (
                "help --contract-generation 2 -- goal ",
                vec!["assess-criterion"],
                vec!["__complete"],
            ),
            (
                "help -- --contract-generation=2 ",
                vec![],
                vec!["goal", "overview"],
            ),
            ("help -- reset --", vec!["--all"], vec!["--mode"]),
            ("help -- start --", vec!["--on-existing"], vec!["--mode"]),
            ("help workspace ", vec!["mode", "show"], vec!["__complete"]),
            ("help runtime ", vec!["reset"], vec!["__complete"]),
            (
                "",
                vec!["help", "goal", "outcome"],
                vec!["daemon", "runtime", "job", "__complete"],
            ),
            ("workspace ", vec!["show", "repair", "remove"], vec!["mode"]),
            ("outcome ", vec!["lookup"], vec!["job"]),
        ] {
            // Bash can split an attached equals-form at its '=' word break.
            if words.contains(" = ") && shell != "bash" {
                continue;
            }
            let mut tokens = format!("podway {words}")
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if words.is_empty() || words.ends_with(' ') {
                tokens.push(String::new());
            }
            let quoted = tokens
                .iter()
                .map(|word| format!("'{word}'"))
                .collect::<Vec<_>>()
                .join(" ");
            // The spaced '=' tokens model Bash word breaks in attached raw syntax.
            let line = format!("podway {}", words.replace(" = ", "="));
            let harness = match shell {
                "bash" => format!(
                    "{script}\nCOMP_WORDS=({quoted}); COMP_CWORD={}; COMP_LINE='{line}'; COMP_POINT={}; _podway; printf '%s\\n' \"${{COMPREPLY[@]}}\"",
                    tokens.len() - 1,
                    line.len()
                ),
                // compadd requires an interactive completion context. Capture the generated
                // function's actual candidates; bash/fish also exercise prefix filtering.
                "zsh" => format!(
                    "compdef() {{ :; }}\n{script}\ncompadd() {{ shift; print -rl -- \"$@\"; }}\nwords=({quoted}); CURRENT={}; _podway",
                    tokens.len()
                ),
                "fish" => format!("{script}\ncomplete -C 'podway {words}'"),
                _ => unreachable!(),
            };
            let child = Command::new(shell)
                .args(["-c", &harness])
                .current_dir(&root)
                .env("PODWAY_COMPLETION_QUERY_MARKER", &query_marker)
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                )
                .output()
                .unwrap();
            assert!(
                !query_marker.exists(),
                "{shell} {words}: queried workspace state"
            );
            assert!(
                child.status.success(),
                "{shell} {words}: {}",
                String::from_utf8_lossy(&child.stderr)
            );
            assert!(
                child.stderr.is_empty(),
                "{shell} {words}: {}",
                String::from_utf8_lossy(&child.stderr)
            );
            let output = String::from_utf8(child.stdout).unwrap();
            let candidates = output
                .lines()
                .map(|line| line.split('\t').next().unwrap())
                .collect::<Vec<_>>();
            for candidate in required {
                assert!(
                    candidates.contains(&candidate),
                    "{shell} {words}: missing {candidate}: {candidates:?}"
                );
            }
            for candidate in excluded {
                assert!(
                    !candidates.contains(&candidate),
                    "{shell} {words}: unexpected {candidate}: {candidates:?}"
                );
            }
        }
        if shell == "bash" {
            // Identical split words can describe an attached value or a literal '='.
            // Keep the raw command line, cursor and quoted words faithful to each case.
            let split = ["podway", "--idempotency-key", "=", "check", "help", ""];
            for (line, tokens, required, queries, nounset) in [
                (
                    "podway --idempotency-key=check help ",
                    split.to_vec(),
                    "overview",
                    false,
                ),
                (
                    "podway --idempotency-key = check help ",
                    split.to_vec(),
                    "calls-preserved-item",
                    true,
                ),
                (
                    "podway --idempotency-key =check help ",
                    split.to_vec(),
                    "overview",
                    false,
                ),
                (
                    "podway --idempotency-key=check workspace ",
                    vec!["podway", "--idempotency-key", "=", "check", "workspace", ""],
                    "calls-preserved-item",
                    true,
                ),
                (
                    "podway help --idempotency-key = goal ",
                    vec!["podway", "help", "--idempotency-key", "=", "goal", ""],
                    "revise",
                    false,
                ),
                (
                    "podway help --idempotency-key= goal ",
                    vec!["podway", "help", "--idempotency-key", "=", "goal", ""],
                    "revise",
                    false,
                ),
                (
                    "podway --idempotency-key = --contract-generation=check help ",
                    vec![
                        "podway",
                        "--idempotency-key",
                        "=",
                        "--contract-generation",
                        "=",
                        "check",
                        "help",
                        "",
                    ],
                    "overview",
                    false,
                ),
                (
                    "  true; podway --worktree '한 글' --contract-generation=check help ",
                    vec![
                        "podway",
                        "--worktree",
                        "'한 글'",
                        "--contract-generation",
                        "=",
                        "check",
                        "help",
                        "",
                    ],
                    "overview",
                    false,
                ),
                (
                    "echo podway --contract-generation = check; podway --contract-generation=check help ",
                    vec!["podway", "--contract-generation", "=", "check", "help", ""],
                    "overview",
                    false,
                ),
                (
                    "echo podway --idempotency-key=check; podway --worktree '한 글' --idempotency-key = check help ",
                    vec![
                        "podway",
                        "--worktree",
                        "'한 글'",
                        "--idempotency-key",
                        "=",
                        "check",
                        "help",
                        "",
                    ],
                    "calls-preserved-item",
                    true,
                ),
                (
                    "podway --worktree '한 글' --contract-generation=check help <CURSOR>--contract-generation = check",
                    vec![
                        "podway",
                        "--worktree",
                        "'한 글'",
                        "--contract-generation",
                        "=",
                        "check",
                        "help",
                        "",
                    ],
                    "overview",
                    false,
                ),
                (
                    "podway --idempotency-key = check help =checkhelp",
                    vec![
                        "podway",
                        "--idempotency-key",
                        "=",
                        "check",
                        "help",
                        "=checkhelp",
                    ],
                    "",
                    true,
                ),
                (
                    "podway --idempotency-key = check help =check<CURSOR>help",
                    vec![
                        "podway",
                        "--idempotency-key",
                        "=",
                        "check",
                        "help",
                        "=checkhelp",
                    ],
                    "",
                    true,
                ),
            ]
            .into_iter()
            .map(|(line, tokens, required, queries)| (line, tokens, required, queries, false))
            .chain([
                (
                    "podway workspace ",
                    vec!["podway", "workspace", ""],
                    "show",
                    false,
                    true,
                ),
                (
                    "podway help --contract-generation 2 goal ",
                    vec!["podway", "help", "--contract-generation", "2", "goal", ""],
                    "revise",
                    false,
                    true,
                ),
                (
                    "podway --contract-generation=check help ",
                    vec!["podway", "--contract-generation", "=", "check", "help", ""],
                    "overview",
                    false,
                    true,
                ),
            ])
            {
                let point = line.find("<CURSOR>").unwrap_or(line.len());
                let line = line.replace("<CURSOR>", "");
                let quote = |word: &str| format!("'{}'", word.replace('\'', "'\\''"));
                let quoted = tokens
                    .iter()
                    .map(|word| quote(word))
                    .collect::<Vec<_>>()
                    .join(" ");
                let harness = format!(
                    "{script}\nCOMP_WORDS=({quoted}); COMP_CWORD={}; COMP_LINE={}; COMP_POINT={point}; _podway; printf '%s\\n' \"${{COMPREPLY[@]}}\"",
                    tokens.len() - 1,
                    quote(&line),
                );
                let child = Command::new("bash")
                    .args([if nounset { "-uc" } else { "-c" }, &harness])
                    .current_dir(&root)
                    .env("PODWAY_COMPLETION_QUERY_MARKER", &query_marker)
                    .env(
                        "PATH",
                        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                    )
                    .output()
                    .unwrap();
                assert_eq!(
                    query_marker.exists(),
                    queries,
                    "bash {line}: query boundary"
                );
                if queries {
                    assert_eq!(fs::read_to_string(&query_marker).unwrap(), "query");
                    fs::remove_file(&query_marker).unwrap();
                }
                assert!(child.status.success(), "bash {line}: {child:?}");
                assert!(child.stderr.is_empty(), "bash {line}: {child:?}");
                let output = String::from_utf8(child.stdout).unwrap();
                if !required.is_empty() {
                    assert!(
                        output.lines().any(|word| word == required),
                        "bash {line}: missing {required}: {output}"
                    );
                }
            }
        }
    }
    assert!(!root.join(".podway").exists());
    fs::remove_dir_all(root).unwrap();
}
