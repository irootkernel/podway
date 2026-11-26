//! Reproduce the HANDS usability changes through the native CLI.

use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use podway_protocol::ErrorEnvelopeV2;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::cli_fixture::{Fixture, GuardedChild, bounded_output, podway_binary};

const BASIS_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_DIGEST: &str =
    "sha256:2222222222222222222222222222222222222222222222222222222222222222";
const GROUP_MISMATCH: &str =
    "The supplied or retained group definition does not match the expected digest.";
const GROUP_ACTIVATION: &str =
    "Group activation requires the current unselected base anchor and valid prior work.";
const REQUEST_INVALID: &str = "Request is malformed or violates schema.";
const ACTIVATE: &[&str] = &["group", "activate", "--stdin"];

#[test]
fn existing_default_preset_is_accepted_without_rewriting_config() {
    let fixture = Fixture::new();
    let config = fixture.0.join(".podway/config.yaml");
    let bytes = b"schema: podway.workspace/v1\ndefault_preset: sw-dev-v2\n";
    fs::create_dir(config.parent().unwrap()).unwrap();
    fs::write(&config, bytes).unwrap();
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    assert_eq!(
        fixture.run(&["doctor", "--deep"])["result"]["healthy"],
        true
    );
    assert_eq!(fs::read(&config).unwrap(), bytes);
    for repair in [
        ["init", "--repair", "--idempotency-key", "init-repair"],
        [
            "workspace",
            "repair",
            "--idempotency-key",
            "workspace-repair",
        ],
    ] {
        fixture.run(&repair);
        assert_eq!(fs::read(&config).unwrap(), bytes);
    }
    fixture.run(&[
        "reset",
        "--all",
        "--yes",
        "--if-workspace-uuid",
        initialized["workspace"]["uuid"].as_str().unwrap(),
        "--idempotency-key",
        "reset-all",
    ]);
    assert_eq!(fs::read(config).unwrap(), bytes);
    assert_eq!(
        fixture.run(&["doctor", "--deep"])["result"]["healthy"],
        true
    );
}

const HANDS003_PROCEDURE_FILE: &str = ".podway/procedures/hands003.yaml";
const HANDS003_START_TASK: &str = "Verify in-worktree absolute Procedure paths";

fn write_hands003_procedure(fixture: &Fixture) -> PathBuf {
    let source = fixture.0.join(HANDS003_PROCEDURE_FILE);
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let preset = podway_presets::lookup_v3("small-change-v3").unwrap();
    fs::write(&source, preset.base_yaml).unwrap();
    source
}

fn hands003_start_args<'a>(path: &'a str, digest: &'a str, key: &'a str) -> [&'a str; 9] {
    [
        "start",
        "--procedure",
        path,
        "--expect-procedure-digest",
        digest,
        "--task",
        HANDS003_START_TASK,
        "--idempotency-key",
        key,
    ]
}

fn hands003_output_at(directory: &Path, args: &[&str]) -> Output {
    let child = Command::new(podway_binary())
        .arg("--json")
        .args(args)
        .current_dir(directory)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    bounded_output(GuardedChild::new(child), args)
}

#[test]
fn absolute_procedure_preview_uses_the_relative_path_in_output() {
    let fixture = Fixture::new();
    let source = write_hands003_procedure(&fixture);
    let relative = fixture.run(&["procedure", "preview", HANDS003_PROCEDURE_FILE]);
    let absolute = fixture.run(&["procedure", "preview", source.to_str().unwrap()]);
    assert_eq!(absolute["result"], relative["result"]);
    assert_eq!(absolute["result"]["file"], HANDS003_PROCEDURE_FILE);
    assert_eq!(absolute["result"]["start_argv"][3], HANDS003_PROCEDURE_FILE);
    assert!(!fixture.0.join(".podway/runtime").exists());
}

#[test]
fn absolute_procedure_preview_normalizes_lexical_components_only() {
    let fixture = Fixture::new();
    write_hands003_procedure(&fixture);
    let noisy_relative = ".podway/./procedures//hands003.yaml";
    let noisy_absolute = format!("{}/{noisy_relative}", fixture.0.display());
    let clean = fixture.run(&["procedure", "preview", HANDS003_PROCEDURE_FILE]);
    let relative = fixture.run(&["procedure", "preview", noisy_relative]);
    assert_eq!(relative["result"]["file"], noisy_relative);
    assert_eq!(relative["result"]["start_argv"][3], noisy_relative);
    let absolute = fixture.run(&["procedure", "preview", &noisy_absolute]);
    assert_eq!(absolute["result"], clean["result"]);
}

#[test]
fn absolute_procedure_roots_are_refused_before_start_dispatch() {
    let fixture = Fixture::new();
    let nested = fixture.0.join("nested");
    fs::create_dir(&nested).unwrap();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let before = fixture.podway_snapshot();
    let digest = podway_presets::lookup_v3("small-change-v3")
        .unwrap()
        .base_digest;
    let root = fixture.0.to_str().unwrap();
    let nested = nested.to_str().unwrap();
    let mut nested_start = vec!["--worktree", nested];
    nested_start.extend(hands003_start_args(nested, digest, "root-source"));

    for (output, message) in [
        (
            fixture.output(&["procedure", "preview", root]),
            "preview requires a safe UTF-8 worktree-relative path",
        ),
        (
            fixture.output(&[
                "start",
                "--procedure",
                root,
                "--task",
                HANDS003_START_TASK,
                "--dry-run",
            ]),
            "procedure must be worktree-relative",
        ),
        (
            fixture.output(&hands003_start_args(root, digest, "root-source")),
            "procedure must be worktree-relative",
        ),
        (
            hands003_output_at(&fixture.0, &nested_start),
            "procedure must be worktree-relative",
        ),
    ] {
        refuse(output, "REQUEST_INVALID", 2, message);
    }
    fixture.assert_podway_snapshot(&before);
    let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", "root-source"]);
    assert_eq!(lookup["result"]["found"], false);
    refuse(
        fixture.output(&["status"]),
        "SESSION_NOT_FOUND",
        1,
        "No current task session exists.",
    );
}

#[test]
fn procedure_preview_help_distinguishes_preview_and_start_roots() {
    let fixture = Fixture::new();
    let help = fixture.run(&["help", "procedure", "preview"]);
    let help = help["result"]["text"].as_str().unwrap();
    assert!(help.contains("podway procedure preview <file>"), "{help}");
    assert!(help.contains("current directory"), "{help}");
    assert!(help.contains("Git worktree root"), "{help}");
}

#[test]
fn absolute_procedure_start_uses_the_relative_request_identity() {
    use podway_core::{Sha256Digest, WorkspaceId};
    use podway_protocol::{
        SessionStartCommandV1, SessionStartSourceV1, SessionStartV1, SliceCommandV1,
        canonical_direct_start_digest_with_observation_v1,
    };

    let fixture = Fixture::new();
    let source = write_hands003_procedure(&fixture);
    let noisy_relative = ".podway/./procedures//hands003.yaml";
    let noisy_absolute = format!("{}/{noisy_relative}", fixture.0.display());
    let digest = podway_presets::lookup_v3("small-change-v3")
        .unwrap()
        .base_digest;
    for path in [noisy_relative, noisy_absolute.as_str()] {
        let dry_run = fixture.run(&[
            "start",
            "--procedure",
            path,
            "--expect-procedure-digest",
            digest,
            "--task",
            HANDS003_START_TASK,
            "--dry-run",
        ]);
        assert_eq!(dry_run["result"]["procedure_digest"], digest);
        assert!(!fixture.0.join(".podway/runtime").exists());
    }
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    let relative = fixture.output(&hands003_start_args(
        noisy_relative,
        digest,
        "noisy-relative",
    ));
    let relative_error = serde_json::from_slice::<ErrorEnvelopeV2>(&relative.stdout).unwrap();
    assert_eq!(relative_error.as_value()["code"], "PROCEDURE_INVALID");
    let absolute = fixture.run(&hands003_start_args(&noisy_absolute, digest, "absolute"));
    assert_eq!(absolute["result"]["procedure_digest"], digest);
    let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", "absolute"]);
    let expected_digest = canonical_direct_start_digest_with_observation_v1(
        &SliceCommandV1::SessionStart(SessionStartCommandV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: HANDS003_PROCEDURE_FILE.to_owned(),
                },
                expected_procedure_digest: Some(Sha256Digest::new(digest.to_owned()).unwrap()),
                task_title: HANDS003_START_TASK.to_owned(),
                dry_run: false,
            },
            preconditions: None,
        }),
        &WorkspaceId::new(initialized["workspace"]["uuid"].as_str().unwrap()).unwrap(),
        &Sha256Digest::new(digest.to_owned()).unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(
        lookup["result"]["outcome"]["request_digest"],
        expected_digest.as_str()
    );
    assert_eq!(lookup["result"]["outcome"]["response"], absolute);
    fs::remove_file(source).unwrap();
    assert_eq!(
        fixture.run(&hands003_start_args(
            HANDS003_PROCEDURE_FILE,
            digest,
            "absolute"
        )),
        absolute
    );
}

#[test]
fn absolute_procedure_start_from_a_subdirectory_selects_the_named_source() {
    let fixture = Fixture::new();
    let local = fixture.0.join("nested/inner");
    fs::create_dir_all(&local).unwrap();
    let wanted = podway_presets::lookup_v3("small-change-v3").unwrap();
    let decoy = podway_presets::lookup_v3("sw-dev-v3").unwrap();
    fs::write(fixture.0.join("same.yaml"), decoy.base_yaml).unwrap();
    let selected = local.join("same.yaml");
    fs::write(&selected, wanted.base_yaml).unwrap();

    let nested_preview =
        |path: &Path| hands003_output_at(&local, &["procedure", "preview", path.to_str().unwrap()]);
    let relative = nested_preview(Path::new("same.yaml"));
    let absolute = nested_preview(&selected);
    assert!(relative.status.success(), "{relative:?}");
    assert!(absolute.status.success(), "{absolute:?}");
    let relative: Value = serde_json::from_slice(&relative.stdout).unwrap();
    let absolute: Value = serde_json::from_slice(&absolute.stdout).unwrap();
    assert_eq!(absolute["result"], relative["result"]);
    assert_eq!(absolute["result"]["file"], "same.yaml");
    assert_eq!(absolute["result"]["start_argv"][3], "same.yaml");
    assert_eq!(absolute["result"]["digest"], wanted.base_digest);
    for path in [
        fixture.0.join("same.yaml"),
        local.join("../inner/same.yaml"),
    ] {
        refuse(
            nested_preview(&path),
            "REQUEST_INVALID",
            2,
            "preview requires a safe UTF-8 worktree-relative path",
        );
    }

    // The relative dry-run remains cwd-based and store-independent.
    for path in ["same.yaml", selected.to_str().unwrap()] {
        let dry_run = hands003_output_at(
            &local,
            &[
                "start",
                "--procedure",
                path,
                "--expect-procedure-digest",
                wanted.base_digest,
                "--task",
                HANDS003_START_TASK,
                "--dry-run",
            ],
        );
        assert_eq!(dry_run.status.code(), Some(0), "{dry_run:?}");
        assert!(dry_run.stderr.is_empty(), "{dry_run:?}");
        let dry_run: Value = serde_json::from_slice(&dry_run.stdout).unwrap();
        assert_eq!(dry_run["result"]["procedure_digest"], wanted.base_digest);
        assert_eq!(dry_run["result"]["dry_run"], true);
        assert!(!fixture.0.join(".podway").exists());
    }

    fixture.run(&["init", "--idempotency-key", "init"]);
    let args = hands003_start_args(selected.to_str().unwrap(), wanted.base_digest, "nested");
    let absolute = hands003_output_at(&local, &args);
    assert_eq!(absolute.status.code(), Some(0), "{absolute:?}");
    assert!(absolute.stderr.is_empty());
    let absolute: Value = serde_json::from_slice(&absolute.stdout).unwrap();
    assert_eq!(absolute["result"]["procedure_digest"], wanted.base_digest);
    assert_eq!(
        fixture.run(&hands003_start_args(
            "nested/inner/same.yaml",
            wanted.base_digest,
            "nested"
        )),
        absolute
    );

    for worktree in [
        "nested/inner",
        "nested//inner",
        "nested/./inner",
        "nested/..",
    ] {
        let mut explicit = vec!["--worktree", worktree];
        explicit.extend(args);
        let replay = hands003_output_at(&fixture.0, &explicit);
        assert_eq!(replay.status.code(), Some(0), "{replay:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&replay.stdout).unwrap(),
            absolute
        );
    }

    let outside = Fixture::new();
    let alias = outside.0.join("worktree-alias");
    symlink(&fixture.0, &alias).unwrap();
    let aliased = alias.join("nested/inner/same.yaml");
    let aliased_subdirectory = format!("{}/nested/./inner", alias.display());
    for (worktree, source) in [
        (alias.to_str().unwrap(), &aliased),
        (alias.to_str().unwrap(), &selected),
        (aliased_subdirectory.as_str(), &aliased),
    ] {
        let mut explicit = vec!["--worktree", worktree];
        explicit.extend(hands003_start_args(
            source.to_str().unwrap(),
            wanted.base_digest,
            "nested",
        ));
        let replay = hands003_output_at(&outside.0, &explicit);
        assert_eq!(replay.status.code(), Some(0), "{replay:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&replay.stdout).unwrap(),
            absolute
        );
    }
}

#[test]
fn absolute_procedure_refusals_preserve_path_admission_and_state() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let original = write_hands003_procedure(&fixture);
    let other = write_hands003_procedure(&outside);
    let digest = podway_presets::lookup_v3("small-change-v3")
        .unwrap()
        .base_digest;
    fixture.run(&["init", "--idempotency-key", "init"]);
    let subdirectory = fixture.0.join("nested");
    fs::create_dir(&subdirectory).unwrap();
    for directory in [&fixture.0, &subdirectory] {
        symlink(original.parent().unwrap(), directory.join("linked")).unwrap();
    }
    let snapshot = || {
        let mut snapshot = fixture.podway_snapshot();
        // SQLite read admission can update volatile reader locks in SHM.
        for (path, bytes, _, _) in &mut snapshot {
            if path.ends_with("state.sqlite3-shm")
                && let Some(bytes) = bytes
            {
                bytes.clear();
            }
        }
        snapshot
    };
    let before = snapshot();
    let procedure_name = original.file_name().unwrap();
    let linked = Path::new("linked").join(procedure_name);
    let parent = original
        .parent()
        .unwrap()
        .join("../procedures")
        .join(procedure_name);
    for path in [&other, &parent] {
        let path = path.to_str().unwrap();
        for output in [
            fixture.output(&["procedure", "preview", path]),
            fixture.output(&hands003_start_args(path, digest, "rejected-path")),
        ] {
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout).unwrap();
            assert_eq!(error.as_value()["code"], "REQUEST_INVALID");
        }
    }
    let absolute_link = fixture.0.join(&linked);
    let with_parent_root = original.parent().unwrap().join("../..");
    let path_with_parent_root = with_parent_root.join(HANDS003_PROCEDURE_FILE);
    let mut matching_parent_args = vec!["--worktree", with_parent_root.to_str().unwrap()];
    matching_parent_args.extend(hands003_start_args(
        path_with_parent_root.to_str().unwrap(),
        digest,
        "rejected-path",
    ));
    let output = hands003_output_at(&fixture.0, &matching_parent_args);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "REQUEST_INVALID");
    for (relative, absolute) in [
        (
            fixture.output(&["procedure", "preview", linked.to_str().unwrap()]),
            fixture.output(&["procedure", "preview", absolute_link.to_str().unwrap()]),
        ),
        (
            fixture.output(&hands003_start_args(
                linked.to_str().unwrap(),
                digest,
                "rejected-link",
            )),
            fixture.output(&hands003_start_args(
                absolute_link.to_str().unwrap(),
                digest,
                "rejected-link",
            )),
        ),
    ] {
        assert!(!relative.status.success());
        assert_eq!(absolute.status, relative.status, "{absolute:?}");
        let relative = serde_json::from_slice::<ErrorEnvelopeV2>(&relative.stdout).unwrap();
        let absolute = serde_json::from_slice::<ErrorEnvelopeV2>(&absolute.stdout).unwrap();
        for field in ["code", "message", "retryable", "details"] {
            assert_eq!(absolute.as_value()[field], relative.as_value()[field]);
        }
    }
    for (alias_name, target) in [
        ("worktree-alias", &fixture.0),
        ("subdirectory-alias", &subdirectory),
    ] {
        let alias = outside.0.join(alias_name);
        symlink(target, &alias).unwrap();
        let aliased_selection = alias.join("linked");
        let aliased_source = aliased_selection.join(procedure_name);
        let mut explicit = vec!["--worktree", aliased_selection.to_str().unwrap()];
        explicit.extend(hands003_start_args(
            aliased_source.to_str().unwrap(),
            digest,
            "rejected-link",
        ));
        let output = hands003_output_at(&outside.0, &explicit);
        let target_source = target.join(&linked);
        let relative = target_source.strip_prefix(&fixture.0).unwrap();
        let relative = fixture.output(&hands003_start_args(
            relative.to_str().unwrap(),
            digest,
            "rejected-link",
        ));
        assert!(!relative.status.success(), "{relative:?}");
        assert_eq!(output.status, relative.status, "{alias_name}: {output:?}");
        let relative = serde_json::from_slice::<ErrorEnvelopeV2>(&relative.stdout).unwrap();
        let absolute = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout).unwrap();
        for field in ["code", "message", "retryable", "details"] {
            assert_eq!(absolute.as_value()[field], relative.as_value()[field]);
        }
    }
    for key in ["rejected-path", "rejected-link"] {
        let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", key]);
        assert_eq!(lookup["result"]["found"], false);
    }
    assert_eq!(snapshot(), before);
}

#[test]
fn scaffold_v3_admits_a_distinct_custom_procedure() {
    let fixture = Fixture::new();
    let scaffold = fixture.run(&["procedure", "scaffold"]);
    let document = scaffold["result"]["document"].as_str().unwrap();
    assert!(
        document
            .lines()
            .any(|line| line == "id: custom-procedure-v3")
    );
    fs::write(fixture.0.join("scaffold.yaml"), document).unwrap();
    let check = fixture.run(&[
        "procedure",
        "check",
        "scaffold.yaml",
        "--warnings-as-errors",
    ]);
    assert_eq!(check["result"]["digest"], scaffold["result"]["digest"]);
    let preview = fixture.run(&["procedure", "preview", "scaffold.yaml"]);
    let graph = fixture.run(&["procedure", "graph", "scaffold.yaml", "--format", "mermaid"]);
    assert_eq!(preview["result"]["digest"], scaffold["result"]["digest"]);
    assert_eq!(preview["result"]["document_id"], "custom-procedure-v3");
    assert_eq!(preview["result"]["admissible"], true);
    assert_eq!(graph["result"]["digest"], scaffold["result"]["digest"]);
    assert_eq!(graph["result"]["format"], "mermaid");
    assert_eq!(graph["result"]["projection"], preview["result"]["mermaid"]);
    assert!(
        graph["result"]["projection"]
            .as_str()
            .unwrap()
            .starts_with("flowchart"),
        "{graph:?}"
    );
    for preset in podway_presets::list_v3() {
        assert_ne!(scaffold["result"]["digest"], preset.base_digest);
        assert_ne!(scaffold["result"]["digest"], preset.group_digest);
    }
    for preset in podway_presets::list() {
        assert_ne!(scaffold["result"]["digest"], preset.shipped_digest);
    }
}

const LEGACY_STORE_15_WRITER_SHA256: &str =
    "c9923c6c1fee8c31e595c8163863051ebc5d62ae651c3a947b8662cd3aa927a7";
const LEGACY_STATUS_15_JSON: &str = concat!(
    r#"{"code":"LEGACY_RUNTIME_STATE_UNSUPPORTED","command":"session.status","details":{"markers":[],"recovery":{"action":"reset_legacy_state","argv":["podway","--json","reset","--all","--yes","--idempotency-key","<new-idempotency-key>"],"command":"workspace.reset_all","reason":"Replace <new-idempotency-key> with a fresh caller-owned key, then explicitly reset unsupported older runtime state.","requires_explicit_authorization":true},"schema":"podway.legacy-runtime-state-unsupported-details/v2","store_schema_version":15},"exit_code":5,"generated_at":"<generated_at>","message":"Older workspace state requires an explicitly confirmed reset before initializing a current Store.","request_id":"<request_id>","retryable":false,"schema":"podway.error/v2"}"#,
    "\n",
);

#[test]
fn authentic_store_15_preserves_json_and_adds_human_history_guidance() {
    let Some(writer) = std::env::var_os("PODWAY_TEST_LEGACY_V017_BINARY") else {
        eprintln!("Authentic Store 15 case requires PODWAY_TEST_LEGACY_V017_BINARY.");
        return;
    };
    let writer = PathBuf::from(writer);
    assert_eq!(writer.canonicalize().unwrap(), writer);
    assert!(fs::symlink_metadata(&writer).unwrap().is_file());
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&writer).unwrap())),
        LEGACY_STORE_15_WRITER_SHA256
    );

    let producer = Fixture::new();
    for args in [
        &["init", "--idempotency-key", "init"][..],
        &[
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "HANDS-002 authentic Store fixture",
            "--idempotency-key",
            "start",
        ],
    ] {
        let child = Command::new(&writer)
            .arg("--worktree")
            .arg(&producer.0)
            .args(["--contract-generation", "2", "--json"])
            .args(args)
            .current_dir(&producer.0)
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = bounded_output(GuardedChild::new(child), args);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    }
    let producer_before = producer.podway_snapshot();

    // The stopped native writer can leave transient SHM behind. Build the
    // no-SHM witness from the closed producer's unchanged file bytes.
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join(".podway/runtime")).unwrap();
    for (path, bytes, _, _) in &producer_before {
        let Some(bytes) = bytes else { continue };
        if path.ends_with("state.sqlite3-shm") {
            continue;
        }
        let source = producer.0.join(".podway").join(path);
        let destination = fixture.0.join(".podway").join(path);
        fs::copy(source, &destination).unwrap();
        assert_eq!(&fs::read(destination).unwrap(), bytes, "{path:?}");
    }
    let runtime = fixture.0.join(".podway/runtime");
    let store = fs::read(runtime.join("state.sqlite3")).unwrap();
    assert_eq!(u32::from_be_bytes(store[60..64].try_into().unwrap()), 15);
    assert!(!runtime.join("state.sqlite3-shm").exists());
    let before = fixture.podway_snapshot();
    for args in [
        &["--json", "status"][..],
        &["--contract-generation", "2", "--json", "status"],
    ] {
        let output = fixture.raw_output(args);
        assert_eq!(output.status.code(), Some(5), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        let envelope = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout).unwrap();
        let mut actual = String::from_utf8(output.stdout).unwrap();
        for field in ["request_id", "generated_at"] {
            let value = envelope.as_value()[field].as_str().unwrap();
            actual = actual.replace(
                &format!(r#""{field}":"{value}""#),
                &format!(r#""{field}":"<{field}>""#),
            );
        }
        assert_eq!(actual, LEGACY_STATUS_15_JSON, "{args:?}");
        fixture.assert_podway_snapshot(&before);
        assert!(!runtime.join("state.sqlite3-shm").exists());
    }
    let human = fixture.raw_output(&["status"]);
    assert_eq!(human.status.code(), Some(5), "{human:?}");
    assert!(human.stdout.is_empty(), "{human:?}");
    fixture.assert_podway_snapshot(&before);
    assert!(!runtime.join("state.sqlite3-shm").exists());
    producer.assert_podway_snapshot(&producer_before);
    assert_eq!(
        String::from_utf8(human.stderr).unwrap(),
        concat!(
            "error: LEGACY_RUNTIME_STATE_UNSUPPORTED: Older workspace state requires an explicitly confirmed reset before initializing a current Store.\n",
            "reason: Replace <new-idempotency-key> with a fresh caller-owned key, then explicitly reset unsupported older runtime state.\n",
            "hint: stored Store generation: 15\n",
            "hint: to keep history, finish or abandon the session with its original writer: Podway v0.2.13 through v0.2.17\n",
            "hint: with a verified v0.2.15-v0.2.17 binary, --contract-generation 2 --json observe may name the writer release; otherwise use retained provenance\n",
            "hint: reset --all destroys current and retained history and requires explicit authorization\n",
        )
    );
    println!("Verified authentic v0.2.17 Store 15 in a no-SHM witness.");
}

fn refuse(output: Output, code: &str, exit: i32, message: &str) {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout).unwrap();
    let error = error.as_value();
    assert_eq!(error["code"], code);
    assert_eq!(error["exit_code"], exit);
    assert_eq!(error["retryable"], exit == 4);
    assert_eq!(error["details"], json!({}));
    assert_eq!(error["message"], message);
}

fn observe(fixture: &Fixture) -> Value {
    fixture.run(&["--contract-generation", "2", "observe"])
}

fn template(fixture: &Fixture, command: &str, key: &str) -> Value {
    let observation = observe(fixture);
    let template = observation["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == command)
        .unwrap();
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!(key);
    input["working_copy_basis"] = json!({"descriptor":"Owned test fixture","digest":BASIS_DIGEST});
    input
}

fn small_change() -> Fixture {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Exercise HANDS refusals",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "set",
        "scope-summary",
        "Record fixture scope.",
        "--idempotency-key",
        "scope",
    ]);
    fixture
}

fn activate(fixture: &Fixture, input: &Value) -> Output {
    fixture.output_with_stdin(ACTIVATE, input.to_string().as_bytes())
}

fn activate_first(fixture: &Fixture) -> Value {
    let output = activate(
        fixture,
        &template(fixture, "group.activate", "activate-first"),
    );
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn group_review(fixture: &Fixture, verdict: &str, stage: &str, score: &str, exit: &str) {
    fn set(fixture: &Fixture, item: &str, text: &str) {
        fixture.run(&["set", item, text, "--idempotency-key", item]);
    }
    fn complete(fixture: &Fixture, key: &str) {
        fixture.run(&["complete", "--idempotency-key", key]);
    }
    set(
        fixture,
        "implementation-summary",
        "Bounded native fixture work.",
    );
    complete(fixture, "complete-implement");
    set(
        fixture,
        "verification-command",
        "Record fixture verification.",
    );
    set(fixture, "verification-exit-status", exit);
    complete(fixture, "complete-verify");
    set(fixture, "assessment-verdict", verdict);
    set(fixture, "assessment-stage", stage);
    set(fixture, "assessment-score", score);
    if stage == "scored" {
        set(
            fixture,
            "scored-basis",
            "Exercise the scored replan route in an isolated fixture.",
        );
    }
    complete(fixture, "complete-assessment");
}

const STATUS_HINT: &str =
    "hint: use --json for the complete result, or podway status --verbose for the full human dump";

fn assert_human_status_lines(fixture: &Fixture, expected: &[&str]) {
    for generation in ["1", "2"] {
        for route in ["status", "next"] {
            let output = fixture.raw_output(&["--contract-generation", generation, route]);
            assert_eq!(
                output.status.code(),
                Some(0),
                "{generation} {route}: {output:?}"
            );
            assert!(output.stderr.is_empty());
            let text = String::from_utf8(output.stdout).unwrap();
            let mut lines = text.lines();
            assert!(lines.next().unwrap().starts_with("workspace: "));
            assert!(lines.next().unwrap().starts_with("session: "));
            let lines: Vec<_> = lines.collect();
            assert_eq!(lines, expected, "{generation} {route}");
        }
    }
}

#[test]
fn human_status_and_next_use_ordered_scoped_summaries() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Human status lines",
        "--idempotency-key",
        "start",
    ]);
    assert_human_status_lines(&fixture, &["lifecycle: prepared", STATUS_HINT]);

    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "block",
        "--reason",
        "Wait for the fixture.",
        "--idempotency-key",
        "block",
    ]);
    assert_human_status_lines(
        &fixture,
        &[
            "lifecycle: running",
            "attempt: base inspect",
            "ready: false",
            "missing items: scope-summary",
            "open blockers: 1",
            "operations: session.block, session.cancel, session.retry, session.unblock, item.clear, item.record_many, item.set, group.activate",
            STATUS_HINT,
        ],
    );

    fixture.run(&["unblock", "--all", "--idempotency-key", "unblock"]);
    fixture.run(&[
        "set",
        "scope-summary",
        "Record the fixture scope.",
        "--idempotency-key",
        "scope",
    ]);
    assert_human_status_lines(
        &fixture,
        &[
            "lifecycle: running",
            "attempt: base inspect",
            "ready: true",
            "operations: session.block, session.cancel, session.retry, item.clear, item.record_many, item.set, group.activate",
            STATUS_HINT,
        ],
    );
    let activation = activate_first(&fixture);
    let execution = activation["result"]["execution_id"].as_str().unwrap();
    let attempt = format!("attempt: group implement group {execution}");
    assert_human_status_lines(
        &fixture,
        &[
            "lifecycle: running",
            &attempt,
            "ready: false",
            "missing items: implementation-summary",
            "operations: session.block, session.cancel, session.retry, session.rework, item.clear, item.record_many, item.set, session.complete",
            STATUS_HINT,
        ],
    );
    for generation in ["1", "2"] {
        let scoped_schema = if generation == "1" {
            "podway.observation-result/v7"
        } else {
            "podway.observation-result/v8"
        };
        for route in ["status", "next"] {
            let quiet =
                fixture.raw_output(&["--contract-generation", generation, route, "--quiet"]);
            assert!(quiet.status.success() && quiet.stdout.is_empty() && quiet.stderr.is_empty());
            let json = fixture.run(&["--contract-generation", generation, route]);
            assert_eq!(json["result"]["schema"], "podway.observation-result/v7");
        }
        for (route, schema) in [
            (
                &["status", "--verbose"][..],
                "podway.scoped-verbose-status-result/v1",
            ),
            (
                &["status", "--compact"][..],
                "podway.scoped-compact-status-result/v1",
            ),
            (&["observe"][..], scoped_schema),
        ] {
            let mut args = vec!["--contract-generation", generation];
            args.extend_from_slice(route);
            let human = fixture.raw_output(&args);
            assert_eq!(human.status.code(), Some(0), "{human:?}");
            assert!(human.stderr.is_empty());
            let human = String::from_utf8(human.stdout).unwrap();
            let full = human.split_once("\nresult: ").unwrap().1;
            let json = fixture.run(&args);
            assert_eq!(serde_json::from_str::<Value>(full).unwrap(), json["result"]);
            assert_eq!(json["result"]["schema"], schema);
        }
        refuse(
            fixture.output(&["--contract-generation", generation, "next", "--verbose"]),
            "REQUEST_INVALID",
            2,
            "--verbose applies only to status",
        );
    }

    group_review(&fixture, "success", "not-scored", "0", "0");
    fixture.run(&[
        "decide",
        "--option",
        "ready",
        "--reason",
        "Fixture work is ready.",
        "--idempotency-key",
        "ready",
    ]);
    for (item, value) in [
        ("implementation-summary", "Return current fixture work."),
        (
            "verification-command",
            "Record successful fixture verification.",
        ),
        ("verification-exit-status", "0"),
    ] {
        fixture.run(&[
            "set",
            item,
            value,
            "--idempotency-key",
            &format!("return-{item}"),
        ]);
    }
    fixture.run(&["group", "return", "--idempotency-key", "return"]);
    fixture.run(&[
        "set",
        "closeout-note",
        "Finish the owned fixture.",
        "--idempotency-key",
        "closeout",
    ]);
    fixture.run(&["complete", "--idempotency-key", "finish"]);
    assert_human_status_lines(&fixture, &["lifecycle: completed", STATUS_HINT]);
}

fn replacement_fixture() -> Fixture {
    let fixture = small_change();
    activate_first(&fixture);
    group_review(&fixture, "failure", "scored", "80", "1");
    let mut replan = template(&fixture, "group.replan", "replan");
    replan["reason"] = json!("Exercise replacement activation refusals.");
    replan["feedback"] = json!("Revisit the isolated fixture scope.");
    let output = fixture.output_with_stdin(
        &["group", "replan", "--stdin"],
        replan.to_string().as_bytes(),
    );
    assert!(output.status.success(), "{output:?}");
    fixture.run(&[
        "set",
        "scope-summary",
        "Record the replacement scope.",
        "--idempotency-key",
        "scope-revised",
    ]);
    fixture
}

fn assert_no_outcome(fixture: &Fixture, key: &str) {
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["found"],
        false
    );
}

#[test]
fn hands001_blank_reasons_name_the_cli_option() {
    let fixture = small_change();
    let before = observe(&fixture);
    for route in ["block", "retry", "cancel"] {
        refuse(
            fixture.output(&[route, "--reason", "   ", "--idempotency-key", route]),
            "REQUEST_INVALID",
            2,
            "--reason must contain a non-whitespace character",
        );
        assert_no_outcome(&fixture, route);
    }
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_stdin_reports_the_replacement_placeholder_and_pointer() {
    let fixture = replacement_fixture();
    let input = template(&fixture, "group.activate", "unfilled-prior-work");
    assert_eq!(input["prior_work"], "<prior-work-dispositions-array>");
    let before = observe(&fixture);
    refuse(
        activate(&fixture, &input),
        "REQUEST_INVALID",
        2,
        "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<prior-work-dispositions-array>\" at /prior_work",
    );
    assert_no_outcome(&fixture, "unfilled-prior-work");
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_stdin_pointer_preserves_json_and_escapes_human_output() {
    let fixture = Fixture::new();
    let diagnostic = "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\"";
    for (key, escaped) in [
        ("row\nname", r"row\nname"),
        (r"row\nname", r"row\\nname"),
        ("row\u{1b}[31m", r"row\u{1b}[31m"),
    ] {
        let document = json!({
            "schema": "podway.group-activate-input/v1",
            (key): "<first>",
        });
        let input = document.to_string();
        let output = fixture.human_output_with_stdin(ACTIVATE, input.as_bytes());
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("error: {diagnostic} at /{escaped}\n"),
        );
        refuse(
            activate(&fixture, &document),
            "REQUEST_INVALID",
            2,
            &format!("{diagnostic} at /{key}"),
        );
    }
}

#[test]
fn hands001_stdin_pointer_overflow_keeps_the_same_refusal_class() {
    let fixture = Fixture::new();
    let key = "x".repeat(65_536);
    let document = json!({
        "schema": "podway.group-activate-input/v1",
        (key): "<first>"
    });
    refuse(
        activate(&fixture, &document),
        "REQUEST_INVALID",
        2,
        "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\" (RFC 6901 pointer exceeds the error message length limit)",
    );
}

#[test]
fn hands001_human_pointer_escape_expansion_stays_bounded() {
    let fixture = Fixture::new();
    let diagnostic = "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\"";
    for key in ["\u{1b}".repeat(20_000), "\\".repeat(40_000)] {
        let document = json!({
            "schema": "podway.group-activate-input/v1",
            (&key): "<first>",
        });
        let input = document.to_string();
        let output = fixture.human_output_with_stdin(ACTIVATE, input.as_bytes());
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        let length = stderr.chars().count();
        assert!(
            length <= 65_536,
            "human diagnostic output was {length} scalars"
        );
        assert_eq!(
            stderr,
            format!(
                "error: {diagnostic} (escaped RFC 6901 pointer exceeds the error message length limit)\n"
            ),
        );
        refuse(
            activate(&fixture, &document),
            "REQUEST_INVALID",
            2,
            &format!("{diagnostic} at /{key}"),
        );
    }
}

#[test]
fn hands001_native_unknown_criterion_refusal_leaves_no_state_or_outcome() {
    let (fixture, before, _) = super::e2e_adopt001_scoped_cli::scoped_criterion_fixture(true);
    let template = before["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == "goal.assess_criteria")
        .unwrap();
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!("unknown-criterion");
    input["actor"] = Value::Null;
    input["assessments"] = json!([{
        "criterion": "unknown",
        "status": "satisfied",
        "reason": "Exercise the unknown criterion refusal.",
        "citations": []
    }]);
    refuse(
        fixture.output_with_stdin(
            &["goal", "assess-criteria", "--stdin"],
            input.to_string().as_bytes(),
        ),
        "REQUEST_INVALID",
        2,
        &format!("{REQUEST_INVALID} (criterion assessment repeats or names an unknown criterion)."),
    );
    assert_no_outcome(&fixture, "unknown-criterion");
    let after = observe(&fixture);
    assert_eq!(after["result"], before["result"]);
    assert_eq!(after["workspace"], before["workspace"]);
}

#[test]
fn hands001_native_off_anchor_activation_refusal_leaves_no_state_or_outcome() {
    let fixture = small_change();
    let mut input = template(&fixture, "group.activate", "off-anchor");
    activate_first(&fixture);
    let before = observe(&fixture);
    assert!(
        before["result"]["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["command"] != "group.activate")
    );
    let mut cursor = before["result"]["cursor"].clone();
    cursor.as_object_mut().unwrap().remove("session");
    input["context"]["cursor"] = cursor;
    refuse(
        activate(&fixture, &input),
        "GROUP_ACTIVATION_NOT_ALLOWED",
        1,
        &format!(
            "{GROUP_ACTIVATION} The current attempt is not the unselected base anchor; observe and use the current group.activate template."
        ),
    );
    assert_no_outcome(&fixture, "off-anchor");
    let after = observe(&fixture);
    assert_eq!(after["result"], before["result"]);
    assert_eq!(after["workspace"], before["workspace"]);
}

#[test]
fn hands001_base_history_requires_the_group_execution_for_retained_group_digests() {
    let fixture = small_change();
    let activated = activate_first(&fixture);
    let execution = activated["result"]["execution_id"].as_str().unwrap();
    let selected = observe(&fixture);
    let digest = selected["result"]["selection"]["definition_digest"]
        .as_str()
        .unwrap();
    group_review(&fixture, "success", "not-scored", "0", "0");
    fixture.run(&[
        "decide",
        "--option",
        "ready",
        "--reason",
        "Fixture verification passed.",
        "--idempotency-key",
        "ready",
    ]);
    for (item, value) in [
        ("implementation-summary", "Return current fixture work."),
        (
            "verification-command",
            "Record successful fixture verification.",
        ),
        ("verification-exit-status", "0"),
    ] {
        fixture.run(&[
            "set",
            item,
            value,
            "--idempotency-key",
            &format!("return-{item}"),
        ]);
    }
    fixture.run(&["group", "return", "--idempotency-key", "return"]);
    let before = observe(&fixture);
    let session = before["session"]["id"].as_str().unwrap();
    for generation in ["1", "2"] {
        for route in ["dag", "attempts"] {
            let mut args = vec![
                "--contract-generation",
                generation,
                "history",
                route,
                "--session",
                session,
                "--definition-digest",
                digest,
            ];
            refuse(
                fixture.output(&args),
                "REQUEST_INVALID",
                2,
                &format!(
                    "{REQUEST_INVALID} --definition-digest names a retained group definition; add --group-execution <uuid> from group list."
                ),
            );
            *args.last_mut().unwrap() = OTHER_DIGEST;
            refuse(
                fixture.output(&args),
                "GROUP_DEFINITION_MISMATCH",
                4,
                GROUP_MISMATCH,
            );
            *args.last_mut().unwrap() = digest;
            args.extend(["--group-execution", execution]);
            fixture.run(&args);
        }
    }
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_wrong_activation_digest_reports_admitted_source_identity() {
    let fixture = small_change();
    let mut input = template(&fixture, "group.activate", "wrong-definition");
    let digest = input["expected_definition"].as_str().unwrap().to_owned();
    input["expected_definition"] = json!(OTHER_DIGEST);
    let before = observe(&fixture);
    refuse(
        activate(&fixture, &input),
        "GROUP_DEFINITION_MISMATCH",
        4,
        &format!("{GROUP_MISMATCH} Supplied source digest: {digest}."),
    );
    assert_no_outcome(&fixture, "wrong-definition");
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_replacement_refuses_empty_and_unknown_prior_work_with_distinct_reasons() {
    let fixture = replacement_fixture();
    let mut input = template(&fixture, "group.activate", "empty-prior-work");
    input["prior_work"] = json!([]);
    let before = observe(&fixture);
    refuse(
        activate(&fixture, &input),
        "GROUP_ACTIVATION_NOT_ALLOWED",
        1,
        &format!(
            "{GROUP_ACTIVATION} Prior work is invalid (activation history requires one to 128 prior-work dispositions after the first group)."
        ),
    );
    assert_no_outcome(&fixture, "empty-prior-work");
    input["context"]["idempotency_key"] = json!("unknown-prior-work");
    input["prior_work"] = json!([{
        "group_execution_id":"00000000-0000-4000-8000-000000000012",
        "choice":"revalidate","rationale":"Exercise an unknown fixture execution."
    }]);
    refuse(
        activate(&fixture, &input),
        "GROUP_ACTIVATION_NOT_ALLOWED",
        1,
        &format!(
            "{GROUP_ACTIVATION} Prior work is invalid (prior work does not identify a retained same-session group)."
        ),
    );
    assert_no_outcome(&fixture, "unknown-prior-work");
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn hands001_removal_reports_every_missing_confirmation_in_order() {
    let fixture = Fixture::new();
    let init = fixture.run(&["init", "--idempotency-key", "init"]);
    let workspace = init["workspace"]["uuid"].as_str().unwrap();
    let before = fixture.podway_snapshot();
    let prefix = "JSON workspace removal requires --force, --yes and --if-workspace-uuid; missing:";
    for (flags, missing) in [
        (vec![], "--force, --yes, --if-workspace-uuid"),
        (vec!["--force"], "--yes, --if-workspace-uuid"),
        (vec!["--yes"], "--force, --if-workspace-uuid"),
        (vec!["--if-workspace-uuid", workspace], "--force, --yes"),
        (vec!["--force", "--yes"], "--if-workspace-uuid"),
        (vec!["--force", "--if-workspace-uuid", workspace], "--yes"),
        (vec!["--yes", "--if-workspace-uuid", workspace], "--force"),
    ] {
        let mut args = vec![
            "workspace",
            "remove",
            "--idempotency-key",
            "refused-removal",
        ];
        args.extend(flags);
        refuse(
            fixture.output(&args),
            "REQUEST_INVALID",
            2,
            &format!("{prefix} {missing}"),
        );
    }
    let prefix =
        "non-interactive workspace removal requires --yes and --if-workspace-uuid; missing:";
    for (flags, missing) in [
        (vec![], "--yes, --if-workspace-uuid"),
        (vec!["--yes"], "--if-workspace-uuid"),
        (vec!["--if-workspace-uuid", workspace], "--yes"),
    ] {
        let mut args = vec![
            "workspace",
            "remove",
            "--idempotency-key",
            "refused-removal",
        ];
        args.extend(flags);
        let output = fixture.raw_output(&args);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr, format!("error: {prefix} {missing}\n"));
    }
    assert_eq!(fixture.podway_snapshot(), before);
    assert_no_outcome(&fixture, "refused-removal");
}
