//! V2AUT-001: canonical authoring form for Procedure v2 (dossier section 11.1).
//!
//! Section 11.1's requirements are properties, not examples, so this file asserts them as
//! properties over a corpus: output is deterministic and idempotent, author order is preserved
//! where order is meaningful, formatting never changes the canonical semantic digest, nothing is
//! silently discarded, and a document whose canonical authoring form would exceed
//! `SOURCE_PROJECTION_MAX_CHARACTERS` is rejected rather than truncated.
//!
//! One golden pins the emitter's actual bytes. That golden is the reviewable specification of the
//! layout — materialized defaults, key order, quoting, indentation — and every property below is
//! what keeps it from being merely one accepted answer among many.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use podway_config::{
    AuthoringContext, AuthoringStage, ConfigError, FormatFailure, FormatRequest,
    FormattedProcedureV2, ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2,
    finalize_diagnostics, format_procedure_v2, parse_procedure_document, sniff_procedure_schema,
    validate_procedure_v2,
};
use podway_core::{
    AuthoringDiagnostic, AuthoringDiagnosticCode, AuthoringSeverity, MAX_AUTHORING_DIAGNOSTICS,
    SOURCE_PROJECTION_MAX_CHARACTERS, SourceLocation,
};
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", path.display()))
}

fn admit(source: &str, format: ProcedureDocumentFormat) -> ValidatedProcedureV2 {
    match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => {
            validate_procedure_v2(parsed).expect("corpus document validates")
        }
        Ok(ParsedProcedure::V1(_)) => panic!("expected v2 dispatch, got v1"),
        Err(error) => panic!("corpus document must parse: {error}"),
    }
}

fn format_ok(source: &str, format: ProcedureDocumentFormat) -> FormattedProcedureV2 {
    match format_procedure_v2(FormatRequest {
        source,
        source_path: "workflow.yaml",
        format,
    }) {
        Ok(formatted) => formatted,
        Err(failure) => panic!("expected a formatted document, got {failure:?}"),
    }
}

fn format_yaml(source: &str) -> FormattedProcedureV2 {
    format_ok(source, ProcedureDocumentFormat::Yaml)
}

fn diagnostics(source: &str, format: ProcedureDocumentFormat) -> Vec<AuthoringDiagnostic> {
    match format_procedure_v2(FormatRequest {
        source,
        source_path: "workflow.yaml",
        format,
    }) {
        Err(FormatFailure::Diagnostics(diagnostics)) => diagnostics,
        other => panic!("expected diagnostics, got {other:?}"),
    }
}

/// Every YAML corpus member: a name and a source document.
fn yaml_corpus() -> Vec<(&'static str, String)> {
    vec![
        (
            "equivalent-procedure.yaml",
            fixture("tests/fixtures/v2/procedures/equivalent-procedure.yaml"),
        ),
        ("kitchen-sink", KITCHEN_SINK_YAML.to_owned()),
        ("branch", BRANCH_YAML.to_owned()),
        ("commented", COMMENTED_YAML.to_owned()),
        ("minimal", MINIMAL_YAML.to_owned()),
    ]
}

// ---------------------------------------------------------------------------------------------
// Corpus documents
// ---------------------------------------------------------------------------------------------

/// Every emitter branch in one document: all six item types, both optional integer bounds, a skip
/// policy, an optional evidence reference with a selector, definition-level descriptions and
/// instructions, option criteria, evidence guidance, a rework route, and an empty root
/// `description` (the one string the plain-scalar predicate must quote for being empty).
const KITCHEN_SINK_YAML: &str = r#"schema: podway.procedure/v2
id: kitchen-sink
version: "1"
name: Kitchen sink
purpose: Exercise every emitter branch in one document.
description: ""
node_definitions:
  gather:
    type: action
    title: Gather
    intent: Collect every recorded item kind.
    description: An action with every item type.
    instructions:
      - Read the brief.
      - Record everything.
    items:
      - id: agreed
        type: confirm
        prompt: Everything is agreed.
        help: Confirm before continuing.
        required: true
      - id: summary
        type: text
        prompt: Summarize the work.
        required: false
        min_length: 1
        max_length: 200
        multiline: false
      - id: kind
        type: choice
        prompt: Which kind?
        required: true
        choices:
          - first
          - second
      - id: count
        type: integer
        prompt: How many?
        required: false
        minimum: -5
        maximum: 5
      - id: findings
        type: list
        prompt: List the findings.
        required: false
        min_items: 1
        max_items: 3
        max_item_length: 40
        unique: false
      - id: report
        type: artifact
        prompt: Attach the report.
        required: false
        allowed_media_types:
          - text/plain
          - application/pdf
  choose:
    type: decision
    title: Choose
    description: A decision with guidance.
    objective: Choose the branch.
    prompt: Which branch?
    evidence_guidance:
      - Read the gathered summary.
    items:
      - id: rationale
        type: text
        prompt: Why?
        required: true
    options:
      - id: left
        label: Left
        criteria: When the left path applies.
      - id: right
        label: Right
    reason:
      required: true
      prompt: Explain the choice.
graph:
  entry: collect
  nodes:
    - id: collect
      use: gather
      skip:
        allowed: true
        reason_required: true
      next: decide
    - id: decide
      use: choose
      evidence_from:
        - node: collect
          required: false
          items:
            - summary
            - findings
      routes:
        left:
          to: finish
          effect: advance
        right:
          to: collect
          effect: rework
    - id: finish
      use: gather
      terminal: true
manual_rework:
  allowed_targets:
    - collect
    - decide
"#;

/// Two independent terminals behind one decision. Swapping either the two definitions (no meaning)
/// or the two terminal placements (meaning) leaves every closed reference intact, which is what
/// makes it the right document for the author-order property.
const BRANCH_YAML: &str = r#"schema: podway.procedure/v2
id: branch
version: "1"
name: Branch
purpose: Two independent terminals behind one decision.
node_definitions:
  pick:
    type: decision
    title: Pick
    objective: Choose a branch.
    prompt: Which branch?
    options:
      - id: left
        label: Left
      - id: right
        label: Right
    reason:
      required: true
  work:
    type: action
    title: Work
    intent: Do the work.
graph:
  entry: start
  nodes:
    - id: start
      use: pick
      routes:
        left:
          to: alpha
          effect: advance
        right:
          to: beta
          effect: advance
    - id: alpha
      use: work
      terminal: true
    - id: beta
      use: work
      terminal: true
"#;

const DEFINITION_PICK: &str = r#"  pick:
    type: decision
    title: Pick
    objective: Choose a branch.
    prompt: Which branch?
    options:
      - id: left
        label: Left
      - id: right
        label: Right
    reason:
      required: true
"#;
const DEFINITION_WORK: &str = r#"  work:
    type: action
    title: Work
    intent: Do the work.
"#;

/// The four supported comment placements: a leading block, a block before a nested mapping key, a
/// block before a sequence element, and a trailing block. Blank lines around them are deliberate:
/// the formatter owns vertical whitespace, so they must not survive.
const COMMENTED_YAML: &str = r#"# Podway procedure, annotated.
# The formatter must not discard this block.

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
    instructions:
      # The first instruction matters most.
      - Read the brief.
      - Record the outcome.
graph:
  entry: start
  nodes:
    # The entry placement.
    - id: start
      use: work
      next: finish
    - id: finish
      use: work
      terminal: true
# Nothing follows; this is the trailing block.
"#;

/// The smallest legal Procedure v2 document: one action definition, one terminal placement.
const MINIMAL_YAML: &str = r#"schema: podway.procedure/v2
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

/// A Procedure v1 document: never a `procedure format` target.
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

// ---------------------------------------------------------------------------------------------
// 1. The golden: the emitter's bytes, reviewable
// ---------------------------------------------------------------------------------------------

/// The exact canonical authoring form of `tests/fixtures/v2/procedures/equivalent-procedure.yaml`.
///
/// Every layout rule is visible here and nowhere else: `version` is quoted because `2` would
/// resolve to an integer; the text item's `min_length`, `max_length`, and `multiline` are
/// materialized although the source omits two of them; the evidence reference carries an explicit
/// `required`; items read `id, type, prompt, ..., required` in schema `properties` order rather
/// than in the canonical projection's insertion order; `node_definitions`, `routes`, and `outcomes`
/// keep author order rather than the byte-sorted order the digest sees; a sequence's `- ` marker
/// sits at the child indent with the element's first key on the marker line.
const GOLDEN_EQUIVALENT_PROCEDURE_YAML: &str = r#"schema: podway.procedure/v2
id: fixture-equivalence
version: "2"
name: Fixture equivalence
purpose: Lock YAML and JSON to one structural Procedure value.
description: A reviewable v2 known answer.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Do the work
    intent: Record the result.
    instructions:
      - Work outside Podway.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        min_length: 0
        max_length: 1000
        multiline: true
  assess:
    type: decision
    title: Assess the goal
    objective: Determine the goal outcome.
    prompt: Which outcome applies?
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
      prompt: Explain the assessment.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
graph:
  entry: perform
  nodes:
    - id: perform
      use: work
      next: decide
    - id: decide
      use: assess
      evidence_from:
        - node: perform
          required: true
          items:
            - result
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: work
      terminal: true
manual_rework:
  allowed_targets:
    - perform
"#;

/// The exact canonical authoring form of the JSON half of the same fixture pair.
const GOLDEN_EQUIVALENT_PROCEDURE_JSON: &str = r#"{
  "schema": "podway.procedure/v2",
  "id": "fixture-equivalence",
  "version": "2",
  "name": "Fixture equivalence",
  "purpose": "Lock YAML and JSON to one structural Procedure value.",
  "description": "A reviewable v2 known answer.",
  "goal_tracking": true,
  "node_definitions": {
    "work": {
      "type": "action",
      "title": "Do the work",
      "intent": "Record the result.",
      "instructions": [
        "Work outside Podway."
      ],
      "items": [
        {
          "id": "result",
          "type": "text",
          "prompt": "Record the result.",
          "required": true,
          "min_length": 0,
          "max_length": 1000,
          "multiline": true
        }
      ]
    },
    "assess": {
      "type": "decision",
      "title": "Assess the goal",
      "objective": "Determine the goal outcome.",
      "prompt": "Which outcome applies?",
      "options": [
        {
          "id": "achieved",
          "label": "Achieved"
        },
        {
          "id": "not-achieved",
          "label": "Not achieved"
        },
        {
          "id": "superseded",
          "label": "Superseded"
        }
      ],
      "reason": {
        "required": true,
        "prompt": "Explain the assessment."
      },
      "assessment": {
        "target": "session_goal",
        "outcomes": {
          "achieved": "achieved",
          "not-achieved": "not_achieved",
          "superseded": "superseded"
        }
      }
    }
  },
  "graph": {
    "entry": "perform",
    "nodes": [
      {
        "id": "perform",
        "use": "work",
        "next": "decide"
      },
      {
        "id": "decide",
        "use": "assess",
        "evidence_from": [
          {
            "node": "perform",
            "required": true,
            "items": [
              "result"
            ]
          }
        ],
        "routes": {
          "achieved": {
            "to": "finish",
            "effect": "advance"
          },
          "not-achieved": {
            "to": "finish",
            "effect": "advance"
          },
          "superseded": {
            "to": "finish",
            "effect": "advance"
          }
        }
      },
      {
        "id": "finish",
        "use": "work",
        "terminal": true
      }
    ]
  },
  "manual_rework": {
    "allowed_targets": [
      "perform"
    ]
  }
}
"#;

#[test]
fn v2aut001_golden_canonical_authoring_form_is_byte_pinned_for_both_encodings() {
    let yaml = fixture("tests/fixtures/v2/procedures/equivalent-procedure.yaml");
    let formatted = format_yaml(&yaml);
    assert_eq!(formatted.document(), GOLDEN_EQUIVALENT_PROCEDURE_YAML);
    assert!(
        formatted.changed(),
        "the fixture omits documented defaults, so canonical form must differ from it"
    );

    let json = fixture("tests/fixtures/v2/procedures/equivalent-procedure.json");
    let formatted_json = format_ok(&json, ProcedureDocumentFormat::Json);
    assert_eq!(formatted_json.document(), GOLDEN_EQUIVALENT_PROCEDURE_JSON);

    // Layout invariants the golden embodies, asserted so a future golden edit cannot quietly break
    // one of them.
    for document in [
        GOLDEN_EQUIVALENT_PROCEDURE_YAML,
        GOLDEN_EQUIVALENT_PROCEDURE_JSON,
    ] {
        assert!(!document.contains('\r'));
        assert!(!document.contains("\n\n"));
        assert!(document.ends_with('\n'));
        assert!(!document.ends_with("\n\n"));
        assert!(!document.starts_with('\u{feff}'));
        assert!(!document.contains("---"));
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Digest preservation, idempotence, fixpoint, determinism
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut001_formatting_preserves_the_canonical_bytes_and_the_digest() {
    for (name, source) in yaml_corpus() {
        let before = admit(&source, ProcedureDocumentFormat::Yaml);
        let formatted = format_yaml(&source);
        let after = admit(formatted.document(), ProcedureDocumentFormat::Yaml);

        assert_eq!(after.digest(), before.digest(), "{name}");
        assert_eq!(
            after.canonical_json().as_bytes(),
            before.canonical_json().as_bytes(),
            "{name}"
        );
        // The digest the formatter reports is the digest of the document it produced.
        assert_eq!(formatted.digest(), before.digest(), "{name}");
    }
}

#[test]
fn v2aut001_formatting_is_idempotent_and_its_output_is_a_fixpoint() {
    for (name, source) in yaml_corpus() {
        let once = format_yaml(&source);
        let twice = format_yaml(once.document());

        assert_eq!(twice.document(), once.document(), "{name}");
        assert!(
            !twice.changed(),
            "{name}: canonical form must already be canonical"
        );
        // The output re-parses and re-validates: it is a legal Procedure v2 document, not merely
        // pretty text.
        admit(once.document(), ProcedureDocumentFormat::Yaml);
    }
}

#[test]
fn v2aut001_formatting_is_deterministic_across_one_hundred_runs() {
    for (name, source) in yaml_corpus() {
        let expected = format_yaml(&source).document().to_owned();
        for run in 0..100 {
            assert_eq!(
                format_yaml(&source).document(),
                expected,
                "{name} run {run}"
            );
        }
    }
    let json = fixture("tests/fixtures/v2/procedures/equivalent-procedure.json");
    let expected = format_ok(&json, ProcedureDocumentFormat::Json)
        .document()
        .to_owned();
    for run in 0..100 {
        assert_eq!(
            format_ok(&json, ProcedureDocumentFormat::Json).document(),
            expected,
            "json run {run}"
        );
    }
}

#[test]
fn v2aut001_yaml_and_json_share_a_digest_while_their_documents_differ() {
    let yaml = format_yaml(&fixture(
        "tests/fixtures/v2/procedures/equivalent-procedure.yaml",
    ));
    let json = format_ok(
        &fixture("tests/fixtures/v2/procedures/equivalent-procedure.json"),
        ProcedureDocumentFormat::Json,
    );

    assert_eq!(yaml.digest(), json.digest());
    assert_ne!(
        yaml.document(),
        json.document(),
        "there is no cross-format conversion: each encoding formats to itself"
    );
    assert!(yaml.document().starts_with("schema: podway.procedure/v2\n"));
    assert!(json.document().starts_with("{\n"));
    // The JSON authoring text is not Canonical JSON v1: that form is single-line and byte-sorted.
    assert_ne!(
        json.document().trim_end(),
        admit(
            &fixture("tests/fixtures/v2/procedures/equivalent-procedure.json"),
            ProcedureDocumentFormat::Json
        )
        .canonical_json()
        .as_str()
    );
}

// ---------------------------------------------------------------------------------------------
// 3. Author order is preserved where order is not meaning
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut001_author_map_order_survives_formatting_without_moving_the_digest() {
    let swapped = BRANCH_YAML.replace(
        &format!("{DEFINITION_PICK}{DEFINITION_WORK}"),
        &format!("{DEFINITION_WORK}{DEFINITION_PICK}"),
    );
    assert_ne!(swapped, BRANCH_YAML, "the swap anchor must match");

    let original = format_yaml(BRANCH_YAML);
    let reordered = format_yaml(&swapped);

    assert_eq!(
        original.digest(),
        reordered.digest(),
        "map key order is not meaning"
    );
    assert_ne!(
        original.document(),
        reordered.document(),
        "map key order is still the author's text"
    );

    let position = |document: &str, key: &str| {
        document
            .find(&format!("\n  {key}:\n"))
            .unwrap_or_else(|| panic!("{key} must appear as a node_definitions key"))
    };
    assert!(position(original.document(), "pick") < position(original.document(), "work"));
    assert!(position(reordered.document(), "work") < position(reordered.document(), "pick"));

    // Array order is meaning, and formatting preserves it too — with a different digest.
    let placements = BRANCH_YAML.replace(
        "    - id: alpha\n      use: work\n      terminal: true\n    - id: beta\n      use: work\n      terminal: true\n",
        "    - id: beta\n      use: work\n      terminal: true\n    - id: alpha\n      use: work\n      terminal: true\n",
    );
    assert_ne!(placements, BRANCH_YAML);
    let placements = format_yaml(&placements);
    assert_ne!(placements.digest(), original.digest());
    assert!(
        placements.document().find("- id: beta").unwrap()
            < placements.document().find("- id: alpha").unwrap()
    );
}

// ---------------------------------------------------------------------------------------------
// 4. Scalar round-trip over an adversarial table
// ---------------------------------------------------------------------------------------------

/// Strings whose plain YAML rendering would resolve to something else, would open an indicator,
/// would be trimmed, or would leave the line lexer in a non-terminal state. `description` accepts
/// any text of up to 1,000 characters, so it is the one field that can hold all of them.
fn adversarial_scalars() -> Vec<String> {
    let mut values: Vec<String> = [
        "",
        "   ",
        "true",
        "True",
        "TRUE",
        "false",
        "no",
        "YES",
        "off",
        "y",
        "n",
        "~",
        "null",
        "Null",
        "NULL",
        "0",
        "007",
        "1_000",
        "0x10",
        "0o7",
        "0b101",
        "+5",
        "-0",
        "1.5",
        "1e3",
        ".inf",
        ".nan",
        "-",
        "- x",
        "a: b",
        "a:",
        "x # y",
        "#x",
        "*x",
        "&x",
        "!x",
        "@x",
        "%x",
        "`x",
        "|x",
        ">x",
        "{x}",
        "[x]",
        "?x",
        ",x",
        "line1\nline2",
        "tab\there",
        "carriage\rreturn",
        "\u{85}",
        "\u{2028}line separator",
        "\u{2029}paragraph separator",
        "zero\u{0}width",
        "bom\u{feff}inside",
        "emoji \u{1f3af} target",
        "trailing ",
        " leading",
        "  padded  ",
        "don't",
        "say \"hi\"",
        "a- |",
        "a [b",
        "back\\slash",
        "<placeholder>",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    values.push("r".repeat(1_000));
    values
}

/// Renders `value` into a YAML double-quoted scalar so the *source* can hold any string, including
/// the ones the emitter itself would have to escape.
fn yaml_double_quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character
                if character.is_control()
                    || matches!(character, '\u{2028}' | '\u{2029}' | '\u{feff}') =>
            {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

fn adversarial_yaml(description: &str) -> String {
    format!(
        "schema: podway.procedure/v2\nid: adversarial\nversion: \"1\"\nname: Adversarial\n\
         purpose: Round-trip adversarial scalars.\ndescription: {}\nnode_definitions:\n  work:\n    \
         type: action\n    title: Work\n    intent: Do the work.\ngraph:\n  entry: only\n  nodes:\n    \
         - id: only\n      use: work\n      terminal: true\n",
        yaml_double_quoted(description)
    )
}

fn adversarial_json(description: &str) -> String {
    format!(
        concat!(
            r#"{{"schema":"podway.procedure/v2","id":"adversarial","version":"1","#,
            r#""name":"Adversarial","purpose":"Round-trip adversarial scalars.","description":{},"#,
            r#""node_definitions":{{"work":{{"type":"action","title":"Work","intent":"Do the work."}}}},"#,
            r#""graph":{{"entry":"only","nodes":[{{"id":"only","use":"work","terminal":true}}]}}}}"#,
        ),
        serde_json::to_string(description).expect("a string always serializes")
    )
}

fn description_of(document: &str, format: ProcedureDocumentFormat) -> String {
    match parse_procedure_document(document.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed
            .description()
            .expect("the corpus always declares a description")
            .to_owned(),
        Ok(ParsedProcedure::V1(_)) => panic!("expected v2"),
        Err(error) => panic!("formatted output must re-parse: {error}"),
    }
}

#[test]
fn v2aut001_adversarial_scalars_survive_formatting_in_both_encodings() {
    for value in adversarial_scalars() {
        let yaml = format_yaml(&adversarial_yaml(&value));
        assert_eq!(
            description_of(yaml.document(), ProcedureDocumentFormat::Yaml),
            value,
            "yaml round-trip of {value:?}"
        );
        assert_eq!(
            format_yaml(yaml.document()).document(),
            yaml.document(),
            "yaml idempotence for {value:?}"
        );

        let json = format_ok(&adversarial_json(&value), ProcedureDocumentFormat::Json);
        assert_eq!(
            description_of(json.document(), ProcedureDocumentFormat::Json),
            value,
            "json round-trip of {value:?}"
        );
        assert_eq!(
            yaml.digest(),
            json.digest(),
            "both encodings must digest identically for {value:?}"
        );

        // A multi-line string never becomes a block scalar: the emitter writes it as one
        // double-quoted line with `\n` escapes.
        if value.contains('\n') {
            assert!(yaml.document().contains("\\n"), "{value:?}");
            assert!(!yaml.document().contains("description: |"), "{value:?}");
            assert!(!yaml.document().contains("description: >"), "{value:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 5. Comments
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut001_full_line_comment_blocks_survive_formatting_and_reformatting() {
    let formatted = format_yaml(COMMENTED_YAML);
    let document = formatted.document();

    // Every authored comment line is still present, exactly once, with its text verbatim.
    for comment in [
        "# Podway procedure, annotated.",
        "# The formatter must not discard this block.",
        "# The reusable contract every placement below uses.",
        "# The first instruction matters most.",
        "# The entry placement.",
        "# Nothing follows; this is the trailing block.",
    ] {
        assert_eq!(
            document.matches(comment).count(),
            1,
            "{comment:?} must survive exactly once:\n{document}"
        );
    }

    // Each block sits immediately above the line it annotates, at that line's indentation.
    assert!(document.starts_with(
        "# Podway procedure, annotated.\n# The formatter must not discard this block.\nschema: podway.procedure/v2\n"
    ));
    assert!(document.contains("  # The reusable contract every placement below uses.\n  work:\n"));
    assert!(
        document.contains("      # The first instruction matters most.\n      - Read the brief.\n")
    );
    assert!(document.contains("    # The entry placement.\n    - id: start\n"));
    assert!(document.ends_with("# Nothing follows; this is the trailing block.\n"));
    // Blank lines are the formatter's to own, so the authored ones are gone.
    assert!(!document.contains("\n\n"));

    // Comments never reach the digest, and a second pass reproduces the first byte for byte.
    assert_eq!(format_yaml(document).document(), document);
    let stripped: String = COMMENTED_YAML
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(format_yaml(&stripped).digest(), formatted.digest());
}

// ---------------------------------------------------------------------------------------------
// 6. Unsupported source constructs
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut001_unsupported_source_constructs_are_rejected_before_any_output() {
    // Every case the lexical scan itself detects, pinned at the construct's first character and at
    // the node it sits on.
    let scanned: [(&str, String, u32, u32, &str); 7] = [
        (
            "inline trailing comment",
            MINIMAL_YAML.replace("id: minimal\n", "id: minimal # the identifier\n"),
            2,
            13,
            "id",
        ),
        (
            "block scalar",
            MINIMAL_YAML.replace(
                "    intent: Do the work.\n",
                "    intent: |\n      Do the work.\n",
            ),
            10,
            13,
            "node_definitions[work].intent",
        ),
        (
            "multi-line flow collection",
            MINIMAL_YAML.replace(
                "  nodes:\n    - id: only\n      use: work\n      terminal: true\n",
                "  nodes: [{id: only,\n    use: work, terminal: true}]\n",
            ),
            13,
            21,
            "graph.nodes",
        ),
        (
            "multi-line quoted scalar",
            MINIMAL_YAML.replace(
                "    title: Work\n",
                "    title: \"Work\n      continued\"\n",
            ),
            9,
            17,
            "node_definitions[work].title",
        ),
        (
            "lone carriage return",
            format!("{}\r", MINIMAL_YAML.trim_end()),
            16,
            21,
            "graph.nodes[only].terminal",
        ),
        (
            "literal next line",
            MINIMAL_YAML.replace("name: Minimal\n", "name: \"Mini\u{85}mal\"\n"),
            4,
            12,
            "name",
        ),
        (
            "literal line separator",
            MINIMAL_YAML.replace("name: Minimal\n", "name: \"Mini\u{2028}mal\"\n"),
            4,
            12,
            "name",
        ),
    ];

    for (name, source, line, column, field) in scanned {
        let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);
        assert_eq!(diagnostics.len(), 1, "{name}: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].code(),
            AuthoringDiagnosticCode::SourceConstructUnsupported,
            "{name}"
        );
        assert_eq!(
            diagnostics[0].severity(),
            AuthoringSeverity::Error,
            "{name}"
        );
        assert_eq!(
            (
                diagnostics[0].location().line(),
                diagnostics[0].location().column()
            ),
            (line, column),
            "{name}: {:?}",
            diagnostics[0]
        );
        assert_eq!(diagnostics[0].field(), field, "{name}");
    }

    // Two constructs the YAML reader refuses before the scan ever runs. The pipeline order is parse,
    // then validate, then scan, so these arrive as the parse stage's own diagnostic — the same
    // stable code, anchored at the document start because no index could be built.
    for (name, source) in [
        (
            "leading byte order mark",
            format!("{}{MINIMAL_YAML}", '\u{feff}'),
        ),
        (
            "carriage return inside a quoted scalar",
            MINIMAL_YAML.replace("name: Minimal\n", "name: \"Mini\rmal\"\n"),
        ),
    ] {
        let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);
        assert_eq!(diagnostics.len(), 1, "{name}");
        assert_eq!(
            diagnostics[0].code(),
            AuthoringDiagnosticCode::SourceConstructUnsupported,
            "{name}: {:?}",
            diagnostics[0]
        );
    }
}

#[test]
fn v2aut001_multiple_construct_violations_are_reported_in_line_and_column_order() {
    let source = MINIMAL_YAML
        .replace("id: minimal\n", "id: minimal # first\n")
        .replace("  entry: only\n", "  entry: only # second\n");
    let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);

    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    let positions = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.location().line(), diagnostic.location().column()))
        .collect::<Vec<_>>();
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(positions, sorted);
    assert_eq!(positions[0].0, 2);
    assert!(positions[1].0 > positions[0].0);
    // The field names the node the stray comment sits on, so the author knows where to look.
    assert_eq!(diagnostics[0].field(), "id");
    assert_eq!(diagnostics[1].field(), "graph.entry");
}

// ---------------------------------------------------------------------------------------------
// 6b. Marker lines that anchor no node (V2AUT-003)
//
// `--write` turns a silent comment relocation into a silent edit of the author's file, so the two
// styles whose lines carry a marker but no node are refused rather than reformatted. Neither style
// is one the emitter can produce, so refusing them costs the round trip nothing.
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut003_a_bare_sequence_marker_is_rejected_rather_than_relocating_the_comment_above_it() {
    // The hazard, exactly: the element anchors at its first key one line *below* the marker, so the
    // marker line owns no path and the block above it would be re-emitted at the end of the
    // document instead of where the author put it.
    let source = MINIMAL_YAML.replace(
        "    - id: only\n",
        "    # About the only placement.\n    -\n      id: only\n",
    );
    let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);

    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].code(),
        AuthoringDiagnosticCode::SourceConstructUnsupported
    );
    assert_eq!(diagnostics[0].severity(), AuthoringSeverity::Error);
    assert_eq!(
        (
            diagnostics[0].location().line(),
            diagnostics[0].location().column()
        ),
        (15, 5),
        "{:?}",
        diagnostics[0]
    );
    assert_eq!(
        diagnostics[0].message(),
        "This source uses a sequence marker whose entry does not start on the marker line, which \
         canonical authoring form cannot represent."
    );
    // The `field` is the document root precisely because the marker line anchors nothing — the
    // absence this diagnostic exists to report.
    assert_eq!(diagnostics[0].field(), "$");
}

#[test]
fn v2aut003_a_document_marker_line_is_rejected_at_column_one() {
    for (name, source, line) in [
        (
            "document start",
            format!("# Leading note.\n---\n{MINIMAL_YAML}"),
            2,
        ),
        ("document end", format!("{MINIMAL_YAML}...\n"), 17),
    ] {
        let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);
        assert_eq!(diagnostics.len(), 1, "{name}: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].code(),
            AuthoringDiagnosticCode::SourceConstructUnsupported,
            "{name}"
        );
        assert_eq!(
            (
                diagnostics[0].location().line(),
                diagnostics[0].location().column()
            ),
            (line, 1),
            "{name}: {:?}",
            diagnostics[0]
        );
        assert_eq!(
            diagnostics[0].message(),
            "This source uses a document start or end marker, which canonical authoring form \
             cannot represent.",
            "{name}"
        );
    }
}

#[test]
fn v2aut003_a_dash_that_is_content_rather_than_a_marker_stays_supported() {
    // The canonical style — the element's first key on the marker line — is the whole corpus, and
    // it keeps formatting. A `-` or `---` written as a *value* is content the lexer reads in a
    // quoted state, never a marker.
    let corpus = format_yaml(MINIMAL_YAML);
    assert!(corpus.document().contains("    - id: only\n"));

    let source = MINIMAL_YAML
        .replace("name: Minimal\n", "name: \"---\"\n")
        .replace("    title: Work\n", "    title: \"-\"\n");
    let formatted = format_yaml(&source);
    assert!(
        formatted.document().contains("name: \"---\"\n")
            && formatted.document().contains("    title: \"-\"\n"),
        "{}",
        formatted.document()
    );
    assert_eq!(
        format_yaml(formatted.document()).document(),
        formatted.document(),
        "the quoted forms are a fixpoint of the scan that rejects the marker forms"
    );
}

/// The general closure of the relocation class the two marker kinds belong to: a comment above
/// *any* content line that anchors no node — a mapping value written below its key, a quoted value
/// on its own line, a folded plain scalar's continuation — is rejected rather than silently
/// re-emitted at the end of the document. Block scalars are already rejected, which pushes long
/// prose toward exactly these layouts, so this is the case a real author hits first.
#[test]
fn v2aut003_a_comment_above_an_unanchored_content_line_is_rejected_rather_than_relocated() {
    for (name, replacement, line, column) in [
        (
            "value below its key",
            "    intent:\n      # Why the work matters.\n      Do the work.\n",
            12,
            7,
        ),
        (
            "quoted value below its key",
            "    intent:\n      # Why the work matters.\n      \"Do the work.\"\n",
            12,
            7,
        ),
    ] {
        let source = MINIMAL_YAML.replace("    intent: Do the work.\n", replacement);
        let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);
        assert_eq!(diagnostics.len(), 1, "{name}: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].code(),
            AuthoringDiagnosticCode::SourceConstructUnsupported,
            "{name}"
        );
        assert_eq!(
            (
                diagnostics[0].location().line(),
                diagnostics[0].location().column()
            ),
            (line, column),
            "{name}: {:?}",
            diagnostics[0]
        );
        assert_eq!(
            diagnostics[0].message(),
            "This source uses a comment attached to a line that does not begin a node, which \
             canonical authoring form cannot represent.",
            "{name}"
        );
    }
}

/// A comment *inside* a folded plain scalar has no relocation path at all: YAML ends the scalar at
/// the comment line, so the continuation below it is a parse error, reported as an unsupported
/// construct before comment attachment ever runs.
#[test]
fn v2aut003_a_comment_inside_a_folded_scalar_is_a_parse_rejection_not_a_relocation() {
    let source = MINIMAL_YAML.replace(
        "    intent: Do the work.\n",
        "    intent: Do the work\n      # Folded onward.\n      end to end.\n",
    );
    let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].code(),
        AuthoringDiagnosticCode::SourceConstructUnsupported,
        "{:?}",
        diagnostics[0]
    );
}

/// The same layouts stay supported when no comment attaches to them: there is nothing to lose, so
/// the emitter simply normalizes the layout and reports drift.
#[test]
fn v2aut003_an_unanchored_content_line_without_a_comment_still_formats() {
    for (name, replacement) in [
        ("value below its key", "    intent:\n      Do the work.\n"),
        (
            "folded continuation line",
            "    intent: Do the\n      work.\n",
        ),
    ] {
        let source = MINIMAL_YAML.replace("    intent: Do the work.\n", replacement);
        let formatted = format_yaml(&source);
        assert!(formatted.changed(), "{name}: layout must normalize");
        assert_eq!(
            format_yaml(formatted.document()).document(),
            formatted.document(),
            "{name}: the normalized form is a fixpoint"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 7. The emitted-document budget
// ---------------------------------------------------------------------------------------------

/// A document carrying `count` maximal choice items in one definition, ported from
/// `int_v2_procedure_canonical.rs`: every filler item is identical in width, so the item count is an
/// exactly linear dial on the projection size.
fn projection_document(count: usize) -> String {
    let choices: String = (0..32)
        .map(|index| format!("          - \"{:c<118}{index:02}\"\n", ""))
        .collect();
    let items: String = (0..count)
        .map(|index| {
            format!(
                "      - id: i{index:02}\n        type: choice\n        prompt: \"{}\"\n        \
                 help: \"{}\"\n        required: true\n        choices:\n{choices}",
                "p".repeat(300),
                "h".repeat(1_000),
            )
        })
        .collect();
    format!(
        "schema: podway.procedure/v2\nid: projection\nversion: \"1\"\nname: Projection\n\
         purpose: Fill the canonical projection budget.\nnode_definitions:\n  bulk:\n    \
         type: action\n    title: Bulk\n    intent: Hold the filler items.\n    items:\n{items}\
         graph:\n  entry: n\n  nodes:\n    - id: n\n      use: bulk\n      terminal: true\n",
    )
}

fn projection_characters(count: usize) -> usize {
    admit(&projection_document(count), ProcedureDocumentFormat::Yaml)
        .canonical_json()
        .as_str()
        .chars()
        .count()
}

/// A document may validate and still have no canonical authoring form: block YAML with materialized
/// defaults is materially wider than compact Canonical JSON v1. Section 11.1 authorizes exactly this
/// — format is "rejected with `SOURCE_PROJECTION_BUDGET_EXCEEDED` when *its* complete canonical
/// source projection exceeds `SOURCE_PROJECTION_MAX_CHARACTERS`" — and the result schema's
/// `maxLength: 131072` on `document` requires it.
#[test]
fn v2aut001_a_document_at_the_canonical_budget_has_no_canonical_authoring_form() {
    let one = projection_characters(1);
    let step = projection_characters(2) - one;
    let at_limit = 1 + (SOURCE_PROJECTION_MAX_CHARACTERS - one) / step;

    let source = projection_document(at_limit);
    let canonical = projection_characters(at_limit);
    assert!(
        canonical <= SOURCE_PROJECTION_MAX_CHARACTERS,
        "the at-limit document must still validate: {canonical}"
    );

    for run in 0..2 {
        let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);
        assert_eq!(diagnostics.len(), 1, "run {run}");
        assert_eq!(
            diagnostics[0].code(),
            AuthoringDiagnosticCode::SourceProjectionBudgetExceeded,
            "run {run}: {:?}",
            diagnostics[0]
        );
        assert_eq!(diagnostics[0].field(), "$");
    }

    // One item over the *canonical* budget is rejected earlier, by validation, with the same code —
    // the shared `CANONICAL_PROJECTION_FIELD` constant is what keeps the two paths in agreement.
    let over = projection_document(at_limit + 1);
    assert!(matches!(
        parse_procedure_document(over.as_bytes(), ProcedureDocumentFormat::Yaml).and_then(
            |parsed| match parsed {
                ParsedProcedure::V2(parsed) => validate_procedure_v2(parsed).map(|_| ()),
                ParsedProcedure::V1(_) => unreachable!(),
            }
        ),
        Err(ConfigError::OutOfBounds { .. })
    ));
    let diagnostics = diagnostics(&over, ProcedureDocumentFormat::Yaml);
    assert_eq!(
        diagnostics[0].code(),
        AuthoringDiagnosticCode::SourceProjectionBudgetExceeded
    );
}

// ---------------------------------------------------------------------------------------------
// 8. Schema dispatch and the v1 boundary
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut001_sniff_procedure_schema_reads_v1_v2_and_refuses_everything_else() {
    assert_eq!(
        sniff_procedure_schema(V1_YAML.as_bytes(), ProcedureDocumentFormat::Yaml),
        Some("podway.procedure/v1")
    );
    assert_eq!(
        sniff_procedure_schema(MINIMAL_YAML.as_bytes(), ProcedureDocumentFormat::Yaml),
        Some("podway.procedure/v2")
    );
    assert_eq!(
        sniff_procedure_schema(
            fixture("tests/fixtures/v2/procedures/equivalent-procedure.json").as_bytes(),
            ProcedureDocumentFormat::Json
        ),
        Some("podway.procedure/v2")
    );
    for (name, bytes, format) in [
        (
            "garbage",
            b"not a document at all".as_slice(),
            ProcedureDocumentFormat::Yaml,
        ),
        ("empty", b"".as_slice(), ProcedureDocumentFormat::Yaml),
        ("empty json", b"".as_slice(), ProcedureDocumentFormat::Json),
        (
            "no schema",
            b"id: x\n".as_slice(),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "unknown schema",
            b"schema: podway.procedure/v3\n".as_slice(),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "non-string schema",
            b"schema: 3\n".as_slice(),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "invalid utf-8",
            &[0xff, 0xfe, 0x00],
            ProcedureDocumentFormat::Yaml,
        ),
    ] {
        assert_eq!(sniff_procedure_schema(bytes, format), None, "{name}");
    }
}

#[test]
fn v2aut001_a_v1_document_is_reported_as_not_a_procedure_v2_target() {
    assert_eq!(
        format_procedure_v2(FormatRequest {
            source: V1_YAML,
            source_path: "workflow.yaml",
            format: ProcedureDocumentFormat::Yaml,
        }),
        Err(FormatFailure::NotProcedureV2),
        "a v1 document cannot be described by a diagnostic whose schema is const v2"
    );
}

/// A document that declares the v1 schema is refused as a wrong-schema target even when it is
/// also invalid *as v1*. Without the sniff short-circuit the dispatching parser would surface the
/// v1 parse failure, and the formatter would misreport it as a v2 authoring finding whose
/// `procedure_schema` is `const "podway.procedure/v2"` — a claim the document never made.
#[test]
fn v2aut001_a_malformed_v1_document_is_still_not_a_procedure_v2_target() {
    for (name, source) in [
        (
            "v1 with a missing required field",
            "schema: podway.procedure/v1\nid: broken\n",
        ),
        (
            "v1 with an unknown field",
            "schema: podway.procedure/v1\nid: broken\nversion: \"1\"\nname: Broken\nmystery: true\nstages: []\nrework:\n  allow_return_to: any_previous\n",
        ),
    ] {
        assert_eq!(
            format_procedure_v2(FormatRequest {
                source,
                source_path: "workflow.yaml",
                format: ProcedureDocumentFormat::Yaml,
            }),
            Err(FormatFailure::NotProcedureV2),
            "{name}: a declared-v1 document is a wrong-schema command failure, \
             never a v2 authoring finding"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 9. Diagnostics: classification, shape, ordering, truncation
// ---------------------------------------------------------------------------------------------

/// Asserts a diagnostic serializes to a document `assets/schemas/authoring-diagnostic-v1.schema.json`
/// accepts. `podway-config` has no JSON Schema dependency, so this checks the schema's requirements
/// structurally: required fields present, bounded, and severity bound to code.
fn assert_diagnostic_shape(diagnostic: &AuthoringDiagnostic) {
    let value = serde_json::to_value(diagnostic).expect("diagnostics serialize");
    let object = value.as_object().expect("a diagnostic is an object");

    for required in [
        "code",
        "severity",
        "schema",
        "source_path",
        "location",
        "field",
        "message",
        "hint",
    ] {
        assert!(object.contains_key(required), "missing {required}: {value}");
    }
    for key in object.keys() {
        assert!(
            [
                "code",
                "severity",
                "schema",
                "source_path",
                "location",
                "field",
                "node_definition_id",
                "graph_node_id",
                "related_graph_node_ids",
                "message",
                "hint",
            ]
            .contains(&key.as_str()),
            "unexpected property {key}"
        );
    }

    assert_eq!(
        object["schema"],
        Value::String("podway.procedure/v2".into())
    );
    assert_eq!(
        object["severity"],
        Value::String(diagnostic.severity().as_str().into())
    );
    assert!(
        AuthoringDiagnosticCode::ALL
            .iter()
            .any(|code| code.as_str() == diagnostic.code().as_str())
    );

    for (key, maximum) in [
        ("source_path", 4_096),
        ("field", 4_096),
        ("message", 512),
        ("hint", 512),
    ] {
        let text = object[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} is a string"));
        assert!(
            (1..=maximum).contains(&text.chars().count()),
            "{key}: {text:?}"
        );
    }

    let location = object["location"]
        .as_object()
        .expect("location is an object");
    assert_eq!(location.len(), 4);
    for key in ["line", "column", "end_line", "end_column"] {
        let value = location[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{key} is an integer"));
        assert!((1..=1_048_576).contains(&value), "{key}: {value}");
    }
    assert!(location["end_line"].as_u64() >= location["line"].as_u64());
}

#[test]
fn v2aut001_every_emitted_diagnostic_satisfies_the_authoring_diagnostic_schema() {
    let sources: Vec<(&str, String, ProcedureDocumentFormat)> = vec![
        (
            "unknown field",
            MINIMAL_YAML.replace("id: minimal\n", "id: minimal\nbogus: 1\n"),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "dangling use",
            MINIMAL_YAML.replace("      use: work\n", "      use: absent\n"),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "dangling manual rework target",
            format!("{MINIMAL_YAML}manual_rework:\n  allowed_targets:\n    - absent\n"),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "bad identifier",
            MINIMAL_YAML.replace("id: minimal\n", "id: Minimal\n"),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "over-long purpose",
            MINIMAL_YAML.replace(
                "purpose: The smallest legal Procedure v2 document.\n",
                &format!("purpose: {}\n", "p".repeat(501)),
            ),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "duplicate mapping key",
            MINIMAL_YAML.replace("id: minimal\n", "id: minimal\nid: minimal\n"),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "anchor",
            MINIMAL_YAML.replace("  entry: only\n", "  entry: &anchor only\n"),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "inline comment",
            MINIMAL_YAML.replace("id: minimal\n", "id: minimal # nope\n"),
            ProcedureDocumentFormat::Yaml,
        ),
        (
            "json trailing data",
            "{\"schema\":\"podway.procedure/v2\"} junk".to_owned(),
            ProcedureDocumentFormat::Json,
        ),
    ];

    let mut seen = BTreeSet::new();
    for (name, source, format) in sources {
        let diagnostics = diagnostics(&source, format);
        assert!(!diagnostics.is_empty(), "{name}");
        for diagnostic in &diagnostics {
            assert_diagnostic_shape(diagnostic);
            assert_eq!(diagnostic.source_path(), "workflow.yaml", "{name}");
            seen.insert(diagnostic.code().as_str());
        }
    }
    assert_eq!(
        seen,
        BTreeSet::from([
            "AUTHORING_SCHEMA_INVALID",
            "GRAPH_DEFINITION_UNKNOWN",
            "MANUAL_REWORK_TARGET_UNKNOWN",
            "SOURCE_CONSTRUCT_UNSUPPORTED",
        ]),
        "the production mapping (V2AUT-008) refines the closed-reference sources in this corpus \
         into their catalog codes and leaves the rest in the two generic families"
    );
}

#[test]
fn v2aut001_a_closed_reference_failure_is_located_at_the_offending_scalar() {
    let source = MINIMAL_YAML.replace("      use: work\n", "      use: absent\n");
    let diagnostics = diagnostics(&source, ProcedureDocumentFormat::Yaml);

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        AuthoringDiagnosticCode::GraphDefinitionUnknown
    );
    assert_eq!(diagnostics[0].field(), "graph.nodes.use");
    // Line 15 is `      use: absent`; column 7 is where its key starts.
    assert_eq!(diagnostics[0].location().line(), 15);
    assert_eq!(diagnostics[0].location().column(), 7);
    assert!(diagnostics[0].message().contains("absent"));
    assert!(!diagnostics[0].hint().is_empty());
}

#[test]
fn v2aut001_finalize_diagnostics_orders_by_stage_then_position_and_truncates_at_the_cap() {
    let diagnostic = |code: AuthoringDiagnosticCode, line: u32, column: u32, field: &str| {
        AuthoringDiagnostic::new(
            code,
            "workflow.yaml",
            SourceLocation::new(line, column, line, column),
            field,
            "message",
            "hint",
        )
    };

    let finalized = finalize_diagnostics(vec![
        (
            AuthoringStage::Lint,
            diagnostic(AuthoringDiagnosticCode::LargeCycle, 1, 1, "graph"),
        ),
        (
            AuthoringStage::Validate,
            diagnostic(
                AuthoringDiagnosticCode::EntryNodeInvalid,
                9,
                9,
                "graph.entry",
            ),
        ),
        (
            AuthoringStage::Format,
            diagnostic(AuthoringDiagnosticCode::FormatNotCanonical, 40, 1, "$"),
        ),
        (
            AuthoringStage::Vet,
            diagnostic(AuthoringDiagnosticCode::NoTerminalPath, 2, 2, "graph"),
        ),
    ]);
    assert_eq!(
        finalized
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        [
            "FORMAT_NOT_CANONICAL",
            "ENTRY_NODE_INVALID",
            "NO_TERMINAL_PATH",
            "LARGE_CYCLE",
        ],
        "stage order wins over source position"
    );
    assert_eq!(finalized.total(), 4);
    assert!(!finalized.truncated());
    assert!(!finalized.valid(), "an error diagnostic invalidates");

    // Within one stage, source position orders the report.
    let positional = finalize_diagnostics(vec![
        (
            AuthoringStage::Lint,
            diagnostic(AuthoringDiagnosticCode::LargeCycle, 9, 1, "b"),
        ),
        (
            AuthoringStage::Lint,
            diagnostic(AuthoringDiagnosticCode::LargeCycle, 3, 7, "a"),
        ),
        (
            AuthoringStage::Lint,
            diagnostic(AuthoringDiagnosticCode::LargeCycle, 3, 2, "c"),
        ),
    ]);
    assert_eq!(
        positional
            .diagnostics()
            .iter()
            .map(|diagnostic| (diagnostic.location().line(), diagnostic.location().column()))
            .collect::<Vec<_>>(),
        [(3, 2), (3, 7), (9, 1)]
    );
    assert!(
        positional.valid(),
        "warnings alone leave the procedure valid"
    );

    let many = (0..300)
        .map(|index| {
            (
                AuthoringStage::Lint,
                diagnostic(
                    AuthoringDiagnosticCode::UnusedNodeDefinition,
                    index + 1,
                    1,
                    "node_definitions",
                ),
            )
        })
        .collect::<Vec<_>>();
    let truncated = finalize_diagnostics(many);
    assert_eq!(truncated.diagnostics().len(), MAX_AUTHORING_DIAGNOSTICS);
    assert_eq!(truncated.total(), 300);
    assert!(truncated.truncated());
    assert_eq!(truncated.diagnostics()[0].location().line(), 1);
    assert_eq!(
        truncated.diagnostics()[MAX_AUTHORING_DIAGNOSTICS - 1]
            .location()
            .line(),
        u32::try_from(MAX_AUTHORING_DIAGNOSTICS).expect("the cap fits in u32")
    );
}

// ---------------------------------------------------------------------------------------------
// 10. `changed` is byte drift
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut001_changed_reports_byte_drift_with_no_special_cases() {
    let canonical = format_yaml(MINIMAL_YAML).document().to_owned();
    assert!(!format_yaml(&canonical).changed());

    for (name, source) in [
        ("crlf", canonical.replace('\n', "\r\n")),
        ("no trailing newline", canonical.trim_end().to_owned()),
        ("doubled trailing newline", format!("{canonical}\n")),
        ("blank line", canonical.replace("graph:\n", "\ngraph:\n")),
    ] {
        // CRLF carries a `\r` before every `\n`, which `split('\n')` strips as a line terminator, so
        // it is ordinary drift rather than an unsupported construct.
        let formatted = format_yaml(&source);
        assert!(formatted.changed(), "{name}");
        assert_eq!(formatted.document(), canonical, "{name}");
    }
}

// ---------------------------------------------------------------------------------------------
// 11. Emitted key order tracks the schema's declared `properties` order
// ---------------------------------------------------------------------------------------------
//
// Approach chosen (see the two options the task allowed): a hardcoded per-shape expected key order
// table (`EmittedShape::schema_key_order`), checked two ways rather than one raw-text scan of the
// schema. (a) The table's key *set* is asserted equal to the live schema's `properties` key set for
// every shape, using ordinary `serde_json` — safe for set equality even though a plain parse sorts
// object keys, since order plays no part in a set comparison. (b) The table's *order* is asserted
// against every mapping the formatter actually emits, walked out of the formatted YAML text by a
// small indentation-driven scanner below. A raw byte-offset scan of the schema JSON to recover its
// authored property order was the other allowed option; it was passed over because it would have
// needed the same nesting-aware bookkeeping as the emitted-side scanner below to attribute each
// `"properties"` block to the right shape, for a result no more reviewable than a table a reader can
// simply compare against `assets/schemas/procedure-v2.schema.json` by eye.
//
// Coverage requirement (action + decision definitions, all six item kinds, both placement kinds,
// `evidence_from`, `skip`, `reason`, `assessment`, `manual_rework`) is already met by the existing
// `yaml_corpus()` without extending any corpus constant: `kitchen-sink` alone carries every shape
// except `assessment`, and `equivalent-procedure.yaml` (also already in the corpus) carries a
// decision's `assessment`. Scanning the whole corpus costs nothing extra and strengthens the guard.

/// One schema-closed mapping shape the emitted YAML is checked against. Each variant corresponds to
/// one `properties` object in `assets/schemas/procedure-v2.schema.json` — the root document itself,
/// or one `$defs` entry named by [`EmittedShape::schema_def_name`].
#[derive(Clone, Copy, Debug)]
enum EmittedShape {
    Root,
    Graph,
    ActionDefinition,
    DecisionDefinition,
    DecisionOption,
    ReasonPolicy,
    Assessment,
    ActionPlacement,
    DecisionPlacement,
    EvidenceReference,
    SkipPolicy,
    Route,
    ManualRework,
    ConfirmItem,
    TextItem,
    ChoiceItem,
    IntegerItem,
    ListItem,
    ArtifactItem,
}

impl EmittedShape {
    const ALL: &'static [Self] = &[
        Self::Root,
        Self::Graph,
        Self::ActionDefinition,
        Self::DecisionDefinition,
        Self::DecisionOption,
        Self::ReasonPolicy,
        Self::Assessment,
        Self::ActionPlacement,
        Self::DecisionPlacement,
        Self::EvidenceReference,
        Self::SkipPolicy,
        Self::Route,
        Self::ManualRework,
        Self::ConfirmItem,
        Self::TextItem,
        Self::ChoiceItem,
        Self::IntegerItem,
        Self::ListItem,
        Self::ArtifactItem,
    ];

    /// The `$defs` name backing this shape, or `None` for the Procedure document root. `confirm`
    /// items resolve to `item_base_confirm`: `$defs.confirm_item` is only a `$ref` indirection to it
    /// and carries no `properties` object of its own.
    const fn schema_def_name(self) -> Option<&'static str> {
        match self {
            Self::Root => None,
            Self::Graph => Some("graph"),
            Self::ActionDefinition => Some("action_definition"),
            Self::DecisionDefinition => Some("decision_definition"),
            Self::DecisionOption => Some("decision_option"),
            Self::ReasonPolicy => Some("reason_policy"),
            Self::Assessment => Some("assessment"),
            Self::ActionPlacement => Some("action_placement"),
            Self::DecisionPlacement => Some("decision_placement"),
            Self::EvidenceReference => Some("evidence_reference"),
            Self::SkipPolicy => Some("skip_policy"),
            Self::Route => Some("route"),
            Self::ManualRework => Some("manual_rework"),
            Self::ConfirmItem => Some("item_base_confirm"),
            Self::TextItem => Some("text_item"),
            Self::ChoiceItem => Some("choice_item"),
            Self::IntegerItem => Some("integer_item"),
            Self::ListItem => Some("list_item"),
            Self::ArtifactItem => Some("artifact_item"),
        }
    }

    /// The schema's `properties` insertion order for this shape, transcribed by hand from
    /// `assets/schemas/procedure-v2.schema.json`. [`v2aut001_emitted_key_order_matches_the_procedure_schema_properties_order`]
    /// pins this table against the live schema's key *set* so it cannot silently drift.
    const fn schema_key_order(self) -> &'static [&'static str] {
        match self {
            Self::Root => &[
                "schema",
                "id",
                "version",
                "name",
                "purpose",
                "description",
                "goal_tracking",
                "node_definitions",
                "graph",
                "manual_rework",
            ],
            Self::Graph => &["entry", "nodes"],
            Self::ActionDefinition => &[
                "type",
                "title",
                "intent",
                "description",
                "instructions",
                "items",
            ],
            Self::DecisionDefinition => &[
                "type",
                "title",
                "description",
                "objective",
                "prompt",
                "evidence_guidance",
                "items",
                "options",
                "reason",
                "assessment",
            ],
            Self::DecisionOption => &["id", "label", "criteria"],
            Self::ReasonPolicy => &["required", "prompt"],
            Self::Assessment => &["target", "outcomes"],
            Self::ActionPlacement => &["id", "use", "evidence_from", "skip", "next", "terminal"],
            Self::DecisionPlacement => &["id", "use", "evidence_from", "routes"],
            Self::EvidenceReference => &["node", "required", "items"],
            Self::SkipPolicy => &["allowed", "reason_required"],
            Self::Route => &["to", "effect"],
            Self::ManualRework => &["allowed_targets"],
            Self::ConfirmItem => &["id", "type", "prompt", "help", "required"],
            Self::TextItem => &[
                "id",
                "type",
                "prompt",
                "help",
                "required",
                "min_length",
                "max_length",
                "multiline",
            ],
            Self::ChoiceItem => &["id", "type", "prompt", "help", "required", "choices"],
            Self::IntegerItem => &[
                "id", "type", "prompt", "help", "required", "minimum", "maximum",
            ],
            Self::ListItem => &[
                "id",
                "type",
                "prompt",
                "help",
                "required",
                "min_items",
                "max_items",
                "max_item_length",
                "unique",
            ],
            Self::ArtifactItem => &[
                "id",
                "type",
                "prompt",
                "help",
                "required",
                "allowed_media_types",
            ],
        }
    }
}

/// One non-comment, non-blank line of formatted output. List markers (`- `) are peeled here so
/// every row is uniformly a mapping key line or a bare scalar list entry; `indent` for a peeled
/// marker line is already advanced two past the dash, matching where the docstring says the
/// element's first key sits, so it lines up with the plain key lines that continue the same mapping.
struct Row<'a> {
    indent: usize,
    is_element_head: bool,
    text: &'a str,
}

fn formatted_rows(document: &str) -> Vec<Row<'_>> {
    document
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .map(|line| {
            let indent = line.len() - line.trim_start().len();
            let rest = &line[indent..];
            match rest.strip_prefix("- ") {
                Some(tail) => Row {
                    indent: indent + 2,
                    is_element_head: true,
                    text: tail,
                },
                None => Row {
                    indent,
                    is_element_head: false,
                    text: rest,
                },
            }
        })
        .collect()
}

/// The key on a row already known to open a mapping entry (`key:` or `key: value`).
fn key_of<'a>(row: &Row<'a>) -> &'a str {
    row.text
        .split_once(':')
        .map_or(row.text, |(key, _)| key)
        .trim()
}

/// Splits a mapping's own rows into `(key, nested_body)` pairs, one per direct entry, in document
/// order. `nested_body` is every row more indented than the entry's own key line, i.e. its value
/// when that value is itself a mapping or a sequence.
fn direct_entries<'a>(rows: &'a [Row<'a>]) -> Vec<(&'a str, &'a [Row<'a>])> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let base_indent = first.indent;
    let mut entries = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        assert_eq!(
            rows[index].indent, base_indent,
            "misaligned row while scanning a mapping's own keys: {:?}",
            rows[index].text
        );
        let key = key_of(&rows[index]);
        let mut end = index + 1;
        while end < rows.len() && rows[end].indent > base_indent {
            end += 1;
        }
        entries.push((key, &rows[index + 1..end]));
        index = end;
    }
    entries
}

/// Splits a sequence's rows into per-element slices. Only a head row (`- `) *at the sequence's own
/// indent* starts a new element, so a deeper-nested sequence belonging to one element's own value
/// (e.g. a `choice` item's `choices:`) is correctly kept inside that element rather than mistaken for
/// a sibling.
fn split_elements<'a>(rows: &'a [Row<'a>]) -> Vec<&'a [Row<'a>]> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let element_indent = first.indent;
    let mut elements = Vec::new();
    let mut start = 0;
    for index in 1..rows.len() {
        if rows[index].indent == element_indent && rows[index].is_element_head {
            elements.push(&rows[start..index]);
            start = index;
        }
    }
    elements.push(&rows[start..]);
    elements
}

/// The inline scalar value of one direct key within `rows`, e.g. reading `type` back off an item or
/// a node definition to resolve which concrete shape it is.
fn direct_scalar<'a>(rows: &'a [Row<'a>], key: &str) -> Option<&'a str> {
    let base_indent = rows.first()?.indent;
    rows.iter()
        .find(|row| row.indent == base_indent && key_of(row) == key)
        .map(|row| {
            row.text
                .split_once(": ")
                .map_or("", |(_, value)| value.trim())
        })
}

fn item_shape(kind: &str) -> EmittedShape {
    match kind {
        "confirm" => EmittedShape::ConfirmItem,
        "text" => EmittedShape::TextItem,
        "choice" => EmittedShape::ChoiceItem,
        "integer" => EmittedShape::IntegerItem,
        "list" => EmittedShape::ListItem,
        "artifact" => EmittedShape::ArtifactItem,
        other => panic!("unknown item type: {other}"),
    }
}

/// Walks one mapping's rows, recording its own key order under `shape` and recursing into every
/// nested shape [`dispatch`] resolves from `(shape, key)`.
fn walk(rows: &[Row<'_>], shape: EmittedShape, observed: &mut Vec<(EmittedShape, Vec<String>)>) {
    let entries = direct_entries(rows);
    let keys = entries.iter().map(|(key, _)| (*key).to_owned()).collect();
    for (key, body) in &entries {
        dispatch(shape, key, body, observed);
    }
    observed.push((shape, keys));
}

/// Interprets one direct key's nested body according to the fixed Procedure v2 grammar. Keys that
/// hold a scalar, a scalar list (`instructions`, `evidence_guidance`, `choices`,
/// `allowed_media_types`, an evidence reference's `items`, `allowed_targets`), or an open dictionary
/// of scalars (`outcomes`) carry no shape to check and fall through untouched.
fn dispatch(
    parent: EmittedShape,
    key: &str,
    body: &[Row<'_>],
    observed: &mut Vec<(EmittedShape, Vec<String>)>,
) {
    match (parent, key) {
        (EmittedShape::Root, "node_definitions") => {
            for (_, nested) in direct_entries(body) {
                let kind = direct_scalar(nested, "type")
                    .unwrap_or_else(|| panic!("a node definition always declares type"));
                let shape = match kind {
                    "action" => EmittedShape::ActionDefinition,
                    "decision" => EmittedShape::DecisionDefinition,
                    other => panic!("unknown node_definitions type: {other}"),
                };
                walk(nested, shape, observed);
            }
        }
        (EmittedShape::Root, "graph") => walk(body, EmittedShape::Graph, observed),
        (EmittedShape::Root, "manual_rework") => walk(body, EmittedShape::ManualRework, observed),
        (EmittedShape::Graph, "nodes") => {
            for element in split_elements(body) {
                let is_decision = direct_entries(element)
                    .iter()
                    .any(|(key, _)| *key == "routes");
                let shape = if is_decision {
                    EmittedShape::DecisionPlacement
                } else {
                    EmittedShape::ActionPlacement
                };
                walk(element, shape, observed);
            }
        }
        (EmittedShape::ActionDefinition | EmittedShape::DecisionDefinition, "items") => {
            for element in split_elements(body) {
                let kind = direct_scalar(element, "type")
                    .unwrap_or_else(|| panic!("an item always declares type"));
                walk(element, item_shape(kind), observed);
            }
        }
        (EmittedShape::DecisionDefinition, "options") => {
            for element in split_elements(body) {
                walk(element, EmittedShape::DecisionOption, observed);
            }
        }
        (EmittedShape::DecisionDefinition, "reason") => {
            walk(body, EmittedShape::ReasonPolicy, observed);
        }
        (EmittedShape::DecisionDefinition, "assessment") => {
            walk(body, EmittedShape::Assessment, observed);
        }
        (EmittedShape::ActionPlacement | EmittedShape::DecisionPlacement, "evidence_from") => {
            for element in split_elements(body) {
                walk(element, EmittedShape::EvidenceReference, observed);
            }
        }
        (EmittedShape::ActionPlacement, "skip") => walk(body, EmittedShape::SkipPolicy, observed),
        (EmittedShape::DecisionPlacement, "routes") => {
            for (_, nested) in direct_entries(body) {
                walk(nested, EmittedShape::Route, observed);
            }
        }
        _ => {}
    }
}

/// Every mapping the formatter emitted in `document`, paired with the shape it must satisfy and the
/// order its own direct keys actually appeared in.
fn observed_key_orders(document: &str) -> Vec<(EmittedShape, Vec<String>)> {
    let rows = formatted_rows(document);
    let mut observed = Vec::new();
    walk(&rows, EmittedShape::Root, &mut observed);
    observed
}

/// `true` when every element of `observed` appears in `expected`, in order, allowing gaps (an
/// omitted optional field never fails this check; an out-of-order field does).
fn is_subsequence_in_order(observed: &[String], expected: &[&str]) -> bool {
    let mut expected = expected.iter();
    observed
        .iter()
        .all(|key| expected.any(|candidate| *candidate == key.as_str()))
}

fn schema_properties_key_set(schema: &Value, def_name: Option<&str>) -> BTreeSet<String> {
    let object = match def_name {
        Some(name) => &schema["$defs"][name],
        None => schema,
    };
    object["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("{def_name:?} has no properties object in the schema"))
        .keys()
        .cloned()
        .collect()
}

#[test]
fn v2aut001_emitted_key_order_matches_the_procedure_schema_properties_order() {
    let schema: Value = serde_json::from_str(&fixture("assets/schemas/procedure-v2.schema.json"))
        .expect("the procedure v2 schema is valid JSON");

    // Guard (a): the hardcoded tables cannot silently drift from the schema they transcribe. A
    // plain `serde_json` parse sorts object keys, but that is harmless here — set equality does not
    // need insertion order preserved.
    for &shape in EmittedShape::ALL {
        let table: BTreeSet<String> = shape
            .schema_key_order()
            .iter()
            .map(|key| (*key).to_owned())
            .collect();
        let schema_set = schema_properties_key_set(&schema, shape.schema_def_name());
        assert_eq!(
            table, schema_set,
            "{shape:?} key table has drifted from the schema's properties"
        );
    }

    // Guard (b): every mapping the formatter actually emits, across the established corpus, orders
    // its present keys as a subsequence of the schema's declared order.
    for (name, source) in yaml_corpus() {
        let formatted = format_yaml(&source);
        for (shape, keys) in observed_key_orders(formatted.document()) {
            assert!(
                is_subsequence_in_order(&keys, shape.schema_key_order()),
                "{name}: emitted {shape:?} keys {keys:?} are not a subsequence of schema order {:?}",
                shape.schema_key_order()
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 12. V2AUT-002: the drift diagnostic `format --check` reports
// ---------------------------------------------------------------------------------------------
//
// `--check` asks one question — is this source already the canonical rendering? — so the answer is
// one diagnostic or none, never a list of edits. These tests pin where that diagnostic points,
// because the position is the only part of the answer a reader cannot re-derive from the canonical
// document the same result already carries.

/// The drift verdict for a source that formats successfully.
fn drift(source: &str, format: ProcedureDocumentFormat) -> Option<AuthoringDiagnostic> {
    let formatted = format_ok(source, format);
    let context = AuthoringContext::new("workflow.yaml", source, format);
    let diagnostic = formatted.drift_diagnostic(&context);
    assert_eq!(
        diagnostic.is_some(),
        formatted.changed(),
        "the drift verdict and the `changed` flag are the same byte comparison"
    );
    diagnostic
}

fn drift_yaml(source: &str) -> AuthoringDiagnostic {
    drift(source, ProcedureDocumentFormat::Yaml).expect("the source must be reported as drifted")
}

/// Every drifted source used below, so the invariants can be asserted once over all of them.
fn drifted_yaml_corpus() -> Vec<(&'static str, String)> {
    let minimal = MINIMAL_YAML.to_owned();
    let commented = format_yaml(COMMENTED_YAML).document().to_owned();
    vec![
        (
            "reordered root keys",
            minimal.replace(
                "schema: podway.procedure/v2\nid: minimal\n",
                "id: minimal\nschema: podway.procedure/v2\n",
            ),
        ),
        (
            "dedented comment",
            commented.replace(
                "  # The reusable contract every placement below uses.\n",
                "# The reusable contract every placement below uses.\n",
            ),
        ),
        (
            "extra space after a colon",
            minimal.replace("      use: work\n", "      use:  work\n"),
        ),
        (
            "missing trailing newline",
            minimal
                .strip_suffix('\n')
                .expect("the corpus document ends in a newline")
                .to_owned(),
        ),
        ("doubled trailing newline", format!("{minimal}\n")),
        ("crlf line endings", minimal.replace('\n', "\r\n")),
        (
            "blank line",
            minimal.replace("graph:\n", "\ngraph:\n").to_owned(),
        ),
    ]
}

#[test]
fn v2aut002_a_canonical_source_has_no_drift_diagnostic() {
    for (name, source) in yaml_corpus() {
        let canonical = format_yaml(&source).document().to_owned();
        assert_eq!(
            drift(&canonical, ProcedureDocumentFormat::Yaml),
            None,
            "{name}: the canonical rendering of a document is not drifted from itself"
        );
    }

    let json = format_ok(
        &fixture("tests/fixtures/v2/procedures/equivalent-procedure.json"),
        ProcedureDocumentFormat::Json,
    )
    .document()
    .to_owned();
    assert_eq!(
        drift(&json, ProcedureDocumentFormat::Json),
        None,
        "canonical JSON authoring text is not drifted from itself either"
    );
}

#[test]
fn v2aut002_drift_on_the_first_line_is_reported_at_the_first_divergent_column() {
    let diagnostic = drift_yaml(&MINIMAL_YAML.replace(
        "schema: podway.procedure/v2\nid: minimal\n",
        "id: minimal\nschema: podway.procedure/v2\n",
    ));

    assert_eq!(
        diagnostic.code(),
        AuthoringDiagnosticCode::FormatNotCanonical
    );
    assert_eq!(diagnostic.severity(), AuthoringSeverity::Error);
    assert_eq!(
        diagnostic.message(),
        "The source is not in canonical authoring form at this line."
    );
    assert_eq!(
        diagnostic.hint(),
        "Run `podway procedure format <file> --write` to rewrite the file in canonical form."
    );
    assert_eq!(diagnostic.source_path(), "workflow.yaml");
    // `id: minimal` and `schema: ...` share no leading character, so the divergence is the whole
    // line, and the span reaches the end of the source line rather than of the canonical one.
    assert_eq!(diagnostic.location().line(), 1);
    assert_eq!(diagnostic.location().column(), 1);
    assert_eq!(diagnostic.location().end_line(), 1);
    assert_eq!(diagnostic.location().end_column(), 12);
    assert_eq!(diagnostic.field(), "id");
}

#[test]
fn v2aut002_drift_in_the_middle_of_a_document_names_the_line_that_moved() {
    let canonical = format_yaml(COMMENTED_YAML).document().to_owned();
    const NESTED: &str = "  # The reusable contract every placement below uses.";
    let expected_line = canonical
        .lines()
        .position(|line| line == NESTED)
        .expect("the canonical rendering indents the nested comment block")
        + 1;
    assert!(
        expected_line > 1 && expected_line < canonical.lines().count(),
        "the moved line must sit in the middle of the document"
    );

    // A full-line comment belongs to the node it precedes and is re-emitted at that node's indent,
    // so dedenting it is drift even though the comment text itself survives verbatim.
    let diagnostic = drift_yaml(&canonical.replace(
        &format!("{NESTED}\n"),
        "# The reusable contract every placement below uses.\n",
    ));

    assert_eq!(
        diagnostic.location().line(),
        u32::try_from(expected_line).expect("the corpus document is short")
    );
    assert_eq!(diagnostic.location().column(), 1);
    // A comment line declares no node, so no path anchors there and the document root is the
    // honest answer.
    assert_eq!(diagnostic.field(), "$");
}

#[test]
fn v2aut002_a_missing_trailing_newline_is_drift_located_on_the_last_source_line() {
    let source = MINIMAL_YAML
        .strip_suffix('\n')
        .expect("the corpus document ends in a newline");
    let diagnostic = drift_yaml(source);

    // The source's lines are a strict prefix of the canonical rendering's, so the divergence is
    // past the source's end and clamps back onto its last line.
    let last = u32::try_from(source.lines().count()).expect("the corpus document is short");
    assert_eq!(diagnostic.location().line(), last);
    assert_eq!(diagnostic.location().column(), 1);
    assert_eq!(diagnostic.location().end_column(), 21);
    assert_eq!(diagnostic.field(), "graph.nodes[only].terminal");
}

#[test]
fn v2aut002_a_doubled_trailing_newline_is_drift_located_past_the_last_content_line() {
    let source = format!("{MINIMAL_YAML}\n");
    let diagnostic = drift_yaml(&source);

    // The canonical rendering's lines are the strict prefix this time. The position clamps to the
    // source's own newline-split count, which counts the empty segment the extra newline opens.
    let segments = u32::try_from(source.split('\n').count()).expect("the corpus document is short");
    assert_eq!(diagnostic.location().line(), segments);
    assert_eq!(diagnostic.location().column(), 1);
    assert_eq!(diagnostic.location().end_column(), 1);
    assert_eq!(diagnostic.field(), "$");
}

#[test]
fn v2aut002_crlf_line_endings_are_drift_at_the_first_carriage_return() {
    let diagnostic = drift_yaml(&MINIMAL_YAML.replace('\n', "\r\n"));

    // A CRLF source parses to the same model and formats to LF, so every line differs — at the
    // character after the last shared one, which is the `\r`.
    assert_eq!(diagnostic.location().line(), 1);
    assert_eq!(diagnostic.location().column(), 28);
    assert_eq!(diagnostic.location().end_column(), 28);
    assert_eq!(diagnostic.field(), "schema");
}

#[test]
fn v2aut002_every_drifted_source_yields_exactly_one_bounded_diagnostic() {
    for (name, source) in drifted_yaml_corpus() {
        let diagnostic = drift_yaml(&source);
        assert_eq!(
            diagnostic.code(),
            AuthoringDiagnosticCode::FormatNotCanonical,
            "{name}"
        );
        assert_diagnostic_shape(&diagnostic);

        let location = diagnostic.location();
        assert!(
            (1..=1_048_576).contains(&location.line())
                && (1..=1_048_576).contains(&location.column())
                && location.end_line() == location.line()
                && location.end_column() >= location.column(),
            "{name}: {location:?} is not a bounded single-line span"
        );

        // One document, one verdict: the report a caller renders carries a single finding.
        let report = finalize_diagnostics(vec![(AuthoringStage::Format, diagnostic)]);
        assert_eq!(report.diagnostics().len(), 1, "{name}");
        assert_eq!(report.total(), 1, "{name}");
        assert!(!report.truncated(), "{name}");
        assert!(!report.valid(), "{name}");
    }
}

#[test]
fn v2aut002_the_drift_diagnostic_is_deterministic() {
    for (name, source) in drifted_yaml_corpus() {
        let first = drift_yaml(&source);
        for _ in 0..8 {
            assert_eq!(drift_yaml(&source), first, "{name}");
        }
    }
}

#[test]
fn v2aut002_a_document_that_cannot_be_formatted_is_never_also_called_non_canonical() {
    // Each of these fails at a different stage — parse, validate, and the source-construct scan —
    // and none of them reaches the byte comparison, so none of them can report drift on top of the
    // failure that stopped it.
    for (name, source) in [
        (
            "unknown field",
            MINIMAL_YAML.replace("id: minimal\n", "id: minimal\nbogus: 1\n"),
        ),
        (
            "unknown reference",
            MINIMAL_YAML.replace("      use: work\n", "      use: absent\n"),
        ),
        (
            "inline comment",
            MINIMAL_YAML.replace("id: minimal\n", "id: minimal # the identifier\n"),
        ),
    ] {
        let reported = diagnostics(&source, ProcedureDocumentFormat::Yaml);
        assert!(!reported.is_empty(), "{name}");
        assert!(
            reported
                .iter()
                .all(|diagnostic| diagnostic.code() != AuthoringDiagnosticCode::FormatNotCanonical),
            "{name}: an unformattable document reports its own stage, never drift: {reported:?}"
        );
    }
}
