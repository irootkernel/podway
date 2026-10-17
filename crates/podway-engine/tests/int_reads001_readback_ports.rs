//! READS-001 projected-command negative boundaries through the typed Direct
//! Engine boundary.
//!
//! Registered oracles on a post-injection fixture whose only fault is one
//! stale historical attempt's `value_json`: compact status and a history
//! window omitting that value must refuse with the baseline cold-read error;
//! archive-show of the same archived session must refuse while archive-list
//! of the SAME fixture keeps returning its exact session id and unchanged
//! summary; the four-way ordering (current corruption, archived corruption,
//! archive-not-found, expected-session mismatch) is pinned as it exists in
//! the unchanged implementation; a healthy archive control still decodes.

use podway_core::{SessionId, TraceSequenceV2};
use podway_engine::{
    direct_api::{DirectCommandV1 as D, DirectRequestV1, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1 as C, DirectReadCommandV1 as R},
    direct_sessions::DirectSessionCommandV1 as S,
    v2_read_service::GraphStatusTierV2,
};
use podway_protocol::{
    SessionArchiveV1, SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1,
    SessionRetryV1,
};
use podway_store::test_support::corrupt_recorded_slot_value;
use serde_json::Value;

use super::int_v2dfc006_direct_execution::Clock;
use super::int_v2dfc006_runtime_ports::Engine;

/// One action node with a recorded text item so a retried attempt leaves a
/// stale recorded value that corruption can target.
const STALE_VALUE_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: reads001-stale-value
version: "1"
name: READS stale value
purpose: Prove hidden historical value refusal and summary-only listing.
node_definitions:
  work:
    type: action
    title: Work
    intent: Record one bounded value.
    items:
      - id: note
        type: text
        prompt: Record the note.
        required: false
        min_length: 1
        max_length: 200
        multiline: false
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
manual_rework:
  allowed_targets: [work]
"#;

fn identity(engine: &Engine) -> SessionIdentityPreconditionsWireV1 {
    let graph = engine.graph();
    SessionIdentityPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
    }
}

fn fences(engine: &Engine) -> SessionMutationPreconditionsWireV1 {
    let graph = engine.graph();
    SessionMutationPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
        expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
    }
}

fn disposition(engine: &Engine) -> D {
    D::Procedure(
        podway_protocol::ProcedureV2MutationCommandV1::SessionTerminalDisposition(
            podway_protocol::SessionTerminalDispositionV1 {
                disposition: podway_protocol::TerminalDispositionInputV1::NotRequired {
                    reason: "The isolated session needs no external handoff.".into(),
                },
                actor: None,
                preconditions: identity(engine),
            },
        ),
    )
}

fn read_request(
    engine: &Engine,
    command: R,
    expected: Option<SessionId>,
    key: &str,
) -> DirectRequestV1 {
    let mut request = engine.request(
        D::Read {
            command,
            expected_session_id: expected,
        },
        key,
    );
    request.request_id =
        podway_protocol::RequestIdV1::new("00000000-0000-4000-8000-0000000005a1").unwrap();
    request
}

/// Executes one read, asserting envelope boundedness and write-freedom.
fn dispatch_read(engine: &Engine, request: &DirectRequestV1) -> DirectResponseV1 {
    let before = engine.sequence();
    let response = engine.execute(request.clone(), &Clock);
    match &response {
        DirectResponseV1::Success(output) => {
            let bytes = serde_json::to_vec(output).unwrap();
            assert!(bytes.len() <= 1_048_576);
        }
        DirectResponseV1::Failure(error) => {
            let bytes = serde_json::to_vec(error).unwrap();
            assert!(bytes.len() <= 1_048_576);
            assert_eq!(engine.sequence(), before);
        }
    }
    response
}

fn failure_code(response: DirectResponseV1) -> (String, Value) {
    match response {
        DirectResponseV1::Failure(error) => {
            let value = error.into_value();
            (value["code"].as_str().unwrap().to_owned(), value)
        }
        DirectResponseV1::Success(output) => {
            panic!("expected failure, got {}", output.as_value())
        }
    }
}

/// Builds one archived session with a stale recorded value and returns the
/// engine, the archived session id and the stale attempt id. When
/// `corrupt` is set, exactly that stale value is replaced post-archive.
fn archived_stale_fixture(corrupt: bool) -> (Engine, SessionId, podway_core::AttemptId) {
    let mut engine = Engine::new(STALE_VALUE_PROCEDURE);
    engine.set("note", "first recorded value", "set-first");
    let stale_attempt = engine
        .graph()
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    engine.success(
        D::Cursor(C::Retry(SessionRetryV1 {
            reason: "Redo for the stale-value oracle.".into(),
            preconditions: fences(&engine),
        })),
        "retry",
    );
    let session_id = engine.graph().trace().session_id().clone();
    engine.complete("complete");
    engine.success(disposition(&engine), "dispose");
    engine.success(
        D::Session(S::Archive(SessionArchiveV1 {
            preconditions: identity(&engine),
        })),
        "archive",
    );
    if corrupt {
        let changed = corrupt_recorded_slot_value(
            engine.store.layout().database(),
            &stale_attempt,
            "note",
            "{\"kind\":\"mystery\"}",
        )
        .unwrap();
        assert_eq!(changed, 1, "exactly the stale recorded value changes");
    }
    engine.cold();
    (engine, session_id, stale_attempt)
}

#[test]
fn corrupted_archive_lists_exact_session_while_show_and_status_refuse() {
    // One fixture: capture the whole public summary before injection, then
    // corrupt only its stale recorded value and compare everything after.
    let (mut engine, session_id, stale_attempt) = archived_stale_fixture(false);
    let list_before = dispatch_read(
        &engine,
        &read_request(&engine, R::ArchiveList, None, "list-before"),
    );
    let DirectResponseV1::Success(list_before) = list_before else {
        panic!("the healthy pre-injection archive-list must succeed");
    };
    let before_entry = list_before.as_value()["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["session_id"].as_str() == Some(session_id.as_str()))
        .expect("the session is listed before injection")
        .clone();
    // Post-injection preimage: database and WAL bytes, sidecar presence and
    // the committed sequence. Volatile preexisting SHM bytes are excluded.
    let read_image = |engine: &Engine| -> (Option<Vec<u8>>, Option<Vec<u8>>, bool) {
        let database = std::fs::read(engine.store.layout().database()).ok();
        let wal = std::fs::read(engine.store.layout().wal()).ok();
        (database, wal, engine.store.layout().shm().exists())
    };
    let changed = corrupt_recorded_slot_value(
        engine.store.layout().database(),
        &stale_attempt,
        "note",
        "{\"kind\":\"mystery\"}",
    )
    .unwrap();
    assert_eq!(changed, 1, "exactly the stale recorded value changes");
    let preimage = read_image(&engine);
    let sequence_before = engine.sequence();
    engine.cold();
    // Archive-list of the SAME corrupted fixture still succeeds, and the
    // entire public summary Value is exactly unchanged (every
    // revision/slot/time field, not just id/title/lifecycle).
    let list = dispatch_read(
        &engine,
        &read_request(&engine, R::ArchiveList, None, "list-corrupt"),
    );
    let DirectResponseV1::Success(list) = list else {
        panic!("archive-list must stay summary-only and succeed");
    };
    let after_entry = list.as_value()["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["session_id"].as_str() == Some(session_id.as_str()))
        .expect("the corrupted session stays visible with its exact id")
        .clone();
    assert_eq!(after_entry, before_entry, "the summary must be unchanged");
    // Archive-show of that same session refuses with the baseline cold-read
    // error.
    let show = failure_code(dispatch_read(
        &engine,
        &read_request(
            &engine,
            R::ArchiveShow {
                session_id: session_id.clone(),
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            Some(session_id.clone()),
            "show-corrupt",
        ),
    ));
    assert_eq!(show.0, "WORKSPACE_STATE_UNREADABLE", "{:?}", show.1);
    assert_eq!(show.1["retryable"], false);
    assert_eq!(show.1["exit_code"], 5);
    // The public list success and show refusal leave persisted bytes, sidecar
    // presence and the committed sequence exactly as the post-injection
    // preimage recorded them.
    let after = read_image(&engine);
    assert_eq!(after, preimage, "reads must not change DB/WAL bytes");
    assert_eq!(engine.sequence(), sequence_before);
}

#[test]
fn corrupted_current_refuses_status_and_history_window_before_any_projection() {
    // Build a session whose CURRENT graph carries the stale value fault.
    let mut engine = Engine::new(STALE_VALUE_PROCEDURE);
    engine.set("note", "first recorded value", "set-first");
    let stale_attempt = engine
        .graph()
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    engine.success(
        D::Cursor(C::Retry(SessionRetryV1 {
            reason: "Redo for the stale-value oracle.".into(),
            preconditions: fences(&engine),
        })),
        "retry",
    );
    let session_id = engine.graph().trace().session_id().clone();
    // Healthy pre-injection proof: a valid verbose history window whose
    // cursor excludes the stale attempt's trace omits the recorded value and
    // succeeds.
    let windowed = read_request(
        &engine,
        R::Status {
            tier: GraphStatusTierV2::Verbose,
            history_before: Some(TraceSequenceV2::FIRST),
        },
        Some(session_id.clone()),
        "history-healthy",
    );
    let healthy = dispatch_read(&engine, &windowed);
    let DirectResponseV1::Success(healthy) = healthy else {
        panic!("the healthy pre-injection window request must succeed");
    };
    let healthy_text = serde_json::to_string(healthy.as_value()).unwrap();
    assert!(
        !healthy_text.contains("first recorded value"),
        "the windowed output must omit the stale attempt's value"
    );
    // Corrupt only that stale value and reopen cold.
    let changed = corrupt_recorded_slot_value(
        engine.store.layout().database(),
        &stale_attempt,
        "note",
        "{\"kind\":\"mystery\"}",
    )
    .unwrap();
    assert_eq!(changed, 1);
    engine.cold();
    // Compact status refuses with the baseline cold-read error even though
    // compact output would omit the value.
    let compact = failure_code(dispatch_read(
        &engine,
        &read_request(
            &engine,
            R::Status {
                tier: GraphStatusTierV2::Compact,
                history_before: None,
            },
            Some(session_id.clone()),
            "compact-corrupt",
        ),
    ));
    assert_eq!(compact.0, "WORKSPACE_STATE_UNREADABLE", "{:?}", compact.1);
    assert_eq!(compact.1["retryable"], false);
    assert_eq!(compact.1["exit_code"], 5);
    // The SAME window request that succeeded pre-injection now refuses:
    // eager decode dominates the window even though the window omits the
    // corrupted value.
    let history = failure_code(dispatch_read(&engine, &windowed));
    assert_eq!(history.0, "WORKSPACE_STATE_UNREADABLE", "{:?}", history.1);
    assert_eq!(history.1["retryable"], false);
    assert_eq!(history.1["exit_code"], 5);
    // Archive-list also refuses on this corrupted-current fixture: the eager
    // current-view validation is mandatory for every read variant, so a
    // future narrower archive view cannot silently skip it.
    let listed = failure_code(dispatch_read(
        &engine,
        &read_request(&engine, R::ArchiveList, None, "list-corrupt-current"),
    ));
    assert_eq!(listed.0, "WORKSPACE_STATE_UNREADABLE", "{:?}", listed.1);
    assert_eq!(listed.1["retryable"], false);
    assert_eq!(listed.1["exit_code"], 5);
}

#[test]
fn archive_read_ordering_pins_current_corruption_archived_corruption_notfound_and_mismatch() {
    // (a) A corrupted CURRENT graph dominates an archive-show of a session
    // that does not even exist.
    let (engine, session_id, _stale) = archived_stale_fixture(true);
    let missing = SessionId::new("00000000-0000-4000-8000-00000000dead").unwrap();
    // Seed a fresh running current session in the same workspace so the
    // current view is healthy again; the archived corruption then surfaces
    // on archive-show, and not-found ordering is probed on the healthy
    // fixture below.
    let (healthy, healthy_id, _stale) = archived_stale_fixture(false);
    // (b) Healthy current + missing archive: the not-found mapping.
    let not_found = failure_code(dispatch_read(
        &healthy,
        &read_request(
            &healthy,
            R::ArchiveShow {
                session_id: missing.clone(),
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            None,
            "show-missing",
        ),
    ));
    assert_eq!(
        not_found.0, "SESSION_ARCHIVE_NOT_FOUND",
        "{:?}",
        not_found.1
    );
    assert_eq!(not_found.1["retryable"], false);
    assert_eq!(not_found.1["exit_code"], 1);
    // (c) Healthy current + healthy archive + expected-session mismatch:
    // the identity mismatch fires after the archived view decoded.
    let mismatch_id = SessionId::new("00000000-0000-4000-8000-00000000beef").unwrap();
    let mismatch = failure_code(dispatch_read(
        &healthy,
        &read_request(
            &healthy,
            R::ArchiveShow {
                session_id: healthy_id.clone(),
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            Some(mismatch_id.clone()),
            "show-mismatch",
        ),
    ));
    assert_eq!(mismatch.0, "SESSION_ID_MISMATCH", "{:?}", mismatch.1);
    assert_eq!(mismatch.1["retryable"], false);
    assert_eq!(mismatch.1["exit_code"], 4);
    // (d) Current-graph corruption precedes the archived-not-found check:
    // with a corrupted current graph, archive-show reports the current
    // decode error, not the missing archive.
    let mut corrupted_current = Engine::new(STALE_VALUE_PROCEDURE);
    corrupted_current.set("note", "first recorded value", "set-first");
    let stale_attempt = corrupted_current
        .graph()
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    corrupted_current.success(
        D::Cursor(C::Retry(SessionRetryV1 {
            reason: "Redo for the stale-value oracle.".into(),
            preconditions: fences(&corrupted_current),
        })),
        "retry",
    );
    corrupt_recorded_slot_value(
        corrupted_current.store.layout().database(),
        &stale_attempt,
        "note",
        "{\"kind\":\"mystery\"}",
    )
    .unwrap();
    corrupted_current.cold();
    let current_wins = failure_code(dispatch_read(
        &corrupted_current,
        &read_request(
            &corrupted_current,
            R::ArchiveShow {
                session_id: missing,
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            None,
            "show-current-wins",
        ),
    ));
    assert_eq!(
        current_wins.0, "WORKSPACE_STATE_UNREADABLE",
        "{:?}",
        current_wins.1
    );
    // The corrupted-archive engine from (a) with a missing archive proves
    // selection isolation only: archive selection is summary-based, so the
    // corruption of a DIFFERENT archived session never leaks into selecting
    // or decoding an unrelated id. It is not an archived-corruption
    // precedence proof.
    let archived_corrupt = failure_code(dispatch_read(
        &engine,
        &read_request(
            &engine,
            R::ArchiveShow {
                session_id: SessionId::new("00000000-0000-4000-8000-00000000feed").unwrap(),
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            None,
            "show-missing-after-archive",
        ),
    ));
    assert_eq!(archived_corrupt.0, "SESSION_ARCHIVE_NOT_FOUND");
    // (e) Archived corruption paired with a wrong expected session: the
    // archived decode error dominates the identity mismatch.
    let corrupt_mismatch = failure_code(dispatch_read(
        &engine,
        &read_request(
            &engine,
            R::ArchiveShow {
                session_id: session_id.clone(),
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            Some(mismatch_id.clone()),
            "show-corrupt-mismatch",
        ),
    ));
    assert_eq!(
        corrupt_mismatch.0, "WORKSPACE_STATE_UNREADABLE",
        "{:?}",
        corrupt_mismatch.1
    );
    assert_eq!(corrupt_mismatch.1["retryable"], false);
    assert_eq!(corrupt_mismatch.1["exit_code"], 5);
    // (f) Missing archive paired with a wrong expected session: the
    // not-found mapping dominates the identity mismatch.
    let missing_mismatch = failure_code(dispatch_read(
        &healthy,
        &read_request(
            &healthy,
            R::ArchiveShow {
                session_id: SessionId::new("00000000-0000-4000-8000-00000000feed").unwrap(),
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            Some(mismatch_id.clone()),
            "show-missing-mismatch",
        ),
    ));
    assert_eq!(
        missing_mismatch.0, "SESSION_ARCHIVE_NOT_FOUND",
        "{:?}",
        missing_mismatch.1
    );
}
