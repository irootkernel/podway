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

/// A Procedure v1 document, which the aggregate gate has no v2 findings for.
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
// Helpers
// ---------------------------------------------------------------------------------------------

fn check(source: &str) -> ProcedureCheckReport {
    check_procedure_v2(FormatRequest {
        source,
        source_path: SOURCE_PATH,
        format: ProcedureDocumentFormat::Yaml,
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
        Ok(ParsedProcedure::V1(_)) => panic!("expected v2 dispatch, got v1"),
        Err(error) => panic!("the fixture must parse: {error}\n{source}"),
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

/// The complete graph-vet code set delivered by V2GRF-001 and V2GRF-002.
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

/// A model that has no canonical authoring text because it is too large behaves exactly like one
/// that has none because of an unsupported construct: a format-stage finding, no drift comparison,
/// and the advisory stages still run.
///
/// Section 6 of the design tables the construct case and is silent on this one; they are the same
/// event, so they are treated the same way. The generator is ported from
/// `int_v2_procedure_format.rs`: block YAML with materialized defaults is materially wider than
/// compact Canonical JSON v1, so a document can validate and still have no canonical authoring form.
#[test]
fn v2aut005_a_document_over_the_projection_budget_skips_drift_and_still_lints() {
    let source = projection_document(projection_item_count_at_canonical_limit());
    let report = check(&source);

    assert_eq!(
        codes(&report).first().copied(),
        Some("SOURCE_PROJECTION_BUDGET_EXCEEDED"),
    );
    assert!(!codes(&report).contains(&"FORMAT_NOT_CANONICAL"));
    assert_eq!(codes(&report)[1..], lint_codes(&source)[..]);
    assert!(
        codes(&report).contains(&"NO_REACTIVATION_PATH"),
        "the advisory stages must have run on the model: {:?}",
        codes(&report),
    );
    assert!(!report.valid());
    assert_eq!(report.digest(), Some(admit(&source).digest()));
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

/// A Procedure v1 document has no representable v2 findings, so the library answers with the one
/// true thing it can say: the source does not declare the v2 authoring schema.
///
/// The CLI never reaches this arm — it sniffs the schema and refuses a v1 file as a command-level
/// failure — but the library entry point is public and must be total.
#[test]
fn v2aut005_a_v1_document_is_reported_as_a_schema_violation_with_no_digest() {
    let report = check(V1_YAML);

    assert_eq!(codes(&report), vec!["AUTHORING_SCHEMA_INVALID"]);
    assert_eq!(report.diagnostics()[0].field(), "schema");
    assert_eq!(report.digest(), None);
    assert!(!report.valid());
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
    assert_eq!(codes_owned.len(), 11, "vet owns eleven blocking codes");

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

/// A document carrying `count` maximal choice items in one definition, ported from
/// `int_v2_procedure_format.rs`: every filler item is identical in width, so the item count is an
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

/// The largest filler-item count whose *canonical* projection still fits, which is therefore the
/// largest document that validates and still has no canonical authoring form.
fn projection_item_count_at_canonical_limit() -> usize {
    let characters = |count: usize| {
        admit(&projection_document(count))
            .canonical_json()
            .as_str()
            .chars()
            .count()
    };
    let one = characters(1);
    let step = characters(2) - one;
    let count = 1 + (SOURCE_PROJECTION_MAX_CHARACTERS - one) / step;
    assert!(
        characters(count) <= SOURCE_PROJECTION_MAX_CHARACTERS,
        "the at-limit document must still validate",
    );
    count
}
