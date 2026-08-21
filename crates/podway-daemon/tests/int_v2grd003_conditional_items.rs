//! V2GRD-003 production coverage for conditional required-item derivation.

use crate::{int_v2run003_runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2};
use serde_json::{Map, Value, json};

const CONDITIONAL_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: conditional-items
version: "1"
name: Conditional items
purpose: Exercise same-attempt conditional item requirements.
node_definitions:
  work:
    type: action
    title: Work
    intent: Record the selected mode and any required notes.
    items:
      - id: mode
        type: choice
        prompt: Select a mode.
        required: true
        choices: [strict, relaxed]
      - id: notes
        type: text
        prompt: Record strict-mode notes.
        required: false
        required_when:
          - item: mode
            equals: strict
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
"#;

fn active_item<'a>(observation: &'a Map<String, Value>, item_id: &str) -> &'a Value {
    observation["active_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == item_id)
        .unwrap()
}

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
        "unused-observe-key",
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
fn v2grd003_required_now_and_completion_share_the_authoritative_item_snapshot() {
    let fixture = support_phase4_workspace::git_worktrees();
    int_v2run003_runtime::make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("conditional-items.yaml"),
        CONDITIONAL_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        CONDITIONAL_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap();
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = int_v2run003_runtime::selector(fixture.main());
    let manager = Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher(manager, "v2grd003-conditional-items");

    let initialize = int_v2run003_runtime::request(
        903_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2grd003-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = int_v2run003_runtime::request(
        903_002,
        "session.start",
        &selector,
        json!({
            "procedure": "conditional-items.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2GRD-003 conditional items"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2grd003-start",
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
        903_003,
        session_id,
        Map::new(),
        "v2grd003-begin",
    );

    let initial = observe(&dispatcher, &selector, 903_010, session_id);
    assert_eq!(
        initial["status"]["current"]["missing_required_item_count"],
        1
    );
    assert_eq!(active_item(&initial, "mode")["required_now"], true);
    assert_eq!(active_item(&initial, "notes")["required_now"], false);
    assert_eq!(
        active_item(&initial, "notes")["condition_status"]["state"],
        "unevaluable"
    );

    int_v2run003_runtime::mutate_item(
        &dispatcher,
        &selector,
        903_020,
        session_id,
        "item.set",
        "mode",
        json!({"value": "relaxed"}).as_object().unwrap().clone(),
        "v2grd003-relaxed",
    );
    let relaxed = observe(&dispatcher, &selector, 903_030, session_id);
    assert_eq!(
        relaxed["status"]["current"]["readiness"]["can_advance"],
        true
    );
    assert_eq!(active_item(&relaxed, "notes")["required_now"], false);
    assert_eq!(
        active_item(&relaxed, "notes")["condition_status"]["state"],
        "unmet"
    );

    int_v2run003_runtime::mutate_item(
        &dispatcher,
        &selector,
        903_040,
        session_id,
        "item.set",
        "mode",
        json!({"value": "strict"}).as_object().unwrap().clone(),
        "v2grd003-strict",
    );
    let strict = observe(&dispatcher, &selector, 903_050, session_id);
    assert_eq!(
        strict["status"]["current"]["missing_required_item_count"],
        1
    );
    assert_eq!(active_item(&strict, "notes")["required_now"], true);
    assert_eq!(
        active_item(&strict, "notes")["condition_status"]["state"],
        "met"
    );
    assert_eq!(
        active_item(&strict, "notes")["condition_status"]["predicates"][0]["actual"],
        "strict"
    );

    let status = int_v2run003_runtime::status(&dispatcher, &selector, 903_060, session_id);
    let complete = int_v2run003_runtime::request(
        903_061,
        "session.complete",
        &selector,
        Map::new(),
        "v2grd003-complete-missing",
        int_v2run003_runtime::session_preconditions(&status),
    );
    let rejected = int_v2run003_runtime::dispatch(&dispatcher, &complete);
    let ResponseEnvelopeV2::Error(error) = rejected else {
        panic!("conditional required item must block completion")
    };
    assert_eq!(error.code().as_str(), "REQUIRED_ITEMS_MISSING");

    int_v2run003_runtime::mutate_item(
        &dispatcher,
        &selector,
        903_070,
        session_id,
        "item.set",
        "notes",
        json!({"value": "required evidence"})
            .as_object()
            .unwrap()
            .clone(),
        "v2grd003-notes",
    );
    let ready = int_v2run003_runtime::status(&dispatcher, &selector, 903_080, session_id);
    let complete = int_v2run003_runtime::request(
        903_081,
        "session.complete",
        &selector,
        Map::new(),
        "v2grd003-complete",
        int_v2run003_runtime::session_preconditions(&ready),
    );
    let completed = int_v2run003_runtime::v2_result(
        int_v2run003_runtime::dispatch(&dispatcher, &complete),
        "session.complete",
    );
    assert_eq!(completed["session_state"], "completed");
}
