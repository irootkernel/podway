//! V2AUT-004: the Procedure v2 authoring lint engine (dossier section 11.4).
//!
//! Section 11.4 lists twenty-three advisory findings. This file pins each of them as a *known
//! answer*: one minimal document that fires exactly that rule and nothing else, paired with a
//! near-miss twin — the same document one character, one option, or one graph node under the
//! threshold — that fires nothing at all. A rule that fired on both would be untestable as a
//! threshold, and a rule that dragged a second finding along with it would make the pair prove
//! something weaker than it claims.
//!
//! Around the twenty-three sit the engine's own properties: a clean document is silent, lint never
//! changes the digest it read, the report is byte-stable across a hundred runs, every emitted
//! diagnostic satisfies the authoring diagnostic schema structurally, every emitted code is one of
//! the catalog's twenty-three warnings, and a document that fires many rules at once comes back
//! sorted by `(line, column, code, field)`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2,
    lint_procedure_v2, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AuthoringDiagnostic, AuthoringDiagnosticCode, AuthoringSeverity};
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn admit(source: &str) -> ValidatedProcedureV2 {
    match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml) {
        Ok(ParsedProcedure::V2(parsed)) => {
            validate_procedure_v2(parsed).expect("the lint fixture must validate")
        }
        Err(error) => panic!("the lint fixture must parse: {error}\n{source}"),
    }
}

fn lint(source: &str) -> Vec<AuthoringDiagnostic> {
    let validated = admit(source);
    let context = AuthoringContext::new("workflow.yaml", source, ProcedureDocumentFormat::Yaml);
    lint_procedure_v2(&validated, &context)
}

/// The emitted codes in emission order, which is the report's own sort order.
fn codes(source: &str) -> Vec<&'static str> {
    lint(source)
        .iter()
        .map(|diagnostic| diagnostic.code().as_str())
        .collect()
}

/// Asserts that `source` fires exactly `expected` (in order) and that `twin` fires nothing.
fn assert_rule(source: &str, expected: &[&str], twin: &str) {
    assert_eq!(codes(source), expected, "positive fixture:\n{source}");
    assert_eq!(
        codes(twin),
        Vec::<&str>::new(),
        "negative twin must be silent:\n{twin}"
    );
}

/// Structural equivalent of `assets/schemas/authoring-diagnostic-v1.schema.json`, mirroring
/// `int_v2_procedure_format.rs`: `podway-config` carries no JSON Schema dependency, so the
/// schema's requirements are asserted field by field.
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

    if let Some(related) = object.get("related_graph_node_ids") {
        let related = related.as_array().expect("related ids are an array");
        assert!((1..=64).contains(&related.len()));
        let unique = related
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), related.len(), "related ids must be unique");
    }
}

// ---------------------------------------------------------------------------------------------
// Base documents
// ---------------------------------------------------------------------------------------------

/// A document that fires no lint rule: three definitions, all placed; guidance on every field; one
/// declared rework loop; one manual rework target. Every per-rule fixture below is this document
/// with exactly one thing wrong.
const CLEAN_YAML: &str = r#"schema: podway.procedure/v2
id: lint-clean
version: "1"
name: Lint clean
purpose: Exercise the lint engine with a document that has no findings.
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

/// The goal-tracking counterpart: a session-goal assessment decision whose three outcomes route to
/// three distinct places, an early required item, and a revision-safe rework target. Also silent.
const GOAL_CLEAN_YAML: &str = r#"schema: podway.procedure/v2
id: lint-goal-clean
version: "1"
name: Lint goal clean
purpose: Exercise the goal-tracking lint rules with a document that has no findings.
goal_tracking: true
node_definitions:
  gather:
    type: action
    title: Gather the inputs
    intent: Collect every input the assessment needs.
    items:
      - id: notes
        type: text
        prompt: Record the gathered notes.
        required: true
  assess:
    type: decision
    title: Assess the session goal
    objective: Decide whether the session goal has been met.
    prompt: Has the session goal been met?
    evidence_guidance:
      - Read the gathered notes before assessing.
    options:
      - id: achieved
        label: Goal achieved
        criteria: Every goal criterion is satisfied.
      - id: not-achieved
        label: Goal not achieved
        criteria: At least one goal criterion is unmet.
      - id: superseded
        label: Goal superseded
        criteria: The goal no longer describes the work.
    reason:
      required: true
      prompt: Explain the recorded assessment outcome.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  publish:
    type: action
    title: Publish the result
    intent: Record the published outcome.
graph:
  entry: gather-inputs
  nodes:
    - id: gather-inputs
      use: gather
      next: assess-goal
    - id: assess-goal
      use: assess
      routes:
        achieved:
          to: publish-result
          effect: advance
        not-achieved:
          to: gather-inputs
          effect: rework
        superseded:
          to: record-supersession
          effect: advance
    - id: publish-result
      use: publish
      terminal: true
    - id: record-supersession
      use: publish
      terminal: true
manual_rework:
  allowed_targets:
    - gather-inputs
"#;

/// The smallest legal Procedure v2 document.
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

/// Graph node names that are pairwise far apart under both confusability clauses, so a generated
/// fixture never trips `GRAPH_NODE_ID_CONFUSING` by accident.
const STAGE_NAMES: [&str; 8] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
];

// ---------------------------------------------------------------------------------------------
// Generated documents
// ---------------------------------------------------------------------------------------------

/// A decision with `option_count` options, each routing to its own terminal action, so the option
/// count is the only thing that varies.
fn option_set_document(option_count: usize) -> String {
    let options = STAGE_NAMES[..option_count]
        .iter()
        .map(|name| {
            format!(
                "      - id: {name}\n        label: Outcome {name}\n        criteria: Choose when the {name} outcome applies.\n"
            )
        })
        .collect::<String>();
    let routes = STAGE_NAMES[..option_count]
        .iter()
        .map(|name| {
            format!("        {name}:\n          to: {name}-stage\n          effect: advance\n")
        })
        .collect::<String>();
    let terminals = STAGE_NAMES[..option_count]
        .iter()
        .map(|name| format!("    - id: {name}-stage\n      use: stage\n      terminal: true\n"))
        .collect::<String>();
    format!(
        r#"schema: podway.procedure/v2
id: lint-options
version: "1"
name: Lint options
purpose: Exercise the option-set lint rules with a generated decision.
node_definitions:
  stage:
    type: action
    title: Stage the work
    intent: Record the staged outcome.
  review:
    type: decision
    title: Review the work
    objective: Decide which review outcome the work reached.
    prompt: Which review outcome applies?
    evidence_guidance:
      - Read the staged outcome before deciding.
    options:
{options}    reason:
      required: true
      prompt: Explain the recorded review outcome.
graph:
  entry: collect-notes
  nodes:
    - id: collect-notes
      use: stage
      next: review-work
    - id: review-work
      use: review
      routes:
{routes}{terminals}manual_rework:
  allowed_targets:
    - collect-notes
"#
    )
}

/// A chain of `chain` action placements closed into one rework loop by a decision, producing a
/// strongly connected component of `chain + 1` nodes.
fn rework_region_document(chain: usize) -> String {
    let nodes = STAGE_NAMES[..chain]
        .iter()
        .enumerate()
        .map(|(offset, name)| {
            let next = STAGE_NAMES
                .get(offset.saturating_add(1))
                .filter(|_| offset.saturating_add(1) < chain)
                .map_or_else(|| "review-work".to_owned(), |next| format!("{next}-stage"));
            format!("    - id: {name}-stage\n      use: stage\n      next: {next}\n")
        })
        .collect::<String>();
    format!(
        r#"schema: podway.procedure/v2
id: lint-rework-region
version: "1"
name: Lint rework region
purpose: Exercise the cycle lint rule with a generated rework region.
node_definitions:
  stage:
    type: action
    title: Stage the work
    intent: Record the staged outcome.
  review:
    type: decision
    title: Review the work
    objective: Decide whether the staged work is complete.
    prompt: Is the staged work complete?
    evidence_guidance:
      - Read the staged outcome before deciding.
    options:
      - id: again
        label: Work needs another pass
        criteria: The staged work is not complete yet.
      - id: done
        label: Work is complete
        criteria: The staged work is complete.
    reason:
      required: true
      prompt: Explain why another pass is or is not needed.
graph:
  entry: alpha-stage
  nodes:
{nodes}    - id: review-work
      use: review
      routes:
        again:
          to: alpha-stage
          effect: rework
        done:
          to: publish-result
          effect: advance
    - id: publish-result
      use: stage
      terminal: true
manual_rework:
  allowed_targets:
    - alpha-stage
"#
    )
}

/// A goal-tracking document whose session-goal assessment sits `stages + 1` transitions from the
/// only terminal action.
fn assessment_distance_document(stages: usize) -> String {
    let nodes = STAGE_NAMES[..stages]
        .iter()
        .enumerate()
        .map(|(offset, name)| {
            let next = STAGE_NAMES
                .get(offset.saturating_add(1))
                .filter(|_| offset.saturating_add(1) < stages)
                .map_or_else(
                    || "publish-result".to_owned(),
                    |next| format!("{next}-stage"),
                );
            format!("    - id: {name}-stage\n      use: stage\n      next: {next}\n")
        })
        .collect::<String>();
    format!(
        r#"schema: podway.procedure/v2
id: lint-assessment-distance
version: "1"
name: Lint assessment distance
purpose: Exercise the assessment-placement lint rule with a generated tail.
goal_tracking: true
node_definitions:
  stage:
    type: action
    title: Stage the work
    intent: Record the staged outcome.
  assess:
    type: decision
    title: Assess the session goal
    objective: Decide whether the session goal has been met.
    prompt: Has the session goal been met?
    evidence_guidance:
      - Read the staged outcome before assessing.
    options:
      - id: achieved
        label: Goal achieved
        criteria: Every goal criterion is satisfied.
      - id: not-achieved
        label: Goal not achieved
        criteria: At least one goal criterion is unmet.
      - id: superseded
        label: Goal superseded
        criteria: The goal no longer describes the work.
    reason:
      required: true
      prompt: Explain the recorded assessment outcome.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
graph:
  entry: collect-notes
  nodes:
    - id: collect-notes
      use: stage
      next: assess-goal
    - id: assess-goal
      use: assess
      routes:
        achieved:
          to: alpha-stage
          effect: advance
        not-achieved:
          to: collect-notes
          effect: rework
        superseded:
          to: alpha-stage
          effect: advance
{nodes}    - id: publish-result
      use: stage
      terminal: true
manual_rework:
  allowed_targets:
    - collect-notes
"#
    )
}

/// A nine-node linear procedure whose `manual_rework.allowed_targets` names `target_count` of its
/// eight non-terminal nodes.
fn broad_rework_document(target_count: usize) -> String {
    let nodes = STAGE_NAMES
        .iter()
        .enumerate()
        .map(|(offset, name)| {
            let next = STAGE_NAMES.get(offset.saturating_add(1)).map_or_else(
                || "publish-result".to_owned(),
                |next| format!("{next}-stage"),
            );
            format!("    - id: {name}-stage\n      use: stage\n      next: {next}\n")
        })
        .collect::<String>();
    let targets = STAGE_NAMES[..target_count]
        .iter()
        .map(|name| format!("    - {name}-stage\n"))
        .collect::<String>();
    format!(
        r#"schema: podway.procedure/v2
id: lint-broad-rework
version: "1"
name: Lint broad rework
purpose: Exercise the manual rework breadth lint rule with a generated chain.
node_definitions:
  stage:
    type: action
    title: Stage the work
    intent: Record the staged outcome.
graph:
  entry: alpha-stage
  nodes:
{nodes}    - id: publish-result
      use: stage
      terminal: true
manual_rework:
  allowed_targets:
{targets}"#
    )
}

// ---------------------------------------------------------------------------------------------
// Engine properties
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_a_clean_document_fires_nothing() {
    assert_eq!(codes(CLEAN_YAML), Vec::<&str>::new());
    assert_eq!(codes(GOAL_CLEAN_YAML), Vec::<&str>::new());
}

#[test]
fn v2aut004_the_smallest_legal_document_fires_only_the_reactivation_advisory() {
    // The minimal document declares no manual rework targets, which is a legal authoring choice
    // and exactly the one section 11.4 asks lint to surface for confirmation.
    assert_eq!(codes(MINIMAL_YAML), ["NO_REACTIVATION_PATH"]);
}

#[test]
fn v2aut004_lint_never_changes_the_model_it_read() {
    for source in [CLEAN_YAML, GOAL_CLEAN_YAML, MINIMAL_YAML] {
        let validated = admit(source);
        let before = validated.digest().clone();
        let canonical_before = validated.canonical_json().as_str().to_owned();
        let context = AuthoringContext::new("workflow.yaml", source, ProcedureDocumentFormat::Yaml);

        let _ = lint_procedure_v2(&validated, &context);

        assert_eq!(validated.digest(), &before);
        assert_eq!(validated.canonical_json().as_str(), canonical_before);
    }
}

#[test]
fn v2aut004_the_report_is_byte_identical_across_one_hundred_runs() {
    let source = many_findings_document();
    let validated = admit(&source);
    let context = AuthoringContext::new("workflow.yaml", &source, ProcedureDocumentFormat::Yaml);
    let baseline = serde_json::to_string(&lint_procedure_v2(&validated, &context))
        .expect("diagnostics serialize");
    assert!(!baseline.is_empty());

    for run in 0..100 {
        // A fresh context each run so the lazily built path index cannot make the first run
        // special.
        let validated = admit(&source);
        let context =
            AuthoringContext::new("workflow.yaml", &source, ProcedureDocumentFormat::Yaml);
        let report = serde_json::to_string(&lint_procedure_v2(&validated, &context))
            .expect("diagnostics serialize");
        assert_eq!(report, baseline, "run {run} differed");
    }
}

#[test]
fn v2aut004_every_emitted_diagnostic_is_a_catalog_warning_with_a_valid_shape() {
    let warnings = AuthoringDiagnosticCode::ALL
        .iter()
        .filter(|code| code.severity() == AuthoringSeverity::Warning)
        .map(|code| code.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(warnings.len(), 23);

    let mut seen = BTreeSet::new();
    for source in every_firing_document() {
        for diagnostic in lint(&source) {
            assert_diagnostic_shape(&diagnostic);
            assert_eq!(diagnostic.severity(), AuthoringSeverity::Warning);
            assert!(
                warnings.contains(diagnostic.code().as_str()),
                "{} is not one of the 23 lint warnings",
                diagnostic.code().as_str()
            );
            assert_eq!(diagnostic.source_path(), "workflow.yaml");
            seen.insert(diagnostic.code().as_str());
        }
    }
    assert_eq!(seen, warnings, "every lint rule must have a firing fixture");
}

#[test]
fn v2aut004_a_document_with_many_findings_is_sorted_by_line_column_code_and_field() {
    let source = many_findings_document();
    let report = lint(&source);
    assert!(report.len() >= 6, "the fixture must fire several rules");

    let keys = report
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.location().line(),
                diagnostic.location().column(),
                diagnostic.code().as_str(),
                diagnostic.field().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "the report must be sorted: {keys:#?}");
}

/// One document breaking several unrelated rules at once: a weak purpose, a weak intent, weak
/// decision guidance, an unplaced definition, a decision with nothing to consult, and no
/// reactivation path.
fn many_findings_document() -> String {
    CLEAN_YAML
        .replace(
            "purpose: Exercise the lint engine with a document that has no findings.",
            "purpose: Do it right",
        )
        .replace("intent: Collect every input the review needs.", "intent: Do it right")
        .replace(
            "objective: Decide whether the gathered work is complete.",
            "objective: Do it right",
        )
        .replace("prompt: Is the gathered work complete?", "prompt: Do it right")
        .replace(
            "    evidence_guidance:\n      - Read the gathered notes before deciding.\n",
            "",
        )
        .replace(
            "  publish:\n",
            "  spare:\n    type: action\n    title: Spare definition\n    intent: Record something nobody placed.\n  publish:\n",
        )
        .replace("manual_rework:\n  allowed_targets:\n    - gather-inputs\n", "")
}

/// Every per-rule firing fixture, so the shape and severity assertions cover all twenty-three.
fn every_firing_document() -> Vec<String> {
    vec![
        unused_definition_document(),
        option_set_document(1),
        indistinguishable_labels_document(),
        identical_routes_document(),
        CLEAN_YAML.replace(
            "purpose: Exercise the lint engine with a document that has no findings.",
            "purpose: Do it right",
        ),
        CLEAN_YAML.replace(
            "intent: Collect every input the review needs.",
            "intent: Do it right",
        ),
        CLEAN_YAML.replace(
            "objective: Decide whether the gathered work is complete.",
            "objective: Do it right",
        ),
        CLEAN_YAML.replace(
            "prompt: Is the gathered work complete?",
            "prompt: Do it right",
        ),
        CLEAN_YAML.replace(
            "criteria: Every gathered input is present and correct.",
            "criteria: Do it.",
        ),
        CLEAN_YAML.replace(
            "prompt: Explain why the work is or is not complete.",
            "prompt: Do it.",
        ),
        missing_evidence_guidance_document(),
        unresolvable_optional_evidence_document(true),
        late_clarification_document(false),
        assessment_distance_document(4),
        broad_rework_document(8),
        option_set_document(6),
        rework_region_document(8),
        duplicated_definition_document(true),
        confusable_ids_document("review-works"),
        divergent_rework_document(),
        CLEAN_YAML.replace(
            "manual_rework:\n  allowed_targets:\n    - gather-inputs\n",
            "",
        ),
        unsafe_revision_target_document(),
        multiple_assessment_sources_document(2),
    ]
}

// ---------------------------------------------------------------------------------------------
// Rule 1: UNUSED_NODE_DEFINITION
// ---------------------------------------------------------------------------------------------

fn unused_definition_document() -> String {
    CLEAN_YAML.replace(
        "  publish:\n",
        "  spare:\n    type: action\n    title: Spare definition\n    intent: Record something nobody placed.\n  publish:\n",
    )
}

#[test]
fn v2aut004_an_unplaced_node_definition_is_reported_once() {
    let source = unused_definition_document();
    // The twin places the same definition, so only its placement differs.
    let twin = source
        .replace(
            "    - id: publish-result\n",
            "    - id: spare-step\n      use: spare\n      next: publish-result\n    - id: publish-result\n",
        )
        .replace("      next: review-work\n", "      next: spare-step\n");
    assert_rule(&source, &["UNUSED_NODE_DEFINITION"], &twin);

    let diagnostic = &lint(&source)[0];
    assert_eq!(diagnostic.node_definition_id(), Some("spare"));
    assert_eq!(diagnostic.field(), "node_definitions[spare]");
}

// ---------------------------------------------------------------------------------------------
// Rule 2: SINGLE_OPTION_DECISION
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_a_decision_with_one_option_is_reported() {
    assert_rule(
        &option_set_document(1),
        &["SINGLE_OPTION_DECISION"],
        &option_set_document(2),
    );

    let report = lint(&option_set_document(1));
    assert_eq!(report[0].node_definition_id(), Some("review"));
    assert_eq!(report[0].related_graph_node_ids(), ["review-work"]);
    assert_eq!(report[0].field(), "node_definitions[review].options");
}

// ---------------------------------------------------------------------------------------------
// Rule 3: INDISTINGUISHABLE_OPTION_LABELS
// ---------------------------------------------------------------------------------------------

fn indistinguishable_labels_document() -> String {
    CLEAN_YAML.replace("label: Work is incomplete", "label: WORK   is complete.")
}

#[test]
fn v2aut004_option_labels_that_normalize_alike_are_reported_on_the_later_option() {
    // Case, internal spacing, and trailing punctuation are all normalized away; the twin differs
    // in a word, so it survives normalization.
    assert_rule(
        &indistinguishable_labels_document(),
        &["INDISTINGUISHABLE_OPTION_LABELS"],
        &CLEAN_YAML.replace(
            "label: Work is incomplete",
            "label: WORK   is complete again.",
        ),
    );

    let report = lint(&indistinguishable_labels_document());
    assert_eq!(
        report[0].field(),
        "node_definitions[review].options[incomplete]"
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 4: IDENTICAL_EFFECTIVE_ROUTES
// ---------------------------------------------------------------------------------------------

fn identical_routes_document() -> String {
    CLEAN_YAML.replace(
        "        incomplete:\n          to: gather-inputs\n          effect: rework\n",
        "        incomplete:\n          to: publish-result\n          effect: advance\n",
    )
}

#[test]
fn v2aut004_options_that_lead_to_the_same_place_are_reported_once_per_placement() {
    assert_rule(
        &identical_routes_document(),
        &["IDENTICAL_EFFECTIVE_ROUTES"],
        CLEAN_YAML,
    );

    let report = lint(&identical_routes_document());
    assert_eq!(report[0].graph_node_id(), Some("review-work"));
    assert_eq!(report[0].field(), "graph.nodes[review-work].routes");
}

#[test]
fn v2aut004_a_goal_assessment_placement_is_exempt_from_identical_effective_routes() {
    // Section 7.4 lets all three goal outcomes converge on one terminal action; the options stay
    // distinguishable by the outcome each records, so the convergence is not a defect.
    let converged = GOAL_CLEAN_YAML
        .replace(
            "        superseded:\n          to: record-supersession\n          effect: advance\n",
            "        superseded:\n          to: publish-result\n          effect: advance\n",
        )
        .replace(
            "    - id: record-supersession\n      use: publish\n      terminal: true\n",
            "",
        );
    assert_eq!(codes(&converged), Vec::<&str>::new());

    // The control: the identical convergence on a decision that declares no assessment fires.
    assert_eq!(
        codes(&identical_routes_document()),
        ["IDENTICAL_EFFECTIVE_ROUTES"]
    );
}

// ---------------------------------------------------------------------------------------------
// Rules 5 to 8: the primary guidance fields
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_weak_purpose_intent_objective_and_prompt_are_reported_at_eleven_characters() {
    // Eleven characters is under the primary minimum of twelve; the twin adds one character.
    for (weak, strong, code, field) in [
        (
            "purpose: Exercise the lint engine with a document that has no findings.",
            "purpose: Do it right",
            "WEAK_PURPOSE_GUIDANCE",
            "purpose",
        ),
        (
            "intent: Collect every input the review needs.",
            "intent: Do it right",
            "WEAK_INTENT_GUIDANCE",
            "node_definitions[gather].intent",
        ),
        (
            "objective: Decide whether the gathered work is complete.",
            "objective: Do it right",
            "WEAK_OBJECTIVE_GUIDANCE",
            "node_definitions[review].objective",
        ),
        (
            "prompt: Is the gathered work complete?",
            "prompt: Do it right",
            "WEAK_PROMPT_GUIDANCE",
            "node_definitions[review].prompt",
        ),
    ] {
        let source = CLEAN_YAML.replace(weak, strong);
        let twin = CLEAN_YAML.replace(weak, &format!("{strong}."));
        assert_rule(&source, &[code], &twin);
        assert_eq!(lint(&source)[0].field(), field);
    }
}

#[test]
fn v2aut004_guidance_is_weak_when_it_is_one_word_a_placeholder_or_an_unfilled_span() {
    for weak in [
        "purpose: Unmistakablyverylongsingleword",
        "purpose: \"TODO: state the real purpose here\"",
        "purpose: <describe what this procedure accomplishes>",
    ] {
        let source = CLEAN_YAML.replace(
            "purpose: Exercise the lint engine with a document that has no findings.",
            weak,
        );
        assert_eq!(codes(&source), ["WEAK_PURPOSE_GUIDANCE"], "{weak}");
    }

    // Angle brackets inside ordinary prose are not an unfilled template slot: only a value that is
    // nothing but one bracketed span is. Markup, comparisons, and multi-span values all stay clean.
    for prose in [
        "purpose: Ship the <thing to be named> safely",
        "purpose: Verify the rendered page shows <h1> before <h2> markup",
        "purpose: \"Confirm 0 < latency and p99 > 1s on the gate\"",
        "purpose: Ship it after <the review board and the release manager both sign off> today",
    ] {
        let source = CLEAN_YAML.replace(
            "purpose: Exercise the lint engine with a document that has no findings.",
            prose,
        );
        assert_eq!(codes(&source), Vec::<&str>::new(), "{prose}");
    }
}

// ---------------------------------------------------------------------------------------------
// Rule 9: WEAK_CRITERIA_GUIDANCE
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_weak_or_absent_option_criteria_are_reported_at_seven_characters() {
    // Seven characters is under the secondary minimum of eight; the twin adds one character.
    let source = CLEAN_YAML.replace(
        "criteria: Every gathered input is present and correct.",
        "criteria: Do it n",
    );
    let twin = CLEAN_YAML.replace(
        "criteria: Every gathered input is present and correct.",
        "criteria: Do it no",
    );
    assert_rule(&source, &["WEAK_CRITERIA_GUIDANCE"], &twin);
    assert_eq!(
        lint(&source)[0].field(),
        "node_definitions[review].options[complete].criteria"
    );

    // Absent criteria are the same failure from the reader's side and are reported identically.
    let absent = CLEAN_YAML.replace(
        "        criteria: Every gathered input is present and correct.\n",
        "",
    );
    assert_eq!(codes(&absent), ["WEAK_CRITERIA_GUIDANCE"]);
}

// ---------------------------------------------------------------------------------------------
// Rule 10: WEAK_REASON_GUIDANCE
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_a_required_reason_without_a_useful_prompt_is_reported() {
    let source = CLEAN_YAML.replace(
        "prompt: Explain why the work is or is not complete.",
        "prompt: Do it n",
    );
    let twin = CLEAN_YAML.replace(
        "prompt: Explain why the work is or is not complete.",
        "prompt: Do it no",
    );
    assert_rule(&source, &["WEAK_REASON_GUIDANCE"], &twin);
    assert_eq!(
        lint(&source)[0].field(),
        "node_definitions[review].reason.prompt"
    );

    let absent = CLEAN_YAML.replace(
        "      prompt: Explain why the work is or is not complete.\n",
        "",
    );
    assert_eq!(codes(&absent), ["WEAK_REASON_GUIDANCE"]);
}

// ---------------------------------------------------------------------------------------------
// Rule 11: EVIDENCE_GUIDANCE_MISSING
// ---------------------------------------------------------------------------------------------

fn missing_evidence_guidance_document() -> String {
    CLEAN_YAML.replace(
        "    evidence_guidance:\n      - Read the gathered notes before deciding.\n",
        "",
    )
}

#[test]
fn v2aut004_a_decision_with_neither_evidence_reference_nor_guidance_is_reported() {
    // The twin removes the guidance too, and replaces it with a concrete evidence reference: one
    // of the two is enough.
    let twin = missing_evidence_guidance_document().replace(
        "    - id: review-work\n      use: review\n",
        "    - id: review-work\n      use: review\n      evidence_from:\n        - node: gather-inputs\n",
    );
    assert_rule(
        &missing_evidence_guidance_document(),
        &["EVIDENCE_GUIDANCE_MISSING"],
        &twin,
    );

    let report = lint(&missing_evidence_guidance_document());
    assert_eq!(report[0].graph_node_id(), Some("review-work"));
    assert_eq!(report[0].node_definition_id(), Some("review"));
    assert_eq!(report[0].field(), "graph.nodes[review-work]");
}

// ---------------------------------------------------------------------------------------------
// Rule 12: OPTIONAL_EVIDENCE_UNRESOLVABLE
// ---------------------------------------------------------------------------------------------

/// `review-work` reads back the terminal `publish-result`, which no path reaches before it.
fn unresolvable_optional_evidence_document(optional: bool) -> String {
    CLEAN_YAML.replace(
        "    - id: review-work\n      use: review\n",
        &format!(
            "    - id: review-work\n      use: review\n      evidence_from:\n        - node: publish-result\n          required: {}\n",
            !optional
        ),
    )
}

#[test]
fn v2aut004_an_optional_evidence_reference_no_path_can_resolve_is_reported() {
    // The required twin is silent here on purpose: an unreachable *required* source is vet's
    // EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER, a hard error, not a lint advisory.
    assert_rule(
        &unresolvable_optional_evidence_document(true),
        &["OPTIONAL_EVIDENCE_UNRESOLVABLE"],
        &unresolvable_optional_evidence_document(false),
    );

    let report = lint(&unresolvable_optional_evidence_document(true));
    assert_eq!(report[0].graph_node_id(), Some("review-work"));
    assert_eq!(report[0].related_graph_node_ids(), ["publish-result"]);
    assert_eq!(
        report[0].field(),
        "graph.nodes[review-work].evidence_from[publish-result]"
    );

    // An optional reference that a path *can* resolve stays silent.
    let resolvable = CLEAN_YAML.replace(
        "    - id: review-work\n      use: review\n",
        "    - id: review-work\n      use: review\n      evidence_from:\n        - node: gather-inputs\n          required: false\n",
    );
    assert_eq!(codes(&resolvable), Vec::<&str>::new());
}

// ---------------------------------------------------------------------------------------------
// Rule 13: GOAL_CLARIFICATION_PATH_MISSING
// ---------------------------------------------------------------------------------------------

/// A goal-tracking document whose first three breadth-first levels hold only actions that record
/// nothing; `records_early` gives the level-two action a required item.
fn late_clarification_document(records_early: bool) -> String {
    let items = if records_early {
        "    items:\n      - id: notes\n        type: text\n        prompt: Record the gathered notes.\n        required: true\n"
    } else {
        ""
    };
    format!(
        r#"schema: podway.procedure/v2
id: lint-late-clarification
version: "1"
name: Lint late clarification
purpose: Exercise the goal-clarification lint rule with a long silent prefix.
goal_tracking: true
node_definitions:
  stage:
    type: action
    title: Stage the work
    intent: Record the staged outcome.
  share:
    type: action
    title: Share the work
    intent: Record the shared outcome.
{items}  assess:
    type: decision
    title: Assess the session goal
    objective: Decide whether the session goal has been met.
    prompt: Has the session goal been met?
    evidence_guidance:
      - Read the staged outcome before assessing.
    options:
      - id: achieved
        label: Goal achieved
        criteria: Every goal criterion is satisfied.
      - id: not-achieved
        label: Goal not achieved
        criteria: At least one goal criterion is unmet.
      - id: superseded
        label: Goal superseded
        criteria: The goal no longer describes the work.
    reason:
      required: true
      prompt: Explain the recorded assessment outcome.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
graph:
  entry: collect-notes
  nodes:
    - id: collect-notes
      use: stage
      next: sort-notes
    - id: sort-notes
      use: stage
      next: share-notes
    - id: share-notes
      use: share
      next: assess-goal
    - id: assess-goal
      use: assess
      routes:
        achieved:
          to: publish-result
          effect: advance
        not-achieved:
          to: collect-notes
          effect: rework
        superseded:
          to: publish-result
          effect: advance
    - id: publish-result
      use: stage
      terminal: true
manual_rework:
  allowed_targets:
    - collect-notes
"#
    )
}

#[test]
fn v2aut004_goal_tracking_without_an_early_clarification_path_is_reported() {
    // The prefix is the entry placement plus two breadth-first levels; the twin gives the
    // level-two action a required item, which is exactly what "clarifies the goal" means here.
    assert_rule(
        &late_clarification_document(false),
        &["GOAL_CLARIFICATION_PATH_MISSING"],
        &late_clarification_document(true),
    );
    assert_eq!(
        lint(&late_clarification_document(false))[0].field(),
        "goal_tracking"
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 14: GOAL_ASSESSMENT_TOO_EARLY
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_an_assessment_far_from_every_terminal_action_is_reported() {
    // Four staged nodes put the terminal five transitions away; three put it four away, which is
    // the threshold itself and therefore silent.
    assert_rule(
        &assessment_distance_document(4),
        &["GOAL_ASSESSMENT_TOO_EARLY"],
        &assessment_distance_document(3),
    );

    let report = lint(&assessment_distance_document(4));
    assert_eq!(report[0].graph_node_id(), Some("assess-goal"));
    assert_eq!(report[0].field(), "graph.nodes[assess-goal]");
}

// ---------------------------------------------------------------------------------------------
// Rule 15: MANUAL_REWORK_TARGETS_BROAD
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_manual_rework_targets_covering_half_the_graph_are_reported() {
    assert_rule(
        &broad_rework_document(8),
        &["MANUAL_REWORK_TARGETS_BROAD"],
        &broad_rework_document(7),
    );

    let report = lint(&broad_rework_document(8));
    assert_eq!(report[0].related_graph_node_ids().len(), 8);
    assert_eq!(report[0].field(), "manual_rework.allowed_targets");
}

// ---------------------------------------------------------------------------------------------
// Rule 16: LARGE_OPTION_SET
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_a_decision_with_six_options_is_reported() {
    assert_rule(
        &option_set_document(6),
        &["LARGE_OPTION_SET"],
        &option_set_document(5),
    );
    assert_eq!(
        lint(&option_set_document(6))[0].field(),
        "node_definitions[review].options"
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 17: LARGE_CYCLE
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_a_rework_region_of_nine_nodes_is_reported_once_at_its_earliest_member() {
    // Eight chained actions plus the deciding node form a nine-node component; seven plus the
    // decision form eight, which is the threshold itself.
    assert_rule(
        &rework_region_document(8),
        &["LARGE_CYCLE"],
        &rework_region_document(7),
    );

    let report = lint(&rework_region_document(8));
    assert_eq!(report[0].graph_node_id(), Some("alpha-stage"));
    assert_eq!(report[0].field(), "graph.nodes[alpha-stage]");
    assert_eq!(
        report[0].related_graph_node_ids(),
        [
            "alpha-stage",
            "bravo-stage",
            "charlie-stage",
            "delta-stage",
            "echo-stage",
            "foxtrot-stage",
            "golf-stage",
            "hotel-stage",
            "review-work",
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 18: DUPLICATED_NODE_DEFINITION
// ---------------------------------------------------------------------------------------------

/// A second action definition placed in the chain, identical to `gather` when `identical`.
fn duplicated_definition_document(identical: bool) -> String {
    let intent = if identical {
        "Collect every input the review needs."
    } else {
        "Collect the second round of inputs."
    };
    CLEAN_YAML
        .replace(
            "  review:\n",
            &format!(
                "  regather:\n    type: action\n    title: Gather the inputs\n    intent: {intent}\n    items:\n      - id: notes\n        type: text\n        prompt: Record the gathered notes.\n        required: true\n  review:\n"
            ),
        )
        .replace("      next: review-work\n", "      next: regather-inputs\n")
        .replace(
            "    - id: review-work\n",
            "    - id: regather-inputs\n      use: regather\n      next: review-work\n    - id: review-work\n",
        )
}

#[test]
fn v2aut004_two_definitions_that_differ_only_by_identifier_are_reported_on_the_later_one() {
    assert_rule(
        &duplicated_definition_document(true),
        &["DUPLICATED_NODE_DEFINITION"],
        &duplicated_definition_document(false),
    );

    let report = lint(&duplicated_definition_document(true));
    assert_eq!(report[0].node_definition_id(), Some("regather"));
    assert_eq!(report[0].field(), "node_definitions[regather]");
}

// ---------------------------------------------------------------------------------------------
// Rule 19: GRAPH_NODE_ID_CONFUSING
// ---------------------------------------------------------------------------------------------

fn confusable_ids_document(renamed: &str) -> String {
    CLEAN_YAML.replace("gather-inputs", renamed)
}

#[test]
fn v2aut004_graph_node_identifiers_one_edit_apart_are_reported_once_per_pair() {
    // `review-works` is one insertion from `review-work`; `review-notes` is four edits away and
    // therefore readable.
    assert_rule(
        &confusable_ids_document("review-works"),
        &["GRAPH_NODE_ID_CONFUSING"],
        &confusable_ids_document("review-notes"),
    );

    // The finding anchors on the author-later member of the pair, which is `review-work`: the
    // renamed entry placement is authored first.
    let report = lint(&confusable_ids_document("review-works"));
    assert_eq!(report[0].graph_node_id(), Some("review-work"));
    assert_eq!(report[0].related_graph_node_ids(), ["review-works"]);
    assert_eq!(report[0].field(), "graph.nodes[review-work]");
}

#[test]
fn v2aut004_hyphenation_only_differences_are_confusing() {
    let source = CLEAN_YAML
        .replace(
            "  publish:\n",
            "  reviewer:\n    type: decision\n    title: Review the review\n    objective: Decide whether the review itself holds up.\n    prompt: Does the review hold up?\n    evidence_guidance:\n      - Read the recorded review before deciding.\n    options:\n      - id: sound\n        label: Review is sound\n        criteria: The recorded review is sound.\n      - id: unsound\n        label: Review is unsound\n        criteria: The recorded review is not sound.\n    reason:\n      required: true\n      prompt: Explain the recorded review verdict.\n  publish:\n",
        )
        .replace(
            "        complete:\n          to: publish-result\n          effect: advance\n",
            "        complete:\n          to: reviewwork\n          effect: advance\n",
        )
        .replace(
            "    - id: publish-result\n",
            "    - id: reviewwork\n      use: reviewer\n      routes:\n        sound:\n          to: publish-result\n          effect: advance\n        unsound:\n          to: gather-inputs\n          effect: rework\n    - id: publish-result\n",
        );
    assert_eq!(codes(&source), ["GRAPH_NODE_ID_CONFUSING"]);
    assert_eq!(lint(&source)[0].graph_node_id(), Some("reviewwork"));
}

#[test]
fn v2aut004_a_pairwise_rule_stops_at_its_finding_cap() {
    // Five mutually confusable identifiers make ten pairs; the quadratic rules report at most
    // eight findings each so a pathological document still produces a readable report.
    let nodes = ["review-a", "review-b", "review-c", "review-d", "review-e"];
    let placements = nodes
        .iter()
        .enumerate()
        .map(|(offset, id)| {
            nodes.get(offset.saturating_add(1)).map_or_else(
                || format!("    - id: {id}\n      use: work\n      terminal: true\n"),
                |next| format!("    - id: {id}\n      use: work\n      next: {next}\n"),
            )
        })
        .collect::<String>();
    let source = format!(
        r#"schema: podway.procedure/v2
id: lint-confusable
version: "1"
name: Lint confusable
purpose: Exercise the pairwise finding cap with mutually confusable identifiers.
node_definitions:
  work:
    type: action
    title: Work the stage
    intent: Record the staged outcome.
graph:
  entry: review-a
  nodes:
{placements}manual_rework:
  allowed_targets:
    - review-a
"#
    );

    let report = codes(&source);
    assert_eq!(report.len(), 8);
    assert!(report.iter().all(|code| *code == "GRAPH_NODE_ID_CONFUSING"));
}

// ---------------------------------------------------------------------------------------------
// Rule 20: REWORK_TOPOLOGY_CONFUSING
// ---------------------------------------------------------------------------------------------

/// A decision whose two rework options resume from two different graph nodes.
fn divergent_rework_document() -> String {
    CLEAN_YAML
        .replace("      next: review-work\n", "      next: stage-notes\n")
        .replace(
            "    - id: review-work\n",
            "    - id: stage-notes\n      use: publish\n      next: review-work\n    - id: review-work\n",
        )
        .replace(
            "      - id: incomplete\n        label: Work is incomplete\n        criteria: Some gathered input is missing or wrong.\n",
            "      - id: incomplete\n        label: Work is incomplete\n        criteria: Some gathered input is missing or wrong.\n      - id: restart\n        label: Work must restart\n        criteria: The gathered input is unusable.\n",
        )
        .replace(
            "        incomplete:\n          to: gather-inputs\n          effect: rework\n",
            "        incomplete:\n          to: stage-notes\n          effect: rework\n        restart:\n          to: gather-inputs\n          effect: rework\n",
        )
}

#[test]
fn v2aut004_rework_options_that_resume_from_different_nodes_are_reported() {
    // The twin drops the third option, leaving one rework route: a single rework target is the
    // ordinary shape, and a rework in-degree of two is deliberately not a trigger either.
    let twin = divergent_rework_document()
        .replace(
            "      - id: restart\n        label: Work must restart\n        criteria: The gathered input is unusable.\n",
            "",
        )
        .replace(
            "        restart:\n          to: gather-inputs\n          effect: rework\n",
            "",
        );
    assert_rule(
        &divergent_rework_document(),
        &["REWORK_TOPOLOGY_CONFUSING"],
        &twin,
    );

    let report = lint(&divergent_rework_document());
    assert_eq!(report[0].graph_node_id(), Some("review-work"));
    assert_eq!(report[0].field(), "graph.nodes[review-work].routes");
}

#[test]
fn v2aut004_a_rework_route_back_to_the_deciding_node_is_reported() {
    let source = CLEAN_YAML.replace(
        "        incomplete:\n          to: gather-inputs\n          effect: rework\n",
        "        incomplete:\n          to: review-work\n          effect: rework\n",
    );
    assert_eq!(codes(&source), ["REWORK_TOPOLOGY_CONFUSING"]);
    assert!(
        lint(&source)[0]
            .message()
            .contains("points back at the deciding node itself")
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 21: NO_REACTIVATION_PATH
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_a_procedure_with_no_manual_rework_targets_is_reported() {
    let source = CLEAN_YAML.replace(
        "manual_rework:\n  allowed_targets:\n    - gather-inputs\n",
        "",
    );
    assert_rule(&source, &["NO_REACTIVATION_PATH"], CLEAN_YAML);

    let report = lint(&source);
    // `manual_rework` is absent from the source, so the location degrades to the document start.
    assert_eq!(report[0].location().line(), 1);
    assert_eq!(report[0].location().column(), 1);
    assert!(!report[0].message().contains("goal_tracking"));
}

#[test]
fn v2aut004_the_reactivation_advisory_gains_the_goal_revision_clause_under_goal_tracking() {
    let source = GOAL_CLEAN_YAML.replace(
        "manual_rework:\n  allowed_targets:\n    - gather-inputs\n",
        "",
    );
    assert_eq!(codes(&source), ["NO_REACTIVATION_PATH"]);
    assert!(
        lint(&source)[0]
            .message()
            .contains("the session goal can never be revised after start")
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 22: GOAL_REVISION_TARGET_UNSAFE
// ---------------------------------------------------------------------------------------------

fn unsafe_revision_target_document() -> String {
    GOAL_CLEAN_YAML.replace(
        "manual_rework:\n  allowed_targets:\n    - gather-inputs\n",
        "manual_rework:\n  allowed_targets:\n    - gather-inputs\n    - publish-result\n",
    )
}

#[test]
fn v2aut004_a_manual_rework_target_that_can_reach_a_terminal_unassessed_is_reported() {
    // `publish-result` is a terminal action, so a path from it reaches a terminal without passing
    // any assessment; `gather-inputs` only reaches terminals through `assess-goal` and stays safe.
    assert_rule(
        &unsafe_revision_target_document(),
        &["GOAL_REVISION_TARGET_UNSAFE"],
        GOAL_CLEAN_YAML,
    );

    let report = lint(&unsafe_revision_target_document());
    assert_eq!(report[0].graph_node_id(), Some("publish-result"));
    assert_eq!(
        report[0].field(),
        "manual_rework.allowed_targets[publish-result]"
    );
}

#[test]
fn v2aut004_an_assessment_placement_is_revision_safe_through_its_own_placement() {
    // Section 7.2: "a path from the target includes the target itself".
    let source = GOAL_CLEAN_YAML.replace(
        "manual_rework:\n  allowed_targets:\n    - gather-inputs\n",
        "manual_rework:\n  allowed_targets:\n    - assess-goal\n",
    );
    assert_eq!(codes(&source), Vec::<&str>::new());
}

// ---------------------------------------------------------------------------------------------
// Rule 23: MULTIPLE_GOAL_ASSESSMENT_SOURCES
// ---------------------------------------------------------------------------------------------

/// A terminal action reading back `sources` session-goal assessment placements.
fn multiple_assessment_sources_document(sources: usize) -> String {
    let references = ["first-review", "second-review"][..sources]
        .iter()
        .map(|source| format!("        - node: {source}\n"))
        .collect::<String>();
    format!(
        r#"schema: podway.procedure/v2
id: lint-assessment-sources
version: "1"
name: Lint assessment sources
purpose: Exercise the assessment read-back lint rule with two assessment placements.
goal_tracking: true
node_definitions:
  gather:
    type: action
    title: Gather the inputs
    intent: Collect every input the assessment needs.
    items:
      - id: notes
        type: text
        prompt: Record the gathered notes.
        required: true
  assess:
    type: decision
    title: Assess the session goal
    objective: Decide whether the session goal has been met.
    prompt: Has the session goal been met?
    evidence_guidance:
      - Read the gathered notes before assessing.
    options:
      - id: achieved
        label: Goal achieved
        criteria: Every goal criterion is satisfied.
      - id: not-achieved
        label: Goal not achieved
        criteria: At least one goal criterion is unmet.
      - id: superseded
        label: Goal superseded
        criteria: The goal no longer describes the work.
    reason:
      required: true
      prompt: Explain the recorded assessment outcome.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  publish:
    type: action
    title: Publish the result
    intent: Record the published outcome.
graph:
  entry: collect-notes
  nodes:
    - id: collect-notes
      use: gather
      next: first-review
    - id: first-review
      use: assess
      routes:
        achieved:
          to: second-review
          effect: advance
        not-achieved:
          to: collect-notes
          effect: rework
        superseded:
          to: second-review
          effect: advance
    - id: second-review
      use: assess
      routes:
        achieved:
          to: publish-result
          effect: advance
        not-achieved:
          to: collect-notes
          effect: rework
        superseded:
          to: publish-result
          effect: advance
    - id: publish-result
      use: publish
      evidence_from:
{references}      terminal: true
manual_rework:
  allowed_targets:
    - collect-notes
"#
    )
}

#[test]
fn v2aut004_a_placement_reading_back_two_assessment_sources_is_reported() {
    assert_rule(
        &multiple_assessment_sources_document(2),
        &["MULTIPLE_GOAL_ASSESSMENT_SOURCES"],
        &multiple_assessment_sources_document(1),
    );

    let report = lint(&multiple_assessment_sources_document(2));
    assert_eq!(report[0].graph_node_id(), Some("publish-result"));
    assert_eq!(
        report[0].related_graph_node_ids(),
        ["first-review", "second-review"]
    );
    assert_eq!(
        report[0].field(),
        "graph.nodes[publish-result].evidence_from"
    );
}

// ---------------------------------------------------------------------------------------------
// The shipped equivalence fixture
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut004_the_shipped_equivalence_fixture_fires_only_its_missing_option_criteria() {
    let path = repo_root().join("tests/fixtures/v2/procedures/equivalent-procedure.yaml");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

    // Truthfully pinned rather than tuned. The fixture's three assessment options declare no
    // `criteria`, and absent criteria are weak by rule 9, so exactly three findings are honest.
    // Nothing else fires, and in particular:
    //   - IDENTICAL_EFFECTIVE_ROUTES does not, even though all three routes lead to `finish` with
    //     `advance`, because the `assess` definition declares a session-goal assessment (rule 4's
    //     exemption; this fixture is the canonical shape that exemption exists for);
    //   - GOAL_REVISION_TARGET_UNSAFE does not, because every path from `perform` passes through
    //     the `decide` assessment before reaching the terminal `finish`;
    //   - GOAL_CLARIFICATION_PATH_MISSING does not, because the entry `perform` records a required
    //     `result` item;
    //   - GOAL_ASSESSMENT_TOO_EARLY does not, because `finish` is one transition from `decide`.
    assert_eq!(
        codes(&source),
        [
            "WEAK_CRITERIA_GUIDANCE",
            "WEAK_CRITERIA_GUIDANCE",
            "WEAK_CRITERIA_GUIDANCE"
        ]
    );

    let fields = lint(&source)
        .iter()
        .map(|diagnostic| diagnostic.field().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        fields,
        [
            "node_definitions[assess].options[achieved].criteria",
            "node_definitions[assess].options[not-achieved].criteria",
            "node_definitions[assess].options[superseded].criteria",
        ]
    );
}
