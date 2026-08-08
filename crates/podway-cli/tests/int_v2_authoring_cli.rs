//! Process-boundary contracts for the Procedure v2 authoring surface (`procedure format`,
//! `procedure vet`, `procedure lint`, `procedure check`, `procedure scaffold`, and
//! `procedure convert`).

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use podway_config::SCAFFOLD_TEMPLATE_MINIMAL;
use podway_protocol::ResponseEnvelopeV1;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

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
        // A test that panics between restricting the directory and restoring it must not strand
        // an unremovable tree under the temp root.
        let _ = fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700));
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

/// Every name in a directory, sorted: the proof that a command created nothing it did not report.
fn entry_names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .expect("the fixture directory must be readable")
        .map(|entry| {
            entry
                .expect("the fixture directory entry must be readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
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

// ---------------------------------------------------------------------------------------------
// V2AUT-002: `--check`
// ---------------------------------------------------------------------------------------------

/// The pinned human summary for a source that is already in canonical authoring form.
///
/// `--check` on a clean file must say so in one line and say nothing else: the whole point of the
/// mode is that it is quiet enough to run in a loop over a tree.
fn canonical_summary(path: &Path) -> String {
    format!("{} is in canonical authoring form\n", path.display())
}

fn format_check_json(path: &Path) -> Output {
    run(&[
        "--json",
        "procedure",
        "format",
        &path.display().to_string(),
        "--check",
    ])
}

fn format_check_text(path: &Path) -> Output {
    run(&[
        "procedure",
        "format",
        &path.display().to_string(),
        "--check",
    ])
}

/// `MINIMAL_V2_YAML` with one extra space after a key, which is drift and nothing else: the
/// document still parses, validates, and renders.
fn drifted_source() -> String {
    MINIMAL_V2_YAML.replace("name: Minimal\n", "name:   Minimal\n")
}

#[test]
fn v2aut002_check_reports_a_canonical_file_as_a_source_result_with_no_drift() {
    let fixture = FixtureDirectory::new("check-canonical");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let output = format_check_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.format");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-source-result/v1");
    assert_eq!(result["operation"], "format");
    assert_eq!(result["target_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["mode"], "check");
    assert_eq!(result["changed"], false);
    assert!(
        result["target_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71),
        "{result}"
    );
    // The source result schema requires `document` in every mode, and requiring it is right: a
    // client that learns a file has drifted can act on the answer without a second invocation.
    assert_eq!(result["document"], MINIMAL_V2_YAML);

    // Text mode is the one-line verdict, not the document.
    let text = format_check_text(&path);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert!(text.stderr.is_empty());
    assert_eq!(
        String::from_utf8(text.stdout).expect("the summary must be UTF-8"),
        canonical_summary(&path)
    );
}

#[test]
fn v2aut002_check_reports_a_drifted_file_as_one_format_not_canonical_finding() {
    let fixture = FixtureDirectory::new("check-drift");
    let path = fixture.write("drifted.yaml", &drifted_source());

    let output = format_check_json(&path);
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
    assert_eq!(diagnostics.len(), 1, "drift is one verdict, not a diff");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic["code"], "FORMAT_NOT_CANONICAL");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["schema"], "podway.procedure/v2");
    assert_eq!(diagnostic["source_path"], path.display().to_string());
    assert_eq!(diagnostic["field"], "name");
    // `name:   Minimal` and `name: Minimal` share six characters on the fourth line.
    assert_eq!(diagnostic["location"]["line"], 4);
    assert_eq!(diagnostic["location"]["column"], 7);
    assert_eq!(diagnostic["location"]["end_line"], 4);
    assert_eq!(diagnostic["location"]["end_column"], 16);
    assert_eq!(
        diagnostic["message"],
        "The source is not in canonical authoring form at this line."
    );
    assert_eq!(
        diagnostic["hint"],
        "Run `podway procedure format <file> --write` to rewrite the file in canonical form."
    );

    // The human report is the same one-line-per-finding render every authoring command uses.
    let text = format_check_text(&path);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    assert!(text.stderr.is_empty());
    assert_eq!(
        String::from_utf8(text.stdout).expect("the report must be UTF-8"),
        format!(
            "{}:4:7 error FORMAT_NOT_CANONICAL The source is not in canonical authoring form at \
             this line.\n",
            path.display()
        )
    );
}

#[test]
fn v2aut002_check_never_touches_the_file_it_reads() {
    let fixture = FixtureDirectory::new("check-readonly");
    let canonical = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let drifted = fixture.write("drifted.yaml", &drifted_source());
    let before = (identity(&canonical), identity(&drifted));

    // Both verdicts, in both renderings: nothing on any of the four paths opens the file for
    // writing, so the bytes and the modification time are the proof rather than the intent.
    for path in [&canonical, &drifted] {
        format_check_json(path);
        format_check_text(path);
    }

    assert_eq!(
        (identity(&canonical), identity(&drifted)),
        before,
        "--check must leave every byte and every mtime exactly as it found them"
    );
    assert_eq!(
        fs::read_to_string(&drifted).expect("the drifted fixture must still be readable"),
        drifted_source(),
        "--check must not rewrite the drifted file it reports on"
    );
    assert_eq!(
        entry_names(&fixture.root),
        vec!["drifted.yaml".to_owned(), "minimal.yaml".to_owned()],
        "--check must not leave a temporary file behind"
    );
}

#[test]
fn v2aut002_quiet_check_reports_only_through_its_exit_code() {
    let fixture = FixtureDirectory::new("check-quiet");
    let canonical = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let drifted = fixture.write("drifted.yaml", &drifted_source());

    for (path, expected) in [(&canonical, 0), (&drifted, 1)] {
        let output = run(&[
            "--quiet",
            "procedure",
            "format",
            &path.display().to_string(),
            "--check",
        ]);
        assert_eq!(output.status.code(), Some(expected), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn v2aut002_check_is_deterministic_across_processes() {
    let fixture = FixtureDirectory::new("check-determinism");
    let drifted = fixture.write("drifted.yaml", &drifted_source());

    let first = format_check_text(&drifted);
    let second = format_check_text(&drifted);
    assert_eq!(first.status.code(), second.status.code());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stderr, second.stderr);
}

/// A v1 document is refused for being the wrong schema before `--check` gets a say, because the
/// diagnostics family it would have to answer in is pinned to `podway.procedure/v2`.
#[test]
fn v2aut002_check_on_a_v1_document_is_still_a_schema_failure() {
    let fixture = FixtureDirectory::new("check-v1");
    let path = fixture.write("v1.yaml", V1_YAML);

    let output = format_check_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);
}

/// A document that cannot be rendered reports the stage that stopped it. `--check` adds nothing:
/// "not canonical" is not a statement anyone can act on when the canonical form does not exist.
#[test]
fn v2aut002_check_on_an_unformattable_document_reports_only_the_earlier_stage() {
    let fixture = FixtureDirectory::new("check-unformattable");
    let path = fixture.write(
        "inline-comment.yaml",
        &MINIMAL_V2_YAML.replace("id: minimal\n", "id: minimal # the identifier\n"),
    );

    let output = format_check_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    let diagnostics = envelope["result"]["diagnostics"]
        .as_array()
        .expect("the diagnostics result must carry an array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "SOURCE_CONSTRUCT_UNSUPPORTED");
}

// ---------------------------------------------------------------------------------------------
// V2AUT-003: `--write`
// ---------------------------------------------------------------------------------------------

/// The pinned human summary for a file the rewrite actually replaced.
///
/// A clean file reuses `canonical_summary` instead, so the two lines together tell an author which
/// of the two things happened without reading the JSON.
fn rewritten_summary(path: &Path) -> String {
    format!("{} rewritten in canonical authoring form\n", path.display())
}

/// A Procedure v2 document whose sequence entry starts one line *below* its `- ` marker.
///
/// The marker line anchors no node, so the comment above it has nothing to re-attach to and would
/// silently move to the end of the document. Under `--write` that is an unannounced edit of the
/// author's file, so the source is refused instead.
const BARE_MARKER_V2_YAML: &str = r#"schema: podway.procedure/v2
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
    # About the only placement.
    -
      id: only
      use: work
      terminal: true
"#;

fn format_write_json(path: &Path) -> Output {
    run(&[
        "--json",
        "procedure",
        "format",
        &path.display().to_string(),
        "--write",
    ])
}

fn format_write_text(path: &Path) -> Output {
    run(&[
        "procedure",
        "format",
        &path.display().to_string(),
        "--write",
    ])
}

fn mode_bits(path: &Path) -> u32 {
    fs::metadata(path)
        .expect("fixture metadata must be readable")
        .permissions()
        .mode()
        & 0o7777
}

/// The message every "the rejection reached the filesystem" assertion shares.
const NO_STAGING_FILE: &str = "a refused --write must leave no staging file behind";

#[test]
fn v2aut003_write_replaces_a_drifted_file_with_the_bytes_format_would_have_printed() {
    let fixture = FixtureDirectory::new("write-drift");
    let path = fixture.write("drifted.yaml", &drifted_source());
    let sibling = fixture.write("sibling.yaml", MINIMAL_V2_YAML);
    let sibling_before = identity(&sibling);

    // The canonical bytes, obtained from the non-writing mode first so the comparison below is
    // against an independently produced answer rather than against the write's own output.
    let rendered = format_text(&path);
    assert_eq!(rendered.status.code(), Some(0), "{rendered:?}");

    let output = format_write_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.format");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-source-result/v1");
    assert_eq!(result["operation"], "format");
    assert_eq!(result["target_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["mode"], "write");
    assert_eq!(result["changed"], true);
    assert_eq!(result["document"], MINIMAL_V2_YAML);
    assert!(
        result["target_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71),
        "{result}"
    );

    assert_eq!(
        fs::read(&path).expect("the rewritten file must be readable"),
        rendered.stdout,
        "the file must hold exactly the bytes `procedure format` prints"
    );
    assert_eq!(
        identity(&sibling),
        sibling_before,
        "--write names one file and touches only that file"
    );
    assert_eq!(
        entry_names(&fixture.root),
        vec!["drifted.yaml".to_owned(), "sibling.yaml".to_owned()],
        "the staging file must not survive a successful rewrite"
    );
}

#[test]
fn v2aut003_write_reports_the_rewrite_in_one_summary_line() {
    let fixture = FixtureDirectory::new("write-summary");
    let drifted = fixture.write("drifted.yaml", &drifted_source());
    let canonical = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let rewrite = format_write_text(&drifted);
    assert_eq!(rewrite.status.code(), Some(0), "{rewrite:?}");
    assert!(rewrite.stderr.is_empty());
    assert_eq!(
        String::from_utf8(rewrite.stdout).expect("the summary must be UTF-8"),
        rewritten_summary(&drifted)
    );

    // A clean file reports the same verdict `--check` reports, because the same thing happened:
    // nothing.
    let clean = format_write_text(&canonical);
    assert_eq!(clean.status.code(), Some(0), "{clean:?}");
    assert_eq!(
        String::from_utf8(clean.stdout).expect("the summary must be UTF-8"),
        canonical_summary(&canonical)
    );
}

#[test]
fn v2aut003_write_preserves_the_permission_bits_of_the_file_it_replaces() {
    // 0o664 is the discriminating case: under the prevailing 022 umask, creating the staging file
    // with the captured mode alone would yield 0o644, so only the explicit fchmod reproduces it.
    for bits in [0o644, 0o600, 0o664] {
        let fixture = FixtureDirectory::new("write-mode");
        let path = fixture.write("drifted.yaml", &drifted_source());
        fs::set_permissions(&path, fs::Permissions::from_mode(bits))
            .expect("the fixture permissions must be settable");

        let output = format_write_json(&path);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(one_json(&output)["result"]["changed"], true);
        assert_eq!(
            mode_bits(&path),
            bits,
            "a rewrite carries the original mode, not the process umask's opinion of it"
        );
    }
}

#[test]
fn v2aut003_write_on_a_canonical_file_writes_nothing_at_all() {
    let fixture = FixtureDirectory::new("write-canonical");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let before = identity(&path);

    let output = format_write_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    let result = one_json(&output)["result"].clone();
    assert_eq!(result["schema"], "podway.procedure-source-result/v1");
    assert_eq!(result["mode"], "write");
    assert_eq!(result["changed"], false);
    assert_eq!(result["document"], MINIMAL_V2_YAML);

    // Not "rewritten with identical bytes": not written. The modification time is the observable
    // difference, and a build system watching the tree depends on it.
    assert_eq!(
        identity(&path),
        before,
        "an already-canonical file keeps its bytes and its modification time"
    );
    assert_eq!(entry_names(&fixture.root), vec!["minimal.yaml".to_owned()]);
}

#[test]
fn v2aut003_write_preserves_full_line_comments_and_lands_in_canonical_form() {
    let fixture = FixtureDirectory::new("write-comments");
    // Drift the commented document so the rewrite has something to do.
    let path = fixture.write(
        "commented.yaml",
        &COMMENTED_V2_YAML.replace("name: Commented\n", "name:   Commented\n"),
    );

    let output = format_write_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(one_json(&output)["result"]["changed"], true);

    let rewritten = fs::read_to_string(&path).expect("the rewritten file must be readable");
    for comment in [
        "# Podway procedure, annotated.",
        "  # The reusable contract every placement below uses.",
        "# Nothing follows; this is the trailing block.",
    ] {
        assert!(
            rewritten.lines().any(|line| line == comment),
            "--write must not discard {comment:?}: {rewritten}"
        );
    }

    // The file it left behind is one `--check` accepts, which is the round trip the mode promises.
    let check = format_check_text(&path);
    assert_eq!(check.status.code(), Some(0), "{check:?}");
    assert_eq!(
        String::from_utf8(check.stdout).expect("the summary must be UTF-8"),
        canonical_summary(&path)
    );
}

#[test]
fn v2aut003_write_is_idempotent_across_processes() {
    let fixture = FixtureDirectory::new("write-idempotent");
    let path = fixture.write("drifted.yaml", &drifted_source());

    let first = format_write_json(&path);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(one_json(&first)["result"]["changed"], true);
    let after_first = identity(&path);

    let second = format_write_json(&path);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    assert_eq!(
        one_json(&second)["result"]["changed"],
        false,
        "the second run has nothing to do"
    );
    assert_eq!(
        identity(&path),
        after_first,
        "a fixpoint is not rewritten, so even its modification time is stable"
    );
}

/// An inline trailing comment is a construct canonical authoring form cannot represent. The
/// rejection has to arrive before the filesystem is touched — a partially rewritten file that drops
/// the author's comment is the exact failure `--write` exists to make impossible.
#[test]
fn v2aut003_an_unsupported_source_construct_is_refused_before_any_write() {
    let fixture = FixtureDirectory::new("write-inline-comment");
    let path = fixture.write(
        "inline-comment.yaml",
        &MINIMAL_V2_YAML.replace("id: minimal\n", "id: minimal # the identifier\n"),
    );
    let before = identity(&path);
    let names = entry_names(&fixture.root);

    let output = format_write_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(
        envelope["result"]["schema"],
        "podway.procedure-diagnostics-result/v1"
    );
    let diagnostics = envelope["result"]["diagnostics"]
        .as_array()
        .expect("the diagnostics result must carry an array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "SOURCE_CONSTRUCT_UNSUPPORTED");

    assert_eq!(identity(&path), before, "a refused --write changes nothing");
    assert_eq!(entry_names(&fixture.root), names, "{NO_STAGING_FILE}");
}

/// The F2 case end to end: a comment above a bare `- ` marker. The formatter could render this
/// document, but only by moving the comment somewhere the author did not put it, so the source is
/// refused with a located finding instead.
#[test]
fn v2aut003_a_bare_sequence_marker_is_refused_before_any_write() {
    let fixture = FixtureDirectory::new("write-bare-marker");
    let path = fixture.write("bare-marker.yaml", BARE_MARKER_V2_YAML);
    let before = identity(&path);
    let names = entry_names(&fixture.root);

    let output = format_write_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    let diagnostics = envelope["result"]["diagnostics"]
        .as_array()
        .expect("the diagnostics result must carry an array");
    assert_eq!(diagnostics.len(), 1, "{envelope}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic["code"], "SOURCE_CONSTRUCT_UNSUPPORTED");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["location"]["line"], 15);
    assert_eq!(diagnostic["location"]["column"], 5);
    assert_eq!(
        diagnostic["message"],
        "This source uses a sequence marker whose entry does not start on the marker line, which \
         canonical authoring form cannot represent."
    );

    assert_eq!(identity(&path), before, "a refused --write changes nothing");
    assert_eq!(entry_names(&fixture.root), names, "{NO_STAGING_FILE}");
}

/// The general relocation guard, end to end: a comment above a mapping value written on the line
/// below its key would re-emit at the end of the document, so `--write` refuses it and the file is
/// untouched.
#[test]
fn v2aut003_a_comment_above_an_unanchored_line_is_refused_before_any_write() {
    let fixture = FixtureDirectory::new("write-unanchored-comment");
    let source = MINIMAL_V2_YAML.replace(
        "    intent: Do the work.\n",
        "    intent:\n      # Why the work matters.\n      Do the work.\n",
    );
    let path = fixture.write("unanchored.yaml", &source);
    let before = identity(&path);
    let names = entry_names(&fixture.root);

    let output = format_write_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    let diagnostics = envelope["result"]["diagnostics"]
        .as_array()
        .expect("the diagnostics result must carry an array");
    assert_eq!(diagnostics.len(), 1, "{envelope}");
    assert_eq!(diagnostics[0]["code"], "SOURCE_CONSTRUCT_UNSUPPORTED");
    assert_eq!(
        diagnostics[0]["message"],
        "This source uses a comment attached to a line that does not begin a node, which \
         canonical authoring form cannot represent."
    );

    assert_eq!(identity(&path), before, "a refused --write changes nothing");
    assert_eq!(entry_names(&fixture.root), names, "{NO_STAGING_FILE}");
}

#[test]
fn v2aut003_a_v1_document_is_refused_before_any_write() {
    let fixture = FixtureDirectory::new("write-v1");
    let path = fixture.write("v1.yaml", V1_YAML);
    let before = identity(&path);
    let names = entry_names(&fixture.root);

    let output = format_write_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);

    assert_eq!(identity(&path), before, "a refused --write changes nothing");
    assert_eq!(entry_names(&fixture.root), names, "{NO_STAGING_FILE}");
}

/// The leaf is opened `O_NOFOLLOW`, so a symlink named as the procedure is refused at the read —
/// long before the rewrite would have had a descriptor to rename over.
#[test]
fn v2aut003_a_symlinked_target_is_refused_and_neither_the_link_nor_its_target_changes() {
    let fixture = FixtureDirectory::new("write-symlink");
    let target = fixture.write("drifted.yaml", &drifted_source());
    let link = fixture.root.join("link.yaml");
    std::os::unix::fs::symlink(&target, &link).expect("the fixture symlink must be creatable");
    let before = identity(&target);

    let output = format_write_json(&link);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "PATH_OUTSIDE_WORKTREE");
    assert_eq!(envelope["exit_code"], 5);

    assert_eq!(
        identity(&target),
        before,
        "the symlink's target must be untouched"
    );
    assert!(
        fs::symlink_metadata(&link)
            .expect("the link must still exist")
            .file_type()
            .is_symlink(),
        "the link itself must not be replaced by a regular file"
    );
    assert_eq!(
        entry_names(&fixture.root),
        vec!["drifted.yaml".to_owned(), "link.yaml".to_owned()]
    );
}

/// A directory the process cannot write is the one failure that reaches the filesystem, and the
/// answer is a catalogued error rather than a panic or a truncated file. `INTERNAL_ERROR` is the
/// code this CLI already uses for a local I/O failure that says nothing about the request.
#[test]
fn v2aut003_an_unwritable_directory_is_a_catalogued_failure_that_leaves_the_original_intact() {
    let fixture = FixtureDirectory::new("write-unwritable");
    let path = fixture.write("drifted.yaml", &drifted_source());
    let before = identity(&path);

    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o500))
        .expect("the fixture directory permissions must be settable");
    let output = format_write_json(&path);
    // Restored before any assertion so a failure here still leaves a removable fixture behind.
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700))
        .expect("the fixture directory permissions must be restorable");

    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.format");
    assert_eq!(envelope["code"], "INTERNAL_ERROR");
    assert_eq!(envelope["exit_code"], 6);
    assert_eq!(envelope["retryable"], false);

    // The renderer falls back to a generic failure when it cannot build a valid typed envelope, so
    // decoding it is the proof that this failure is well formed as authored.
    let typed: ResponseEnvelopeV1 =
        serde_json::from_value(envelope).expect("the write failure must be a typed envelope");
    let ResponseEnvelopeV1::Error(typed) = typed else {
        panic!("a refused write is an error envelope");
    };
    assert_eq!(typed.code().as_str(), "INTERNAL_ERROR");
    assert_eq!(typed.exit_code().get(), 6);
    assert_eq!(typed.command().as_str(), "procedure.format");

    assert_eq!(
        identity(&path),
        before,
        "the original must survive a failed rewrite byte for byte"
    );
    assert_eq!(entry_names(&fixture.root), vec!["drifted.yaml".to_owned()]);
}

// ---------------------------------------------------------------------------------------------
// V2GRF-002: `procedure vet`
// ---------------------------------------------------------------------------------------------

fn vet_json(path: &Path) -> Output {
    run(&["--json", "procedure", "vet", &path.display().to_string()])
}

fn oversized_static_v2_yaml() -> String {
    let instructions = (0..16)
        .map(|_| format!("      - {}\n", "i".repeat(1_000)))
        .collect::<String>();
    let items = (0..64)
        .map(|index| {
            format!(
                "      - id: item-{index}-identifier\n        type: text\n        prompt: {}\n        required: true\n        max_length: 1\n",
                "p".repeat(300),
            )
        })
        .collect::<String>();
    format!(
        "schema: podway.procedure/v2\nid: cli-vet-budget\nversion: \"1\"\nname: CLI vet budget\npurpose: Prove the standalone route runs complete resource-budget vetting.\nnode_definitions:\n  work:\n    type: action\n    title: {}\n    intent: {}\n    description: {}\n    instructions:\n{instructions}    items:\n{items}graph:\n  entry: work\n  nodes:\n    - id: work\n      use: work\n      terminal: true\n",
        "t".repeat(120),
        "n".repeat(300),
        "d".repeat(1_000),
    )
}

#[test]
fn v2grf002_a_clean_document_passes_the_standalone_vet_without_writing() {
    let fixture = FixtureDirectory::new("vet-clean");
    let path = fixture.write("clean.yaml", MINIMAL_V2_YAML);
    let before = identity(&path);

    let output = vet_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.vet");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "vet");
    assert_eq!(result["procedure_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["valid"], true);
    assert_eq!(result["diagnostics"], serde_json::json!([]));
    assert_eq!(result["diagnostics_truncated"], false);
    assert_eq!(result["diagnostics_total"], 0);
    assert_eq!(
        result["digest"],
        one_json(&format_json(&path))["result"]["target_digest"],
        "vet and format must bind the same canonical Procedure digest"
    );

    let text = run(&["procedure", "vet", &path.display().to_string()]);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        format!("{}: graph vetting passed\n", path.display())
    );
    assert!(text.stderr.is_empty());

    let repeated = one_json(&vet_json(&path));
    assert_eq!(
        repeated["result"], *result,
        "the result must be deterministic"
    );
    assert_eq!(
        identity(&path),
        before,
        "vet must not alter the source file"
    );
    assert_eq!(entry_names(&fixture.root), vec!["clean.yaml".to_owned()]);
}

#[test]
fn v2grf002_a_validated_graph_rejection_keeps_its_digest_and_vet_identity() {
    let fixture = FixtureDirectory::new("vet-graph-error");
    let source = MINIMAL_V2_YAML
        .replace(
            "graph:\n",
            "  stranded:\n    type: action\n    title: Stranded\n    intent: Remain unreachable.\ngraph:\n",
        )
        .replace(
            "      terminal: true\n",
            "      terminal: true\n    - id: stranded\n      use: stranded\n      terminal: true\n",
        );
    let path = fixture.write("unreachable.yaml", &source);

    let output = vet_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.vet");
    let result = &envelope["result"];
    assert_eq!(result["operation"], "vet");
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics_total"], 1);
    assert_eq!(result["diagnostics_truncated"], false);
    assert!(
        result["digest"].as_str().is_some(),
        "a structurally validated graph retains its digest: {result}"
    );
    let diagnostic = &result["diagnostics"][0];
    assert_eq!(diagnostic["code"], "UNREACHABLE_GRAPH_NODE");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["schema"], "podway.procedure/v2");
    assert_eq!(diagnostic["source_path"], path.display().to_string());
    assert!(diagnostic["location"]["line"].as_u64().is_some());
}

#[test]
fn v2grf002_the_standalone_vet_exposes_resource_budget_rejections() {
    let fixture = FixtureDirectory::new("vet-budget-error");
    let path = fixture.write("oversized.yaml", &oversized_static_v2_yaml());

    let output = vet_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["command"], "procedure.vet");
    let result = &envelope["result"];
    assert_eq!(result["operation"], "vet");
    assert_eq!(result["valid"], false);
    assert!(result["digest"].as_str().is_some());
    assert!(result["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "NEXT_STATIC_BUDGET_EXCEEDED")
    }));
}

#[test]
fn v2grf002_a_validation_failure_stops_before_vet_and_has_no_digest() {
    let fixture = FixtureDirectory::new("vet-invalid");
    let path = fixture.write(
        "invalid.yaml",
        &MINIMAL_V2_YAML.replace("      use: work\n", "      use: absent\n"),
    );

    let output = vet_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["command"], "procedure.vet");
    let result = &envelope["result"];
    assert_eq!(result["operation"], "vet");
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics_total"], 1);
    assert!(result.get("digest").is_none());
    assert_ne!(result["diagnostics"][0]["code"], "UNREACHABLE_GRAPH_NODE");
}

#[test]
fn v2grf002_v1_and_missing_inputs_keep_the_existing_process_failure_contracts() {
    let fixture = FixtureDirectory::new("vet-process-errors");
    let v1 = fixture.write("v1.yaml", V1_YAML);

    let unsupported = vet_json(&v1);
    assert_eq!(unsupported.status.code(), Some(1), "{unsupported:?}");
    let envelope = one_json(&unsupported);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.vet");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);
    assert_eq!(envelope["retryable"], false);

    let missing = vet_json(&fixture.root.join("absent.yaml"));
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    let envelope = one_json(&missing);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.vet");
    assert_eq!(envelope["code"], "PROCEDURE_NOT_FOUND");
    assert_eq!(envelope["exit_code"], 1);
}

#[test]
fn v2grf002_quiet_vet_reports_only_through_the_exit_code() {
    let fixture = FixtureDirectory::new("vet-quiet");
    let path = fixture.write("clean.yaml", MINIMAL_V2_YAML);
    let output = run(&["--quiet", "procedure", "vet", &path.display().to_string()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

// ---------------------------------------------------------------------------------------------
// V2AUT-004: `procedure lint`
// ---------------------------------------------------------------------------------------------

/// A Procedure v2 document that fires no lint rule, mirroring the config-side clean base.
const CLEAN_V2_YAML: &str = r#"schema: podway.procedure/v2
id: lint-clean
version: "1"
name: Lint clean
purpose: Exercise the lint command with a document that has no findings.
node_definitions:
  gather:
    type: action
    title: Gather the inputs
    intent: Collect every input the review needs.
    items:
      - id: notes
        type: text
        prompt: Record the gathered notes.
        required: true
  review:
    type: decision
    title: Review the work
    objective: Decide whether the gathered work is complete.
    prompt: Is the gathered work complete?
    evidence_guidance:
      - Read the gathered notes before deciding.
    options:
      - id: complete
        label: Work is complete
        criteria: Every gathered input is present and correct.
      - id: incomplete
        label: Work is incomplete
        criteria: Some gathered input is missing or wrong.
    reason:
      required: true
      prompt: Explain why the work is or is not complete.
  publish:
    type: action
    title: Publish the result
    intent: Record the published outcome.
graph:
  entry: gather-inputs
  nodes:
    - id: gather-inputs
      use: gather
      next: review-work
    - id: review-work
      use: review
      routes:
        complete:
          to: publish-result
          effect: advance
        incomplete:
          to: gather-inputs
          effect: rework
    - id: publish-result
      use: publish
      terminal: true
manual_rework:
  allowed_targets:
    - gather-inputs
"#;

/// The pinned human summary for a document with no advisory findings.
fn no_findings_summary(path: &Path) -> String {
    format!("{}: no lint findings\n", path.display())
}

fn lint_json(path: &Path) -> Output {
    run(&["--json", "procedure", "lint", &path.display().to_string()])
}

fn lint_json_strict(path: &Path) -> Output {
    run(&[
        "--json",
        "procedure",
        "lint",
        &path.display().to_string(),
        "--warnings-as-errors",
    ])
}

/// The catalog's twenty-three warning codes, read from the frozen specification rather than
/// restated here, so a lint finding outside the catalog fails this file rather than passing it.
fn catalog_warning_codes() -> Vec<String> {
    let specification = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/specifications/authoring-diagnostics.json"),
    )
    .expect("the frozen authoring diagnostics catalog must be readable");
    let catalog: Value =
        serde_json::from_str(&specification).expect("the diagnostics catalog must be valid JSON");
    let codes = catalog["diagnostics"]
        .as_array()
        .expect("the catalog lists diagnostics")
        .iter()
        .filter(|entry| entry["severity"] == "warning")
        .filter_map(|entry| entry["code"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(codes.len(), 23, "the lint catalog must carry 23 warnings");
    codes
}

#[test]
fn v2aut004_a_clean_document_lints_to_an_empty_advisory_report() {
    let fixture = FixtureDirectory::new("lint-clean");
    let path = fixture.write("clean.yaml", CLEAN_V2_YAML);

    let output = lint_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.lint");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "lint");
    assert_eq!(result["procedure_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["valid"], true);
    assert_eq!(result["diagnostics"], serde_json::json!([]));
    assert_eq!(result["diagnostics_truncated"], false);
    assert_eq!(result["diagnostics_total"], 0);
    assert!(
        result["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "a validated document reports its digest: {result}"
    );

    let text = run(&["procedure", "lint", &path.display().to_string()]);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        no_findings_summary(&path)
    );
    assert!(text.stderr.is_empty());
}

#[test]
fn v2aut004_a_clean_document_stays_at_exit_zero_under_warnings_as_errors() {
    let fixture = FixtureDirectory::new("lint-clean-strict");
    let path = fixture.write("clean.yaml", CLEAN_V2_YAML);

    let output = lint_json_strict(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(one_json(&output)["result"]["diagnostics_total"], 0);
}

#[test]
fn v2aut004_a_document_with_findings_reports_catalogued_warnings_at_exit_zero() {
    let fixture = FixtureDirectory::new("lint-warnings");
    // The smallest legal document declares no manual rework targets, which section 11.4 asks lint
    // to surface as an advisory so the author confirms the choice was intended.
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let output = lint_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let envelope = one_json(&output);
    let result = &envelope["result"];
    assert_eq!(result["valid"], true, "warnings never invalidate: {result}");
    assert_eq!(result["diagnostics_total"], 1);
    assert_eq!(result["diagnostics_truncated"], false);

    let catalog = catalog_warning_codes();
    let diagnostics = result["diagnostics"]
        .as_array()
        .expect("diagnostics are an array");
    assert!(!diagnostics.is_empty());
    for diagnostic in diagnostics {
        assert_eq!(diagnostic["severity"], "warning");
        assert_eq!(diagnostic["schema"], "podway.procedure/v2");
        assert_eq!(diagnostic["source_path"], path.display().to_string());
        let code = diagnostic["code"]
            .as_str()
            .expect("a diagnostic has a code");
        assert!(
            catalog.iter().any(|candidate| candidate == code),
            "{code} is not a catalogued lint warning"
        );
    }

    let text = run(&["procedure", "lint", &path.display().to_string()]);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    let rendered = String::from_utf8_lossy(&text.stdout);
    assert!(
        rendered.starts_with(&format!("{}:", path.display())),
        "the text report is position-first: {rendered}"
    );
    assert!(
        rendered.contains("warning NO_REACTIVATION_PATH"),
        "{rendered}"
    );
}

#[test]
fn v2aut004_warnings_as_errors_moves_only_the_exit_code() {
    let fixture = FixtureDirectory::new("lint-strict");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let permissive = lint_json(&path);
    let strict = lint_json_strict(&path);
    assert_eq!(permissive.status.code(), Some(0), "{permissive:?}");
    assert_eq!(strict.status.code(), Some(1), "{strict:?}");

    // The envelope carries a per-invocation request id and timestamp; the result body is the
    // document's own answer and must not move at all.
    assert_eq!(
        one_json(&permissive)["result"],
        one_json(&strict)["result"],
        "the result body must be identical under both flag values"
    );
    assert!(strict.stderr.is_empty());
}

#[test]
fn v2aut004_an_invalid_document_reports_one_error_and_is_never_linted() {
    let fixture = FixtureDirectory::new("lint-invalid");
    // A dangling `use`: the document parses and then fails closed-reference validation.
    let path = fixture.write(
        "invalid.yaml",
        &MINIMAL_V2_YAML.replace("      use: work\n", "      use: absent\n"),
    );

    let output = lint_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.lint");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "lint");
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics_total"], 1);
    assert!(
        result.get("digest").is_none(),
        "an inadmissible document has no digest: {result}"
    );
    let diagnostics = result["diagnostics"]
        .as_array()
        .expect("diagnostics are an array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["severity"], "error");

    // The advisory rules never ran, so no lint warning can appear beside the rejection.
    let catalog = catalog_warning_codes();
    let code = diagnostics[0]["code"].as_str().expect("a code");
    assert!(!catalog.iter().any(|candidate| candidate == code));
}

#[test]
fn v2aut004_a_v1_document_is_a_schema_failure_rather_than_a_lint_report() {
    let fixture = FixtureDirectory::new("lint-v1");
    let path = fixture.write("v1.yaml", V1_YAML);

    let output = lint_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.lint");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);
    assert_eq!(envelope["retryable"], false);
}

#[test]
fn v2aut004_a_missing_file_is_a_path_failure() {
    let fixture = FixtureDirectory::new("lint-missing");
    let path = fixture.root.join("absent.yaml");

    let output = lint_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.lint");
    assert_eq!(envelope["code"], "PROCEDURE_NOT_FOUND");
    assert_eq!(envelope["exit_code"], 1);
}

#[test]
fn v2aut004_quiet_reports_only_through_the_exit_code() {
    let fixture = FixtureDirectory::new("lint-quiet");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let permissive = run(&["--quiet", "procedure", "lint", &path.display().to_string()]);
    assert_eq!(permissive.status.code(), Some(0), "{permissive:?}");
    assert!(permissive.stdout.is_empty());
    assert!(permissive.stderr.is_empty());

    let strict = run(&[
        "--quiet",
        "procedure",
        "lint",
        &path.display().to_string(),
        "--warnings-as-errors",
    ]);
    assert_eq!(strict.status.code(), Some(1), "{strict:?}");
    assert!(strict.stdout.is_empty());
    assert!(strict.stderr.is_empty());
}

#[test]
fn v2aut004_lint_never_touches_the_file_it_reads() {
    let fixture = FixtureDirectory::new("lint-readonly");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let before = identity(&path);

    assert_eq!(lint_json(&path).status.code(), Some(0));
    assert_eq!(lint_json_strict(&path).status.code(), Some(1));

    assert_eq!(identity(&path), before);
    assert_eq!(entry_names(&fixture.root), vec!["minimal.yaml".to_owned()]);
}

#[test]
fn v2aut004_lint_is_deterministic_across_processes() {
    let fixture = FixtureDirectory::new("lint-deterministic");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let baseline = one_json(&lint_json(&path))["result"].clone();
    for _ in 0..5 {
        assert_eq!(one_json(&lint_json(&path))["result"], baseline);
    }
}

// ---------------------------------------------------------------------------------------------
// V2AUT-005: `procedure check`
// ---------------------------------------------------------------------------------------------

/// A document every authoring stage accepts: canonical authoring form *and* lint-clean.
///
/// It differs from [`CLEAN_V2_YAML`] in one way that matters here — every item is a `confirm`,
/// because canonical form materializes a `text` item's `min_length`, `max_length`, and `multiline`
/// and a fixture that omitted them would drift.
const CHECK_CLEAN_V2_YAML: &str = r#"schema: podway.procedure/v2
id: check-clean
version: "1"
name: Check clean
purpose: Exercise the aggregate authoring gate with a document that has no findings.
node_definitions:
  gather:
    type: action
    title: Gather the inputs
    intent: Collect every input the review needs.
    items:
      - id: notes
        type: confirm
        prompt: The gathered notes are recorded.
        required: true
  review:
    type: decision
    title: Review the work
    objective: Decide whether the gathered work is complete.
    prompt: Is the gathered work complete?
    evidence_guidance:
      - Read the gathered notes before deciding.
    options:
      - id: complete
        label: Work is complete
        criteria: Every gathered input is present and correct.
      - id: incomplete
        label: Work is incomplete
        criteria: Some gathered input is missing or wrong.
    reason:
      required: true
      prompt: Explain why the work is or is not complete.
  publish:
    type: action
    title: Publish the result
    intent: Record the published outcome.
graph:
  entry: gather-inputs
  nodes:
    - id: gather-inputs
      use: gather
      next: review-work
    - id: review-work
      use: review
      routes:
        complete:
          to: publish-result
          effect: advance
        incomplete:
          to: gather-inputs
          effect: rework
    - id: publish-result
      use: publish
      terminal: true
manual_rework:
  allowed_targets:
    - gather-inputs
"#;

/// The pinned one-line verdict for a document every authoring stage accepted.
fn all_checks_passed_summary(path: &Path) -> String {
    format!("{}: all authoring checks passed\n", path.display())
}

fn check_json(path: &Path) -> Output {
    run(&["--json", "procedure", "check", &path.display().to_string()])
}

fn check_json_strict(path: &Path) -> Output {
    run(&[
        "--json",
        "procedure",
        "check",
        &path.display().to_string(),
        "--warnings-as-errors",
    ])
}

/// The codes of a check result, in reported order.
fn check_codes(output: &Output) -> Vec<String> {
    one_json(output)["result"]["diagnostics"]
        .as_array()
        .expect("diagnostics are an array")
        .iter()
        .map(|diagnostic| {
            diagnostic["code"]
                .as_str()
                .expect("a diagnostic has a code")
                .to_owned()
        })
        .collect()
}

#[test]
fn v2aut005_a_clean_document_passes_every_stage_at_exit_zero() {
    let fixture = FixtureDirectory::new("check-clean");
    let path = fixture.write("clean.yaml", CHECK_CLEAN_V2_YAML);

    let output = check_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.check");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "check");
    assert_eq!(result["procedure_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["valid"], true);
    assert_eq!(result["diagnostics"], serde_json::json!([]));
    assert_eq!(result["diagnostics_truncated"], false);
    assert_eq!(result["diagnostics_total"], 0);
    assert!(
        result["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "an admissible document reports its digest: {result}"
    );

    // The digest the gate reports is the one `procedure format` derives from the same bytes.
    let formatted = one_json(&format_json(&path));
    assert_eq!(result["digest"], formatted["result"]["target_digest"]);

    let text = run(&["procedure", "check", &path.display().to_string()]);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        all_checks_passed_summary(&path)
    );
    assert!(text.stderr.is_empty());

    // The clean verdict survives the strict policy: there is nothing to escalate.
    assert_eq!(check_json_strict(&path).status.code(), Some(0));
}

/// Drift alone fails the gate, and it does so because the catalog says the code is an error — not
/// because check decided formatting is fatal.
#[test]
fn v2aut005_a_drifted_document_fails_because_drift_is_catalogued_as_an_error() {
    let fixture = FixtureDirectory::new("check-drifted");
    // One quoted scalar canonical form writes plain: the model, and therefore every later stage's
    // verdict, is untouched, so `FORMAT_NOT_CANONICAL` is the only possible finding.
    let path = fixture.write(
        "drifted.yaml",
        &CHECK_CLEAN_V2_YAML.replace("name: Check clean\n", "name: \"Check clean\"\n"),
    );

    let output = check_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.check");

    let result = &envelope["result"];
    assert_eq!(result["operation"], "check");
    assert_eq!(result["valid"], false, "drift is an error: {result}");
    assert_eq!(result["diagnostics_total"], 1);
    assert!(
        result["digest"].as_str().is_some(),
        "a drifted document is still admissible and still has a digest: {result}"
    );

    let diagnostics = result["diagnostics"]
        .as_array()
        .expect("diagnostics are an array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "FORMAT_NOT_CANONICAL");
    assert_eq!(diagnostics[0]["severity"], "error");

    // `format --check` sees the same drift, in the same place, through the same constructor.
    let drift = run(&[
        "--json",
        "procedure",
        "format",
        &path.display().to_string(),
        "--check",
    ]);
    assert_eq!(drift.status.code(), Some(1), "{drift:?}");
    assert_eq!(
        one_json(&drift)["result"]["diagnostics"],
        result["diagnostics"],
        "check and format --check must report byte-identical drift"
    );
}

/// A warning-only document is valid, and the exit code is the only thing the strict policy moves.
#[test]
fn v2aut005_a_warning_only_document_exits_zero_until_warnings_are_errors() {
    let fixture = FixtureDirectory::new("check-warnings");
    // Canonical already, and lint-dirty: the smallest legal document declares no manual rework.
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let permissive = check_json(&path);
    assert_eq!(permissive.status.code(), Some(0), "{permissive:?}");
    let result = one_json(&permissive)["result"].clone();
    assert_eq!(result["valid"], true, "warnings never invalidate: {result}");
    assert_eq!(result["diagnostics_total"], 1);
    assert_eq!(check_codes(&permissive), vec!["NO_REACTIVATION_PATH"]);
    assert!(
        !check_codes(&permissive).contains(&"FORMAT_NOT_CANONICAL".to_owned()),
        "the fixture is already canonical"
    );

    let strict = check_json_strict(&path);
    assert_eq!(strict.status.code(), Some(1), "{strict:?}");
    assert!(strict.stderr.is_empty());
    assert_eq!(
        one_json(&strict)["result"],
        result,
        "the flag is a policy about the invocation, not a statement about the document"
    );
}

#[test]
fn v2aut005_an_invalid_document_reports_one_error_without_a_digest() {
    let fixture = FixtureDirectory::new("check-invalid");
    let path = fixture.write(
        "invalid.yaml",
        &MINIMAL_V2_YAML.replace("      use: work\n", "      use: absent\n"),
    );

    let output = check_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let result = one_json(&output)["result"].clone();
    assert_eq!(result["operation"], "check");
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics_total"], 1);
    assert!(
        result.get("digest").is_none(),
        "an inadmissible document has no digest: {result}"
    );

    // No model means no formatting comparison and no advisory findings beside the rejection.
    let codes = check_codes(&output);
    assert_eq!(codes.len(), 1);
    assert!(!codes.contains(&"FORMAT_NOT_CANONICAL".to_owned()));
    assert!(!codes.contains(&"NO_REACTIVATION_PATH".to_owned()));
}

/// The reported order is the section 11.5 pipeline, not source position.
///
/// The fixture makes the two disagree on purpose: `NO_REACTIVATION_PATH` is anchored at the
/// document start and the drift sits nine lines further down, so a report ordered by position alone
/// would lead with the advisory finding.
#[test]
fn v2aut005_the_report_leads_with_the_format_stage_at_the_process_boundary() {
    let fixture = FixtureDirectory::new("check-ordering");
    let path = fixture.write(
        "drifted.yaml",
        &MINIMAL_V2_YAML.replace("    title: Work\n", "    title: \"Work\"\n"),
    );

    let output = check_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        check_codes(&output),
        vec!["FORMAT_NOT_CANONICAL", "NO_REACTIVATION_PATH"],
    );

    let result = one_json(&output)["result"].clone();
    let diagnostics = result["diagnostics"]
        .as_array()
        .expect("diagnostics are an array");
    assert_eq!(result["valid"], false, "one of the two is an error");
    assert_eq!(result["diagnostics_total"], 2);
    assert!(
        diagnostics[0]["location"]["line"].as_u64() > diagnostics[1]["location"]["line"].as_u64(),
        "the ordering proof needs the advisory finding to sit above the drift: {result}"
    );

    // The human report is the same order, one line per finding, position first.
    let text = run(&["procedure", "check", &path.display().to_string()]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let rendered = String::from_utf8_lossy(&text.stdout);
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), 2, "{rendered}");
    assert!(
        lines[0].contains("error FORMAT_NOT_CANONICAL"),
        "{rendered}"
    );
    assert!(
        lines[1].contains("warning NO_REACTIVATION_PATH"),
        "{rendered}"
    );
}

#[test]
fn v2aut005_a_v1_document_is_a_schema_failure_rather_than_a_check_report() {
    let fixture = FixtureDirectory::new("check-v1");
    let path = fixture.write("v1.yaml", V1_YAML);

    let output = check_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.check");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);
    assert_eq!(envelope["retryable"], false);
}

#[test]
fn v2aut005_a_missing_file_is_a_path_failure() {
    let fixture = FixtureDirectory::new("check-missing");
    let path = fixture.root.join("absent.yaml");

    let output = check_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.check");
    assert_eq!(envelope["code"], "PROCEDURE_NOT_FOUND");
    assert_eq!(envelope["exit_code"], 1);
}

#[test]
fn v2aut005_quiet_reports_only_through_the_exit_code() {
    let fixture = FixtureDirectory::new("check-quiet");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let permissive = run(&["--quiet", "procedure", "check", &path.display().to_string()]);
    assert_eq!(permissive.status.code(), Some(0), "{permissive:?}");
    assert!(permissive.stdout.is_empty());
    assert!(permissive.stderr.is_empty());

    let strict = run(&[
        "--quiet",
        "procedure",
        "check",
        &path.display().to_string(),
        "--warnings-as-errors",
    ]);
    assert_eq!(strict.status.code(), Some(1), "{strict:?}");
    assert!(strict.stdout.is_empty());
    assert!(strict.stderr.is_empty());
}

#[test]
fn v2aut005_check_never_touches_the_file_it_reads() {
    let fixture = FixtureDirectory::new("check-readonly");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let before = identity(&path);

    assert_eq!(check_json(&path).status.code(), Some(0));
    assert_eq!(check_json_strict(&path).status.code(), Some(1));

    assert_eq!(identity(&path), before);
    assert_eq!(entry_names(&fixture.root), vec!["minimal.yaml".to_owned()]);
}

#[test]
fn v2aut005_check_is_deterministic_across_processes() {
    let fixture = FixtureDirectory::new("check-deterministic");
    let path = fixture.write(
        "drifted.yaml",
        &MINIMAL_V2_YAML.replace("    title: Work\n", "    title: \"Work\"\n"),
    );

    let baseline = one_json(&check_json(&path))["result"].clone();
    for _ in 0..5 {
        assert_eq!(one_json(&check_json(&path))["result"], baseline);
    }
}

// ---------------------------------------------------------------------------------------------
// V2AUT-006: `procedure scaffold`
// ---------------------------------------------------------------------------------------------

fn scaffold_json(arguments: &[&str]) -> Output {
    let mut argv = vec!["--json", "procedure", "scaffold"];
    argv.extend_from_slice(arguments);
    run(&argv)
}

fn scaffold_text(arguments: &[&str]) -> Output {
    let mut argv = vec!["procedure", "scaffold"];
    argv.extend_from_slice(arguments);
    run(&argv)
}

#[test]
fn v2aut006_scaffold_emits_the_template_as_a_source_result_with_no_file_fields() {
    let output = scaffold_json(&[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());

    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.scaffold");
    assert!(envelope["warnings"].is_array());

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-source-result/v1");
    assert_eq!(result["operation"], "scaffold");
    assert_eq!(result["target_schema"], "podway.procedure/v2");
    assert_eq!(result["template"], "minimal");
    assert_eq!(result["document"], SCAFFOLD_TEMPLATE_MINIMAL);
    assert!(
        result["target_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71),
        "the source result must carry the canonical semantic digest: {result}"
    );

    // The source result schema's `scaffold` branch forbids all four: a scaffold names no file, was
    // produced in no mode, changed nothing, and converted nothing.
    for absent in ["file", "mode", "changed", "source_schema", "source_digest"] {
        assert!(
            result.get(absent).is_none(),
            "a scaffold result must not carry {absent}: {result}"
        );
    }

    // `--template minimal` is the default spelled out, so it must produce the identical result.
    let explicit = one_json(&scaffold_json(&["--template", "minimal"]));
    assert_eq!(explicit["result"], *result);
}

#[test]
fn v2aut006_scaffold_text_mode_writes_the_template_bytes_exactly() {
    let output = scaffold_text(&[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        SCAFFOLD_TEMPLATE_MINIMAL.as_bytes(),
        "text stdout must be the template with no added newline"
    );
}

/// The roadmap gate, executed end to end: what `scaffold` writes is what the rest of the toolchain
/// accepts, with no editing step in between.
#[test]
fn v2aut006_a_scaffolded_file_passes_format_check_and_the_aggregate_gate() {
    let fixture = FixtureDirectory::new("scaffold-pipeline");
    let scaffolded = scaffold_text(&[]);
    assert_eq!(scaffolded.status.code(), Some(0), "{scaffolded:?}");
    let path = fixture.root.join("scaffolded.yaml");
    fs::write(&path, &scaffolded.stdout).expect("the scaffolded document must be writable");

    let formatted = format_check_text(&path);
    assert_eq!(
        formatted.status.code(),
        Some(0),
        "a scaffolded file is already canonical: {formatted:?}"
    );
    assert_eq!(formatted.stdout, canonical_summary(&path).into_bytes());

    let checked = run(&[
        "procedure",
        "check",
        &path.display().to_string(),
        "--warnings-as-errors",
    ]);
    assert_eq!(
        checked.status.code(),
        Some(0),
        "a scaffolded file must report no finding at all: {checked:?}"
    );

    // The file is a real procedure, not merely a well-formed one: `validate` is a v1 route, so the
    // proof that the document is usable is that the v2 gate reports a digest for it.
    let envelope = one_json(&run(&[
        "--json",
        "procedure",
        "check",
        &path.display().to_string(),
    ]));
    assert_eq!(envelope["result"]["valid"], true);
    assert_eq!(envelope["result"]["diagnostics_total"], 0);
    assert_eq!(
        envelope["result"]["digest"],
        one_json(&scaffold_json(&[]))["result"]["target_digest"],
        "the digest scaffold advertises must be the digest the file has"
    );
}

#[test]
fn v2aut006_an_unknown_template_is_a_usage_failure() {
    let output = scaffold_json(&["--template", "kitchen-sink"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.scaffold");
    assert_eq!(envelope["code"], "REQUEST_INVALID");
    assert_eq!(envelope["exit_code"], 2);
    assert_eq!(envelope["retryable"], false);

    // Nothing is emitted on the way to a usage failure, in either mode.
    let text = scaffold_text(&["--template", "kitchen-sink"]);
    assert_eq!(text.status.code(), Some(2), "{text:?}");
    assert!(text.stdout.is_empty());
}

#[test]
fn v2aut006_scaffold_is_deterministic_across_processes() {
    let baseline = scaffold_text(&[]).stdout;
    let result = one_json(&scaffold_json(&[]))["result"].clone();
    for _ in 0..5 {
        assert_eq!(scaffold_text(&[]).stdout, baseline);
        assert_eq!(one_json(&scaffold_json(&[]))["result"], result);
    }
}

#[test]
fn v2aut006_quiet_suppresses_the_template_without_changing_the_exit_code() {
    let output = run(&["--quiet", "procedure", "scaffold"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

// ---------------------------------------------------------------------------------------------
// V2AUT-007: `procedure convert`
// ---------------------------------------------------------------------------------------------

fn convert_json(path: &Path) -> Output {
    run(&[
        "--json",
        "procedure",
        "convert",
        &path.display().to_string(),
    ])
}

fn convert_text(path: &Path) -> Output {
    run(&["procedure", "convert", &path.display().to_string()])
}

/// A shipped v1 preset, copied into the fixture directory so the conversion's read-only claim can
/// be checked against a directory the test owns.
fn shipped_preset(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("assets/presets/{name}.yaml"));
    fs::read_to_string(path).expect("a shipped preset must be readable")
}

#[test]
fn v2aut007_convert_reports_both_digests_and_the_source_digest_is_the_v1_validate_digest() {
    let fixture = FixtureDirectory::new("convert");
    let path = fixture.write("sw-dev.yaml", &shipped_preset("sw-dev"));

    let output = convert_json(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());

    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.convert");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-source-result/v1");
    assert_eq!(result["operation"], "convert");
    assert_eq!(result["source_schema"], "podway.procedure/v1");
    assert_eq!(result["target_schema"], "podway.procedure/v2");
    assert!(
        result["document"]
            .as_str()
            .is_some_and(|document| document.starts_with("schema: podway.procedure/v2\n")),
        "the candidate declares the v2 schema: {result}"
    );

    // The reported source digest must be the digest the v1 command reports for the same file, or
    // the two ends of the conversion cannot be pinned against each other.
    let validated = one_json(&run(&[
        "--json",
        "procedure",
        "validate",
        &path.display().to_string(),
    ]));
    assert_eq!(
        result["source_digest"], validated["result"]["digest"],
        "convert's source_digest is `procedure validate`'s digest for the same document"
    );
    assert_ne!(result["target_digest"], result["source_digest"]);

    // The source result schema's `convert` branch forbids all four.
    for absent in ["file", "mode", "changed", "template"] {
        assert!(
            result.get(absent).is_none(),
            "a convert result must not carry {absent}: {result}"
        );
    }
}

/// The documented pipeline, executed end to end: what convert writes is what the rest of the
/// toolchain accepts, with no editing step in between.
#[test]
fn v2aut007_a_converted_document_is_canonical_and_passes_the_aggregate_gate() {
    let fixture = FixtureDirectory::new("convert-pipeline");
    // `analysis` is here as well as `sw-dev` because it is the preset that sits closest to a v2
    // bound: its `max_items: 200` is exactly the v2 list cap, so the pipeline is proved at the
    // ceiling and not only in the middle of the range.
    for preset in ["analysis", "sw-dev"] {
        let source = fixture.write(&format!("{preset}.yaml"), &shipped_preset(preset));

        let converted = convert_text(&source);
        assert_eq!(converted.status.code(), Some(0), "{preset}: {converted:?}");
        let candidate = fixture.root.join(format!("{preset}-v2.yaml"));
        fs::write(&candidate, &converted.stdout).expect("the candidate must be writable");

        let formatted = format_check_text(&candidate);
        assert_eq!(
            formatted.status.code(),
            Some(0),
            "{preset}: a converted document is already canonical: {formatted:?}"
        );
        assert_eq!(formatted.stdout, canonical_summary(&candidate).into_bytes());

        let checked = run(&[
            "procedure",
            "check",
            &candidate.display().to_string(),
            "--warnings-as-errors",
        ]);
        assert_eq!(
            checked.status.code(),
            Some(0),
            "{preset}: a converted document must report no finding at all: {checked:?}"
        );
        assert_eq!(
            one_json(&check_json(&candidate))["result"]["digest"],
            one_json(&convert_json(&source))["result"]["target_digest"],
            "{preset}: the digest convert advertises must be the digest the written file has"
        );
    }
}

/// Text mode writes the candidate bytes and nothing else, so `> file` is the documented usage.
#[test]
fn v2aut007_text_mode_writes_the_candidate_bytes_exactly() {
    let fixture = FixtureDirectory::new("convert-text");
    let path = fixture.write("v1.yaml", V1_YAML);

    let output = convert_text(&path);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        one_json(&convert_json(&path))["result"]["document"]
            .as_str()
            .expect("the result carries the document")
            .as_bytes(),
        "text stdout and the JSON `document` are the same bytes"
    );
}

/// Convert reads. It creates nothing, rewrites nothing, and leaves the v1 file byte-identical.
#[test]
fn v2aut007_convert_never_writes_anything() {
    let fixture = FixtureDirectory::new("convert-readonly");
    let path = fixture.write("sw-dev.yaml", &shipped_preset("sw-dev"));
    let before = identity(&path);

    assert_eq!(convert_json(&path).status.code(), Some(0));
    assert_eq!(convert_text(&path).status.code(), Some(0));

    assert_eq!(
        identity(&path),
        before,
        "the v1 source is neither rewritten nor touched"
    );
    assert_eq!(
        entry_names(&fixture.root),
        vec!["sw-dev.yaml".to_owned()],
        "convert creates no candidate file and no staging file"
    );
}

/// A document that already declares v2 has nothing to convert. It is a wrong-schema command
/// failure rather than an authoring finding: the document is finished, not defective.
#[test]
fn v2aut007_a_v2_document_is_a_schema_failure_rather_than_a_conversion() {
    let fixture = FixtureDirectory::new("convert-v2");
    let path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);

    let output = convert_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.convert");
    assert_eq!(envelope["code"], "PROCEDURE_SCHEMA_UNSUPPORTED");
    assert_eq!(envelope["exit_code"], 1);
    assert_eq!(envelope["retryable"], false);

    let text = convert_text(&path);
    assert_eq!(text.status.code(), Some(1));
    assert!(text.stdout.is_empty());
    assert!(!text.stderr.is_empty());
}

/// A malformed v1 document reports exactly what `procedure validate` reports for it: convert
/// admits v1 through the same `parse_procedure_v1` entry point, so the v1 error surface is shared
/// rather than reimplemented.
#[test]
fn v2aut007_a_malformed_v1_document_reports_the_same_failure_procedure_validate_reports() {
    let fixture = FixtureDirectory::new("convert-malformed");
    let path = fixture.write("broken.yaml", "schema: podway.procedure/v1\nid: broken\n");

    let converted = one_json(&convert_json(&path));
    let validated = one_json(&run(&[
        "--json",
        "procedure",
        "validate",
        &path.display().to_string(),
    ]));

    assert_eq!(converted["schema"], "podway.error/v1");
    assert_eq!(converted["command"], "procedure.convert");
    assert_eq!(converted["code"], "PROCEDURE_INVALID");
    assert_eq!(converted["exit_code"], 1);
    assert_eq!(
        converted["code"], validated["code"],
        "the v1 admission failure is the same failure both commands report"
    );
    assert_eq!(converted["message"], validated["message"]);
}

/// A v1 value Procedure v2 cannot hold is a diagnostics result against the v1 path that carries it.
/// v1 admits `max_items` up to 1,000 and Procedure v2 caps a list at 200 entries, so the shipped
/// `analysis` preset — which sits exactly on the v2 cap and converts — is pushed one bound past it.
#[test]
fn v2aut007_a_v1_value_v2_cannot_hold_is_reported_against_its_v1_path() {
    let fixture = FixtureDirectory::new("convert-overflow");
    let oversized = shipped_preset("analysis").replace("max_items: 200", "max_items: 1000");
    let path = fixture.write("analysis.yaml", &oversized);

    let output = convert_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.convert");

    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "convert");
    // The candidate is what is being diagnosed, so the family's `procedure_schema` names v2 even
    // though every `field` below points into the v1 source the author has to edit.
    assert_eq!(result["procedure_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics_total"], 1);
    assert_eq!(result["diagnostics_truncated"], false);
    assert!(
        result.get("digest").is_none(),
        "there is no procedure for a digest to describe: {result}"
    );

    let diagnostic = &result["diagnostics"][0];
    assert_eq!(diagnostic["code"], "AUTHORING_SCHEMA_INVALID");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["schema"], "podway.procedure/v2");
    assert_eq!(diagnostic["source_path"], path.display().to_string());
    assert_eq!(
        diagnostic["field"],
        "stages[collect-sources].items[sources].max_items"
    );
    assert!(
        diagnostic["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("Procedure v1 source")),
        "the remedy is an edit to the v1 file, not to a v2 file that does not exist: {diagnostic}"
    );
}

#[test]
fn v2aut007_a_missing_file_is_a_path_failure() {
    let fixture = FixtureDirectory::new("convert-missing");
    let path = fixture.root.join("absent.yaml");

    let output = convert_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.error/v1");
    assert_eq!(envelope["command"], "procedure.convert");
    assert_eq!(envelope["code"], "PROCEDURE_NOT_FOUND");
    assert_eq!(envelope["exit_code"], 1);
}

#[test]
fn v2aut007_convert_is_deterministic_across_processes() {
    let fixture = FixtureDirectory::new("convert-determinism");
    let path = fixture.write("sw-dev.yaml", &shipped_preset("sw-dev"));

    let baseline = convert_text(&path).stdout;
    let result = one_json(&convert_json(&path))["result"].clone();
    for _ in 0..2 {
        assert_eq!(convert_text(&path).stdout, baseline);
        assert_eq!(one_json(&convert_json(&path))["result"], result);
    }
}

#[test]
fn v2aut007_quiet_suppresses_the_candidate_without_changing_the_exit_code() {
    let fixture = FixtureDirectory::new("convert-quiet");
    let path = fixture.write("v1.yaml", V1_YAML);

    let output = run(&[
        "--quiet",
        "procedure",
        "convert",
        &path.display().to_string(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

// ---------------------------------------------------------------------------------------------
// V2AUT-008: `procedure validate` dispatches on the declared schema
// ---------------------------------------------------------------------------------------------

fn validate_text(path: &Path) -> Output {
    run(&["procedure", "validate", &path.display().to_string()])
}

fn validate_json(path: &Path) -> Output {
    run(&[
        "--json",
        "procedure",
        "validate",
        &path.display().to_string(),
    ])
}

/// The exact stdout and exit code `procedure validate` produced for `V1_YAML` before the v2
/// dispatch existed, captured from the binary at the parent commit.
///
/// This is the byte-identity pin. The dispatch inserts a decode-only schema sniff between the
/// format decision and the v1 parser, and only a document that positively declares Procedure v2
/// leaves the v1 path — so a v1 document must produce these bytes, not merely an equivalent result.
const V1_VALIDATE_TEXT: &str =
    "Release (sha256:40265a5ce34cd76f257b1c7cbc783b30ebaa6702bc00dc161954975fed1dee77)\n";

#[test]
fn v2aut008_a_v1_document_keeps_its_exact_validate_output() {
    let fixtures = FixtureDirectory::new("v1-validate");
    let path = fixtures.write("release.yaml", V1_YAML);

    let text = validate_text(&path);
    assert_eq!(text.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&text.stdout), V1_VALIDATE_TEXT);
    assert!(text.stderr.is_empty());

    // JSON mode keeps the v1 envelope and the v1 result family; only `request_id` and
    // `generated_at` are per-run.
    let json = validate_json(&path);
    assert_eq!(json.status.code(), Some(0));
    let value = one_json(&json);
    assert_eq!(value["schema"], "podway.output/v1");
    assert_eq!(value["command"], "procedure.validate");
    let result = &value["result"];
    assert_eq!(result["schema"], "podway.procedure-validation-result/v1");
    assert_eq!(
        result["digest"],
        "sha256:40265a5ce34cd76f257b1c7cbc783b30ebaa6702bc00dc161954975fed1dee77"
    );
    assert_eq!(result["procedure"]["id"], "release");
    assert!(
        result.get("diagnostics").is_none(),
        "v1 reports no findings"
    );
}

#[test]
fn v2aut008_a_document_declaring_no_known_schema_stays_on_the_v1_path() {
    let fixtures = FixtureDirectory::new("v1-fallthrough");
    // A missing schema, an unknown schema, and an undecodable document are all `None` from the
    // sniff, which is not a positive dispatch signal: each must reach the v1 parser and report the
    // v1 failure surface it always reported.
    for (name, contents) in [
        ("no-schema.yaml", "id: nothing\nname: Nothing\n"),
        (
            "unknown-schema.yaml",
            "schema: podway.procedure/v9\nid: x\nname: X\n",
        ),
        ("undecodable.yaml", "schema: [\n"),
    ] {
        let path = fixtures.write(name, contents);
        let output = validate_text(&path);
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with("error: "), "{name}: {stderr}");

        let json = one_json(&validate_json(&path));
        assert_eq!(json["schema"], "podway.error/v1", "{name}");
        assert_eq!(json["command"], "procedure.validate", "{name}");
    }
}

#[test]
fn v2aut008_a_v2_document_validates_into_the_diagnostics_family() {
    let fixtures = FixtureDirectory::new("v2-validate-ok");
    let path = fixtures.write("workflow.yaml", MINIMAL_V2_YAML);

    let output = validate_json(&path);
    assert_eq!(output.status.code(), Some(0));
    let value = one_json(&output);
    assert_eq!(value["schema"], "podway.output/v2");
    assert_eq!(value["command"], "procedure.validate");

    let result = &value["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "validate");
    assert_eq!(result["procedure_schema"], "podway.procedure/v2");
    assert_eq!(result["file"], path.display().to_string());
    assert_eq!(result["valid"], true);
    assert_eq!(result["diagnostics"], Value::Array(Vec::new()));
    assert_eq!(result["diagnostics_truncated"], false);
    assert_eq!(result["diagnostics_total"], 0);
    assert!(
        result["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "an admissible document reports its digest"
    );

    let text = validate_text(&path);
    assert_eq!(text.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        format!(
            "{}: valid ({})\n",
            path.display(),
            result["digest"].as_str().expect("digest is a string")
        )
    );
    assert!(text.stderr.is_empty());
}

/// Three negative graph recipes taken to the process boundary.
///
/// The config-level suite proves the whole mapping; these prove the command really carries it —
/// that the code, the field, the location, and the exit code survive the CLI, the envelope, and
/// JSON serialization.
#[test]
fn v2aut008_negative_recipes_reach_the_process_boundary_with_their_codes() {
    let fixtures = FixtureDirectory::new("v2-validate-negative");
    let cases: &[(&str, String, &str, &str)] = &[
        (
            // negative-cases.json: "set graph.entry to an undefined node"
            "unknown-entry",
            MINIMAL_V2_YAML.replace("  entry: only\n", "  entry: ghost\n"),
            "ENTRY_NODE_INVALID",
            "graph.entry",
        ),
        (
            // negative-cases.json: "make two graph references resolve ambiguously"
            "ambiguous-placement-id",
            MINIMAL_V2_YAML.replace(
                "    - id: only\n",
                "    - id: only\n      use: work\n      terminal: true\n    - id: only\n",
            ),
            "AMBIGUOUS_GRAPH_REFERENCE",
            "graph.nodes",
        ),
        (
            // negative-cases.json: "allow manual rework to an undefined placement"
            "manual-rework-target-invalid",
            format!("{MINIMAL_V2_YAML}manual_rework:\n  allowed_targets:\n    - ghost\n"),
            "MANUAL_REWORK_TARGET_UNKNOWN",
            "manual_rework.allowed_targets",
        ),
    ];

    for (id, source, code, field) in cases {
        let path = fixtures.write(&format!("{id}.yaml"), source);
        let output = validate_json(&path);
        assert_eq!(output.status.code(), Some(1), "{id}");

        let value = one_json(&output);
        assert_eq!(value["schema"], "podway.output/v2", "{id}");
        let result = &value["result"];
        assert_eq!(result["valid"], false, "{id}");
        assert_eq!(result["diagnostics_total"], 1, "{id}");
        assert_eq!(result["diagnostics_truncated"], false, "{id}");
        assert!(result.get("digest").is_none(), "{id}: no digest exists");

        let diagnostics = result["diagnostics"].as_array().expect("an array");
        assert_eq!(diagnostics.len(), 1, "{id}: validate is single-error");
        assert_eq!(diagnostics[0]["code"], *code, "{id}");
        assert_eq!(diagnostics[0]["field"], *field, "{id}");
        assert_eq!(diagnostics[0]["severity"], "error", "{id}");
        assert_eq!(diagnostics[0]["source_path"], path.display().to_string());
        assert!(
            diagnostics[0]["location"]["line"]
                .as_u64()
                .is_some_and(|line| line >= 1),
            "{id}"
        );
        assert!(
            !diagnostics[0]["hint"].as_str().expect("a hint").is_empty(),
            "{id}"
        );

        // Text mode renders the same finding on one line and exits the same way.
        let text = validate_text(&path);
        assert_eq!(text.status.code(), Some(1), "{id}");
        let rendered = String::from_utf8_lossy(&text.stdout);
        assert_eq!(rendered.lines().count(), 1, "{id}: {rendered}");
        assert!(rendered.contains(code), "{id}: {rendered}");
    }
}

#[test]
fn v2aut008_warnings_as_errors_is_a_no_op_for_a_v2_document() {
    let fixtures = FixtureDirectory::new("v2-validate-wae");
    // Validate emits only error-severity findings, so the policy has nothing to promote: the
    // result body and the exit code are identical with and without the flag, for an admissible
    // document and for a rejected one alike.
    for (name, contents) in [
        ("clean.yaml", MINIMAL_V2_YAML.to_owned()),
        (
            "rejected.yaml",
            MINIMAL_V2_YAML.replace("      use: work\n", "      use: absent\n"),
        ),
    ] {
        let path = fixtures.write(name, &contents);
        let plain = validate_json(&path);
        let flagged = run(&[
            "--json",
            "procedure",
            "validate",
            &path.display().to_string(),
            "--warnings-as-errors",
        ]);
        assert_eq!(plain.status.code(), flagged.status.code(), "{name}");
        assert_eq!(
            one_json(&plain)["result"],
            one_json(&flagged)["result"],
            "{name}"
        );
    }
}

fn graph_json(path: &Path) -> Output {
    run(&[
        "--json",
        "procedure",
        "graph",
        &path.display().to_string(),
        "--format",
        "json",
    ])
}

fn graph_text(path: &Path) -> Output {
    run(&[
        "procedure",
        "graph",
        &path.display().to_string(),
        "--format",
        "json",
    ])
}

fn oversized_graph_projection_source() -> String {
    let option_ids = (0..8)
        .map(|index| format!("option-{index:02}-{}", "x".repeat(54)))
        .collect::<Vec<_>>();
    let node_ids = (0..63)
        .map(|index| format!("node-{index:02}-{}", "x".repeat(56)))
        .collect::<Vec<_>>();
    let terminal_id = format!("terminal-{}", "x".repeat(55));
    let options = option_ids
        .iter()
        .map(|id| format!("      - id: {id}\n        label: Choice\n"))
        .collect::<String>();
    let nodes = node_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let target = node_ids.get(index + 1).unwrap_or(&terminal_id);
            let routes = option_ids
                .iter()
                .map(|option| {
                    format!(
                        "        {option}:\n          to: {target}\n          effect: advance\n"
                    )
                })
                .collect::<String>();
            format!("    - id: {id}\n      use: choose\n      routes:\n{routes}")
        })
        .collect::<String>();
    format!(
        "schema: podway.procedure/v2\nid: projection-cap\nversion: \"1\"\nname: Projection cap\npurpose: Prove graph projections are independently bounded.\nnode_definitions:\n  choose:\n    type: decision\n    title: Choose\n    objective: Select a route.\n    prompt: Which route?\n    options:\n{options}    reason:\n      required: true\n  finish:\n    type: action\n    title: Finish\n    intent: Finish.\ngraph:\n  entry: {}\n  nodes:\n{nodes}    - id: {terminal_id}\n      use: finish\n      terminal: true\n",
        node_ids[0]
    )
}

#[test]
fn v2grf003_graph_json_is_deterministic_digest_bound_and_read_only() {
    let fixture = FixtureDirectory::new("graph-success");
    let yaml_path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let json_path = fixture.write(
        "minimal.json",
        r#"{"schema":"podway.procedure/v2","id":"minimal","version":"1","name":"Minimal","purpose":"The smallest legal Procedure v2 document.","node_definitions":{"work":{"type":"action","title":"Work","intent":"Do the work."}},"graph":{"entry":"only","nodes":[{"id":"only","use":"work","terminal":true}]}}"#,
    );
    let yaml_identity = identity(&yaml_path);
    let json_identity = identity(&json_path);
    let entries = entry_names(&fixture.root);

    let first = graph_json(&yaml_path);
    let second = graph_json(&yaml_path);
    let from_json = graph_json(&json_path);
    for output in [&first, &second, &from_json] {
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stderr.is_empty());
    }
    let first = one_json(&first);
    let second = one_json(&second);
    let from_json = one_json(&from_json);
    assert_eq!(first["schema"], "podway.output/v2");
    assert_eq!(first["command"], "procedure.graph");
    let result = &first["result"];
    assert_eq!(result["schema"], "podway.procedure-graph-result/v1");
    assert_eq!(result["procedure_schema"], "podway.procedure/v2");
    assert_eq!(result["format"], "json");
    assert_eq!(result, &second["result"]);
    assert_eq!(result, &from_json["result"]);

    let projection = result["projection"]
        .as_str()
        .expect("graph result must contain the projection");
    assert!(!projection.ends_with('\n'));
    let graph: Value = serde_json::from_str(projection).expect("projection must be JSON");
    assert_eq!(graph["procedure_schema"], "podway.procedure/v2");
    assert_eq!(graph["procedure_digest"], result["procedure_digest"]);
    assert_eq!(graph["entry_graph_node_id"], "only");
    assert_eq!(
        graph["terminal_graph_node_ids"],
        serde_json::json!(["only"])
    );
    assert_eq!(graph["nodes"].as_array().map(Vec::len), Some(1));
    assert_eq!(graph["edges"].as_array().map(Vec::len), Some(0));
    assert!(graph.get("schema").is_none());
    assert!(graph["nodes"][0].get("title").is_none());
    assert_eq!(
        result["projection_digest"],
        format!("sha256:{:x}", Sha256::digest(projection.as_bytes()))
    );

    let text = graph_text(&yaml_path);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert!(text.stderr.is_empty());
    assert_eq!(text.stdout, format!("{projection}\n").as_bytes());

    assert_eq!(identity(&yaml_path), yaml_identity);
    assert_eq!(identity(&json_path), json_identity);
    assert_eq!(entry_names(&fixture.root), entries);
}

#[test]
fn v2grf003_graph_stops_at_validation_or_vet_and_binds_only_validated_digests() {
    let fixture = FixtureDirectory::new("graph-rejections");
    let parse_path = fixture.write("parse.yaml", "schema: podway.procedure/v2\ngraph: [\n");
    let validate_path = fixture.write(
        "validate.yaml",
        &MINIMAL_V2_YAML.replace("      use: work\n", "      use: absent\n"),
    );
    let vet_path = fixture.write(
        "vet.yaml",
        &MINIMAL_V2_YAML.replace(
            "    - id: only\n      use: work\n      terminal: true\n",
            "    - id: only\n      use: work\n      terminal: true\n    - id: unreachable\n      use: work\n      terminal: true\n",
        ),
    );

    for (path, expected_code, has_digest) in [
        (&parse_path, "SOURCE_CONSTRUCT_UNSUPPORTED", false),
        (&validate_path, "GRAPH_DEFINITION_UNKNOWN", false),
        (&vet_path, "UNREACHABLE_GRAPH_NODE", true),
    ] {
        let output = graph_json(path);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stderr.is_empty());
        let envelope = one_json(&output);
        assert_eq!(envelope["schema"], "podway.output/v2");
        assert_eq!(envelope["command"], "procedure.graph");
        let result = &envelope["result"];
        assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
        assert_eq!(result["operation"], "graph");
        assert_eq!(result["valid"], false);
        assert_eq!(result["diagnostics"][0]["code"], expected_code);
        assert_eq!(result.get("digest").is_some(), has_digest);
        assert!(result.get("projection").is_none());
    }
}

#[test]
fn v2grf003_graph_preserves_process_failures_quiet_mode_and_closed_format_usage() {
    let fixture = FixtureDirectory::new("graph-process");
    let v2_path = fixture.write("minimal.yaml", MINIMAL_V2_YAML);
    let v1_path = fixture.write("legacy.yaml", V1_YAML);

    for (path, code) in [
        (
            v1_path.display().to_string(),
            "PROCEDURE_SCHEMA_UNSUPPORTED",
        ),
        (
            fixture.root.join("missing.yaml").display().to_string(),
            "PROCEDURE_NOT_FOUND",
        ),
    ] {
        let output = run(&["--json", "procedure", "graph", &path, "--format", "json"]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let error = one_json(&output);
        assert_eq!(error["schema"], "podway.error/v1");
        assert_eq!(error["command"], "procedure.graph");
        assert_eq!(error["code"], code);
    }

    let quiet = run(&[
        "--quiet",
        "procedure",
        "graph",
        &v2_path.display().to_string(),
        "--format",
        "json",
    ]);
    assert_eq!(quiet.status.code(), Some(0), "{quiet:?}");
    assert!(quiet.stdout.is_empty());
    assert!(quiet.stderr.is_empty());

    for arguments in [
        vec![
            "--json",
            "procedure",
            "graph",
            &v2_path.display().to_string(),
        ],
        vec![
            "--json",
            "procedure",
            "graph",
            &v2_path.display().to_string(),
            "--format",
            "mermaid",
        ],
    ] {
        let output = run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let error = one_json(&output);
        assert_eq!(error["schema"], "podway.error/v1");
        assert_eq!(error["command"], "procedure.graph");
        assert_eq!(error["code"], "REQUEST_INVALID");
    }
}

#[test]
fn v2grf003_graph_reports_its_projection_budget_after_vetting() {
    let fixture = FixtureDirectory::new("graph-projection-cap");
    let path = fixture.write("large.yaml", &oversized_graph_projection_source());

    let output = graph_json(&path);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty());
    let envelope = one_json(&output);
    assert_eq!(envelope["schema"], "podway.output/v2");
    assert_eq!(envelope["command"], "procedure.graph");
    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.procedure-diagnostics-result/v1");
    assert_eq!(result["operation"], "graph");
    assert_eq!(result["valid"], false);
    assert!(
        result["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    assert_eq!(
        result["diagnostics"][0]["code"],
        "GRAPH_PROJECTION_BUDGET_EXCEEDED"
    );
    assert_eq!(result["diagnostics"][0]["field"], "$");
    assert!(result.get("projection").is_none());
}
