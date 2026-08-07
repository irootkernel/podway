//! Process-boundary contracts for the Procedure v2 authoring surface (`procedure format`).

use std::{
    fs,
    os::unix::fs::PermissionsExt,
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
