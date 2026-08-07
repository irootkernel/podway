//! V2MOD-006: closed semantic validation of a parsed Procedure v2 model (dossier section 11.2).
//!
//! Validation is a discrete second stage: `parse_procedure_document` stays permissive about every
//! cross-declaration reference, and `validate_procedure_v2` resolves the document's closed
//! reference set against the declarations the same document makes. Path analysis — reachability,
//! terminal paths, cycles, dominance, skip interaction, the read-back and next-static budgets, and
//! goal-assessment option/outcome coverage — is vet's (section 11.3, V2GRF-001/V2GRF-002) and is
//! deliberately absent here.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use podway_config::{
    AuthoringContext, ConfigError, MAX_PROCEDURE_DOCUMENT_BYTES, MAX_PROCEDURE_DOCUMENT_DEPTH,
    MAX_PROCEDURE_DOCUMENT_NODES, ParsedProcedure, ParsedProcedureV2, ProcedureDocumentFormat,
    ValidatedProcedureV2, config_error_diagnostic, decode_procedure_document,
    parse_procedure_document, validate_procedure_v2,
};
use serde::Deserialize;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------------------------
// Stage helpers
// ---------------------------------------------------------------------------------------------

fn parse(text: &str, format: ProcedureDocumentFormat) -> Result<ParsedProcedureV2, ConfigError> {
    match parse_procedure_document(text.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => Ok(parsed),
        Ok(ParsedProcedure::V1(_)) => panic!("expected v2 dispatch, got v1"),
        Err(error) => Err(error),
    }
}

/// Runs both stages in order and returns the first failure, mirroring how an authoring command
/// composes them (V2AUT-005).
fn admit(text: &str, format: ProcedureDocumentFormat) -> Result<ValidatedProcedureV2, ConfigError> {
    validate_procedure_v2(parse(text, format)?)
}

fn accept_yaml(text: &str) -> ValidatedProcedureV2 {
    admit(text, ProcedureDocumentFormat::Yaml).expect("document must parse and validate")
}

fn reject_yaml(text: &str) -> ConfigError {
    admit(text, ProcedureDocumentFormat::Yaml).expect_err("document must be rejected")
}

fn reject_validation(text: &str, format: ProcedureDocumentFormat) -> ConfigError {
    let parsed = parse(text, format).expect("document must parse; validation owns the rejection");
    validate_procedure_v2(parsed).expect_err("validation must reject the document")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(relative: &str) -> Vec<u8> {
    let path = repo_root().join(relative);
    fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", path.display()))
}

// ---------------------------------------------------------------------------------------------
// Document builders
//
// Every authored mapping (`node_definitions`, `routes`, assessment `outcomes`) is written in
// alphabetical key order so the YAML and JSON forms of one case produce structurally equal parsed
// models: `serde_json::Map` is built without `preserve_order`, so `json!` emits its keys sorted,
// while the YAML text keeps author order.
// ---------------------------------------------------------------------------------------------

fn yaml_document(extra: &str, definitions: &str, nodes: &str, trailer: &str) -> String {
    format!(
        "schema: podway.procedure/v2\nid: p\nversion: \"1\"\nname: P\npurpose: P.\n{extra}\
         node_definitions:\n{definitions}graph:\n  entry: start\n  nodes:\n{nodes}{trailer}",
    )
}

fn json_document(
    goal_tracking: bool,
    definitions: Value,
    nodes: Value,
    trailer: Option<Value>,
) -> String {
    let mut document = json!({
        "schema": "podway.procedure/v2",
        "id": "p",
        "version": "1",
        "name": "P",
        "purpose": "P.",
        "node_definitions": definitions,
        "graph": {"entry": "start", "nodes": nodes},
    });
    if goal_tracking {
        document["goal_tracking"] = json!(true);
    }
    if let Some(manual_rework) = trailer {
        document["manual_rework"] = manual_rework;
    }
    document.to_string()
}

const ACTION_ACT: &str = "  act:\n    type: action\n    title: Act\n    intent: Do the work.\n";
const START_TERMINAL: &str = "    - id: start\n      use: act\n      terminal: true\n";

fn action_act_json() -> Value {
    json!({"act": {"type": "action", "title": "Act", "intent": "Do the work."}})
}

// ---------------------------------------------------------------------------------------------
// Stage separation
// ---------------------------------------------------------------------------------------------

#[test]
fn parse_stays_permissive_and_validation_owns_the_closed_reference_rejection() {
    // The exact document of `int_v2_procedure.rs::v2_does_not_perform_semantic_or_canonical_validation`.
    let yaml = concat!(
        "schema: podway.procedure/v2\n",
        "id: semantic-deferred\n",
        "version: \"1\"\n",
        "name: Semantic deferred\n",
        "purpose: Parse without semantic checks.\n",
        "node_definitions:\n",
        "  used:\n",
        "    type: action\n",
        "    title: Used\n",
        "    intent: Used.\n",
        "  unused:\n",
        "    type: action\n",
        "    title: Unused\n",
        "    intent: Never placed.\n",
        "graph:\n",
        "  entry: start\n",
        "  nodes:\n",
        "    - id: start\n",
        "      use: used\n",
        "      next: undefined-target\n",
        "    - id: orphan\n",
        "      use: undefined-definition\n",
        "      next: start\n",
    );

    let parsed = parse(yaml, ProcedureDocumentFormat::Yaml).expect("parsing stays permissive");
    // Fixed order: the entry placement is authored first, so its dangling `next` is the first
    // failure even though a later placement also names an unknown definition.
    assert_eq!(
        validate_procedure_v2(parsed).expect_err("validation rejects the dangling references"),
        ConfigError::UnknownV2Reference {
            field: "graph.nodes.next",
            value: "undefined-target".to_owned(),
        },
    );
}

#[test]
fn an_unreferenced_node_definition_is_not_a_validation_error() {
    // Section 11.4 owns `UNUSED_NODE_DEFINITION` as a lint warning; validation must accept it.
    let yaml = yaml_document(
        "",
        &format!(
            "{ACTION_ACT}  spare:\n    type: action\n    title: Spare\n    intent: Never placed.\n"
        ),
        START_TERMINAL,
        "",
    );
    let validated = accept_yaml(&yaml);
    assert_eq!(validated.parsed().node_definitions().len(), 2);
}

#[test]
fn an_evidence_selector_resolves_against_a_decision_source_definitions_items() {
    // Section 8.1 selectors name items of the source *definition*, and a decision definition may
    // declare items too, so selector resolution must not assume an action source.
    let document = |selected: &str| {
        yaml_document(
            "",
            concat!(
                "  act:\n    type: action\n    title: Act\n    intent: Do the work.\n",
                "  pick:\n",
                "    type: decision\n",
                "    title: Pick\n",
                "    objective: Decide the outcome.\n",
                "    prompt: Which outcome?\n",
                "    items:\n",
                "      - id: rationale\n",
                "        type: text\n",
                "        prompt: Why?\n",
                "        required: false\n",
                "    options:\n",
                "      - id: yes-option\n",
                "        label: Proceed\n",
                "    reason:\n",
                "      required: true\n",
            ),
            &format!(
                "    - id: start\n      use: act\n      evidence_from:\n        - node: decide\n          items:\n            - {selected}\n      terminal: true\n    - id: decide\n      use: pick\n      routes:\n        yes-option:\n          to: start\n          effect: advance\n",
            ),
            "",
        )
    };
    accept_yaml(&document("rationale"));
    assert_eq!(
        reject_yaml(&document("ghost-item")),
        ConfigError::UnknownV2Reference {
            field: "graph.nodes.evidence_from.items",
            value: "ghost-item".to_owned(),
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Success corpus: the dossier section 5 flagship example
// ---------------------------------------------------------------------------------------------

/// The complete dossier section 5 example (lines 305-600), verbatim. It exercises definition
/// reuse, a skippable source behind an optional reference, action read-back, evidence item
/// selectors, a three-branch join, declared rework routes, session-goal assessment, manual rework,
/// and `goal_tracking: true`.
const FLAGSHIP_YAML: &str = r#"schema: podway.procedure/v2
id: software-change
version: "2"
name: Verified software change
purpose: Deliver a reviewed change with fresh verification evidence.

goal_tracking: true

node_definitions:
  implementation:
    type: action
    title: Implement the change
    intent: Produce an implementation that satisfies the task goal.
    instructions:
      - Implement only the agreed scope.
      - Record the resulting source revision.
    items:
      - id: implementation-summary
        type: text
        prompt: Summarize the implemented change.
        required: true
      - id: source-revision
        type: text
        prompt: Record the resulting source revision.
        required: true

  baseline:
    type: action
    title: Capture the verification baseline
    intent: Record the environment used to interpret later test evidence.
    instructions:
      - Record the toolchain and environment used for verification.
    items:
      - id: environment-fingerprint
        type: text
        prompt: Record the verification environment fingerprint.
        required: true

  test-gate:
    type: action
    title: Run the test gate
    intent: Produce fresh verification evidence for the current change.
    instructions:
      - Run the required test command outside Podway.
      - Record the command, its exit status, and the log digest.
    items:
      - id: test-command
        type: text
        prompt: Record the exact test command that was run.
        required: true
      - id: test-exit-status
        type: integer
        prompt: Record the exit status of the test command.
        required: true
      - id: log-digest
        type: text
        prompt: Record the digest of the captured test log.
        required: true

  evaluate-test:
    type: decision
    title: Evaluate the test result
    objective: Only a change supported by acceptable test evidence may proceed.
    prompt: Is the recorded test result acceptable for the task goal?
    evidence_guidance:
      - Read the recorded test command, exit status, and log digest.
      - Compare against the captured verification baseline when one exists.
    options:
      - id: passed
        label: Tests passed
        criteria: The recorded test run completed successfully.
      - id: failed
        label: Tests failed
        criteria: The recorded test run did not complete successfully.
    reason:
      required: true
      prompt: Explain the selection using the referenced evidence.

  review-work:
    type: action
    title: Review the change
    intent: Review the verified candidate against the task goal.
    instructions:
      - Review the implementation and its verification evidence.
      - Record the review summary and any findings.
    items:
      - id: review-summary
        type: text
        prompt: Summarize the review result.
        required: true
      - id: review-findings
        type: list
        prompt: List unresolved review findings, if any.
        required: false

  assess-goal:
    type: decision
    title: Assess the session goal
    objective: Record whether the current goal revision is supported by fresh evidence.
    prompt: What is the outcome of the current session goal?
    evidence_guidance:
      - Read the latest recorded test command, exit status, and log digest.
      - Read the recorded review summary and findings.
    items:
      - id: assessment-note
        type: text
        prompt: Record observations supporting the criterion assessments.
        required: false
    options:
      - id: achieved
        label: Goal achieved
      - id: not-achieved
        label: Goal not achieved
      - id: superseded
        label: Goal superseded
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
    reason:
      required: true
      prompt: Explain the outcome using the criterion results and evidence.

  finalize-outcome:
    type: action
    title: Finalize the assessed outcome
    intent: Record the outcome note and follow-up for the assessed goal.
    items:
      - id: outcome-note
        type: text
        prompt: Record the outcome note and any follow-up commitments.
        required: true

  confirm-outcome:
    type: decision
    title: Confirm the assessed outcome
    objective: Only a task whose recorded outcome is consistent may close.
    prompt: Is the recorded outcome ready for final closeout?
    evidence_guidance:
      - Compare the outcome note with the recorded goal assessment.
      - Confirm follow-up commitments exist when the goal was not achieved.
    options:
      - id: ready
        label: Ready to close
        criteria: The outcome record is consistent with the assessment.
      - id: incomplete
        label: Outcome record incomplete
        criteria: The outcome record is missing or contradicts the assessment.
    reason:
      required: true
      prompt: Explain the selection using the referenced evidence.

  closeout:
    type: action
    title: Record the closeout
    intent: Produce the final task closeout.
    items:
      - id: closeout-note
        type: text
        prompt: Record the final closeout.
        required: true

graph:
  entry: implement

  nodes:
    - id: implement
      use: implementation
      next: capture-baseline

    - id: capture-baseline
      use: baseline
      skip:
        allowed: true
        reason_required: true
      next: test-after-impl

    - id: test-after-impl
      use: test-gate
      next: decide-after-impl-test

    - id: decide-after-impl-test
      use: evaluate-test
      evidence_from:
        - node: test-after-impl
          required: true
        # a skippable source may back only an optional reference
        - node: capture-baseline
          required: false
      routes:
        passed:
          to: review-change
          effect: advance
        failed:
          to: implement
          effect: rework

    - id: review-change
      use: review-work
      # action read-back: the reviewer receives the implementation summary
      # and the recorded test result through `next` (§6.2, §8.4)
      evidence_from:
        - node: implement
          required: true
        - node: test-after-impl
          required: true
      next: test-after-review

    - id: test-after-review
      use: test-gate
      next: decide-after-review-test

    - id: decide-after-review-test
      use: evaluate-test
      evidence_from:
        - node: test-after-review
          required: true
      routes:
        passed:
          to: assess-session-goal
          effect: advance
        failed:
          to: implement
          effect: rework

    - id: assess-session-goal
      use: assess-goal
      evidence_from:
        - node: test-after-review
          required: true
        - node: review-change
          required: true
          # selector keeps the assessment's read-back within budget
          items:
            - review-summary
      routes:
        achieved:
          to: finish-achieved
          effect: advance
        not-achieved:
          to: finish-not-achieved
          effect: advance
        superseded:
          to: finish-superseded
          effect: advance

    - id: finish-achieved
      use: finalize-outcome
      next: confirm-closeout

    - id: finish-not-achieved
      use: finalize-outcome
      next: confirm-closeout

    - id: finish-superseded
      use: finalize-outcome
      next: confirm-closeout

    - id: confirm-closeout
      use: confirm-outcome
      evidence_from:
        # required references at a join name common dominators; the
        # assessment reference reads back the goal assessment record (§8.4)
        - node: assess-session-goal
          required: true
        - node: review-change
          required: true
          # selector: only the summary reads back, not the findings list
          items:
            - review-summary
        # branch-specific outcome notes: exactly one resolves per traversal
        - node: finish-achieved
          required: false
        - node: finish-not-achieved
          required: false
        - node: finish-superseded
          required: false
      routes:
        ready:
          to: record-closeout
          effect: advance
        incomplete:
          to: assess-session-goal
          effect: rework

    - id: record-closeout
      use: closeout
      terminal: true

manual_rework:
  allowed_targets:
    - implement
    - test-after-impl
    - review-change
"#;

#[test]
fn the_flagship_dossier_example_parses_and_validates() {
    let validated = accept_yaml(FLAGSHIP_YAML);
    let parsed = validated.parsed();
    assert_eq!(parsed.id(), "software-change");
    assert!(
        parsed
            .goal_tracking()
            .is_some_and(|opt_in| opt_in.is_enabled())
    );
    // Definition reuse: nine definitions back thirteen placements.
    assert_eq!(parsed.node_definitions().len(), 9);
    assert_eq!(parsed.graph().node_count(), 13);
    assert_eq!(
        parsed
            .graph()
            .manual_rework()
            .expect("manual rework")
            .targets()
            .len(),
        3,
    );
}

#[test]
fn validation_is_deterministic_and_idempotent() {
    let valid = parse(FLAGSHIP_YAML, ProcedureDocumentFormat::Yaml).expect("flagship parses");
    let first = validate_procedure_v2(valid.clone()).expect("first validation succeeds");
    let second = validate_procedure_v2(valid).expect("second validation succeeds");
    assert_eq!(first, second);

    let invalid_yaml = yaml_document(
        "",
        ACTION_ACT,
        "    - id: start\n      use: ghost\n      terminal: true\n",
        "",
    );
    let invalid =
        parse(&invalid_yaml, ProcedureDocumentFormat::Yaml).expect("invalid model parses");
    let first_error = validate_procedure_v2(invalid.clone()).expect_err("first validation fails");
    let second_error = validate_procedure_v2(invalid).expect_err("second validation fails");
    assert_eq!(first_error, second_error);
    assert_eq!(
        first_error,
        ConfigError::UnknownV2Reference {
            field: "graph.nodes.use",
            value: "ghost".to_owned(),
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Closed cross-reference checks 1-8, proven identical for YAML and JSON source
// ---------------------------------------------------------------------------------------------

struct CrossReferenceCase {
    check: &'static str,
    rejected_yaml: String,
    rejected_json: String,
    expected: ConfigError,
    /// The catalog code `config_error_diagnostic` reports for `expected` (V2AUT-008).
    expected_code: &'static str,
    accepted_yaml: String,
    accepted_json: String,
}

fn cross_reference_cases() -> Vec<CrossReferenceCase> {
    let decision_pick_yaml = concat!(
        "  pick:\n",
        "    type: decision\n",
        "    title: Pick\n",
        "    objective: Decide the outcome.\n",
        "    prompt: Which outcome?\n",
        "    options:\n",
        "      - id: yes-option\n",
        "        label: Proceed\n",
        "    reason:\n",
        "      required: true\n",
    );
    let decision_pick_json = json!({
        "pick": {
            "type": "decision",
            "title": "Pick",
            "objective": "Decide the outcome.",
            "prompt": "Which outcome?",
            "options": [{"id": "yes-option", "label": "Proceed"}],
            "reason": {"required": true},
        }
    });
    let two_option_pick_yaml = concat!(
        "  pick:\n",
        "    type: decision\n",
        "    title: Pick\n",
        "    objective: Decide the outcome.\n",
        "    prompt: Which outcome?\n",
        "    options:\n",
        "      - id: no-option\n",
        "        label: Halt\n",
        "      - id: yes-option\n",
        "        label: Proceed\n",
        "    reason:\n",
        "      required: true\n",
    );
    let two_option_pick_json = json!({
        "pick": {
            "type": "decision",
            "title": "Pick",
            "objective": "Decide the outcome.",
            "prompt": "Which outcome?",
            "options": [
                {"id": "no-option", "label": "Halt"},
                {"id": "yes-option", "label": "Proceed"},
            ],
            "reason": {"required": true},
        }
    });
    let assess_yaml = |mapped: &str| {
        format!(
            "  assess:\n    type: decision\n    title: Assess\n    objective: Assess the goal.\n    prompt: What is the outcome?\n    options:\n      - id: achieved\n        label: Achieved\n      - id: not-achieved\n        label: Not achieved\n      - id: superseded\n        label: Superseded\n    reason:\n      required: true\n    assessment:\n      target: session_goal\n      outcomes:\n        {mapped}: achieved\n        not-achieved: not_achieved\n        superseded: superseded\n",
        )
    };
    let assess_json = |mapped: &str| {
        json!({
            "assess": {
                "type": "decision",
                "title": "Assess",
                "objective": "Assess the goal.",
                "prompt": "What is the outcome?",
                "options": [
                    {"id": "achieved", "label": "Achieved"},
                    {"id": "not-achieved", "label": "Not achieved"},
                    {"id": "superseded", "label": "Superseded"},
                ],
                "reason": {"required": true},
                "assessment": {
                    "target": "session_goal",
                    "outcomes": {mapped: "achieved", "not-achieved": "not_achieved", "superseded": "superseded"},
                },
            }
        })
    };
    let assess_nodes_yaml = concat!(
        "    - id: start\n",
        "      use: assess\n",
        "      routes:\n",
        "        achieved:\n",
        "          to: start\n",
        "          effect: advance\n",
        "        not-achieved:\n",
        "          to: start\n",
        "          effect: advance\n",
        "        superseded:\n",
        "          to: start\n",
        "          effect: advance\n",
    );
    let assess_nodes_json = json!([{
        "id": "start",
        "use": "assess",
        "routes": {
            "achieved": {"to": "start", "effect": "advance"},
            "not-achieved": {"to": "start", "effect": "advance"},
            "superseded": {"to": "start", "effect": "advance"},
        },
    }]);

    vec![
        // Check 1: a placement's `use` names an existing node definition
        // (future stable code GRAPH_DEFINITION_UNKNOWN).
        CrossReferenceCase {
            check: "1 placement use resolves to a node definition",
            rejected_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: ghost\n      terminal: true\n",
                "",
            ),
            rejected_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "ghost", "terminal": true}]),
                None,
            ),
            expected: ConfigError::UnknownV2Reference {
                field: "graph.nodes.use",
                value: "ghost".to_owned(),
            },
            expected_code: "GRAPH_DEFINITION_UNKNOWN",
            accepted_yaml: yaml_document("", ACTION_ACT, START_TERMINAL, ""),
            accepted_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "terminal": true}]),
                None,
            ),
        },
        // Check 2: kind agreement, decision placement half.
        CrossReferenceCase {
            check: "2 a decision placement uses a decision definition",
            rejected_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: act\n      routes:\n        yes-option:\n          to: start\n          effect: advance\n",
                "",
            ),
            rejected_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "routes": {"yes-option": {"to": "start", "effect": "advance"}}}]),
                None,
            ),
            expected: ConfigError::V2ShapeMismatch {
                field: "graph.nodes.use",
                reason: "a decision placement must use a decision definition",
            },
            expected_code: "AUTHORING_SCHEMA_INVALID",
            accepted_yaml: yaml_document(
                "",
                decision_pick_yaml,
                "    - id: start\n      use: pick\n      routes:\n        yes-option:\n          to: start\n          effect: advance\n",
                "",
            ),
            accepted_json: json_document(
                false,
                decision_pick_json.clone(),
                json!([{"id": "start", "use": "pick", "routes": {"yes-option": {"to": "start", "effect": "advance"}}}]),
                None,
            ),
        },
        // Check 2: kind agreement, action placement half.
        CrossReferenceCase {
            check: "2 an action placement uses an action definition",
            rejected_yaml: yaml_document(
                "",
                decision_pick_yaml,
                "    - id: start\n      use: pick\n      terminal: true\n",
                "",
            ),
            rejected_json: json_document(
                false,
                decision_pick_json.clone(),
                json!([{"id": "start", "use": "pick", "terminal": true}]),
                None,
            ),
            expected: ConfigError::V2ShapeMismatch {
                field: "graph.nodes.use",
                reason: "an action placement must use an action definition",
            },
            expected_code: "AUTHORING_SCHEMA_INVALID",
            accepted_yaml: yaml_document("", ACTION_ACT, START_TERMINAL, ""),
            accepted_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "terminal": true}]),
                None,
            ),
        },
        // Check 3: an action `next` target names an existing placement (ROUTE_TARGET_NOT_FOUND).
        CrossReferenceCase {
            check: "3 an action next target resolves to a placement",
            rejected_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: act\n      next: nowhere\n",
                "",
            ),
            rejected_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "next": "nowhere"}]),
                None,
            ),
            expected: ConfigError::UnknownV2Reference {
                field: "graph.nodes.next",
                value: "nowhere".to_owned(),
            },
            expected_code: "ROUTE_TARGET_NOT_FOUND",
            accepted_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: act\n      next: finish\n    - id: finish\n      use: act\n      terminal: true\n",
                "",
            ),
            accepted_json: json_document(
                false,
                action_act_json(),
                json!([
                    {"id": "start", "use": "act", "next": "finish"},
                    {"id": "finish", "use": "act", "terminal": true},
                ]),
                None,
            ),
        },
        // Check 3: a decision route target names an existing placement (ROUTE_TARGET_NOT_FOUND).
        CrossReferenceCase {
            check: "3 a decision route target resolves to a placement",
            rejected_yaml: yaml_document(
                "",
                decision_pick_yaml,
                "    - id: start\n      use: pick\n      routes:\n        yes-option:\n          to: nowhere\n          effect: advance\n",
                "",
            ),
            rejected_json: json_document(
                false,
                decision_pick_json.clone(),
                json!([{"id": "start", "use": "pick", "routes": {"yes-option": {"to": "nowhere", "effect": "advance"}}}]),
                None,
            ),
            expected: ConfigError::UnknownV2Reference {
                field: "graph.nodes.routes.to",
                value: "nowhere".to_owned(),
            },
            expected_code: "ROUTE_TARGET_NOT_FOUND",
            accepted_yaml: yaml_document(
                "",
                decision_pick_yaml,
                "    - id: start\n      use: pick\n      routes:\n        yes-option:\n          to: start\n          effect: advance\n",
                "",
            ),
            accepted_json: json_document(
                false,
                decision_pick_json.clone(),
                json!([{"id": "start", "use": "pick", "routes": {"yes-option": {"to": "start", "effect": "advance"}}}]),
                None,
            ),
        },
        // Check 4: every declared option is routed (DECISION_OPTION_ROUTE_MISSING).
        CrossReferenceCase {
            check: "4 every declared decision option is routed",
            rejected_yaml: yaml_document(
                "",
                two_option_pick_yaml,
                "    - id: start\n      use: pick\n      routes:\n        yes-option:\n          to: start\n          effect: advance\n",
                "",
            ),
            rejected_json: json_document(
                false,
                two_option_pick_json.clone(),
                json!([{"id": "start", "use": "pick", "routes": {"yes-option": {"to": "start", "effect": "advance"}}}]),
                None,
            ),
            expected: ConfigError::V2ShapeMismatch {
                field: "graph.nodes.routes",
                reason: "every declared decision option must have exactly one route",
            },
            expected_code: "DECISION_OPTION_ROUTE_MISSING",
            accepted_yaml: yaml_document(
                "",
                two_option_pick_yaml,
                "    - id: start\n      use: pick\n      routes:\n        no-option:\n          to: start\n          effect: advance\n        yes-option:\n          to: start\n          effect: advance\n",
                "",
            ),
            accepted_json: json_document(
                false,
                two_option_pick_json.clone(),
                json!([{
                    "id": "start",
                    "use": "pick",
                    "routes": {
                        "no-option": {"to": "start", "effect": "advance"},
                        "yes-option": {"to": "start", "effect": "advance"},
                    },
                }]),
                None,
            ),
        },
        // Check 4: no route names an undeclared option (DECISION_ROUTE_OPTION_UNDEFINED).
        CrossReferenceCase {
            check: "4 no route names an undeclared decision option",
            rejected_yaml: yaml_document(
                "",
                decision_pick_yaml,
                "    - id: start\n      use: pick\n      routes:\n        maybe-option:\n          to: start\n          effect: advance\n",
                "",
            ),
            rejected_json: json_document(
                false,
                decision_pick_json.clone(),
                json!([{"id": "start", "use": "pick", "routes": {"maybe-option": {"to": "start", "effect": "advance"}}}]),
                None,
            ),
            expected: ConfigError::V2ShapeMismatch {
                field: "graph.nodes.routes",
                reason: "a route names an option the decision definition does not declare",
            },
            expected_code: "DECISION_ROUTE_OPTION_UNDEFINED",
            accepted_yaml: yaml_document(
                "",
                decision_pick_yaml,
                "    - id: start\n      use: pick\n      routes:\n        yes-option:\n          to: start\n          effect: advance\n",
                "",
            ),
            accepted_json: json_document(
                false,
                decision_pick_json,
                json!([{"id": "start", "use": "pick", "routes": {"yes-option": {"to": "start", "effect": "advance"}}}]),
                None,
            ),
        },
        // Check 5: every evidence source names an existing placement (EVIDENCE_SOURCE_UNKNOWN).
        CrossReferenceCase {
            check: "5 an evidence source resolves to a placement",
            rejected_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: act\n      evidence_from:\n        - node: ghost\n      terminal: true\n",
                "",
            ),
            rejected_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "evidence_from": [{"node": "ghost"}], "terminal": true}]),
                None,
            ),
            expected: ConfigError::UnknownV2Reference {
                field: "graph.nodes.evidence_from.node",
                value: "ghost".to_owned(),
            },
            expected_code: "EVIDENCE_SOURCE_UNKNOWN",
            accepted_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: act\n      evidence_from:\n        - node: source\n      terminal: true\n    - id: source\n      use: act\n      terminal: true\n",
                "",
            ),
            accepted_json: json_document(
                false,
                action_act_json(),
                json!([
                    {"id": "start", "use": "act", "evidence_from": [{"node": "source"}], "terminal": true},
                    {"id": "source", "use": "act", "terminal": true},
                ]),
                None,
            ),
        },
        // Check 5: an evidence source is never the consuming placement
        // (EVIDENCE_SOURCE_SELF_REFERENCE).
        CrossReferenceCase {
            check: "5 an evidence source is not its consuming placement",
            rejected_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: act\n      evidence_from:\n        - node: start\n      terminal: true\n",
                "",
            ),
            rejected_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "evidence_from": [{"node": "start"}], "terminal": true}]),
                None,
            ),
            expected: ConfigError::V2ShapeMismatch {
                field: "graph.nodes.evidence_from.node",
                reason: "an evidence reference must not name its consuming placement",
            },
            expected_code: "EVIDENCE_SOURCE_SELF_REFERENCE",
            accepted_yaml: yaml_document(
                "",
                ACTION_ACT,
                "    - id: start\n      use: act\n      evidence_from:\n        - node: source\n      terminal: true\n    - id: source\n      use: act\n      terminal: true\n",
                "",
            ),
            accepted_json: json_document(
                false,
                action_act_json(),
                json!([
                    {"id": "start", "use": "act", "evidence_from": [{"node": "source"}], "terminal": true},
                    {"id": "source", "use": "act", "terminal": true},
                ]),
                None,
            ),
        },
        // Check 6: every selected item is declared by the source definition
        // (EVIDENCE_SELECTOR_UNKNOWN_ITEM).
        CrossReferenceCase {
            check: "6 an evidence selector resolves to a source definition item",
            rejected_yaml: yaml_document(
                "",
                "  act:\n    type: action\n    title: Act\n    intent: Do the work.\n    items:\n      - id: note\n        type: text\n        prompt: Note?\n        required: true\n",
                "    - id: start\n      use: act\n      evidence_from:\n        - node: source\n          items:\n            - ghost-item\n      terminal: true\n    - id: source\n      use: act\n      terminal: true\n",
                "",
            ),
            rejected_json: json_document(
                false,
                json!({"act": {
                    "type": "action",
                    "title": "Act",
                    "intent": "Do the work.",
                    "items": [{"id": "note", "type": "text", "prompt": "Note?", "required": true}],
                }}),
                json!([
                    {"id": "start", "use": "act", "evidence_from": [{"node": "source", "items": ["ghost-item"]}], "terminal": true},
                    {"id": "source", "use": "act", "terminal": true},
                ]),
                None,
            ),
            expected: ConfigError::UnknownV2Reference {
                field: "graph.nodes.evidence_from.items",
                value: "ghost-item".to_owned(),
            },
            expected_code: "EVIDENCE_SELECTOR_UNKNOWN_ITEM",
            accepted_yaml: yaml_document(
                "",
                "  act:\n    type: action\n    title: Act\n    intent: Do the work.\n    items:\n      - id: note\n        type: text\n        prompt: Note?\n        required: true\n",
                "    - id: start\n      use: act\n      evidence_from:\n        - node: source\n          items:\n            - note\n      terminal: true\n    - id: source\n      use: act\n      terminal: true\n",
                "",
            ),
            accepted_json: json_document(
                false,
                json!({"act": {
                    "type": "action",
                    "title": "Act",
                    "intent": "Do the work.",
                    "items": [{"id": "note", "type": "text", "prompt": "Note?", "required": true}],
                }}),
                json!([
                    {"id": "start", "use": "act", "evidence_from": [{"node": "source", "items": ["note"]}], "terminal": true},
                    {"id": "source", "use": "act", "terminal": true},
                ]),
                None,
            ),
        },
        // Check 7: an assessment outcome only names an option its own definition declares.
        CrossReferenceCase {
            check: "7 an assessment outcome names a declared option",
            rejected_yaml: yaml_document(
                "goal_tracking: true\n",
                &assess_yaml("ghost-option"),
                assess_nodes_yaml,
                "",
            ),
            rejected_json: json_document(
                true,
                assess_json("ghost-option"),
                assess_nodes_json.clone(),
                None,
            ),
            expected: ConfigError::V2ShapeMismatch {
                field: "node_definitions.assessment.outcomes",
                reason: "an assessment outcome names an option the decision definition does not declare",
            },
            expected_code: "AUTHORING_SCHEMA_INVALID",
            accepted_yaml: yaml_document(
                "goal_tracking: true\n",
                &assess_yaml("achieved"),
                assess_nodes_yaml,
                "",
            ),
            accepted_json: json_document(
                true,
                assess_json("achieved"),
                assess_nodes_json.clone(),
                None,
            ),
        },
        // Check 7: a session-goal assessment requires `goal_tracking: true`
        // (GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING).
        CrossReferenceCase {
            check: "7 a session-goal assessment requires the goal tracking opt-in",
            rejected_yaml: yaml_document("", &assess_yaml("achieved"), assess_nodes_yaml, ""),
            rejected_json: json_document(
                false,
                assess_json("achieved"),
                assess_nodes_json.clone(),
                None,
            ),
            expected: ConfigError::V2ShapeMismatch {
                field: "node_definitions.assessment",
                reason: "a session-goal assessment requires the procedure to declare goal_tracking: true",
            },
            expected_code: "GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING",
            accepted_yaml: yaml_document(
                "goal_tracking: true\n",
                &assess_yaml("achieved"),
                assess_nodes_yaml,
                "",
            ),
            accepted_json: json_document(true, assess_json("achieved"), assess_nodes_json, None),
        },
        // Check 8: every manual rework target names an existing placement
        // (MANUAL_REWORK_TARGET_UNKNOWN).
        CrossReferenceCase {
            check: "8 a manual rework target resolves to a placement",
            rejected_yaml: yaml_document(
                "",
                ACTION_ACT,
                START_TERMINAL,
                "manual_rework:\n  allowed_targets: [ghost]\n",
            ),
            rejected_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "terminal": true}]),
                Some(json!({"allowed_targets": ["ghost"]})),
            ),
            expected: ConfigError::UnknownV2Reference {
                field: "manual_rework.allowed_targets",
                value: "ghost".to_owned(),
            },
            expected_code: "MANUAL_REWORK_TARGET_UNKNOWN",
            accepted_yaml: yaml_document(
                "",
                ACTION_ACT,
                START_TERMINAL,
                "manual_rework:\n  allowed_targets: [start]\n",
            ),
            accepted_json: json_document(
                false,
                action_act_json(),
                json!([{"id": "start", "use": "act", "terminal": true}]),
                Some(json!({"allowed_targets": ["start"]})),
            ),
        },
    ]
}

#[test]
fn closed_cross_reference_checks_reject_yaml_and_json_with_identical_diagnostics() {
    let cases = cross_reference_cases();
    assert_eq!(cases.len(), 13, "checks 1-8 are covered by thirteen cases");

    for case in &cases {
        // The two encodings must first agree on the parsed model: validation reads only that
        // model, so identical diagnostics follow structurally rather than by coincidence.
        let from_yaml = parse(&case.rejected_yaml, ProcedureDocumentFormat::Yaml)
            .unwrap_or_else(|error| panic!("check {}: yaml must parse: {error:?}", case.check));
        let from_json = parse(&case.rejected_json, ProcedureDocumentFormat::Json)
            .unwrap_or_else(|error| panic!("check {}: json must parse: {error:?}", case.check));
        assert_eq!(from_yaml, from_json, "check {}", case.check);

        let yaml_error = reject_validation(&case.rejected_yaml, ProcedureDocumentFormat::Yaml);
        let json_error = reject_validation(&case.rejected_json, ProcedureDocumentFormat::Json);
        assert_eq!(yaml_error, json_error, "check {}", case.check);
        assert_eq!(yaml_error, case.expected, "check {}", case.check);

        // The production mapping turns each rejection into its catalog code. Ten of the thirteen
        // checks have an exact code; the remaining three are closed-shape violations the catalog
        // does not name, and stay `AUTHORING_SCHEMA_INVALID` (see the mapping's own comments).
        // Both encodings must land on the same one, since both raise the same `ConfigError`.
        let yaml_context = AuthoringContext::new(
            "case.yaml",
            &case.rejected_yaml,
            ProcedureDocumentFormat::Yaml,
        );
        let diagnostic = config_error_diagnostic(&yaml_error, &yaml_context);
        assert_eq!(
            diagnostic.code().as_str(),
            case.expected_code,
            "check {}",
            case.check,
        );
        assert_eq!(
            diagnostic_code(
                &json_error,
                &case.rejected_json,
                ProcedureDocumentFormat::Json
            ),
            case.expected_code,
            "check {}: both encodings classify identically",
            case.check,
        );
        // Refinement changes the code, never the path the author has to edit.
        assert_eq!(
            diagnostic.field(),
            rejected_field(&yaml_error),
            "check {}",
            case.check,
        );
    }
}

#[test]
fn the_accepted_counterpart_of_every_cross_reference_check_validates_in_both_encodings() {
    for case in &cross_reference_cases() {
        let from_yaml =
            admit(&case.accepted_yaml, ProcedureDocumentFormat::Yaml).unwrap_or_else(|error| {
                panic!("check {}: yaml twin must validate: {error:?}", case.check)
            });
        let from_json =
            admit(&case.accepted_json, ProcedureDocumentFormat::Json).unwrap_or_else(|error| {
                panic!("check {}: json twin must validate: {error:?}", case.check)
            });
        assert_eq!(from_yaml, from_json, "check {}", case.check);
    }
}

// ---------------------------------------------------------------------------------------------
// Fixture-driven negatives: every malformed input is rejected by one of the two stages
// ---------------------------------------------------------------------------------------------

const AUTHORING_SCHEMA_INVALID: &str = "AUTHORING_SCHEMA_INVALID";

/// The catalog code the production mapping reports for a rejection of `source`.
///
/// V2AUT-008 retired this file's prototype classifier: the mapping is
/// `podway_config::config_error_diagnostic`, and these tests assert against it rather than against
/// a second copy of its rules that could agree with the catalog while disagreeing with the shipped
/// command. The full truth table, its raise-site evidence, and the location and message contracts
/// live in `int_v2_authoring_diagnostics.rs`; what this file adds is that every rejection *it*
/// pins carries the code the reader of that rejection would see.
fn diagnostic_code(error: &ConfigError, source: &str, format: ProcedureDocumentFormat) -> String {
    let context = AuthoringContext::new("fixture.yaml", source, format);
    config_error_diagnostic(error, &context)
        .code()
        .as_str()
        .to_owned()
}

/// The authored field a rejection names, or `"$"` when it names none.
///
/// The production diagnostic must report exactly this: classification refines the *code*, never the
/// path the author has to edit.
fn rejected_field(error: &ConfigError) -> &'static str {
    match error {
        ConfigError::UnknownV2Reference { field, .. }
        | ConfigError::V2ShapeMismatch { field, .. } => field,
        other => panic!("case rejections name an authored field: {other:?}"),
    }
}

#[derive(Deserialize)]
struct MalformedCase {
    id: String,
    format: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    bytes_hex: Option<String>,
    expected_code: String,
}

#[derive(Deserialize)]
struct BoundaryCaseFixture {
    dimension: String,
    at_limit: usize,
    one_over: usize,
}

#[derive(Deserialize)]
struct MalformedFixture {
    cases: Vec<MalformedCase>,
    boundary_cases: Vec<BoundaryCaseFixture>,
}

fn malformed_fixture() -> MalformedFixture {
    serde_json::from_slice(&fixture(
        "tests/fixtures/v2/procedures/malformed-inputs.json",
    ))
    .expect("malformed-inputs.json fixture parses as JSON")
}

fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0, "hex fixture value must have even length");
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("valid hex byte"))
        .collect()
}

/// Runs one raw source through both stages and returns the rejection, asserting that a document
/// that survives parsing does not then survive validation.
fn admit_bytes(bytes: &[u8], format: ProcedureDocumentFormat, case: &str) -> ConfigError {
    match parse_procedure_document(bytes, format) {
        Err(error) => error,
        Ok(ParsedProcedure::V1(_)) => panic!("case {case}: expected v2 dispatch, got v1"),
        Ok(ParsedProcedure::V2(parsed)) => validate_procedure_v2(parsed)
            .err()
            .unwrap_or_else(|| panic!("case {case}: expected rejection, parsed and validated")),
    }
}

#[test]
fn every_malformed_input_fixture_case_is_rejected_and_classified() {
    let fixture = malformed_fixture();
    assert_eq!(
        fixture.cases.len(),
        14,
        "the fixture declares fourteen cases"
    );

    for case in &fixture.cases {
        let bytes = match (&case.source, &case.bytes_hex) {
            (Some(source), None) => source.clone().into_bytes(),
            (None, Some(hex)) => decode_hex(hex),
            _ => panic!(
                "case {}: expected exactly one of `source`/`bytes_hex`",
                case.id
            ),
        };
        // A raw-byte recipe is format-agnostic: it must be rejected identically through both
        // dispatches. Every other case declares the encoding it is authored in.
        let formats: &[ProcedureDocumentFormat] = match case.format.as_str() {
            "yaml" => &[ProcedureDocumentFormat::Yaml],
            "json" => &[ProcedureDocumentFormat::Json],
            "binary-recipe" => &[ProcedureDocumentFormat::Yaml, ProcedureDocumentFormat::Json],
            other => panic!("case {}: unsupported fixture format `{other}`", case.id),
        };
        for format in formats {
            let error = admit_bytes(&bytes, *format, &case.id);
            let source = String::from_utf8_lossy(&bytes).into_owned();
            assert_eq!(
                diagnostic_code(&error, &source, *format),
                case.expected_code,
                "case {} ({format:?}): {error:?}",
                case.id,
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Bound dimensions: at-limit is accepted, one-over is rejected deterministically
// ---------------------------------------------------------------------------------------------

/// Dimensions whose at-limit and one-over evidence already lives in another test in this crate.
/// Re-asserting them here would duplicate, not strengthen, the gate.
const DIMENSIONS_COVERED_ELSEWHERE: &[(&str, &str)] = &[
    (
        "procedure version characters",
        "int_v2_procedure.rs::v2_procedure_version_accepts_at_limit_and_rejects_one_over",
    ),
    (
        "procedure name characters",
        "int_v2_procedure.rs::v2_procedure_name_accepts_at_limit_and_rejects_one_over",
    ),
    (
        "procedure purpose characters",
        "int_v2_procedure.rs::v2_procedure_purpose_accepts_at_limit_and_rejects_one_over",
    ),
    (
        "procedure description characters",
        "int_v2_procedure.rs::v2_procedure_description_accepts_at_limit_and_rejects_one_over",
    ),
    (
        "node definitions",
        "int_v2_procedure.rs::v2_node_definitions_count_accepts_64_and_rejects_65",
    ),
];

struct BoundCase {
    dimension: &'static str,
    at_limit: usize,
    one_over: usize,
    rejection: ConfigError,
    at_limit_document: String,
    one_over_document: String,
}

fn bound_case(
    dimension: &'static str,
    at_limit: usize,
    one_over: usize,
    rejection: ConfigError,
    build: impl Fn(usize) -> String,
) -> BoundCase {
    BoundCase {
        dimension,
        at_limit,
        one_over,
        rejection,
        at_limit_document: build(at_limit),
        one_over_document: build(one_over),
    }
}

/// Every v2 constructor surfaces a bound violation through `DomainError::InvalidState`, whose
/// static reason the parser preserves verbatim under a single `procedure v2` field.
const fn bound_violation(reason: &'static str) -> ConfigError {
    ConfigError::InvalidValue {
        field: "procedure v2",
        reason,
    }
}

/// A single action definition `act` whose body lines are supplied by the caller, placed once as
/// the terminal entry node.
fn action_document(body: &str) -> String {
    yaml_document(
        "",
        &format!("  act:\n    type: action\n    title: Act\n    intent: Do the work.\n{body}"),
        START_TERMINAL,
        "",
    )
}

/// An action definition carrying exactly one item, whose type-specific lines are supplied.
fn item_document(item: &str) -> String {
    action_document(&format!("    items:\n      - id: note\n{item}"))
}

/// A decision definition `pick` with the supplied body lines and options, placed once with one
/// route per option so the closed route checks pass at the limit.
fn decision_document(body: &str, options: &[String]) -> String {
    let rendered_options: String = options
        .iter()
        .map(|option| format!("      - id: {option}\n        label: L\n"))
        .collect();
    let routes: String = options
        .iter()
        .map(|option| {
            format!("        {option}:\n          to: start\n          effect: advance\n")
        })
        .collect();
    yaml_document(
        "",
        &format!(
            "  pick:\n    type: decision\n    title: Pick\n    objective: Decide.\n    prompt: Which?\n    reason:\n      required: true\n{body}    options:\n{rendered_options}",
        ),
        &format!("    - id: start\n      use: pick\n      routes:\n{routes}"),
        "",
    )
}

fn option_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("o{index}")).collect()
}

fn one_option() -> Vec<String> {
    option_ids(1)
}

fn text(length: usize) -> String {
    "a".repeat(length)
}

/// `{"a": <nested>}` repeated, so the innermost scalar sits at depth `levels + 1`.
fn nested_json(levels: usize) -> String {
    format!("{}0{}", "{\"a\":".repeat(levels), "}".repeat(levels))
}

/// `{"a":[0, ...]}` with `nodes - 2` array entries: one object node, one array node, and one node
/// per entry.
fn wide_json(nodes: usize) -> String {
    let entries = vec!["0"; nodes - 2].join(",");
    format!("{{\"a\":[{entries}]}}")
}

/// `{"a":"<pad>"}` padded to exactly `bytes` bytes.
fn sized_json(bytes: usize) -> String {
    format!("{{\"a\":\"{}\"}}", "a".repeat(bytes - 8))
}

fn document_bound_cases() -> Vec<BoundCase> {
    vec![
        bound_case(
            "identifier characters",
            64,
            65,
            ConfigError::OutOfBounds {
                field: "NodeDefinitionId",
                min: 1,
                max: 64,
                actual: 65,
            },
            |length| {
                let id = text(length);
                yaml_document(
                    "",
                    &format!(
                        "  {id}:\n    type: action\n    title: Act\n    intent: Do the work.\n"
                    ),
                    &format!("    - id: start\n      use: {id}\n      terminal: true\n"),
                    "",
                )
            },
        ),
        bound_case(
            "graph nodes",
            64,
            65,
            bound_violation("graph node count must be between one and 64"),
            |count| {
                let nodes: String = (0..count)
                    .map(|index| {
                        if index == 0 {
                            START_TERMINAL.to_owned()
                        } else {
                            format!("    - id: n{index}\n      use: act\n      terminal: true\n")
                        }
                    })
                    .collect();
                yaml_document("", ACTION_ACT, &nodes, "")
            },
        ),
        bound_case(
            "definition title characters",
            120,
            121,
            bound_violation("definition title"),
            |length| {
                yaml_document(
                    "",
                    &format!(
                        "  act:\n    type: action\n    title: \"{}\"\n    intent: Do the work.\n",
                        text(length)
                    ),
                    START_TERMINAL,
                    "",
                )
            },
        ),
        bound_case(
            "definition intent characters",
            300,
            301,
            bound_violation("action intent"),
            |length| {
                yaml_document(
                    "",
                    &format!(
                        "  act:\n    type: action\n    title: Act\n    intent: \"{}\"\n",
                        text(length)
                    ),
                    START_TERMINAL,
                    "",
                )
            },
        ),
        bound_case(
            "definition description characters",
            1_000,
            1_001,
            bound_violation("definition description"),
            |length| action_document(&format!("    description: \"{}\"\n", text(length))),
        ),
        bound_case(
            "decision objective characters",
            300,
            301,
            bound_violation("decision objective"),
            |length| {
                yaml_document(
                    "",
                    &format!(
                        "  pick:\n    type: decision\n    title: Pick\n    objective: \"{}\"\n    prompt: Which?\n    reason:\n      required: true\n    options:\n      - id: o0\n        label: L\n",
                        text(length),
                    ),
                    "    - id: start\n      use: pick\n      routes:\n        o0:\n          to: start\n          effect: advance\n",
                    "",
                )
            },
        ),
        bound_case(
            "decision prompt characters",
            500,
            501,
            bound_violation("decision prompt"),
            |length| {
                yaml_document(
                    "",
                    &format!(
                        "  pick:\n    type: decision\n    title: Pick\n    objective: Decide.\n    prompt: \"{}\"\n    reason:\n      required: true\n    options:\n      - id: o0\n        label: L\n",
                        text(length),
                    ),
                    "    - id: start\n      use: pick\n      routes:\n        o0:\n          to: start\n          effect: advance\n",
                    "",
                )
            },
        ),
        bound_case(
            "reason-policy prompt characters",
            300,
            301,
            bound_violation("reason policy prompt"),
            |length| {
                yaml_document(
                    "",
                    &format!(
                        "  pick:\n    type: decision\n    title: Pick\n    objective: Decide.\n    prompt: Which?\n    reason:\n      required: true\n      prompt: \"{}\"\n    options:\n      - id: o0\n        label: L\n",
                        text(length),
                    ),
                    "    - id: start\n      use: pick\n      routes:\n        o0:\n          to: start\n          effect: advance\n",
                    "",
                )
            },
        ),
        bound_case(
            "instructions per definition",
            16,
            17,
            bound_violation("too many definition instructions"),
            |count| {
                let instructions: String = (0..count)
                    .map(|index| format!("      - Step {index}.\n"))
                    .collect();
                action_document(&format!("    instructions:\n{instructions}"))
            },
        ),
        bound_case(
            "instruction characters",
            1_000,
            1_001,
            bound_violation("definition instruction"),
            |length| {
                action_document(&format!(
                    "    instructions:\n      - \"{}\"\n",
                    text(length)
                ))
            },
        ),
        bound_case(
            "items per definition",
            64,
            65,
            bound_violation("too many definition items"),
            |count| {
                let items: String = (0..count)
                .map(|index| format!("      - id: i{index}\n        type: confirm\n        prompt: Done?\n        required: true\n"))
                .collect();
                action_document(&format!("    items:\n{items}"))
            },
        ),
        bound_case(
            "item prompt characters",
            300,
            301,
            bound_violation("item prompt"),
            |length| {
                item_document(&format!(
                    "        type: confirm\n        prompt: \"{}\"\n        required: true\n",
                    text(length)
                ))
            },
        ),
        bound_case(
            "item help characters",
            1_000,
            1_001,
            bound_violation("item help"),
            |length| {
                item_document(&format!(
                    "        type: confirm\n        prompt: Done?\n        help: \"{}\"\n        required: true\n",
                    text(length),
                ))
            },
        ),
        bound_case(
            "text item max_length",
            16_384,
            16_385,
            bound_violation("invalid text length constraints"),
            |maximum| {
                item_document(&format!(
                    "        type: text\n        prompt: Note?\n        required: true\n        max_length: {maximum}\n",
                ))
            },
        ),
        bound_case(
            "list entries",
            200,
            201,
            bound_violation("invalid list item count constraints"),
            |maximum| {
                item_document(&format!(
                    "        type: list\n        prompt: Findings?\n        required: false\n        max_items: {maximum}\n",
                ))
            },
        ),
        bound_case(
            "list entry characters",
            1_000,
            1_001,
            bound_violation("invalid list entry length constraint"),
            |maximum| {
                item_document(&format!(
                    "        type: list\n        prompt: Findings?\n        required: false\n        max_item_length: {maximum}\n",
                ))
            },
        ),
        bound_case(
            "choice count",
            32,
            33,
            bound_violation("choice count must be between one and 32"),
            |count| {
                let choices: String = (0..count)
                    .map(|index| format!("          - c{index}\n"))
                    .collect();
                item_document(&format!(
                    "        type: choice\n        prompt: Which?\n        required: false\n        choices:\n{choices}",
                ))
            },
        ),
        bound_case(
            "choice characters",
            120,
            121,
            bound_violation("choice"),
            |length| {
                item_document(&format!(
                    "        type: choice\n        prompt: Which?\n        required: false\n        choices:\n          - \"{}\"\n",
                    text(length),
                ))
            },
        ),
        bound_case(
            "evidence_from entries",
            8,
            9,
            bound_violation("evidence reference count must be between one and eight"),
            |count| {
                let references: String = (0..count)
                    .map(|index| format!("        - node: s{index}\n"))
                    .collect();
                let sources: String = (0..count)
                    .map(|index| {
                        format!("    - id: s{index}\n      use: act\n      terminal: true\n")
                    })
                    .collect();
                yaml_document(
                    "",
                    ACTION_ACT,
                    &format!(
                        "    - id: start\n      use: act\n      evidence_from:\n{references}      terminal: true\n{sources}"
                    ),
                    "",
                )
            },
        ),
        bound_case(
            "selected evidence items",
            16,
            17,
            bound_violation("selected item count must be between one and sixteen"),
            |count| {
                let items: String = (0..count)
                .map(|index| format!("      - id: i{index}\n        type: confirm\n        prompt: Done?\n        required: true\n"))
                .collect();
                let selected: String = (0..count)
                    .map(|index| format!("            - i{index}\n"))
                    .collect();
                yaml_document(
                    "",
                    &format!(
                        "  act:\n    type: action\n    title: Act\n    intent: Do the work.\n    items:\n{items}"
                    ),
                    &format!(
                        "    - id: start\n      use: act\n      evidence_from:\n        - node: source\n          items:\n{selected}      terminal: true\n    - id: source\n      use: act\n      terminal: true\n",
                    ),
                    "",
                )
            },
        ),
        bound_case(
            "decision options",
            8,
            9,
            bound_violation("decision option count must be between one and eight"),
            |count| decision_document("", &option_ids(count)),
        ),
        bound_case(
            "option label characters",
            120,
            121,
            bound_violation("option label"),
            |length| {
                yaml_document(
                    "",
                    &format!(
                        "  pick:\n    type: decision\n    title: Pick\n    objective: Decide.\n    prompt: Which?\n    reason:\n      required: true\n    options:\n      - id: o0\n        label: \"{}\"\n",
                        text(length),
                    ),
                    "    - id: start\n      use: pick\n      routes:\n        o0:\n          to: start\n          effect: advance\n",
                    "",
                )
            },
        ),
        bound_case(
            "option criteria characters",
            500,
            501,
            bound_violation("option criteria"),
            |length| {
                yaml_document(
                    "",
                    &format!(
                        "  pick:\n    type: decision\n    title: Pick\n    objective: Decide.\n    prompt: Which?\n    reason:\n      required: true\n    options:\n      - id: o0\n        label: L\n        criteria: \"{}\"\n",
                        text(length),
                    ),
                    "    - id: start\n      use: pick\n      routes:\n        o0:\n          to: start\n          effect: advance\n",
                    "",
                )
            },
        ),
        bound_case(
            "evidence guidance entries",
            8,
            9,
            bound_violation("too many evidence guidance entries"),
            |count| {
                let guidance: String = (0..count)
                    .map(|index| format!("      - Consult source {index}.\n"))
                    .collect();
                decision_document(
                    &format!("    evidence_guidance:\n{guidance}"),
                    &one_option(),
                )
            },
        ),
        bound_case(
            "evidence guidance characters",
            200,
            201,
            bound_violation("evidence guidance"),
            |length| {
                decision_document(
                    &format!("    evidence_guidance:\n      - \"{}\"\n", text(length)),
                    &one_option(),
                )
            },
        ),
    ]
}

#[test]
fn document_bound_dimensions_accept_the_limit_and_reject_one_over_deterministically() {
    for case in document_bound_cases() {
        accept_yaml(&case.at_limit_document);

        let first = reject_yaml(&case.one_over_document);
        let second = reject_yaml(&case.one_over_document);
        assert_eq!(
            first, second,
            "{}: rejection must be deterministic",
            case.dimension
        );
        assert_eq!(first, case.rejection, "{}", case.dimension);
        // A bound violation is a closed-schema violation: no catalog code names an individual
        // bound, so the production mapping reports the generic schema code for every dimension.
        assert_eq!(
            diagnostic_code(
                &first,
                &case.one_over_document,
                ProcedureDocumentFormat::Yaml
            ),
            AUTHORING_SCHEMA_INVALID,
            "{}: {first:?}",
            case.dimension,
        );
    }
}

#[test]
fn source_bound_dimensions_accept_the_limit_and_reject_one_over_deterministically() {
    // Total source bytes, nesting depth, and parsed node count are decoder-level dimensions: at
    // their limits no schema-valid procedure document can express them, so they are exercised
    // against the shared bounded decoder directly.
    let bytes = sized_json(MAX_PROCEDURE_DOCUMENT_BYTES);
    assert_eq!(bytes.len(), MAX_PROCEDURE_DOCUMENT_BYTES);
    decode_procedure_document(bytes.as_bytes(), ProcedureDocumentFormat::Json)
        .expect("a document of exactly the maximum byte length is admitted");
    let oversize = sized_json(MAX_PROCEDURE_DOCUMENT_BYTES + 1);
    assert_eq!(
        decode_procedure_document(oversize.as_bytes(), ProcedureDocumentFormat::Json),
        Err(ConfigError::InputTooLarge {
            maximum: MAX_PROCEDURE_DOCUMENT_BYTES,
            actual: MAX_PROCEDURE_DOCUMENT_BYTES + 1,
        }),
    );

    let deep = nested_json(MAX_PROCEDURE_DOCUMENT_DEPTH - 1);
    decode_procedure_document(deep.as_bytes(), ProcedureDocumentFormat::Json)
        .expect("a document of exactly the maximum nesting depth is admitted");
    let deeper = nested_json(MAX_PROCEDURE_DOCUMENT_DEPTH);
    assert_eq!(
        decode_procedure_document(deeper.as_bytes(), ProcedureDocumentFormat::Json),
        Err(ConfigError::InputTooDeep {
            maximum: MAX_PROCEDURE_DOCUMENT_DEPTH,
            actual: MAX_PROCEDURE_DOCUMENT_DEPTH + 1,
        }),
    );

    let wide = wide_json(MAX_PROCEDURE_DOCUMENT_NODES);
    decode_procedure_document(wide.as_bytes(), ProcedureDocumentFormat::Json)
        .expect("a document of exactly the maximum parsed node count is admitted");
    let wider = wide_json(MAX_PROCEDURE_DOCUMENT_NODES + 1);
    assert_eq!(
        decode_procedure_document(wider.as_bytes(), ProcedureDocumentFormat::Json),
        Err(ConfigError::InputTooComplex {
            maximum: MAX_PROCEDURE_DOCUMENT_NODES,
            actual: MAX_PROCEDURE_DOCUMENT_NODES + 1,
        }),
    );
}

#[test]
fn every_fixture_bound_dimension_is_exercised_here_or_named_elsewhere() {
    let fixture = malformed_fixture();
    assert_eq!(
        fixture.boundary_cases.len(),
        33,
        "the fixture declares 33 bound dimensions"
    );

    let mut exercised: Vec<(&str, usize, usize)> = document_bound_cases()
        .iter()
        .map(|case| (case.dimension, case.at_limit, case.one_over))
        .collect();
    // The three decoder-level dimensions of the test above.
    exercised.push((
        "source document bytes",
        MAX_PROCEDURE_DOCUMENT_BYTES,
        MAX_PROCEDURE_DOCUMENT_BYTES + 1,
    ));
    exercised.push((
        "source nesting depth",
        MAX_PROCEDURE_DOCUMENT_DEPTH,
        MAX_PROCEDURE_DOCUMENT_DEPTH + 1,
    ));
    exercised.push((
        "parsed nodes",
        MAX_PROCEDURE_DOCUMENT_NODES,
        MAX_PROCEDURE_DOCUMENT_NODES + 1,
    ));

    let referenced: BTreeSet<&str> = DIMENSIONS_COVERED_ELSEWHERE
        .iter()
        .map(|(dimension, _test)| *dimension)
        .collect();
    let exercised_names: BTreeSet<&str> =
        exercised.iter().map(|(dimension, ..)| *dimension).collect();
    assert!(
        exercised_names.is_disjoint(&referenced),
        "a dimension is either exercised here or referenced elsewhere, never both",
    );

    for boundary in &fixture.boundary_cases {
        let dimension = boundary.dimension.as_str();
        if referenced.contains(dimension) {
            continue;
        }
        let (_, at_limit, one_over) = exercised
            .iter()
            .find(|(name, ..)| *name == dimension)
            .unwrap_or_else(|| panic!("bound dimension `{dimension}` has no coverage"));
        assert_eq!(*at_limit, boundary.at_limit, "{dimension}: at-limit value");
        assert_eq!(*one_over, boundary.one_over, "{dimension}: one-over value");
    }
    assert_eq!(
        exercised_names.len() + referenced.len(),
        fixture.boundary_cases.len(),
        "coverage must account for every declared bound dimension exactly once",
    );
}
