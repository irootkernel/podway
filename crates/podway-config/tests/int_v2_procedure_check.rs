//! V2AUT-005: the aggregate authoring gate for Procedure v2 (dossier section 11.5).
//!
//! Check is not a new analysis. It is the existing stages — parse, validate, the canonical
//! rendering, vet, lint — run under one set of short-circuit rules and merged into one bounded
//! report. So this file asserts the two things that are genuinely new and cannot be inherited from
//! the stages' own suites:
//!
//! - **which stages still run** after each kind of finding (a model-less document stops everything;
//!   a model that cannot be rendered still gets vetted and linted; drift stops nothing), and
//! - **what the merged report says** — reported order, the presence of the digest, and a `valid`
//!   flag derived from catalogued severity rather than asserted.
//!
//! The stage matrix below is the point of the file. Everything else — determinism, the report
//! bound, and the proof that check's drift finding is byte-identical to `format --check`'s — guards
//! the merge itself.

use podway_config::{
    AuthoringContext, FormatRequest, ParsedProcedure, ProcedureCheckReport,
    ProcedureDocumentFormat, ValidatedProcedureV2, check_procedure_v2, format_procedure_v2,
    lint_procedure_v2, parse_procedure_document, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    AuthoringDiagnostic, AuthoringDiagnosticCode, AuthoringSeverity, MAX_AUTHORING_DIAGNOSTICS,
    SOURCE_PROJECTION_MAX_CHARACTERS,
};
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

const SOURCE_PATH: &str = "workflow.yaml";

/// A document in canonical authoring form that fires no advisory rule: the aggregate gate's zero.
///
/// Every item is a `confirm`, because a `text` item materializes `min_length`, `max_length`, and
/// `multiline` in canonical form and a fixture that omitted them would be testing drift instead of
/// cleanliness.
const CLEAN_YAML: &str = r#"schema: podway.procedure/v2
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

/// The smallest legal document, already canonical. It declares no manual rework, so lint reports
/// `NO_REACTIVATION_PATH` against the document start — which is what makes it useful here: a
/// finding at line 1 that must still be reported *after* a format finding further down the file.
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

/// [`MINIMAL_YAML`] with one quoted scalar that canonical form writes plain: the model, and
/// therefore the digest, is untouched and only the bytes have drifted.
fn drifted_yaml() -> String {
    MINIMAL_YAML.replace("    title: Work\n", "    title: \"Work\"\n")
}

/// [`MINIMAL_YAML`] with a trailing inline comment, which canonical authoring form cannot
/// reattach. YAML reads the comment as whitespace, so the model is identical to `MINIMAL_YAML`'s
/// and every stage after the rendering has exactly the same work to do.
fn inline_comment_yaml() -> String {
    MINIMAL_YAML.replace("name: Minimal\n", "name: Minimal # the smallest one\n")
}

fn oversized_static_yaml() -> String {
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
        "schema: podway.procedure/v2\nid: check-budget\nversion: \"1\"\nname: Check budget\npurpose: Prove check carries the complete vet rule set.\nnode_definitions:\n  work:\n    type: action\n    title: {}\n    intent: {}\n    description: {}\n    instructions:\n{instructions}    items:\n{items}graph:\n  entry: work\n  nodes:\n    - id: work\n      use: work\n      terminal: true\n",
        "t".repeat(120),
        "n".repeat(300),
        "d".repeat(1_000),
    )
}

/// [`MINIMAL_YAML`] with a dangling placement reference: parses, fails closed-reference validation.
fn invalid_yaml() -> String {
    MINIMAL_YAML.replace("      use: work\n", "      use: absent\n")
}

/// A document declaring an unsupported Procedure schema.
const UNSUPPORTED_SCHEMA_YAML: &str = r#"schema: podway.procedure/v3
id: future
version: "1"
name: Future
"#;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn check(source: &str) -> ProcedureCheckReport {
    check_procedure_v2(FormatRequest {
        source,
        source_path: SOURCE_PATH,
        format: ProcedureDocumentFormat::Yaml,
    })
}

fn check_json(source: &str) -> ProcedureCheckReport {
    check_procedure_v2(FormatRequest {
        source,
        source_path: SOURCE_PATH,
        format: ProcedureDocumentFormat::Json,
    })
}

fn codes(report: &ProcedureCheckReport) -> Vec<&'static str> {
    report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().as_str())
        .collect()
}

fn admit(source: &str) -> ValidatedProcedureV2 {
    match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml) {
        Ok(ParsedProcedure::V2(parsed)) => {
            validate_procedure_v2(parsed).expect("the fixture must validate")
        }
        Err(error) => panic!("the fixture must parse: {error}\n{source}"),
    }
}

fn admit_json(source: &str) -> ValidatedProcedureV2 {
    match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Json) {
        Ok(ParsedProcedure::V2(parsed)) => {
            validate_procedure_v2(parsed).expect("the fixture must validate")
        }
        Err(error) => panic!("the fixture must parse: {error}"),
    }
}

fn context(source: &str) -> AuthoringContext<'_> {
    AuthoringContext::new(SOURCE_PATH, source, ProcedureDocumentFormat::Yaml)
}

/// The advisory codes the lint stage reports for `source`, so a check assertion can name what it
/// expects to have been carried through rather than restating the rule set.
fn lint_codes(source: &str) -> Vec<&'static str> {
    let validated = admit(source);
    lint_procedure_v2(&validated, &context(source))
        .iter()
        .map(|diagnostic| diagnostic.code().as_str())
        .collect()
}

fn serialized(diagnostics: &[AuthoringDiagnostic]) -> Vec<Value> {
    diagnostics
        .iter()
        .map(|diagnostic| serde_json::to_value(diagnostic).expect("a diagnostic serializes"))
        .collect()
}

/// The complete graph-vet code set delivered by V2GRF-001, V2GRF-002, and V2GSC-004.
const VET_SUBSET: &[AuthoringDiagnosticCode] = &[
    AuthoringDiagnosticCode::UnreachableGraphNode,
    AuthoringDiagnosticCode::NoTerminalPath,
    AuthoringDiagnosticCode::GoalAssessmentOptionUnmapped,
    AuthoringDiagnosticCode::GoalAssessmentOutcomeUnreachable,
    AuthoringDiagnosticCode::GoalAssessmentNotDominatingTerminal,
    AuthoringDiagnosticCode::EvidenceSourceDoesNotDominateConsumer,
    AuthoringDiagnosticCode::SkippableEvidenceSource,
    AuthoringDiagnosticCode::GraphCycleInvalid,
    AuthoringDiagnosticCode::ReworkTargetNotDominating,
    AuthoringDiagnosticCode::ReadbackBudgetExceeded,
    AuthoringDiagnosticCode::NextStaticBudgetExceeded,
    AuthoringDiagnosticCode::DecisionRecordBudgetExceeded,
    AuthoringDiagnosticCode::EvidencePreviewBudgetExceeded,
    AuthoringDiagnosticCode::PageTokenBudgetExceeded,
    AuthoringDiagnosticCode::NextFrameBudgetExceeded,
    AuthoringDiagnosticCode::ObservationStaticMembersBudgetExceeded,
];

// ---------------------------------------------------------------------------------------------
// 1. The stage matrix
// ---------------------------------------------------------------------------------------------

/// Drift does not stop the pipeline, and the report reads as section 11.5's pipeline rather than as
/// the order the stages had to run in.
///
/// The proof is positional: `NO_REACTIVATION_PATH` is anchored at the document start (line 1) and
/// the drift sits on line 9, so a report sorted by position alone would put lint first. It does not,
/// because the merge key leads with the stage.
#[test]
fn v2aut005_a_drifted_and_lint_dirty_document_reports_format_before_lint() {
    let source = drifted_yaml();
    let report = check(&source);

    let advisory = lint_codes(&source);
    assert!(
        advisory.contains(&"NO_REACTIVATION_PATH"),
        "the fixture must be lint-dirty: {advisory:?}"
    );

    let mut expected = vec!["FORMAT_NOT_CANONICAL"];
    expected.extend(advisory.iter().copied());
    assert_eq!(
        codes(&report),
        expected,
        "the format finding leads the report even though its line is the higher one",
    );

    let drift = &report.diagnostics()[0];
    assert!(
        drift.location().line() > report.diagnostics()[1].location().line(),
        "the ordering proof needs the lint finding to sit above the drift: {drift:?}",
    );

    // `valid` is read off the catalog, not assumed: `FORMAT_NOT_CANONICAL` is bound to *error*
    // severity in `assets/specifications/authoring-diagnostics.json`, so a document whose only
    // defect is its formatting is genuinely invalid, and the aggregate gate must say so. Every
    // advisory finding beside it stays a warning and contributes nothing to that verdict.
    assert_eq!(drift.severity(), AuthoringSeverity::Error);
    assert!(!report.valid(), "a drifted document is not valid");
    assert!(
        report.diagnostics()[1..]
            .iter()
            .all(|diagnostic| diagnostic.severity() == AuthoringSeverity::Warning),
        "only the drift is an error",
    );

    // The document is admissible, so it has a digest — a formatting defect never takes it away.
    assert_eq!(report.digest(), Some(admit(&source).digest()));
    assert_eq!(report.total(), u32::try_from(expected.len()).unwrap());
    assert!(!report.truncated());
}

/// A source the formatter cannot reproduce skips the drift comparison and nothing else.
///
/// There is no canonical text to compare against, so claiming the source is "not in canonical form"
/// would be an unfounded second finding about the same defect. The model still exists, so vet and
/// lint still run — the whole reason check does not stop at the rendering stage.
#[test]
fn v2aut005_a_construct_violation_suppresses_drift_but_not_lint() {
    let source = inline_comment_yaml();
    let report = check(&source);

    let advisory = lint_codes(&source);
    let mut expected = vec!["SOURCE_CONSTRUCT_UNSUPPORTED"];
    expected.extend(advisory.iter().copied());
    assert_eq!(codes(&report), expected);
    assert!(
        !codes(&report).contains(&"FORMAT_NOT_CANONICAL"),
        "an unrenderable document is never also called non-canonical",
    );
    assert!(
        !advisory.is_empty(),
        "the fixture must still reach the advisory stage",
    );

    assert!(!report.valid());
    assert_eq!(
        report.digest(),
        Some(admit(&source).digest()),
        "the construct scan runs after validation, so the digest survives it",
    );
}

#[test]
fn v2gsc004_check_reports_canonical_projection_character_and_byte_overflow() {
    let exact_filler = projection_filler_at_canonical_limit();
    let accepted_source = projection_document(exact_filler);
    assert_eq!(
        admit_json(&accepted_source)
            .canonical_json()
            .as_str()
            .chars()
            .count(),
        SOURCE_PROJECTION_MAX_CHARACTERS,
        "the baseline must be the exact canonical boundary"
    );

    let rejected = check_json(&projection_document(exact_filler + 1));
    assert_eq!(codes(&rejected), ["SOURCE_PROJECTION_BUDGET_EXCEEDED"]);
    assert!(rejected.diagnostics()[0].message().contains("characters"));
    assert_eq!(rejected.digest(), None);

    const MULTIBYTE_CHARACTERS: usize = 1_000;
    let exact_byte_filler = exact_filler - 2 * MULTIBYTE_CHARACTERS;
    let accepted_source =
        projection_document_with_multibyte(exact_byte_filler, MULTIBYTE_CHARACTERS);
    assert_eq!(
        admit_json(&accepted_source).canonical_json().as_str().len(),
        podway_core::MAX_PROCEDURE_DOCUMENT_BYTES,
    );
    let rejected = check_json(&projection_document_with_multibyte(
        exact_byte_filler + 1,
        MULTIBYTE_CHARACTERS,
    ));
    assert_eq!(codes(&rejected), ["SOURCE_PROJECTION_BUDGET_EXCEEDED"]);
    assert!(
        rejected.diagnostics()[0]
            .message()
            .contains("encoded bytes")
    );
    assert_eq!(rejected.digest(), None);
}

/// A document with no model stops everything: one finding, no digest, and no advisory noise beside
/// the rejection.
#[test]
fn v2aut005_an_invalid_document_reports_one_validate_finding_and_nothing_else() {
    let source = invalid_yaml();
    let report = check(&source);

    assert_eq!(report.diagnostics().len(), 1);
    assert_eq!(report.total(), 1);
    assert!(!report.truncated());
    assert!(!report.valid());
    assert_eq!(
        report.digest(),
        None,
        "an inadmissible document has no digest to report findings about",
    );

    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.severity(), AuthoringSeverity::Error);
    assert_eq!(diagnostic.source_path(), SOURCE_PATH);
    // The advisory rules never ran, so no warning can appear beside the rejection.
    assert_ne!(diagnostic.severity(), AuthoringSeverity::Warning);

    // A document that does not even parse stops at the same place, for the same reason.
    let unparseable = check("schema: podway.procedure/v2\nid: [\n");
    assert_eq!(unparseable.diagnostics().len(), 1);
    assert_eq!(unparseable.digest(), None);
    assert!(!unparseable.valid());
}

/// A clean canonical document is the gate's zero: no findings at all, and one digest that three
/// independent paths agree on.
#[test]
fn v2aut005_a_clean_canonical_document_reports_nothing_and_one_agreed_digest() {
    let report = check(CLEAN_YAML);

    assert_eq!(codes(&report), Vec::<&str>::new());
    assert!(report.valid());
    assert_eq!(report.total(), 0);
    assert!(!report.truncated());

    let validated = admit(CLEAN_YAML);
    let formatted = format_procedure_v2(FormatRequest {
        source: CLEAN_YAML,
        source_path: SOURCE_PATH,
        format: ProcedureDocumentFormat::Yaml,
    })
    .expect("the clean fixture formats");
    assert!(
        !formatted.changed(),
        "the clean fixture must already be canonical",
    );
    assert_eq!(report.digest(), Some(validated.digest()));
    assert_eq!(report.digest(), Some(formatted.digest()));
}

/// A non-v2 document has no representable v2 findings, so the library reports only the unsupported
/// schema. The public library entry point remains total for every declared schema value.
#[test]
fn v2aut005_an_unsupported_schema_is_reported_with_no_digest() {
    let report = check(UNSUPPORTED_SCHEMA_YAML);

    assert_eq!(codes(&report), vec!["AUTHORING_SCHEMA_INVALID"]);
    assert_eq!(report.diagnostics()[0].field(), "schema");
    assert_eq!(report.digest(), None);
    assert!(!report.valid());
}

#[test]
fn v2ast003_invalid_check_result_fields_have_stable_authoring_diagnostics() {
    let source = r#"schema: podway.procedure/v2
id: p
version: "1"
name: P
purpose: Record a bound result.
node_definitions:
  work:
    type: action
    title: Work
    intent: Record it.
    items:
      - id: verification
        type: check_result
        prompt: Record it.
        required: true
        operation_id: make-test
        operation_digest: sha256:ABC
        accepted_outcomes: [pass]
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
"#;
    let report = check(source);
    assert_eq!(codes(&report), vec!["AUTHORING_SCHEMA_INVALID"]);
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.field(), "operation_digest");
    assert_eq!(diagnostic.location().line(), 17);
    assert_eq!(report.digest(), None);
}

// ---------------------------------------------------------------------------------------------
// 2. The vet seam
// ---------------------------------------------------------------------------------------------

/// The check pipeline exposes all structural and budget findings produced by vet.
#[test]
fn v2grf002_the_check_pipeline_runs_the_complete_vet_rule_set() {
    for source in [CLEAN_YAML, MINIMAL_YAML] {
        let validated = admit(source);
        assert_eq!(
            vet_procedure_v2(&validated, &context(source)),
            Vec::new(),
            "both fixtures are vetted clean",
        );
    }

    let codes_owned: std::collections::BTreeSet<&str> = VET_SUBSET
        .iter()
        .map(|code| AuthoringDiagnosticCode::as_str(*code))
        .collect();
    assert_eq!(codes_owned.len(), 16, "vet owns sixteen blocking codes");

    let source = MINIMAL_YAML.replace(
        "    - id: only\n",
        "    - id: orphan\n      use: work\n      terminal: true\n    - id: only\n",
    );
    assert!(
        codes(&check(&source)).contains(&"UNREACHABLE_GRAPH_NODE"),
        "check must expose the structural vet findings"
    );

    assert!(
        codes(&check(&oversized_static_yaml())).contains(&"NEXT_STATIC_BUDGET_EXCEEDED"),
        "check must carry the resource-budget half of vet as well"
    );
}

// ---------------------------------------------------------------------------------------------
// 3. Merge properties
// ---------------------------------------------------------------------------------------------

/// The report is byte-stable. Nothing in the merge — not the stage interleave, not the sort, not
/// the bound — may leak an allocator's or a hash map's iteration order.
#[test]
fn v2aut005_the_report_is_byte_stable_across_a_hundred_runs() {
    for source in [
        CLEAN_YAML.to_owned(),
        drifted_yaml(),
        inline_comment_yaml(),
        invalid_yaml(),
        finding_heavy_document(),
    ] {
        let first = check(&source);
        let expected = serialized(first.diagnostics());
        for run in 1..100 {
            let repeat = check(&source);
            assert_eq!(repeat, first, "run {run} diverged");
            assert_eq!(serialized(repeat.diagnostics()), expected, "run {run}");
        }
    }
}

/// Check's drift finding *is* `format --check`'s drift finding: the same constructor, reached from
/// the same rendering, serializing to the same bytes.
///
/// This is the only guard against the two commands answering "has this file drifted, and where"
/// differently, which would make `podway procedure check` unusable as the aggregate gate.
#[test]
fn v2aut005_the_drift_finding_is_the_one_format_check_reports() {
    let source = drifted_yaml();
    let formatted = format_procedure_v2(FormatRequest {
        source: &source,
        source_path: SOURCE_PATH,
        format: ProcedureDocumentFormat::Yaml,
    })
    .expect("the drifted fixture still formats");
    let expected = formatted
        .drift_diagnostic(&context(&source))
        .expect("the fixture has drifted");

    let report = check(&source);
    let actual = report
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == AuthoringDiagnosticCode::FormatNotCanonical)
        .expect("check reports the drift");

    assert_eq!(actual, &expected);
    assert_eq!(
        serde_json::to_string(actual).expect("serializes"),
        serde_json::to_string(&expected).expect("serializes"),
    );
}

/// The bound is coherent: `total` counts what the stages found, `diagnostics` is the retained
/// prefix, and `truncated` says which of the two the caller is holding.
#[test]
fn v2aut005_the_report_bound_is_coherent_on_a_finding_heavy_document() {
    let source = finding_heavy_document();
    let report = check(&source);

    assert!(
        report.total() > u32::try_from(MAX_AUTHORING_DIAGNOSTICS).unwrap(),
        "the fixture must overflow the bound to test it: {}",
        report.total(),
    );
    assert!(report.truncated());
    assert_eq!(report.diagnostics().len(), MAX_AUTHORING_DIAGNOSTICS);
    assert_eq!(
        report.total(),
        u32::try_from(lint_codes(&source).len()).unwrap(),
        "every finding on this fixture is advisory, so lint's own count is the total",
    );

    // The retained prefix is the *sorted* prefix, not the first 256 findings the stages produced.
    let retained = codes(&report);
    assert_eq!(retained.len(), MAX_AUTHORING_DIAGNOSTICS);
    assert_eq!(retained, &lint_codes(&source)[..MAX_AUTHORING_DIAGNOSTICS]);

    // Below the bound, the two counts agree and nothing is dropped.
    let small = check(MINIMAL_YAML);
    assert!(!small.truncated());
    assert_eq!(
        small.total(),
        u32::try_from(small.diagnostics().len()).unwrap(),
    );
}

/// Every diagnostic the aggregate gate reports is a catalogued code carrying its catalogued
/// severity, whichever stage produced it.
#[test]
fn v2aut005_every_reported_finding_keeps_its_catalogued_identity() {
    for source in [
        CLEAN_YAML.to_owned(),
        drifted_yaml(),
        inline_comment_yaml(),
        invalid_yaml(),
        finding_heavy_document(),
    ] {
        let report = check(&source);
        for diagnostic in report.diagnostics() {
            assert_eq!(diagnostic.severity(), diagnostic.code().severity());
            assert_eq!(diagnostic.schema(), "podway.procedure/v2");
            assert_eq!(diagnostic.source_path(), SOURCE_PATH);
            assert!(!diagnostic.field().is_empty());
            assert!(!diagnostic.message().is_empty());
        }
        assert_eq!(
            report.valid(),
            !report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity() == AuthoringSeverity::Error),
            "validity is the absence of an error and nothing else",
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------------------------

/// A document whose sixty-three unused decision definitions each draw eleven advisory findings, so
/// the merged report overflows `MAX_AUTHORING_DIAGNOSTICS`.
///
/// Sixty-four is the node-definition ceiling, so this is very nearly the most finding-dense
/// document the schema admits — and it is comfortably past the bound, which is what the truncation
/// path needs in order to be tested at all rather than asserted vacuously.
fn finding_heavy_document() -> String {
    let options: String = (0..6)
        .map(|index| format!("      - id: o{index}\n        label: L{index}\n"))
        .collect();
    let definitions: String = (0..63)
        .map(|index| {
            format!(
                "  d{index:02}:\n    type: decision\n    title: D\n    objective: o\n    \
                 prompt: p\n    options:\n{options}    reason:\n      required: true\n"
            )
        })
        .collect();
    format!(
        "schema: podway.procedure/v2\nid: dense\nversion: \"1\"\nname: Dense\n\
         purpose: Fill the authoring diagnostic bound with advisory findings.\n\
         node_definitions:\n  work:\n    type: action\n    title: Work\n    \
         intent: Perform the only placed action.\n{definitions}\
         graph:\n  entry: only\n  nodes:\n    - id: only\n      use: work\n      terminal: true\n"
    )
}

/// A compact JSON source whose canonical form is larger because omitted text defaults are
/// materialized. Every added ASCII description character grows the projection by exactly one.
fn projection_document(filler_characters: usize) -> String {
    projection_document_with_multibyte(filler_characters, 0)
}

fn projection_document_with_multibyte(
    filler_characters: usize,
    multibyte_characters: usize,
) -> String {
    let mut remaining = filler_characters;
    let mut remaining_multibyte = multibyte_characters;
    let definitions = (0..podway_core::MAX_NODE_DEFINITIONS_PER_PROCEDURE_V2)
        .map(|index| {
            let definitions_left = podway_core::MAX_NODE_DEFINITIONS_PER_PROCEDURE_V2 - index;
            let characters = remaining.saturating_sub(definitions_left - 1).min(1_000);
            remaining -= characters;
            let multibyte = remaining_multibyte.min(characters);
            remaining_multibyte -= multibyte;
            (
                format!("d{index:04}"),
                serde_json::json!({
                    "type": "action",
                    "title": "Work",
                    "description": format!(
                        "{}{}",
                        "한".repeat(multibyte),
                        "a".repeat(characters - multibyte),
                    ),
                    "intent": "Record a value.",
                    "items": [{
                        "id": "value",
                        "type": "text",
                        "prompt": "Value?",
                        "required": false,
                    }],
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    assert_eq!(remaining, 0);
    assert_eq!(remaining_multibyte, 0);
    serde_json::json!({
        "schema": "podway.procedure/v2",
        "id": "projection",
        "version": "1",
        "name": "Projection",
        "purpose": "Fill the canonical projection budget.",
        "node_definitions": definitions,
        "graph": {
            "entry": "n",
            "nodes": [{"id": "n", "use": "d0000", "terminal": true}],
        }
    })
    .to_string()
}

fn projection_filler_at_canonical_limit() -> usize {
    let minimum = podway_core::MAX_NODE_DEFINITIONS_PER_PROCEDURE_V2;
    let base = admit_json(&projection_document(minimum))
        .canonical_json()
        .as_str()
        .chars()
        .count();
    minimum + SOURCE_PROJECTION_MAX_CHARACTERS - base
}
