//! Production and Store-boundary coverage for V2RUN-008 runtime recovery closure.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::SessionId;
use podway_daemon::server::RequestDispatcherV1;
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2};
use serde_json::{Map, Value, json};

const PROCEDURE: &str = include_str!("fixtures/retry-procedure.yaml");

fn start(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
) -> String {
    let initialize = runtime::request(
        80_001,
        "workspace.init",
        selector,
        Map::new(),
        "v2run008-init",
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
        80_002,
        "session.start",
        selector,
        json!({
            "procedure": "v2run008.yaml",
            "expected_procedure_digest": digest,
            "task_title": "V2RUN-008 concurrency and recovery"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run008-start",
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
        80_003,
        &session_id,
        Map::new(),
        "v2run008-begin",
    );
    session_id
}

fn status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &podway_protocol::WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
    verbose: bool,
) -> Map<String, Value> {
    let payload = if verbose {
        json!({"verbose": true}).as_object().unwrap().clone()
    } else {
        Map::new()
    };
    let request = runtime::request(
        request_number,
        "session.status",
        selector,
        payload,
        "unused-v2run008-status-key",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    runtime::v2_result(runtime::dispatch(dispatcher, &request), "session.status")
}

fn counter<'a>(status: &'a Map<String, Value>, graph_node_id: &str) -> &'a Value {
    status["counters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|counter| counter["graph_node_id"] == graph_node_id)
        .unwrap()
}

#[test]
fn v2run008_concurrent_callers_restart_and_long_retry_traversal_keep_one_cursor() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("v2run008.yaml"), PROCEDURE).unwrap();
    let selector = runtime::selector(fixture.main());
    let mut manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let production = Arc::new(runtime::dispatcher(
        Arc::clone(&manager),
        "v2run008-concurrent",
    ));
    let session_id = start(production.as_ref(), &selector);
    let before = status(production.as_ref(), &selector, 80_010, &session_id, false);
    let preconditions = runtime::session_preconditions(&before);
    let left_request = runtime::request(
        80_011,
        "session.retry",
        &selector,
        json!({"reason": "concurrent retry left"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run008-concurrent-left",
        preconditions.clone(),
    );
    let right_request = runtime::request(
        80_012,
        "session.retry",
        &selector,
        json!({"reason": "concurrent retry right"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run008-concurrent-right",
        preconditions,
    );
    let barrier = Arc::new(Barrier::new(3));
    let left_dispatcher = Arc::clone(&production);
    let left_barrier = Arc::clone(&barrier);
    let left = thread::spawn(move || {
        left_barrier.wait();
        runtime::dispatch(left_dispatcher.as_ref(), &left_request)
    });
    let right_dispatcher = Arc::clone(&production);
    let right_barrier = Arc::clone(&barrier);
    let right = thread::spawn(move || {
        right_barrier.wait();
        runtime::dispatch(right_dispatcher.as_ref(), &right_request)
    });
    barrier.wait();
    let responses = [left.join().unwrap(), right.join().unwrap()];
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(response, ResponseEnvelopeV2::OutputV2(_)))
            .count(),
        1
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(response, ResponseEnvelopeV2::Error(error) if error.code().as_str() == "SESSION_REVISION_CONFLICT"))
            .count(),
        1
    );
    let after_concurrent = status(production.as_ref(), &selector, 80_020, &session_id, false);
    assert_eq!(after_concurrent["trace_length"], 2);
    assert_eq!(after_concurrent["current"]["attempt"]["attempt_number"], 2);
    assert_eq!(counter(&after_concurrent, "work")["attempt_count"], 2);
    assert_eq!(after_concurrent["queue"]["queued_count"], 0);
    assert!(after_concurrent["queue"]["running_job_id"].is_null());

    drop(production);
    fs::remove_file(fixture.main().join("v2run008.yaml")).unwrap();
    let mut production = runtime::dispatcher(Arc::clone(&manager), "v2run008-long-cycle");
    for index in 0..40_u64 {
        let before = status(
            &production,
            &selector,
            80_100 + index * 2,
            &session_id,
            false,
        );
        let retry = runtime::request(
            80_101 + index * 2,
            "session.retry",
            &selector,
            json!({"reason": format!("unbounded retry traversal {index}")})
                .as_object()
                .unwrap()
                .clone(),
            &format!("v2run008-long-retry-{index}"),
            runtime::session_preconditions(&before),
        );
        assert!(matches!(
            runtime::dispatch(&production, &retry),
            ResponseEnvelopeV2::OutputV2(_)
        ));
        if index == 19 {
            drop(production);
            drop(manager);
            manager = Arc::new(runtime::manager(fixture.temporary_path()));
            production = runtime::dispatcher(Arc::clone(&manager), "v2run008-restarted");
        }
    }

    let final_status = status(&production, &selector, 80_300, &session_id, true);
    assert_eq!(final_status["trace_length"], 42);
    assert_eq!(final_status["current"]["attempt"]["attempt_number"], 42);
    assert_eq!(counter(&final_status, "work")["attempt_count"], 42);
    assert_eq!(counter(&final_status, "work")["rework_traversal_count"], 0);
    assert_eq!(
        final_status["current_trace_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let stale_window = final_status["stale_attempt_history"]["entries"]
        .as_array()
        .unwrap();
    assert!(!stale_window.is_empty());
    assert!(stale_window.len() <= 32);
    assert!(stale_window.len() < 41);
    assert_eq!(
        final_status["stale_attempt_history"]["trace_truncated"],
        true
    );
    assert_eq!(final_status["queue"]["queued_count"], 0);
    assert!(final_status["queue"]["running_job_id"].is_null());
}
