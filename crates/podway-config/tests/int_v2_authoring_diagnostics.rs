//! V2AUT-008: the closed `ConfigError` → authoring-diagnostic mapping (dossier sections 11.2 and
//! 11.6).
//!
//! This file is the contract for `podway_config::config_error_diagnostic`, the one production
//! answer to "what does the author see when a Procedure v2 document is refused". Three obligations,
//! asserted separately because they fail separately:
//!
//! 1. **Totality.** Every `ConfigError` variant classifies into a code the catalog declares, with a
//!    well-formed field, location, message, and hint. Nothing panics, nothing is dropped, and
//!    nothing invents a code the catalog does not carry.
//! 2. **The truth table.** Each refinement is asserted against a document that really reaches that
//!    raise site, so the table records what the pipeline does rather than what it was meant to do.
//!    The unrefined rows are asserted too: leaving a rejection on the generic code is a decision,
//!    and a decision that is not pinned is a decision that drifts.
//! 3. **The vet boundary.** The graph fixture's nineteen negative recipes split into the ones
//!    validate can prove today and the ones that need path analysis. Both halves are asserted, so
//!    the day V2GRF-001 lands a rule, the boundary test — not a reviewer — notices.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use podway_config::{
    AuthoringContext, AuthoringStage, ConfigError, ParsedProcedure, ProcedureDocumentFormat,
    ValidatedProcedureV2, config_error_diagnostic, finalize_diagnostics, parse_procedure_document,
    validate_procedure_v2,
};
use podway_core::{AuthoringDiagnostic, AuthoringDiagnosticCode, AuthoringSeverity};
use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

const SOURCE_PATH: &str = "workflow.yaml";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs both authoring stages and returns the first failure.
fn reject(source: &str) -> ConfigError {
    match admit(source) {
        Ok(_) => panic!("document must be rejected:\n{source}"),
        Err(error) => error,
    }
}

fn admit(source: &str) -> Result<ValidatedProcedureV2, ConfigError> {
    match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)? {
        ParsedProcedure::V2(parsed) => validate_procedure_v2(parsed),
        ParsedProcedure::V1(_) => panic!("expected v2 dispatch, got v1"),
    }
}

/// The production diagnostic for a document's own rejection.
fn diagnose(source: &str) -> AuthoringDiagnostic {
    let error = reject(source);
    let context = AuthoringContext::new(SOURCE_PATH, source, ProcedureDocumentFormat::Yaml);
    config_error_diagnostic(&error, &context)
}

/// The production diagnostic for a hand-built `ConfigError`, read against a real source so the
/// location rules run.
fn diagnose_error(error: &ConfigError, source: &str) -> AuthoringDiagnostic {
    let context = AuthoringContext::new(SOURCE_PATH, source, ProcedureDocumentFormat::Yaml);
    config_error_diagnostic(error, &context)
}

/// Asserts every bound the authoring-diagnostic schema fixes, and that the code is catalogued.
///
/// Mirrors `assert_diagnostic_shape` in `int_v2_procedure_format.rs` deliberately: the two files
/// assert the same schema from opposite directions — that file over the format stage's emissions,
/// this one over the classifier's — and a shared helper would let one hide a regression in the
/// other by being weakened once.
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
    assert!(
        AuthoringDiagnosticCode::ALL
            .iter()
            .any(|code| code.as_str() == diagnostic.code().as_str()),
        "{} is not a catalogued code",
        diagnostic.code().as_str()
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
    assert!(location["end_column"].as_u64() >= location["column"].as_u64());
}

// ---------------------------------------------------------------------------------------------
// Corpus
//
// One base document that reaches every closed-reference check, mutated by `str::replace` so each
// case differs from the accepted document by exactly the edit its name describes. Anything a
// mutation does not touch keeps validating, which is what makes the resulting `ConfigError` the
// consequence of that one edit.
// ---------------------------------------------------------------------------------------------

const BASE: &str = r#"schema: podway.procedure/v2
id: diagnostics
version: "1"
name: Diagnostics
purpose: Exercise the authoring diagnostic mapping end to end.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Work
    intent: Do the work this node owns.
    items:
      - id: note
        type: text
        prompt: Record what happened.
        required: true
  assess:
    type: decision
    title: Assess
    objective: Assess the session goal.
    prompt: What is the outcome of this session?
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Close the session out.
graph:
  entry: start
  nodes:
    - id: start
      use: work
      next: review
    - id: review
      use: assess
      evidence_from:
        - node: start
          items:
            - note
      routes:
        achieved:
          to: done
          effect: advance
        not-achieved:
          to: start
          effect: rework
        superseded:
          to: done
          effect: advance
    - id: done
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - start
"#;

/// `BASE` with one substring replaced, asserting the substring was actually there.
fn mutate(from: &str, to: &str) -> String {
    assert!(BASE.contains(from), "base document has no {from:?}");
    BASE.replacen(from, to, 1)
}

// ---------------------------------------------------------------------------------------------
// The truth table
// ---------------------------------------------------------------------------------------------

/// One row: a document, the code the production mapping reports for it, and the authored field it
/// points the author at.
struct Row {
    label: &'static str,
    source: String,
    code: &'static str,
    field: &'static str,
}

fn row(label: &'static str, source: String, code: &'static str, field: &'static str) -> Row {
    Row {
        label,
        source,
        code,
        field,
    }
}

/// Every `(ConfigError` shape, code, field) the v2 parse and validate stages can produce, each
/// reached through a real document.
///
/// The refinements below are keyed on constants the raise sites themselves read — the `field` of a
/// `UnknownV2Reference`, the `(field, reason)` of a `V2ShapeMismatch` or an `InvalidValue` — never
/// on a string typed out at the classifier. The rows that stay `AUTHORING_SCHEMA_INVALID` record
/// the cases where the catalog has no code that fits; each is explained where the mapping makes the
/// choice.
fn truth_table() -> Vec<Row> {
    vec![
        // -- UnknownV2Reference, refined by the authored field ------------------------------
        row(
            "placement use names no definition",
            mutate("      use: work\n", "      use: absent\n"),
            "GRAPH_DEFINITION_UNKNOWN",
            "graph.nodes.use",
        ),
        row(
            "action next names no placement",
            mutate("      next: review\n", "      next: ghost\n"),
            "ROUTE_TARGET_NOT_FOUND",
            "graph.nodes.next",
        ),
        row(
            "decision route target names no placement",
            mutate(
                "        achieved:\n          to: done\n",
                "        achieved:\n          to: ghost\n",
            ),
            "ROUTE_TARGET_NOT_FOUND",
            "graph.nodes.routes.to",
        ),
        row(
            "evidence source names no placement",
            mutate("        - node: start\n", "        - node: ghost\n"),
            "EVIDENCE_SOURCE_UNKNOWN",
            "graph.nodes.evidence_from.node",
        ),
        row(
            "evidence selector names no item",
            mutate("            - note\n", "            - ghost\n"),
            "EVIDENCE_SELECTOR_UNKNOWN_ITEM",
            "graph.nodes.evidence_from.items",
        ),
        row(
            "manual rework target names no placement",
            mutate("    - start\n", "    - ghost\n"),
            "MANUAL_REWORK_TARGET_UNKNOWN",
            "manual_rework.allowed_targets",
        ),
        // -- V2ShapeMismatch, refined by (field, reason) ------------------------------------
        row(
            "route names an undeclared option",
            mutate(
                "        superseded:\n          to: done\n          effect: advance\n",
                "        superseded:\n          to: done\n          effect: advance\n        ghost:\n          to: done\n          effect: advance\n",
            ),
            "DECISION_ROUTE_OPTION_UNDEFINED",
            "graph.nodes.routes",
        ),
        row(
            "a declared option has no route",
            mutate(
                "        superseded:\n          to: done\n          effect: advance\n",
                "",
            ),
            "DECISION_OPTION_ROUTE_MISSING",
            "graph.nodes.routes",
        ),
        row(
            "evidence names its consuming placement",
            mutate("        - node: start\n", "        - node: review\n"),
            "EVIDENCE_SOURCE_SELF_REFERENCE",
            "graph.nodes.evidence_from.node",
        ),
        row(
            "assessment without the goal tracking opt-in",
            mutate("goal_tracking: true\n", ""),
            "GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING",
            "node_definitions.assessment",
        ),
        // No catalog code fits an outcome mapping that names an undeclared *option*:
        // `GOAL_ASSESSMENT_OUTCOME_UNKNOWN` is the unknown-outcome case and
        // `GOAL_ASSESSMENT_OPTION_UNMAPPED` is the opposite direction (and vet's).
        row(
            "assessment outcome names an undeclared option",
            mutate("        achieved: achieved\n", "        ghost: achieved\n"),
            "AUTHORING_SCHEMA_INVALID",
            "node_definitions.assessment.outcomes",
        ),
        // No catalog code names a definition of the wrong kind.
        row(
            "an action placement uses a decision definition",
            mutate("      use: work\n", "      use: assess\n"),
            "AUTHORING_SCHEMA_INVALID",
            "graph.nodes.use",
        ),
        row(
            "a decision placement uses an action definition",
            mutate("      use: assess\n", "      use: work\n"),
            "AUTHORING_SCHEMA_INVALID",
            "graph.nodes.use",
        ),
        // -- InvalidValue, refined by the static reason its core raise site carries ---------
        row(
            "graph entry names no placement",
            mutate("  entry: start\n", "  entry: ghost\n"),
            "ENTRY_NODE_INVALID",
            "graph.entry",
        ),
        row(
            "two placements share an identifier",
            mutate("    - id: done\n", "    - id: start\n"),
            "AMBIGUOUS_GRAPH_REFERENCE",
            "graph.nodes",
        ),
        row(
            "an assessment maps an option to an unknown outcome",
            mutate("        achieved: achieved\n", "        achieved: bogus\n"),
            "GOAL_ASSESSMENT_OUTCOME_UNKNOWN",
            "node_definitions.assessment.outcomes",
        ),
        row(
            "an action placement declares both dispositions",
            mutate(
                "      next: review\n",
                "      next: review\n      terminal: true\n",
            ),
            "ACTION_DISPOSITION_INVALID",
            "graph.nodes",
        ),
        row(
            "an action placement declares neither disposition",
            mutate("      terminal: true\n", ""),
            "ACTION_DISPOSITION_INVALID",
            "graph.nodes",
        ),
        // -- Unrefined: the closed schema itself, and the shared decoder --------------------
        // `terminal: false` is a value violation of the schema's `const: true`, not a claim about
        // which disposition the placement declared, so it stays generic on purpose.
        row(
            "terminal declared false",
            mutate("      terminal: true\n", "      terminal: false\n"),
            "AUTHORING_SCHEMA_INVALID",
            "graph.nodes.terminal",
        ),
        row(
            "a decision placement declares a skip policy",
            mutate(
                "      use: assess\n",
                "      use: assess\n      skip:\n        allowed: true\n        reason_required: true\n",
            ),
            "DECISION_SKIP_NOT_ALLOWED",
            "graph.nodes",
        ),
        row(
            "a malformed identifier",
            mutate("id: diagnostics\n", "id: Diagnostics\n"),
            "AUTHORING_SCHEMA_INVALID",
            "procedure.id",
        ),
        row(
            "a value past its bound",
            mutate(
                "purpose: Exercise the authoring diagnostic mapping end to end.\n",
                &format!("purpose: {}\n", "p".repeat(501)),
            ),
            "AUTHORING_SCHEMA_INVALID",
            "procedure.purpose",
        ),
        row(
            "an unknown field",
            mutate("id: diagnostics\n", "id: diagnostics\nbogus: 1\n"),
            "AUTHORING_SCHEMA_INVALID",
            "$",
        ),
        row(
            "a duplicate mapping key",
            mutate("id: diagnostics\n", "id: diagnostics\nid: diagnostics\n"),
            "SOURCE_CONSTRUCT_UNSUPPORTED",
            "$",
        ),
        row(
            "a YAML anchor",
            mutate("  entry: start\n", "  entry: &anchor start\n"),
            "SOURCE_CONSTRUCT_UNSUPPORTED",
            "$",
        ),
    ]
}

#[test]
fn v2aut008_the_config_error_mapping_reports_the_documented_code_and_field() {
    for row in truth_table() {
        let diagnostic = diagnose(&row.source);
        assert_eq!(
            diagnostic.code().as_str(),
            row.code,
            "{}: {:?}",
            row.label,
            reject(&row.source)
        );
        assert_eq!(diagnostic.field(), row.field, "{}", row.label);
        assert_diagnostic_shape(&diagnostic);
    }
}

#[test]
fn v2aut008_every_reported_code_is_an_error_the_catalog_declares() {
    let catalog = catalog_severities();
    for row in truth_table() {
        let diagnostic = diagnose(&row.source);
        assert_eq!(
            catalog.get(diagnostic.code().as_str()).copied(),
            Some("error"),
            "{}: validate reports only catalogued error-severity codes",
            row.label,
        );
        assert_eq!(
            diagnostic.severity(),
            AuthoringSeverity::Error,
            "{}",
            row.label
        );
    }
}

#[test]
fn v2aut008_classification_is_deterministic_and_independent_of_repetition() {
    for row in truth_table() {
        let first = serde_json::to_value(diagnose(&row.source)).expect("serializes");
        for _ in 0..8 {
            assert_eq!(
                serde_json::to_value(diagnose(&row.source)).expect("serializes"),
                first,
                "{}",
                row.label
            );
        }
    }
}

#[test]
fn v2aut008_the_message_names_the_rule_and_the_hint_is_stable_per_code() {
    let mut hints: std::collections::BTreeMap<&'static str, String> =
        std::collections::BTreeMap::new();
    for row in truth_table() {
        let diagnostic = diagnose(&row.source);
        // Every message ends in a period and embeds the rejection's own text, which carries the
        // offending value: the author is told the rule and the value in one sentence.
        assert!(diagnostic.message().ends_with('.'), "{}", row.label);
        assert!(
            diagnostic.message().chars().count() > 20,
            "{}: {}",
            row.label,
            diagnostic.message()
        );
        // One hint per code, whatever document produced it.
        if let Some(previous) = hints.get(row.code) {
            assert_eq!(previous, diagnostic.hint(), "{}", row.label);
        } else {
            hints.insert(row.code, diagnostic.hint().to_owned());
        }
    }
    // Every refined code carries its own opening sentence rather than the generic one.
    let generic = "The Procedure source violates the closed v2 authoring schema:";
    for row in truth_table() {
        let diagnostic = diagnose(&row.source);
        if row.code == "AUTHORING_SCHEMA_INVALID" {
            assert!(diagnostic.message().starts_with(generic), "{}", row.label);
        } else {
            assert!(
                !diagnostic.message().starts_with(generic),
                "{} reports {} but reads as the generic schema violation",
                row.label,
                row.code
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Totality over `ConfigError`
// ---------------------------------------------------------------------------------------------

/// Every variant of `ConfigError`, including the ones the v2 stages cannot reach.
///
/// `classify` matches this enum exhaustively — there is no top-level `_` arm, so the compiler
/// refuses a new variant that nobody classified. This list is the runtime half of that argument: it
/// proves the classification of each variant is a *catalogued* code with a well-formed diagnostic,
/// which the compiler cannot check. The two together are what "no variant is silently swallowed"
/// means.
fn every_config_error_variant() -> Vec<(&'static str, ConfigError)> {
    vec![
        (
            "InvalidSchema",
            ConfigError::InvalidSchema {
                expected: "podway.procedure/v2",
                actual: "podway.procedure/v9".to_owned(),
            },
        ),
        (
            "InvalidIdentifier",
            ConfigError::InvalidIdentifier {
                field: "procedure.id",
                value: "Diagnostics".to_owned(),
            },
        ),
        (
            "InvalidValue",
            ConfigError::InvalidValue {
                field: "graph.nodes.terminal",
                reason: "terminal must be true",
            },
        ),
        (
            "OutOfBounds",
            ConfigError::OutOfBounds {
                field: "procedure.purpose",
                min: 1,
                max: 500,
                actual: 501,
            },
        ),
        // The projection field is `pub(crate)`, so the literal is written out here on purpose: an
        // independent witness that the constant `procedure_v2_canonical` reports under and the one
        // `procedure_v2_diagnostics` switches on are the same string.
        (
            "OutOfBounds at the projection bound",
            ConfigError::OutOfBounds {
                field: "canonical source projection",
                min: 1,
                max: 131_072,
                actual: 131_073,
            },
        ),
        (
            "DuplicateValue",
            ConfigError::DuplicateValue {
                field: "stage.id",
                value: "one".to_owned(),
            },
        ),
        (
            "UnknownReturnTarget",
            ConfigError::UnknownReturnTarget {
                stage_id: "one".to_owned(),
            },
        ),
        (
            "UnknownV2Reference",
            ConfigError::UnknownV2Reference {
                field: "graph.nodes.use",
                value: "absent".to_owned(),
            },
        ),
        (
            "V2ShapeMismatch",
            ConfigError::V2ShapeMismatch {
                field: "graph.nodes.routes",
                reason: "every declared decision option must have exactly one route",
            },
        ),
        (
            "Serialization",
            ConfigError::Serialization("boom".to_owned()),
        ),
        ("NonCanonicalNumber", ConfigError::NonCanonicalNumber),
        ("InvalidDigest", ConfigError::InvalidDigest),
        (
            "InputTooLarge",
            ConfigError::InputTooLarge {
                maximum: 1_048_576,
                actual: 1_048_577,
            },
        ),
        (
            "InputTooDeep",
            ConfigError::InputTooDeep {
                maximum: 64,
                actual: 65,
            },
        ),
        (
            "InputTooComplex",
            ConfigError::InputTooComplex {
                maximum: 100_000,
                actual: 100_001,
            },
        ),
        (
            "DuplicateKey",
            ConfigError::DuplicateKey {
                key: "id".to_owned(),
            },
        ),
        (
            "UnsupportedYamlFeature",
            ConfigError::UnsupportedYamlFeature { feature: "anchor" },
        ),
        (
            "InvalidDocument with a schema reason",
            ConfigError::InvalidDocument {
                reason: "unknown field `bogus`".to_owned(),
            },
        ),
        (
            "InvalidDocument with a decoder reason",
            ConfigError::InvalidDocument {
                reason: "procedure document is not valid UTF-8".to_owned(),
            },
        ),
        (
            "WarningsAsErrors",
            ConfigError::WarningsAsErrors {
                warnings: Vec::new(),
            },
        ),
        (
            "CoreAdmission",
            ConfigError::CoreAdmission {
                reason: "boom".to_owned(),
            },
        ),
    ]
}

#[test]
fn v2aut008_every_config_error_variant_classifies_into_the_catalog_without_panicking() {
    let catalog = catalog_severities();
    for (label, error) in every_config_error_variant() {
        let diagnostic = diagnose_error(&error, BASE);
        assert_diagnostic_shape(&diagnostic);
        assert_eq!(
            catalog.get(diagnostic.code().as_str()).copied(),
            Some("error"),
            "{label}: {} is not a catalogued error code",
            diagnostic.code().as_str(),
        );
    }
}

#[test]
fn v2aut008_the_variants_unreachable_from_v2_classify_defensively_rather_than_precisely() {
    // Verified by grep at V2AUT-008: every `DuplicateValue` raise site is in the v1 procedure or
    // workspace validators, and the four below are v1 admission or canonicalization failures. None
    // is reachable from `parse_procedure_document`'s v2 arm or from `validate_procedure_v2`. They
    // must still classify — a diagnostic path that can panic is worse than an imprecise code — so
    // each lands on the generic schema code rather than on a graph code it cannot have earned.
    for (label, error) in every_config_error_variant() {
        let unreachable = matches!(
            label,
            "DuplicateValue"
                | "UnknownReturnTarget"
                | "Serialization"
                | "InvalidDigest"
                | "WarningsAsErrors"
                | "CoreAdmission"
        );
        if unreachable {
            assert_eq!(
                diagnose_error(&error, BASE).code().as_str(),
                "AUTHORING_SCHEMA_INVALID",
                "{label}",
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Locations
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut008_a_shape_and_a_value_locate_the_exact_offending_line() {
    // `graph.nodes.routes.to` occurs three times in the base document. The offending value is what
    // picks one, and the shape is what stops a same-valued scalar elsewhere from answering.
    let source = mutate(
        "        not-achieved:\n          to: start\n",
        "        not-achieved:\n          to: ghost\n",
    );
    let diagnostic = diagnose(&source);
    assert_eq!(diagnostic.code().as_str(), "ROUTE_TARGET_NOT_FOUND");

    let offending = source
        .lines()
        .position(|line| line.trim() == "to: ghost")
        .expect("the mutation is in the document")
        + 1;
    assert_eq!(
        diagnostic.location().line(),
        u32::try_from(offending).expect("small line number"),
    );
    // The span opens at the key and closes at the end of that line.
    assert_eq!(diagnostic.location().column(), 11);
    assert_eq!(
        diagnostic.location().end_line(),
        diagnostic.location().line()
    );
    assert_eq!(diagnostic.location().end_column(), 20);
}

#[test]
fn v2aut008_a_shape_lookup_never_answers_from_a_different_shape() {
    // `ghost` appears as a manual rework target and nowhere else. A `use` shape must not find it.
    let source = mutate("    - start\n", "    - ghost\n");
    let diagnostic = diagnose(&source);
    assert_eq!(diagnostic.code().as_str(), "MANUAL_REWORK_TARGET_UNKNOWN");
    let offending = source
        .lines()
        .position(|line| line.trim() == "- ghost")
        .expect("the mutation is in the document")
        + 1;
    assert_eq!(
        diagnostic.location().line(),
        u32::try_from(offending).expect("small line number"),
    );
}

/// The evidence selector deliberately has no shape lookup: the same item identifier can be legal
/// in one `evidence_from` entry and illegal in another, and a first-source-order match could send
/// the author to the legal line. The degraded location must never be the legal occurrence.
#[test]
fn v2aut008_an_unknown_selector_never_points_at_a_legal_occurrence_of_the_same_item() {
    // `note` is legal under `review`'s entry (source `start` declares it) and illegal under
    // `done`'s new entry (source `review` uses `assess`, which declares no items). The legal
    // occurrence comes first in source order — exactly the case a shape match would get wrong.
    let source = mutate(
        "    - id: done\n      use: finish\n      terminal: true\n",
        "    - id: done\n      use: finish\n      evidence_from:\n        - node: review\n          items:\n            - note\n      terminal: true\n",
    );
    let diagnostic = diagnose(&source);
    assert_eq!(diagnostic.code().as_str(), "EVIDENCE_SELECTOR_UNKNOWN_ITEM");

    let legal_line = source
        .lines()
        .position(|line| line.trim() == "- note")
        .expect("the legal occurrence is in the document")
        + 1;
    assert_ne!(
        diagnostic.location().line(),
        u32::try_from(legal_line).expect("small line number"),
        "the location must not name the entry where the selector is legal"
    );
}

#[test]
fn v2aut008_a_field_with_no_value_falls_back_to_its_longest_present_prefix() {
    // `GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING` names `node_definitions.assessment`, a shape the
    // source never spells: `assessment` is nested under a definition key. The prefix rule lands on
    // `node_definitions`, which the source does declare, rather than on nothing.
    let source = mutate("goal_tracking: true\n", "");
    let diagnostic = diagnose(&source);
    assert_eq!(
        diagnostic.code().as_str(),
        "GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING"
    );
    let anchor = source
        .lines()
        .position(|line| line == "node_definitions:")
        .expect("the base document declares node_definitions")
        + 1;
    assert_eq!(
        diagnostic.location().line(),
        u32::try_from(anchor).expect("small line number"),
    );
    assert_eq!(diagnostic.location().column(), 1);
}

#[test]
fn v2aut008_a_diagnostic_about_a_path_no_source_declares_lands_on_the_document_start() {
    // A decoder failure names no authored field at all.
    let error = ConfigError::InputTooLarge {
        maximum: 1_048_576,
        actual: 1_048_577,
    };
    let diagnostic = diagnose_error(&error, BASE);
    assert_eq!(diagnostic.field(), "$");
    assert_eq!(diagnostic.location().line(), 1);
    assert_eq!(diagnostic.location().column(), 1);
    assert_eq!(diagnostic.location().end_line(), 1);
    assert_eq!(diagnostic.location().end_column(), 1);
}

#[test]
fn v2aut008_locations_survive_a_source_that_cannot_be_indexed() {
    // The index is built from parser events; a source the scanner cannot walk yields a truncated
    // index. Classification must still produce a schema-valid location rather than failing.
    let diagnostic = diagnose_error(
        &ConfigError::UnknownV2Reference {
            field: "graph.nodes.use",
            value: "absent".to_owned(),
        },
        "schema: podway.procedure/v2\n  : : :\n",
    );
    assert_diagnostic_shape(&diagnostic);
    assert_eq!(diagnostic.code().as_str(), "GRAPH_DEFINITION_UNKNOWN");
}

// ---------------------------------------------------------------------------------------------
// Report assembly
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut008_a_single_stage_rejection_finalizes_into_one_invalid_report() {
    let source = mutate("      use: work\n", "      use: absent\n");
    let context = AuthoringContext::new(SOURCE_PATH, &source, ProcedureDocumentFormat::Yaml);
    let report = finalize_diagnostics(vec![(
        AuthoringStage::Validate,
        config_error_diagnostic(&reject(&source), &context),
    )]);

    assert_eq!(report.diagnostics().len(), 1);
    assert_eq!(report.total(), 1);
    assert!(!report.truncated());
    assert!(
        !report.valid(),
        "an error-severity finding makes it invalid"
    );
}

// ---------------------------------------------------------------------------------------------
// The vet boundary: `tests/fixtures/v2/graphs/negative-cases.json`
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct NegativeCase {
    id: String,
    expected_code: String,
}

#[derive(Deserialize)]
struct NegativeFixture {
    cases: Vec<NegativeCase>,
}

fn negative_cases() -> Vec<NegativeCase> {
    let path = repo_root().join("tests/fixtures/v2/graphs/negative-cases.json");
    let fixture: NegativeFixture = serde_json::from_slice(
        &fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
    )
    .expect("negative-cases.json parses as JSON");
    fixture.cases
}

/// Every code the parse and validate stages can report, as the truth table above establishes.
///
/// This set is the boundary: a recipe whose code is in it must be provable today, and a recipe
/// whose code is outside it must be unprovable today. Both directions are asserted, so adding a
/// rule without moving this set fails, and moving this set without a rule fails too.
fn validate_reachable_codes() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "AUTHORING_SCHEMA_INVALID",
        "SOURCE_CONSTRUCT_UNSUPPORTED",
        "SOURCE_PROJECTION_BUDGET_EXCEEDED",
        "ENTRY_NODE_INVALID",
        "GRAPH_DEFINITION_UNKNOWN",
        "ROUTE_TARGET_NOT_FOUND",
        "ACTION_DISPOSITION_INVALID",
        "DECISION_OPTION_ROUTE_MISSING",
        "DECISION_ROUTE_OPTION_UNDEFINED",
        "DECISION_SKIP_NOT_ALLOWED",
        "GOAL_ASSESSMENT_OUTCOME_UNKNOWN",
        "GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING",
        "EVIDENCE_SOURCE_UNKNOWN",
        "EVIDENCE_SOURCE_SELF_REFERENCE",
        "EVIDENCE_SELECTOR_UNKNOWN_ITEM",
        "MANUAL_REWORK_TARGET_UNKNOWN",
        "AMBIGUOUS_GRAPH_REFERENCE",
    ])
}

#[test]
fn v2aut008_the_reachable_code_set_is_exactly_what_the_truth_table_produces() {
    let mut produced: BTreeSet<&'static str> = truth_table().iter().map(|row| row.code).collect();
    // The projection bound needs a document at ~131 KB to reach end to end; it is proven there by
    // `int_v2_procedure_canonical.rs` and `int_v2_procedure_format.rs`, and by the variant walk
    // above. Recorded here so the boundary set stays complete.
    produced.insert("SOURCE_PROJECTION_BUDGET_EXCEEDED");
    assert_eq!(produced, validate_reachable_codes());
}

/// The recipes the parse and validate stages prove today, and the document that proves each.
///
/// Each mutation is the fixture's own `mutation` sentence, applied to the base document.
fn provable_recipes() -> Vec<(&'static str, String)> {
    vec![
        (
            // "encode the same node definition key twice in source"
            "duplicate-definition-source-key",
            mutate(
                "  finish:\n    type: action\n",
                "  work:\n    type: action\n    title: Work again\n    intent: Do the work twice.\n  finish:\n    type: action\n",
            ),
        ),
        (
            // "make two graph references resolve ambiguously"
            "ambiguous-placement-id",
            mutate("    - id: done\n", "    - id: start\n"),
        ),
        (
            // "set graph.entry to an undefined node"
            "unknown-entry",
            mutate("  entry: start\n", "  entry: ghost\n"),
        ),
        (
            // "remove the route for one decision option"
            "option-without-route",
            mutate(
                "        superseded:\n          to: done\n          effect: advance\n",
                "",
            ),
        ),
        (
            // "add a route for an undefined decision option"
            "route-for-undefined-option",
            mutate(
                "        superseded:\n          to: done\n          effect: advance\n",
                "        superseded:\n          to: done\n          effect: advance\n        ghost:\n          to: done\n          effect: advance\n",
            ),
        ),
        (
            // "reference an undefined source placement"
            "unknown-evidence-source",
            mutate("        - node: start\n", "        - node: ghost\n"),
        ),
        (
            // "reference the consuming placement itself"
            "self-evidence-source",
            mutate("        - node: start\n", "        - node: review\n"),
        ),
        (
            // "select an undefined source item"
            "unknown-evidence-item",
            mutate("            - note\n", "            - ghost\n"),
        ),
        (
            // "allow manual rework to an undefined placement"
            "manual-rework-target-invalid",
            mutate("    - start\n", "    - ghost\n"),
        ),
    ]
}

/// The recipes whose mutation validate admits, because deciding them needs a path through the
/// graph. Each is V2GRF-001/V2GRF-002's, and each stays `implementation_status: planned`.
fn vet_deferred_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            // "remove every finite route to a terminal": the only terminal action becomes a
            // self-loop, so no path reaches a terminal. Every reference still resolves.
            "no-terminal-route",
            mutate("      terminal: true\n", "      next: done\n"),
        ),
        (
            // "append an unreferenced graph node"
            "unreachable-node",
            mutate(
                "    - id: done\n",
                "    - id: orphan\n      use: finish\n      terminal: true\n    - id: done\n",
            ),
        ),
        (
            // "add a forward cycle that is not declared rework": the achieved route advances
            // backwards to the entry.
            "invalid-cycle",
            mutate(
                "        achieved:\n          to: done\n",
                "        achieved:\n          to: start\n",
            ),
        ),
        (
            // "route rework to a non-dominating placement"
            "rework-target-not-dominating",
            mutate(
                "        not-achieved:\n          to: start\n          effect: rework\n",
                "        not-achieved:\n          to: done\n          effect: rework\n",
            ),
        ),
        (
            // "make a required source not strictly dominate its consumer": `done` is downstream of
            // `review`, so it cannot produce evidence `review` reads.
            "evidence-source-not-dominating",
            mutate(
                "        - node: start\n          items:\n            - note\n",
                "        - node: done\n",
            ),
        ),
        (
            // "mark a required evidence source skippable"
            "skippable-required-source",
            mutate(
                "      use: work\n      next: review\n",
                "      use: work\n      skip:\n        allowed: true\n        reason_required: true\n      next: review\n",
            ),
        ),
        (
            // "place goal assessment off one terminal path": the entry can reach the terminal
            // without passing the assessment.
            "goal-assessment-not-dominating",
            mutate("      next: review\n", "      next: done\n"),
        ),
        (
            // "remove one assessment option mapping": a fourth option is declared and left
            // unmapped, keeping the mapping count at its minimum of three.
            "assessment-option-unmapped",
            mutate(
                "      - id: superseded\n        label: Superseded\n",
                "      - id: superseded\n        label: Superseded\n      - id: deferred\n        label: Deferred\n",
            )
            .replacen(
                "        superseded:\n          to: done\n          effect: advance\n",
                "        superseded:\n          to: done\n          effect: advance\n        deferred:\n          to: done\n          effect: advance\n",
                1,
            ),
        ),
        (
            // "remove the only option for one goal outcome": every option now maps to `achieved`,
            // so `not_achieved` and `superseded` are unreachable.
            "assessment-outcome-unreachable",
            mutate(
                "        not-achieved: not_achieved\n        superseded: superseded\n",
                "        not-achieved: achieved\n        superseded: achieved\n",
            ),
        ),
    ]
}

#[test]
fn v2aut008_every_negative_recipe_is_either_validate_provable_or_vet_deferred() {
    let cases = negative_cases();
    assert_eq!(cases.len(), 19, "the fixture declares nineteen recipes");

    let provable: BTreeSet<&str> = provable_recipes().iter().map(|(id, _)| *id).collect();
    let deferred: BTreeSet<&str> = vet_deferred_documents().iter().map(|(id, _)| *id).collect();
    // `readback-over-budget` needs a document sized against the read-back budget rather than a
    // one-line mutation; it is covered by the code-set argument below and by V2GRF-002.
    let deferred_without_document = BTreeSet::from(["readback-over-budget"]);

    let reachable = validate_reachable_codes();
    for case in &cases {
        let id = case.id.as_str();
        let in_provable = provable.contains(id);
        let in_deferred = deferred.contains(id) || deferred_without_document.contains(id);
        assert!(
            in_provable ^ in_deferred,
            "{id}: every recipe belongs to exactly one half of the boundary"
        );
        assert_eq!(
            in_provable,
            reachable.contains(case.expected_code.as_str()),
            "{id}: the split must agree with the reachable code set ({})",
            case.expected_code,
        );
    }
    assert_eq!(provable.len(), 9);
    assert_eq!(deferred.len() + deferred_without_document.len(), 10);
}

#[test]
fn v2aut008_the_provable_recipes_report_their_expected_code_through_the_whole_pipeline() {
    let expected: std::collections::BTreeMap<String, String> = negative_cases()
        .into_iter()
        .map(|case| (case.id, case.expected_code))
        .collect();

    for (id, source) in provable_recipes() {
        let diagnostic = diagnose(&source);
        assert_eq!(
            diagnostic.code().as_str(),
            expected[id].as_str(),
            "{id}: {:?}",
            reject(&source)
        );
        assert_diagnostic_shape(&diagnostic);
    }
}

#[test]
fn v2aut008_the_vet_deferred_recipes_are_admitted_by_validate() {
    let expected: std::collections::BTreeMap<String, String> = negative_cases()
        .into_iter()
        .map(|case| (case.id, case.expected_code))
        .collect();

    for (id, source) in vet_deferred_documents() {
        // The truthful boundary: validate resolves the closed reference set and nothing else, so
        // every one of these documents is admissible today. When V2GRF-001 lands a rule, the
        // rejection arrives from vet, not from here, and this assertion still holds.
        admit(&source).unwrap_or_else(|error| {
            panic!("{id}: validate must admit a path-analysis defect, got {error:?}")
        });
        assert!(
            !validate_reachable_codes().contains(expected[id].as_str()),
            "{id}: {} must stay outside the validate-reachable set",
            expected[id],
        );
    }
}

#[test]
fn v2aut008_the_base_document_the_recipes_mutate_is_itself_admissible() {
    // Every recipe above is "the base document, plus one edit". That only means what it says if the
    // base document validates.
    admit(BASE).expect("the base document must parse and validate");
}

// ---------------------------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------------------------

fn catalog_severities() -> std::collections::BTreeMap<String, &'static str> {
    #[derive(Deserialize)]
    struct Entry {
        code: String,
        severity: String,
    }
    #[derive(Deserialize)]
    struct Catalog {
        diagnostics: Vec<Entry>,
    }

    let path = repo_root().join("assets/specifications/authoring-diagnostics.json");
    let catalog: Catalog = serde_json::from_slice(
        &fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
    )
    .expect("authoring-diagnostics.json parses as JSON");

    catalog
        .diagnostics
        .into_iter()
        .map(|entry| {
            let severity = match entry.severity.as_str() {
                "error" => "error",
                "warning" => "warning",
                other => panic!("unknown severity {other}"),
            };
            (entry.code, severity)
        })
        .collect()
}

#[test]
fn v2aut008_every_hint_is_distinct_per_reachable_code() {
    // A hint that repeats across codes is a hint that stopped saying anything specific. The
    // reachable set is small enough to require this outright.
    let mut hints: std::collections::BTreeMap<String, &'static str> =
        std::collections::BTreeMap::new();
    for row in truth_table() {
        let diagnostic = diagnose(&row.source);
        if let Some(previous) = hints.insert(diagnostic.hint().to_owned(), row.code) {
            assert_eq!(
                previous, row.code,
                "{} and {} share a hint",
                previous, row.code
            );
        }
    }
}
