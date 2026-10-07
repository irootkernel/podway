//! Ports of the retained fixture conformance tests; daemon copies remain until cutover.

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};

pub(crate) const DECISION_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: decision-runtime
version: "2"
name: Decision runtime
purpose: Exercise a fully fenced decision with required items and resolved evidence.
node_definitions:
  work:
    type: action
    title: Record work
    intent: Produce evidence for the decision.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        min_length: 1
        max_length: 200
        multiline: false
  review:
    type: decision
    title: Review work
    objective: Choose whether to accept the recorded work.
    prompt: Accept the work?
    items:
      - id: note
        type: text
        prompt: Record the decision note.
        required: true
        min_length: 1
        max_length: 200
        multiline: false
    options:
      - id: accept
        label: Accept
      - id: reject
        label: Reject
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the decision fixture.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: review
    - id: review
      use: review
      evidence_from:
        - node: work
          required: true
          items:
            - result
      routes:
        accept:
          to: finish
          effect: advance
        reject:
          to: work
          effect: rework
    - id: finish
      use: finish
      terminal: true
"#;

#[test]
fn v2drw001_decision_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(DECISION_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2DRW-001 decision fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "decision.yaml",
        DECISION_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2DRW-001 decision fixture must pass structural vetting: {diagnostics:?}"
    );
}

pub(crate) const READBACK_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: decision-readback
version: "2"
name: Decision read-back
purpose: Exercise bounded immutable decision and rework history through production dispatch.
node_definitions:
  work:
    type: action
    title: Record work
    intent: Produce evidence for review.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        min_length: 1
        max_length: 65536
        multiline: false
  review:
    type: decision
    title: Review work
    objective: Accept or reject the recorded work.
    prompt: Accept the work?
    items:
      - id: note
        type: text
        prompt: Record the decision note.
        required: true
        min_length: 1
        max_length: 200
        multiline: false
    options:
      - id: accept
        label: Accept
      - id: reject
        label: Reject
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the read-back fixture.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: review
    - id: review
      use: review
      evidence_from:
        - node: work
          required: true
          items:
            - result
      routes:
        accept:
          to: finish
          effect: advance
        reject:
          to: work
          effect: rework
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - review
"#;

#[test]
fn v2drw005_readback_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(READBACK_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2DRW-005 read-back fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "decision-readback.yaml",
        READBACK_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2DRW-005 read-back fixture must pass structural vetting: {diagnostics:?}"
    );
}

pub(crate) const FAILURE_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: decision-failure-closure
version: "2"
name: Decision failure closure
purpose: Exercise deterministic decision and rework failures through production dispatch.
node_definitions:
  work:
    type: action
    title: Record work
    intent: Produce evidence for review.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        min_length: 1
        max_length: 200
        multiline: false
  review:
    type: decision
    title: Review work
    objective: Accept or reject the recorded work.
    prompt: Accept the work?
    items:
      - id: note
        type: text
        prompt: Record the decision note.
        required: true
        min_length: 1
        max_length: 200
        multiline: false
    options:
      - id: accept
        label: Accept
      - id: reject
        label: Reject
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the failure fixture.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: review
    - id: review
      use: review
      evidence_from:
        - node: work
          required: true
          items:
            - result
      routes:
        accept:
          to: finish
          effect: advance
        reject:
          to: work
          effect: rework
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - review
"#;

#[test]
fn v2drw006_failure_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(FAILURE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2DRW-006 fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "v2drw006.yaml",
        FAILURE_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2DRW-006 fixture must pass structural vetting: {diagnostics:?}"
    );
}

pub(crate) const ACTION_READBACK_PROCEDURE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/action-readback-procedure.yaml");

#[test]
fn v2run003_action_readback_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        panic!("the V2RUN-003 runtime fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "action-readback-procedure.yaml",
        ACTION_READBACK_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2RUN-003 runtime fixture must pass structural vetting: {diagnostics:?}"
    );
}

pub(crate) const RETRY_PROCEDURE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/retry-procedure.yaml");

#[test]
fn v2run004_retry_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(RETRY_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2RUN-004 retry fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "retry-procedure.yaml",
        RETRY_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2RUN-004 retry fixture must pass structural vetting: {diagnostics:?}"
    );
}

pub(crate) const SKIP_PROCEDURE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/skip-procedure.yaml");

#[test]
fn v2run005_skip_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(SKIP_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("the V2RUN-005 skip fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "skip-procedure.yaml",
        SKIP_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2RUN-005 skip fixture must pass structural vetting: {diagnostics:?}"
    );
}

pub(crate) const STATE_PROCEDURE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/state-derivation-procedure.yaml");

#[test]
fn v2run006_state_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(STATE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        panic!("the V2RUN-006 state fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "state-derivation-procedure.yaml",
        STATE_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    assert!(vet_procedure_v2(&validated, &context).is_empty());
}
