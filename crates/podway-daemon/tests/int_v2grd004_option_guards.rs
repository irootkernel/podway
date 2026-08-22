//! V2GRD-004 production coverage for guarded decision read projections.

use crate::{int_v2run003_runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2};
use serde_json::{Map, Value, json};

const GUARDED_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: guarded-options
version: "1"
name: Guarded options
purpose: Exercise selected-evidence option guards.
node_definitions:
  review:
    type: action
    title: Review
    intent: Record the review result.
    items:
      - id: findings
        type: integer
        prompt: How many findings remain?
        required: true
      - id: notes
        type: text
        prompt: Optional review notes.
        required: false
  decide:
    type: decision
    title: Decide
    objective: Select the supported disposition.
    prompt: Which disposition is supported?
    options:
      - id: approved
        label: Approved
        criteria: No findings remain.
        guards:
          - evidence:
              node: review
              item: findings
            equals: 0
      - id: blocked
        label: Blocked
        criteria: At least one finding remains.
        guards:
          - evidence:
              node: review
              item: findings
            at_least: 1
      - id: noted
        label: Notes recorded
        criteria: Optional notes were recorded.
        guards:
          - evidence:
              node: review
              item: notes
            non_empty: true
      - id: fallback
        label: Fallback
        criteria: No guarded disposition is needed.
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the guarded workflow.
graph:
  entry: review
  nodes:
    - id: review
      use: review
      next: decide
    - id: decide
      use: decide
      evidence_from:
        - node: review
          required: true
          items: [findings, notes]
      routes:
        approved:
          to: finish
          effect: advance
        blocked:
          to: finish
          effect: advance
        noted:
          to: finish
          effect: advance
        fallback:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
"#;

fn observe(
    dispatcher: &impl podway_daemon::server::RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
) -> Map<String, Value> {
    let request = int_v2run003_runtime::request(
        request_number,
        "session.observe",
        selector,
        Map::new(),
        "v2grd004-observe",
        PreconditionsV1::new(
            Some(podway_core::SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    int_v2run003_runtime::v2_result(
        int_v2run003_runtime::dispatch(dispatcher, &request),
        "session.observe",
    )
}

#[test]
fn v2grd004_next_keeps_all_options_and_projects_authoritative_guard_statuses() {
    let fixture = support_phase4_workspace::git_worktrees();
    int_v2run003_runtime::make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("guarded-options.yaml"),
        GUARDED_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(GUARDED_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap();
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = int_v2run003_runtime::selector(fixture.main());
    let manager = Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher(manager, "v2grd004-option-guards");

    let initialize = int_v2run003_runtime::request(
        904_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2grd004-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = int_v2run003_runtime::request(
        904_002,
        "session.start",
        &selector,
        json!({
            "procedure": "guarded-options.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2GRD-004 option guards"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2grd004-start",
        PreconditionsV1::default(),
    );
    let started = int_v2run003_runtime::v2_result(
        int_v2run003_runtime::dispatch(&dispatcher, &start),
        "session.start",
    );
    let session_id = started["session_id"].as_str().unwrap();
    int_v2run003_runtime::begin(
        &dispatcher,
        &selector,
        904_003,
        session_id,
        Map::new(),
        "v2grd004-begin",
    );
    int_v2run003_runtime::mutate_item(
        &dispatcher,
        &selector,
        904_010,
        session_id,
        "item.set",
        "findings",
        json!({"value": "0"}).as_object().unwrap().clone(),
        "v2grd004-findings",
    );
    let status = int_v2run003_runtime::status(&dispatcher, &selector, 904_020, session_id);
    let complete = int_v2run003_runtime::request(
        904_021,
        "session.complete",
        &selector,
        Map::new(),
        "v2grd004-complete-review",
        int_v2run003_runtime::session_preconditions(&status),
    );
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &complete),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let observation = observe(&dispatcher, &selector, 904_030, session_id);
    let guidance = observation["guidance"].as_object().unwrap();
    assert_eq!(guidance["options"].as_array().unwrap().len(), 4);
    assert_eq!(
        guidance["allowed_option_ids"],
        json!(["approved", "fallback"])
    );
    let statuses = guidance["option_guard_statuses"].as_array().unwrap();
    assert_eq!(statuses.len(), 4);
    assert_eq!(statuses[0]["state"], "met");
    assert_eq!(statuses[0]["predicates"][0]["actual"], 0);
    assert_eq!(statuses[1]["state"], "unmet");
    assert_eq!(statuses[2]["state"], "unevaluable");
    assert!(statuses[2]["predicates"][0].get("actual").is_none());
    assert_eq!(
        statuses[3],
        json!({"option_id": "fallback", "state": "met", "predicates": []})
    );
    assert_eq!(
        guidance["options"][0]["guards"][0]["evidence"]["node"],
        "review"
    );
}
