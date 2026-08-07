//! Process-boundary contracts for the Procedure v2 authoring surface (`procedure format`).

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use podway_protocol::ResponseEnvelopeV1;
use serde_json::Value;

/// The smallest legal Procedure v2 document, already in canonical authoring form.
const MINIMAL_V2_YAML: &str = r#"schema: podway.procedure/v2
id: minimal
version: "1"
name: Minimal
purpose: The smallest legal Procedure v2 document.
node_definitions:
  work:
    type: action
    title: Work
    intent: Do the work.
graph:
  entry: only
  nodes:
    - id: only
      use: work
      terminal: true
"#;

/// The same document with full-line comment blocks, which canonical authoring form reattaches.
const COMMENTED_V2_YAML: &str = r#"# Podway procedure, annotated.
schema: podway.procedure/v2
id: commented
version: "1"
name: Commented
purpose: Prove that full-line comments survive formatting.
node_definitions:
  # The reusable contract every placement below uses.
  work:
    type: action
    title: Work
    intent: Do the work.
graph:
  entry: only
  nodes:
    - id: only
      use: work
      terminal: true
# Nothing follows; this is the trailing block.
"#;

/// A Procedure v1 document: registered, parseable, and never a `procedure format` target.
const V1_YAML: &str = r#"schema: podway.procedure/v1
id: release
version: "1"
name: Release
stages:
  - id: prepare
    title: Prepare
rework:
  allow_return_to: [prepare]
"#;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A temporary directory holding one test's authoring fixtures.
///
/// Fixtures never land under `tests/fixtures/`: that tree is a manifest-tracked contract surface.
struct FixtureDirectory {
    root: PathBuf,
}

impl FixtureDirectory {
    fn new(label: &str) -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "podway-v2aut001-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("authoring fixture directory must be creatable");
        Self { root }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, contents).expect("authoring fixture must be writable");
        path
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_podway"))
        .args(arguments)
        .env(
            "PODWAY_TEST_ACCOUNT_ROOT",
            format!("/tmp/podway-cli-v2aut001-{}", std::process::id()),
        )
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("podway binary must run")
}

fn format_text(path: &Path) -> Output {
    run(&["procedure", "format", &path.display().to_string()])
}

fn format_json(path: &Path) -> Output {
    run(&["--json", "procedure", "format", &path.display().to_string()])
}

fn one_json(output: &Output) -> Value {
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).lines().count(),
        1,
        "JSON mode must emit exactly one stdout object: {output:?}"
    );
    serde_json::from_slice(&output.stdout).expect("stdout must be JSON")
}

fn identity(path: &Path) -> (Vec<u8>, SystemTime) {
    let bytes = fs::read(path).expect("fixture must be readable");
    let modified = fs::metadata(path)
        .expect("fixture metadata must be readable")
        .modified()
        .expect("fixture modification time must be readable");
    (bytes, modified)
}

#[test]
fn v2aut001_format_writes_the_canonical_document_to_stdout_byte_for_byte() {
    let fixture = FixtureDirectory::new("stdout");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let json = format_json(&path);
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    assert!(json.stderr.is_empty());
    let envelope = one_json(&json);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.format");
    assert!(
        envelope["warnings"].is_array(),
        "the v2 envelope always serializes its required warnings array"
    );
    assert!(envelope.get("workspace").is_none());
    assert!(envelope.get("job").is_none());
    assert!(envelope.get("session").is_none());

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-source-result/v1");
    assert_eq!(result["operation"], "format");
    assert_eq!(result["target_schema"], "podway.procedure/v2");
    assert_eq!(result["mode"], "stdout");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(
        result["changed"], false,
        "the fixture is already canonical, so formatting reports no drift"
    );
    assert!(
        result["target_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71),
        "the source result must carry the canonical semantic digest: {result}"
    );
    let document = result["document"]
        .as_str()
        .expect("the source result must carry the document");
    assert_eq!(document, MINIMAL_V2_YAML);

    // Text mode writes exactly the document — the emitted text already ends in one newline, so the
    // renderer must not append a second one.
    let text = format_text(&path);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert!(text.stderr.is_empty());
    assert_eq!(
        text.stdout,
        document.as_bytes(),
        "text stdout must be the document bytes with no added newline"
    );

    // Determinism: the same input renders the same bytes on a second process.
    let repeated = format_text(&path);
    assert_eq!(repeated.stdout, text.stdout);
}

#[test]
fn v2aut001_format_normalizes_a_non_canonical_document_and_reports_the_drift() {
    let fixture = FixtureDirectory::new("drift");
    let drifted = MINIMAL_V2_YAML.replace("name: Minimal\n", "name:   Minimal\n");
    let path = fixture.write("drifted.yaml", &drifted);

    let envelope = one_json(&format_json(&path));
    assert_eq!(envelope["result"]["changed"], true);
    let document = envelope["result"]["document"]
        .as_str()
        .expect("the source result must carry the document");
    assert_eq!(document, MINIMAL_V2_YAML);

    // The emitted document is itself canonical: reformatting it reports no drift.
    let canonical_path = fixture.write("canonical.yaml", document);
    let refreshed = one_json(&format_json(&canonical_path));
    assert_eq!(refreshed["result"]["changed"], false);
    assert_eq!(refreshed["result"]["document"], document);
}

#[test]
fn v2aut001_format_reattaches_full_line_comments() {
    let fixture = FixtureDirectory::new("comments");
    let path = fixture.write("commented.yaml", COMMENTED_V2_YAML);

    let output = format_text(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let document = String::from_utf8(output.stdout).expect("the document must be UTF-8");
    for comment in [
        "# Podway procedure, annotated.",
        "  # The reusable contract every placement below uses.",
        "# Nothing follows; this is the trailing block.",
    ] {
        assert!(
            document.lines().any(|line| line == comment),
            "formatting must not discard {comment:?}: {document}"
        );
    }
}

#[test]
fn v2aut001_an_unrepresentable_source_construct_is_a_structured_diagnostic_success() {
    let fixture = FixtureDirectory::new("diagnostics");
    let source = MINIMAL_V2_YAML.replace("id: minimal\n", "id: minimal # the identifier\n");
    let path = fixture.write("inline-comment.yaml", &source);

    let output = format_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "authoring findings are stdout data, never stderr diagnostics"
    );
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.format");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "format");
    assert_eq!(result["procedure_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics_truncated"], false);
    assert_eq!(result["diagnostics_total"], 1);

    let diagnostics = result["diagnostics"]
        .as_array()
        .expect("the diagnostics result must carry an array");
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic["code"], "SOURCE_CONSTRUCT_UNSUPPORTED");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["schema"], "podway.procedure/v2");
    assert_eq!(diagnostic["source_path"], path.display().to_string());
    assert_eq!(diagnostic["location"]["line"], 2);
    assert_eq!(diagnostic["location"]["column"], 13);
    assert_eq!(diagnostic["field"], "id");
    assert!(
        diagnostic["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty())
    );
    assert!(diagnostic["hint"].as_str().is_some_and(|h| !h.is_empty()));

    // The human report is one line per finding: `<path>:<line>:<column> <severity> <code> <msg>`.
    let text = format_text(&path);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    assert!(text.stderr.is_empty());
    assert_eq!(
        String::from_utf8(text.stdout).expect("the report must be UTF-8"),
        format!(
            "{}:2:13 error SOURCE_CONSTRUCT_UNSUPPORTED {}\n",
            path.display(),
            diagnostic["message"]
                .as_str()
                .expect("the diagnostic must carry a message"),
        ),
    );
}

#[test]
fn v2aut001_check_is_a_registered_capability_this_build_does_not_serve() {
    let fixture = FixtureDirectory::new("check");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let before = identity(&path);

    let output = run(&[
        "--json",
        "procedure",
        "format",
        &path.display().to_string(),
        "--check",
    ]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "UNSUPPORTED_V2_CAPABILITY");
    assert_eq!(envelope["exit_code"], 3);
    assert_eq!(envelope["retryable"], false);
    assert_eq!(
        envelope["details"]["schema"],
        "podway.v2-runtime-error-details/v1"
    );
    assert_eq!(envelope["details"]["kind"], "UNSUPPORTED_V2_CAPABILITY");
    assert_eq!(
        envelope["details"]["capability"],
        "procedure.format --check"
    );

    // The renderer falls back to INTERNAL_ERROR when it cannot build a valid typed envelope, so the
    // decode is the proof that the closed v2 detail family accepted this failure as authored.
    let typed: ResponseEnvelopeV1 =
        serde_json::from_value(envelope).expect("the capability failure must be a typed envelope");
    let ResponseEnvelopeV1::Error(typed) = typed else {
        panic!("an unimplemented capability is an error envelope");
    };
    assert_eq!(typed.code().as_str(), "UNSUPPORTED_V2_CAPABILITY");
    assert_eq!(typed.exit_code().get(), 3);
    assert_eq!(typed.command().as_str(), "procedure.format");

    assert_eq!(identity(&path), before, "--check must not touch the file");
}

#[test]
fn v2aut001_write_is_a_registered_capability_this_build_does_not_serve() {
    let fixture = FixtureDirectory::new("write");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let before = identity(&path);

    let output = run(&[
        "--json",
        "procedure",
        "format",
        &path.display().to_string(),
        "--write",
    ]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["code"], "UNSUPPORTED_V2_CAPABILITY");
    assert_eq!(
        envelope["details"]["capability"],
        "procedure.format --write"
    );

    assert_eq!(identity(&path), before, "--write must not touch the file");
}

#[test]
fn v2aut001_check_and_write_together_are_a_usage_failure() {
    let fixture = FixtureDirectory::new("conflict");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let output = run(&[
        "--json",
        "procedure",
        "format",
        &path.display().to_string(),
        "--check",
        "--write",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "REQUEST_INVALID");
    assert_eq!(envelope["exit_code"], 2);
}

#[test]
fn v2aut001_a_v1_document_is_a_schema_failure_rather_than_a_v2_diagnostic() {
    let fixture = FixtureDirectory::new("v1");
    let path = fixture.write("v1.yaml", V1_YAML);

    let output = format_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);

    let text = format_text(&path);
    assert_eq!(text.status.code(), Some(1));
    assert!(text.stdout.is_empty());
    assert!(
        !text.stderr.is_empty(),
        "a process failure reports on stderr in text mode"
    );
}

/// A declared-v1 document that is also invalid as v1 gets the same wrong-schema failure. The v1
/// parse error must not leak into the v2 authoring pipeline, where it would be misreported as a
/// diagnostics result claiming `procedure_schema: "podway.procedure/v2"` about a v1 document.
#[test]
fn v2aut001_a_malformed_v1_document_is_still_a_schema_failure() {
    let fixture = FixtureDirectory::new("v1-malformed");
    let path = fixture.write(
        "broken-v1.yaml",
        "schema: podway.procedure/v1\nid: broken\n",
    );

    let output = format_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);
}

#[test]
fn v2aut001_a_missing_file_is_a_path_failure_before_any_capability_check() {
    let fixture = FixtureDirectory::new("missing");
    let path = fixture.root.join("absent.yaml");

    for arguments in [
        vec![
            "--json".to_owned(),
            "procedure".to_owned(),
            "format".to_owned(),
            path.display().to_string(),
        ],
        vec![
            "--json".to_owned(),
            "procedure".to_owned(),
            "format".to_owned(),
            path.display().to_string(),
            "--check".to_owned(),
        ],
    ] {
        let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let output = run(&borrowed);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let envelope = one_json(&output);
        assert_eq!(envelope["schema"], "podway.error/v1");
        assert_eq!(envelope["command"], "procedure.format");
        assert_eq!(envelope["code"], "PROCEDURE_NOT_FOUND");
        assert_eq!(envelope["exit_code"], 1);
    }
}

#[test]
fn v2aut001_quiet_suppresses_the_document_without_changing_the_exit_code() {
    let fixture = FixtureDirectory::new("quiet");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let output = run(&[
        "--quiet",
        "procedure",
        "format",
        &path.display().to_string(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn v2aut001_daemon_only_globals_are_rejected_before_execution() {
    let fixture = FixtureDirectory::new("globals");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let output = run(&[
        "--json",
        "--worktree",
        ".",
        "procedure",
        "format",
        &path.display().to_string(),
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "REQUEST_INVALID");
    assert_eq!(envelope["exit_code"], 2);
}
