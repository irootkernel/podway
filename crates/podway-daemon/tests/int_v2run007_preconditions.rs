//! Production vertical coverage for V2RUN-007 mutation fences and exact replay.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AttemptId, Revision, SessionId};
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2};
use serde_json::{Map, Value, json};

const PROCEDURE: &str = include_str!("fixtures/action-readback-procedure.yaml");

fn start(
    dispatcher: &impl podway_daemon::server::RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
) -> String {
    let initialize = runtime::request(
        70_001,
        "workspace.init",
        selector,
        Map::new(),
        "v2run007-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        runtime::dispatch(dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let request = runtime::request(
        70_002,
        "session.start",
        selector,
        json!({
            "procedure": "v2run007.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2RUN-007 mutation fences"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run007-start",
        PreconditionsV1::default(),
    );
    let session_id =
        runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.start")["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
    runtime::begin(
        dispatcher,
        selector,
        70_003,
        &session_id,
        Map::new(),
        "v2run007-begin",
    );
    session_id
}

fn item_set_request(
    number: u64,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    item_id: &str,
    value: &str,
    key: &str,
    preconditions: PreconditionsV1,
) -> (
    podway_protocol::RequestEnvelopeV1,
    podway_daemon::server::DaemonRequestV1,
) {
    runtime::request(
        number,
        "item.set",
        selector,
        json!({"item_id": item_id, "value": value})
            .as_object()
            .unwrap()
            .clone(),
        key,
        preconditions,
    )
}

fn assert_not_admitted_error(response: ResponseEnvelopeV2, code: &str) {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("{code} must be returned before durable admission")
    };
    assert_eq!(error.code().as_str(), code);
    assert_eq!(error.details()["admission"]["admitted"], false);
}

fn item_revision(status: &Map<String, Value>, item_id: &str) -> u64 {
    status["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == item_id)
        .unwrap()["revision"]
        .as_u64()
        .unwrap()
}

#[test]
fn v2run007_preadmission_fences_reject_without_jobs_and_exact_replay_wins() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("v2run007.yaml"), PROCEDURE).unwrap();
    let selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2run007-fences");
    let session_id = start(&dispatcher, &selector);
    let before = runtime::status(&dispatcher, &selector, 70_010, &session_id);
    let session_revision = before["session"]["revision"].as_u64().unwrap();
    let attempt_id = before["current"]["attempt"]["attempt_id"].as_str().unwrap();

    let wrong_session = runtime::request(
        70_009,
        "session.block",
        &selector,
        json!({"reason": "wrong session identity"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run007-wrong-session",
        PreconditionsV1::new(
            Some(SessionId::new("00000000-0000-4000-8000-000000007778").unwrap()),
            Some(Revision::new(session_revision)),
            Some(AttemptId::new(attempt_id).unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &wrong_session),
        "SESSION_ID_MISMATCH",
    );

    let stale_revision = runtime::request(
        70_011,
        "session.block",
        &selector,
        json!({"reason": "stale revision"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run007-stale-revision",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(Revision::new(session_revision + 1)),
            Some(AttemptId::new(attempt_id).unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &stale_revision),
        "SESSION_REVISION_CONFLICT",
    );

    let stale_attempt = runtime::request(
        70_012,
        "session.block",
        &selector,
        json!({"reason": "stale attempt"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run007-stale-attempt",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(Revision::new(session_revision)),
            Some(AttemptId::new("00000000-0000-4000-8000-000000007777").unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &stale_attempt),
        "ATTEMPT_NOT_CURRENT",
    );

    let wrong_item_revision = item_set_request(
        70_013,
        &selector,
        "summary",
        "wrong revision",
        "v2run007-wrong-item-revision",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            None,
            Some(AttemptId::new(attempt_id).unwrap()),
            Some(Revision::new(item_revision(&before, "summary") + 1)),
            None,
            None,
        )
        .unwrap(),
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &wrong_item_revision),
        "ITEM_REVISION_CONFLICT",
    );

    let missing_item = item_set_request(
        70_014,
        &selector,
        "missing",
        "missing item",
        "v2run007-missing-item",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            None,
            Some(AttemptId::new(attempt_id).unwrap()),
            Some(Revision::ZERO),
            None,
            None,
        )
        .unwrap(),
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &missing_item),
        "ITEM_NOT_FOUND",
    );

    let summary_preconditions = runtime::item_preconditions(&before, "summary");
    let note_preconditions = runtime::item_preconditions(&before, "internal-note");
    let set_summary = item_set_request(
        70_020,
        &selector,
        "summary",
        "first value",
        "v2run007-set-summary",
        summary_preconditions.clone(),
    );
    let first = runtime::dispatch(&dispatcher, &set_summary);
    assert_eq!(
        runtime::v2_result(first.clone(), "item.set")["changed"],
        true
    );
    let set_note = item_set_request(
        70_021,
        &selector,
        "internal-note",
        "independent value",
        "v2run007-set-note",
        note_preconditions,
    );
    assert_eq!(
        runtime::v2_result(runtime::dispatch(&dispatcher, &set_note), "item.set")["changed"],
        true,
        "an unrelated item update must not stale the captured item revision"
    );

    let replay = item_set_request(
        70_022,
        &selector,
        "summary",
        "first value",
        "v2run007-set-summary",
        summary_preconditions.clone(),
    );
    let replayed = runtime::dispatch(&dispatcher, &replay);
    assert_eq!(
        runtime::without_request_id(&replayed),
        runtime::without_request_id(&first),
        "exact idempotency replay must precede the now-stale item fence"
    );

    let reused = item_set_request(
        70_023,
        &selector,
        "summary",
        "different request",
        "v2run007-set-summary",
        summary_preconditions.clone(),
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &reused),
        "IDEMPOTENCY_KEY_REUSED",
    );

    let newly_stale_item = item_set_request(
        70_024,
        &selector,
        "summary",
        "stale after success",
        "v2run007-new-stale-item",
        summary_preconditions,
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &newly_stale_item),
        "ITEM_REVISION_CONFLICT",
    );

    let stale_after_items = runtime::request(
        70_025,
        "session.block",
        &selector,
        json!({"reason": "stale after item changes"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run007-stale-after-items",
        runtime::session_preconditions(&before),
    );
    assert_not_admitted_error(
        runtime::dispatch(&dispatcher, &stale_after_items),
        "SESSION_REVISION_CONFLICT",
    );

    let after = runtime::status(&dispatcher, &selector, 70_030, &session_id);
    assert_eq!(after["session"]["revision"], session_revision + 2);
    assert_eq!(item_revision(&after, "summary"), 1);
    assert_eq!(item_revision(&after, "internal-note"), 1);
}
