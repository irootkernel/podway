//! Retained offline authoring, help, and completion contracts through the real CLI.

use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4, decode_result_schema_contract_v2};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Output};

struct Fixture {
    root: PathBuf,
    cli: super::cli_fixture::Fixture,
}

impl Fixture {
    fn new() -> Self {
        let cli = super::cli_fixture::Fixture::new();
        Self {
            root: cli.0.clone(),
            cli,
        }
    }

    fn run(&self, arguments: &[String]) -> Output {
        let argv: Vec<_> = arguments.iter().map(String::as_str).collect();
        let output = self.cli.raw_output(&argv);
        assert!(
            !self.root.join(".podway").exists(),
            "offline commands must not initialize workspace state"
        );
        output
    }

    fn run_from_root(&self, arguments: &[String]) -> Output {
        self.run(arguments)
    }
}

fn one_json(output: &Output) -> Value {
    assert!(output.stdout.ends_with(b"\n"));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    match value["schema"].as_str().unwrap() {
        "podway.output/v4" => OutputEnvelopeV4::from_value(value).unwrap().into_value(),
        "podway.error/v2" => serde_json::from_value::<ErrorEnvelopeV2>(value)
            .unwrap()
            .into_value(),
        schema => panic!("unexpected direct contract generation: {schema}"),
    }
}

#[test]
fn v2dfc011_json_write_failure_returns_a_valid_internal_error() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let directory = fixture.root.join("read-only");
    fs::create_dir(&directory).unwrap();
    // Use valid but noncanonical YAML so formatting reaches the write boundary.
    let scaffold = fixture.run(&["procedure".into(), "scaffold".into()]);
    assert!(scaffold.status.success());
    let source = String::from_utf8(scaffold.stdout)
        .unwrap()
        .replace("version: \"1\"", "version: '1'");
    fs::write(directory.join("procedure.yaml"), source).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
    let output = fixture.run(&[
        "procedure".into(),
        "format".into(),
        "read-only/procedure.yaml".into(),
        "--write".into(),
        "--json".into(),
    ]);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let error = one_json(&output);
    assert_eq!(error["code"], "INTERNAL_ERROR");
    assert_eq!(
        error["details"]["schema"],
        "podway.internal-error-details/v2"
    );
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn v2dfc011_deleted_current_directory_returns_a_valid_internal_error() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, process::CommandExt},
        process::{Command, Stdio},
    };

    let fixture = Fixture::new();
    let directory = fixture.root.join("deleted-cwd");
    fs::create_dir(&directory).unwrap();
    let path = CString::new(directory.as_os_str().as_bytes()).unwrap();
    let mut command = Command::new(super::cli_fixture::podway_binary());
    command
        .args(["--json", "status"])
        .current_dir(&directory)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Only the async-signal-safe syscall runs between fork and exec. The parent
    // keeps its working directory; the fixture owns this empty directory.
    unsafe {
        command.pre_exec(move || {
            if nix::libc::rmdir(path.as_ptr()) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let output = super::cli_fixture::bounded_output(
        super::cli_fixture::GuardedChild::new(command.spawn().unwrap()),
        &["deleted cwd"],
    );
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert_eq!(one_json(&output)["code"], "INTERNAL_ERROR");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn v2dfc011_late_json_usage_errors() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["unknown", "--json"],
        vec!["status", "--unknown", "--json"],
        vec!["status", "-x", "--json"],
    ] {
        let output = fixture.cli.raw_output(&arguments);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert_eq!(one_json(&output)["code"], "REQUEST_INVALID");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    for arguments in [
        vec!["unknown", "--", "--json"],
        vec!["status", "--unknown", "--", "--json"],
        vec!["status", "-x", "--", "--json"],
        vec!["status", "--unknown", "--idempotency-key", "--json"],
    ] {
        let output = fixture.cli.raw_output(&arguments);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(!output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn v2dfc011_retired_family_help_and_catalog_identity() {
    let fixture = Fixture::new();
    for topic in [
        "daemon",
        "runtime",
        "runtime.reset",
        "workspace.mode",
        "job",
    ] {
        let output = fixture.cli.output(&["help", topic]);
        assert!(output.status.success(), "{topic}: {output:?}");
        assert!(
            one_json(&output)["result"]["text"]
                .as_str()
                .unwrap()
                .contains("retired")
        );
    }
    let table: Value = serde_json::from_str(include_str!(
        "../../../assets/specifications/retired-commands-v1.json"
    ))
    .unwrap();
    for arguments in [
        vec!["daemon", "mystery"],
        vec!["runtime", "reset", "mystery"],
        vec!["workspace", "mode", "mystery"],
    ] {
        let output = fixture.cli.output(&arguments);
        let error = one_json(&output);
        assert_eq!(error["code"], "COMMAND_RETIRED");
        assert!(
            table["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["command"] == error["details"]["retired"])
        );
    }
}

#[test]
fn v2dfc003_reserved_output_preserves_large_admitted_procedure_show() {
    let fixture = Fixture::new();
    let definitions = (0..128)
        .map(|index| {
            (
                format!("d{index:04}"),
                json!({
                    "type": "action",
                    "title": "Work",
                    "description": "a".repeat(1_000),
                    "intent": "Record a value.",
                    "items": [{"id": "value", "type": "text", "prompt": "Value?", "required": false}],
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let source = json!({
        "schema": "podway.procedure/v2",
        "id": "large-projection",
        "version": "1",
        "name": "Large projection",
        "purpose": "Preserve the admitted canonical projection in a generic result.",
        "node_definitions": definitions,
        "graph": {
            "entry": "work",
            "nodes": [{"id": "work", "use": "d0000", "terminal": true}],
        },
    });
    let path = fixture.root.join("large-projection.json");
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let output = fixture.run(&[
        "--json".to_owned(),
        "procedure".to_owned(),
        "show".to_owned(),
        path.display().to_string(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v4");
    assert_eq!(envelope["command"], "procedure.show");
    let canonical = envelope["result"]["canonical_json"].as_str().unwrap();
    assert!(canonical.len() > 65_536);
    podway_core::verify_canonical_json_v1(canonical.as_bytes()).unwrap();
    assert!(output.stdout.len() < podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1);
    assert!(podway_protocol::validate_daemon_free_document_v1(
        "podway.output/v4",
        &envelope
    ));
}

#[test]
fn v2grd006_procedure_preview_preserves_typed_guards_in_schema_valid_json() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("typed-guards.yaml"),
        r#"schema: podway.procedure/v2
id: typed-preview
version: "1"
name: Typed preview
purpose: Preserve typed conditions in the local preview.
node_definitions:
  collect:
    type: action
    title: Collect
    intent: Record typed evidence.
    items:
      - id: mode
        type: choice
        prompt: Select the mode.
        required: true
        choices: [strict, relaxed]
      - id: notes
        type: text
        prompt: Record strict-mode notes.
        required: false
        required_when:
          - item: mode
            equals: strict
  decide:
    type: decision
    title: Decide
    objective: Select an evidence-backed outcome.
    prompt: Which outcome applies?
    options:
      - id: noted
        label: Notes recorded
        criteria: Strict-mode notes contain at least one scalar.
        guards:
          - evidence:
              node: collect
              item: notes
            non_empty: true
      - id: continue
        label: Continue without notes
        criteria: No strict-mode notes are required for this outcome.
    reason:
      required: true
      prompt: Explain the selected outcome.
  finish:
    type: action
    title: Finish
    intent: Finish the preview workflow.
graph:
  entry: collect
  nodes:
    - id: collect
      use: collect
      next: decide
    - id: decide
      use: decide
      evidence_from:
        - node: collect
          required: true
          items: [notes]
      routes:
        noted:
          to: finish
          effect: advance
        continue:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
"#,
    )
    .unwrap();

    let output = fixture.run_from_root(&[
        "--json".to_owned(),
        "procedure".to_owned(),
        "preview".to_owned(),
        "typed-guards.yaml".to_owned(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v4");
    assert_eq!(envelope["command"], "procedure.preview");
    let result = envelope["result"].as_object().unwrap();
    assert_eq!(result["schema"], "podway.procedure-preview-result/v1");
    assert!(decode_result_schema_contract_v2(result).is_some());
    let direct_graph = fixture.run_from_root(&[
        "--json".to_owned(),
        "procedure".to_owned(),
        "graph".to_owned(),
        "typed-guards.yaml".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ]);
    assert!(direct_graph.status.success(), "{direct_graph:?}");
    let direct_graph = one_json(&direct_graph);
    assert_eq!(
        direct_graph["result"]["schema"],
        "podway.procedure-graph-result/v1"
    );
    assert!(direct_graph["result"].get("delegated_placements").is_none());
    let collect = result["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["graph_node_id"] == "collect")
        .unwrap();
    assert_eq!(
        collect["items"][1]["required_when"],
        json!([{"item": "mode", "equals": "strict"}])
    );
    let noted = result["graph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge["option_id"] == "noted")
        .unwrap();
    assert_eq!(
        noted["guards"],
        json!([{
            "evidence": {"node": "collect", "item": "notes"},
            "non_empty": true
        }])
    );
}

#[test]
fn v2sdg006_delegated_preview_and_graph_select_v2_without_changing_direct_results() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("delegated.yaml"),
        concat!(
            "schema: podway.procedure/v2\n",
            "id: delegated-preview\n",
            "version: '1'\n",
            "name: Delegated preview\n",
            "purpose: Review the delegation graph.\n",
            "node_definitions:\n",
            "  work:\n",
            "    type: action\n",
            "    title: Work\n",
            "    intent: Review the result.\n",
            "graph:\n",
            "  entry: work\n",
            "  nodes:\n",
            "    - id: work\n",
            "      use: work\n",
            "      terminal: true\n",
            "      delegation:\n",
            "        binding_ref: local-specialist\n",
            "        role_ref: reviewer\n",
            "        role_snapshot_digest: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
            "        max_rounds: 3\n",
            "        after_accept: retain\n",
        ),
    )
    .unwrap();
    for (subcommand, expected_schema) in [
        ("preview", "podway.procedure-preview-result/v2"),
        ("graph", "podway.procedure-graph-result/v2"),
    ] {
        let mut arguments = vec![
            "--json".to_owned(),
            "procedure".to_owned(),
            subcommand.to_owned(),
            "delegated.yaml".to_owned(),
        ];
        if subcommand == "graph" {
            arguments.extend(["--format".to_owned(), "json".to_owned()]);
        }
        let output = fixture.run_from_root(&arguments);
        assert!(output.status.success(), "{output:?}");
        let envelope = one_json(&output);
        assert_eq!(envelope["result"]["schema"], expected_schema);
        assert_eq!(envelope["result"]["delegated_placement_count"], 1);
        assert_eq!(envelope["result"]["delegated_placements"], json!(["work"]));
        assert!(
            decode_result_schema_contract_v2(envelope["result"].as_object().unwrap()).is_some()
        );
    }
    let graph = fixture.run_from_root(&[
        "--json".to_owned(),
        "procedure".to_owned(),
        "graph".to_owned(),
        "delegated.yaml".to_owned(),
        "--format".to_owned(),
        "mermaid".to_owned(),
    ]);
    assert!(graph.status.success(), "{graph:?}");
    let graph = one_json(&graph);
    assert_eq!(
        graph["result"]["schema"],
        "podway.procedure-graph-result/v2"
    );
    assert_eq!(graph["result"]["delegated_placements"], json!(["work"]));
    assert!(
        graph["result"]["projection"]
            .as_str()
            .unwrap()
            .contains("flowchart TD")
    );
    let invalid = fs::read_to_string(fixture.root.join("delegated.yaml"))
        .unwrap()
        .replace(
            "    - id: work\n",
            "    - id: stranded\n      use: work\n      terminal: true\n    - id: work\n",
        );
    fs::write(fixture.root.join("delegated-invalid.yaml"), invalid).unwrap();
    let output = fixture.run_from_root(&[
        "--json".to_owned(),
        "procedure".to_owned(),
        "preview".to_owned(),
        "delegated-invalid.yaml".to_owned(),
    ]);
    assert!(!output.status.success(), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(
        envelope["result"]["schema"],
        "podway.procedure-preview-result/v2"
    );
    assert_eq!(envelope["result"]["admissible"], false);
    assert_eq!(envelope["result"]["checks"]["vet"], false);
    assert_eq!(envelope["result"]["delegated_placement_count"], 1);
    assert_eq!(envelope["result"]["delegated_placements"], json!(["work"]));
    assert!(decode_result_schema_contract_v2(envelope["result"].as_object().unwrap()).is_some());
}

#[test]
fn v2_grammar_rejects_invalid_shapes_before_workspace_execution() {
    let invalid = [
        vec!["goal", "define", "--goal", "Missing criteria."],
        vec!["goal", "define", "--criterion", "tested=Tests pass."],
        vec![
            "goal",
            "define",
            "--goal",
            "Bad criterion.",
            "--criterion",
            "missing-equals",
        ],
        vec![
            "goal",
            "define",
            "--goal",
            "Duplicate criteria.",
            "--criterion",
            "tested=First.",
            "--criterion",
            "tested=Second.",
        ],
        vec![
            "goal",
            "assess-criterion",
            "tested",
            "--status",
            "unknown",
            "--reason",
            "no",
        ],
        vec![
            "goal",
            "assess-criterion",
            "tested",
            "--status",
            "not_applicable",
            "--reason",
            "superseded",
            "--evidence",
            "test",
        ],
        vec![
            "goal",
            "assess-criterion",
            "tested",
            "--status",
            "satisfied",
            "--reason",
            "too many citations",
            "--evidence",
            "one",
            "--evidence",
            "two",
            "--evidence",
            "three",
            "--item",
            "four",
            "--item",
            "five",
        ],
        vec!["status", "--history-before", "3"],
        vec!["status", "--verbose", "--history-before", "0"],
        vec![
            "status",
            "--verbose",
            "--history-before",
            "18446744073709551616",
        ],
        vec![
            "status",
            "--verbose",
            "--history-before",
            "3",
            "--wait-for-idle",
            "--compact",
        ],
        vec!["goal", "unknown"],
    ];
    for command in invalid {
        let fixture = Fixture::new();
        let arguments = command.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let output = fixture.run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
    }
}

#[test]
fn help_and_every_completion_target_publish_the_v2_routes_and_flags() {
    let fixture = Fixture::new();
    let overview = fixture.run(&["--json".to_owned(), "help".to_owned()]);
    assert!(overview.status.success(), "{overview:?}");
    assert!(
        one_json(&overview)["result"]["text"]
            .as_str()
            .unwrap()
            .contains("outcome lookup")
    );
    assert!(
        one_json(&overview)["result"]["text"]
            .as_str()
            .unwrap()
            .contains("analysis-v2")
    );
    assert!(
        one_json(&overview)["result"]["text"]
            .as_str()
            .unwrap()
            .contains("small-change-v2")
    );
    for topic in [
        "session.status",
        "session.observe",
        "session.begin",
        "session.terminal_disposition",
        "session.reset",
        "session.decide",
        "session.rework",
        "delegation.package",
        "delegation.accept",
        "delegation.receipt",
        "goal.define",
        "goal.revise",
        "goal.assess_criterion",
        "outcome.lookup",
    ] {
        let fixture = Fixture::new();
        let output = fixture.run(&["--json".to_owned(), "help".to_owned(), topic.to_owned()]);
        assert!(output.status.success(), "{topic}: {output:?}");
        assert!(
            one_json(&output)["result"]["text"]
                .as_str()
                .unwrap()
                .contains("Usage:")
        );
    }
    let accept_help = fixture.run(&[
        "--json".to_owned(),
        "help".to_owned(),
        "delegation.accept".to_owned(),
    ]);
    let guidance = one_json(&accept_help)["result"]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(guidance.contains("accept --stdin [--json]"));
    assert!(guidance.contains("Record final action evidence and settle the worker"));
    assert!(guidance.contains("outcome lookup with the same key"));
    for (topic, usage) in [
        ("delegation.package", "delegation package [--json]"),
        (
            "delegation.receipt",
            "delegation receipt --session <uuid> --attempt <uuid> [--json] [--if-workspace-uuid <uuid>]",
        ),
    ] {
        let output = fixture.run(&["--json".to_owned(), "help".to_owned(), topic.to_owned()]);
        assert!(output.status.success(), "{topic}: {output:?}");
        assert!(
            one_json(&output)["result"]["text"]
                .as_str()
                .unwrap()
                .contains(usage),
            "{topic} help omits {usage}"
        );
    }
    let fixture = Fixture::new();
    let replacement = fixture.run(&[
        "--json".to_owned(),
        "help".to_owned(),
        "session.start_replace".to_owned(),
    ]);
    assert!(replacement.status.success(), "{replacement:?}");
    let replacement_help = one_json(&replacement)["result"]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(replacement_help.contains("no replace command or replace flags"));
    assert!(replacement_help.contains("--on-existing preserve|delete"));
    assert!(!replacement_help.contains("--goal"));
    for shell in ["bash", "zsh", "fish"] {
        let fixture = Fixture::new();
        let output = fixture.run(&["completions".to_owned(), shell.to_owned()]);
        assert!(output.status.success(), "{shell}: {output:?}");
        let script = String::from_utf8(output.stdout).expect("completion must be UTF-8");
        for token in [
            "archive",
            "session-id",
            "history-before",
            "observe",
            "begin",
            "disposition",
            "on-existing",
            "supersede-reason",
            "progress-summary",
            "decide",
            "rework",
            "goal",
            "assess-criterion",
            "if-goal-revision",
            "analysis-v2",
            "small-change-v2",
            "evidence",
            "delegation package",
            "delegation accept",
            "delegation receipt",
            "page-token",
            "outcome lookup",
        ] {
            assert!(script.contains(token), "{shell} completion omits {token}");
        }
        for flag in ["session", "attempt", "stdin"] {
            let rendered = if shell == "fish" {
                format!("-l {flag}")
            } else {
                format!("--{flag}")
            };
            assert!(
                script.contains(&rendered),
                "{shell} completion omits {rendered}"
            );
        }
        for removed in [
            "procedure convert",
            "docs-only",
            "values='sw-dev'",
            "values='bug-fix'",
        ] {
            assert!(
                !script.contains(removed),
                "{shell} completion retains removed v1 surface {removed}"
            );
        }
        assert!(
            !script
                .split(|character: char| {
                    !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
                })
                .any(|token| token == "analysis"),
            "{shell} completion retains the removed standalone analysis route"
        );
    }
}

#[test]
fn top_level_help_flags_alias_the_canonical_help_command_only() {
    let canonical = Fixture::new().run(&["help".to_owned()]);
    assert!(canonical.status.success(), "{canonical:?}");

    for alias in ["-h", "--help"] {
        let output = Fixture::new().run(&[alias.to_owned()]);
        assert!(output.status.success(), "{alias}: {output:?}");
        assert_eq!(output.stdout, canonical.stdout, "{alias}");
        assert!(output.stderr.is_empty(), "{alias}: {output:?}");
    }
    let no_color = Fixture::new().run(&["--no-color".to_owned(), "--help".to_owned()]);
    assert!(no_color.status.success(), "{no_color:?}");
    assert_eq!(no_color.stdout, canonical.stdout);
    let quiet = Fixture::new().run(&["--quiet".to_owned(), "-h".to_owned()]);
    assert!(quiet.status.success(), "{quiet:?}");
    assert!(quiet.stdout.is_empty(), "{quiet:?}");
    assert!(quiet.stderr.is_empty(), "{quiet:?}");

    for arguments in [
        vec!["--json".to_owned(), "-h".to_owned()],
        vec!["--help".to_owned(), "--json".to_owned()],
    ] {
        let output = Fixture::new().run(&arguments);
        assert!(output.status.success(), "{arguments:?}: {output:?}");
        let envelope = one_json(&output);
        assert_eq!(envelope["command"], "help");
        assert_eq!(envelope["result"]["topic"], Value::Null);
        assert!(
            envelope["result"]["text"]
                .as_str()
                .unwrap()
                .contains("podway --help")
        );
    }

    for arguments in [
        vec!["status".to_owned(), "--help".to_owned()],
        vec!["--help".to_owned(), "status".to_owned()],
    ] {
        let output = Fixture::new().run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
    }
}

#[test]
fn bound005_format_write_preserves_permissions_and_canonical_noop() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let fixture = Fixture::new();
    let scaffold = fixture.run(&["procedure".into(), "scaffold".into()]);
    assert!(scaffold.status.success());
    let source = String::from_utf8(scaffold.stdout)
        .unwrap()
        .replace("version: \"1\"", "version: '1'");
    let file = fixture.root.join("procedure.yaml");
    fs::write(&file, source).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o664)).unwrap();
    let args = ["procedure", "format", "procedure.yaml", "--write", "--json"].map(str::to_owned);
    let first = fixture.run(&args);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(one_json(&first)["result"]["changed"], true);
    let bytes = fs::read(&file).unwrap();
    let before = fs::metadata(&file).unwrap();
    assert_eq!(before.mode() & 0o7777, 0o664);
    let second = fixture.run(&args);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    assert_eq!(one_json(&second)["result"]["changed"], false);
    let after = fs::metadata(&file).unwrap();
    assert_eq!(
        (after.ino(), after.mtime(), after.mtime_nsec(), after.mode()),
        (
            before.ino(),
            before.mtime(),
            before.mtime_nsec(),
            before.mode()
        )
    );
    assert_eq!(fs::read(&file).unwrap(), bytes);
    assert!(fs::read_dir(&fixture.root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".podway-tmp")
    }));
}

#[test]
fn bound005_format_refuses_symlinks_without_changing_the_victim() {
    let fixture = Fixture::new();
    let scaffold = fixture.run(&["procedure".into(), "scaffold".into()]);
    assert!(scaffold.status.success());
    let victim = fixture.root.join("victim.yaml");
    fs::write(&victim, &scaffold.stdout).unwrap();
    let link = fixture.root.join("linked.yaml");
    std::os::unix::fs::symlink("victim.yaml", &link).unwrap();
    let output = fixture.run(&[
        "procedure".into(),
        "format".into(),
        "linked.yaml".into(),
        "--write".into(),
        "--json".into(),
    ]);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    assert_eq!(one_json(&output)["code"], "PATH_OUTSIDE_WORKTREE");
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read(&victim).unwrap(), scaffold.stdout);
    assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("victim.yaml"));
    assert!(fs::read_dir(&fixture.root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".podway-tmp")
    }));
}

#[test]
fn bound005_fresh_preset_preview_remains_offline() {
    let fixture = Fixture::new();
    let output = fixture.run(
        &[
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "Offline preview",
            "--dry-run",
            "--json",
        ]
        .map(str::to_owned),
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = one_json(&output);
    assert_eq!(value["result"]["dry_run"], true);
    assert_eq!(value["result"]["session_state"], "prepared");
    assert!(output.stderr.is_empty());
    assert!(!fixture.root.join(".podway").exists());
}
