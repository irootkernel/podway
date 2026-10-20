//! READS-001 negative data boundaries over unchanged validation.
//!
//! Registered oracles: a stale historical attempt's `value_json` change alone
//! must make fresh cold graph reads fail closed without effects; the same
//! post-injection archived fixture must still list its unchanged summary
//! while its archived graph decode refuses; noncanonical but JSON-valid
//! values must fail the exact typed re-encoding check; multi-fault loading
//! precedence is pinned with real probe-order evidence from the scoped query
//! observers, because the public error is identical for these faults.

use podway_core::UnixMillis;
use podway_store::direct_store::DirectStoreV1;
use podway_store::test_support::{corrupt_recorded_slot_value, query_account::QueryAccountant};

use super::int_reads001_query_inventory::{
    attempt_total, attempt_uuid, encoded_payload_bytes, reads001_fixture, reads001_session,
    reads001_snapshot, recipe_medium, recipe_small,
};
use super::int_v2dfc005_direct_store::{deadline, request, store, success};
use super::int_v2dfc008_persistence::{archive, fixture, image, seed};

const UNKNOWN_KIND: &str = "{\"kind\":\"mystery\"}";
const NONCANONICAL_TEXT_PREFIX: &str = "{\"value\":\"";

fn sidecar_exists(direct: &DirectStoreV1) -> bool {
    direct.layout().shm().exists()
}

#[test]
fn stale_attempt_value_corruption_refuses_cold_graph_and_preserves_bytes() {
    let recipe = recipe_medium();
    let (root, direct, _state) = reads001_fixture(&recipe, 120, 220);
    // Change only a stale historical attempt's value; the active repeat
    // attempt and every other row stay untouched.
    let stale_attempt = 3;
    let changed = corrupt_recorded_slot_value(
        direct.layout().database(),
        &attempt_uuid(120, stale_attempt),
        "item-0",
        UNKNOWN_KIND,
    )
    .unwrap();
    assert_eq!(changed, 1, "exactly one stale slot value changes");
    let after_injection = image(&direct);
    let cold = store(root.path());
    let refused = cold.read(deadline(), |snapshot| snapshot.current_graph());
    assert!(
        refused.is_err(),
        "fresh cold read must refuse the hidden value"
    );
    assert_eq!(
        image(&cold),
        after_injection,
        "the refusing read must not change persisted bytes"
    );
    assert!(
        !sidecar_exists(&cold),
        "a refusing cold read must not create a sidecar"
    );
    // A second fresh open refuses identically: the fault is durable.
    let colder = store(root.path());
    assert!(
        colder
            .read(deadline(), |snapshot| snapshot.current_graph())
            .is_err()
    );
}

#[test]
fn noncanonical_valid_value_json_fails_typed_reencoding() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 121, 221);
    // Fetch one valid canonical value and rewrite it with swapped keys: the
    // same typed value, JSON-valid, but not the exact typed re-encoding.
    let database = direct.layout().database();
    let connection = rusqlite::Connection::open(&database).unwrap();
    let canonical: String = connection
        .query_row(
            "SELECT value_json FROM v2_item_slots WHERE attempt_id=?1 AND item_id='item-0'",
            [attempt_uuid(121, 1).as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(canonical.starts_with("{\"kind\":\"text\",\"value\":\""));
    let value: String = serde_json::from_str::<serde_json::Value>(&canonical)
        .unwrap()
        .get("value")
        .and_then(|value| value.as_str())
        .unwrap()
        .to_owned();
    let noncanonical = format!("{NONCANONICAL_TEXT_PREFIX}{value}\",\"kind\":\"text\"}}");
    drop(connection);
    let changed =
        corrupt_recorded_slot_value(&database, &attempt_uuid(121, 1), "item-0", &noncanonical)
            .unwrap();
    assert_eq!(changed, 1);
    let cold = store(root.path());
    assert!(
        cold.read(deadline(), |snapshot| snapshot.current_graph())
            .is_err(),
        "exact typed re-encoding equality must reject noncanonical bytes"
    );
}

/// Completes and archives one inventory session at Store level, returning the
/// archived session id.
fn archived_inventory_session(
    direct: &DirectStoreV1,
    recipe: &super::int_reads001_query_inventory::Recipe,
    session_number: u64,
    snapshot_number: u64,
) -> podway_core::SessionId {
    let snapshot = reads001_snapshot(recipe, snapshot_number);
    let running = reads001_session(recipe, &snapshot, session_number, 0, false);
    // Complete the active repeat attempt so the session can be archived.
    let active = running
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let completed = running
        .complete_active_action_v2(
            running.trace().revision(),
            &active,
            None,
            UnixMillis::new(500),
        )
        .unwrap()
        .into_state();
    seed(direct, &completed);
    archive(direct, &completed, UnixMillis::new(600));
    completed.trace().session_id().clone()
}

#[test]
fn same_corrupted_archive_lists_and_summaries_succeed_while_graph_load_refuses() {
    // The single-revision variant keeps its active attempt on the terminal
    // work node so Store-level completion and archiving stay valid.
    let mut recipe = recipe_small();
    recipe.goal_revisions = 1;
    // A workspace holds at most one current task, so this database carries
    // only the two archived sessions and no running current session.
    let identity = podway_store::DurableWorktreeIdentityV1::new(
        super::int_v2dfc005_direct_store::digest('a'),
        podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000000122").unwrap(),
        super::int_v2dfc005_direct_store::digest('b'),
    );
    let (root, direct) = super::int_v2dfc008_persistence::fixture(&identity);
    // Archive a completed session with a stale recorded attempt plus one
    // healthy control archive in the same database.
    let healthy = archived_inventory_session(&direct, &recipe, 9001, 9101);
    let corrupted = archived_inventory_session(&direct, &recipe, 9002, 9102);
    let changed = corrupt_recorded_slot_value(
        direct.layout().database(),
        &attempt_uuid(9002, 2),
        "item-0",
        UNKNOWN_KIND,
    )
    .unwrap();
    assert_eq!(changed, 1, "exactly the archived stale value changes");
    let after_injection = image(&direct);
    let cold = store(root.path());
    // The SAME post-injection fixture: the corrupted session's unchanged
    // summary stays listable...
    let page = cold
        .read(deadline(), |snapshot| snapshot.archives(256))
        .unwrap();
    let corrupted_summary = page
        .sessions()
        .iter()
        .find(|summary| summary.session_id() == &corrupted)
        .expect("the corrupted archive stays visible in the list window");
    assert_eq!(corrupted_summary.task_title(), "READS-001 inventory");
    assert_eq!(
        corrupted_summary.lifecycle(),
        podway_core::SessionLifecycle::Completed
    );
    let summary = cold
        .read(deadline(), |snapshot| snapshot.archive_summary(&corrupted))
        .unwrap()
        .expect("archive summary reads summary columns only");
    assert_eq!(summary.task_title(), corrupted_summary.task_title());
    assert_eq!(
        summary.session_revision(),
        corrupted_summary.session_revision()
    );
    // ...while the archived graph decode of that same session refuses.
    assert!(
        cold.read(deadline(), |snapshot| snapshot.archived_graph(&corrupted))
            .is_err(),
        "archive-show must refuse the corrupted archived graph"
    );
    // Healthy control: another archived session in the same database still
    // decodes fully.
    let healthy_graph = cold
        .read(deadline(), |snapshot| snapshot.archived_graph(&healthy))
        .unwrap();
    assert!(healthy_graph.is_some(), "healthy archive still decodes");
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// Multi-fault precedence with real probe-order evidence: the public error is
/// the same `CorruptState` for these faults, so the winning stage is proven
/// by which registered statements actually executed before the failure.
#[test]
fn multi_fault_loading_precedence_is_pinned_by_probe_order() {
    // Cross-attempt: a blocker fault on attempt 2 loads before a slot fault
    // on attempt 4.
    let (root, direct, _state) = reads001_fixture(&recipe_medium(), 123, 223);
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_item_slots SET value_json='{\"kind\":\"mystery\"}' \
             WHERE attempt_id=?1 AND item_id='item-0'",
            [attempt_uuid(123, 4).as_str()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE v2_blockers SET reason=?2 WHERE attempt_id=?1",
            rusqlite::params![attempt_uuid(123, 2).as_str(), "z".repeat(5_000)],
        )
        .unwrap();
    drop(connection);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold.read(deadline(), |snapshot| snapshot.current_graph());
    let report = accountant.finish();
    // Pin today's winning typed classification: the attempt-2 blocker fault
    // classifies as a Blocker corruption and stops loading before attempt
    // 4's slot fault could matter.
    assert!(matches!(
        refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Blocker,
            }
        ))
    ));
    // Error neutrality on the SAME corrupt fixture: the observed failure is
    // byte-identical to an unobserved one.
    let unobserved = store(root.path()).read(deadline(), |snapshot| snapshot.current_graph());
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    let slot_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("value_json IS NOT NULL"))
        .map(|(_, counts)| counts.executes)
        .sum();
    let blocker_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("FROM v2_blockers"))
        .map(|(_, counts)| counts.executes)
        .sum();
    let goal_criteria_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("criterion_ordinal, statement FROM v2_goal_criteria"))
        .map(|(_, counts)| counts.executes)
        .sum();
    // One bulk cursor per family; three slot validation scopes prove
    // attempts 0-2 reached their original turns. The attempt-2 blocker
    // fault still wins before attempt 4's slot fault; goals never start.
    assert_eq!(slot_executes, 1);
    assert_eq!(report.retention.slot_scopes, 3);
    assert_eq!(blocker_executes, 1);
    assert_eq!(goal_criteria_executes, 0);
    drop(root);

    // Cross-stage: a slot fault on attempt 1 stops workflow memory before
    // any goal statement runs.
    let (root, direct, _state) = reads001_fixture(&recipe_medium(), 124, 224);
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_item_slots SET value_json='{\"kind\":\"mystery\"}' \
             WHERE attempt_id=?1 AND item_id='item-0'",
            [attempt_uuid(124, 1).as_str()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE v2_goal_criteria SET statement=?1 WHERE goal_revision=1 AND criterion_ordinal=0",
            rusqlite::params!["x".repeat(5_000)],
        )
        .unwrap();
    drop(connection);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold.read(deadline(), |snapshot| snapshot.current_graph());
    let report = accountant.finish();
    // Pin today's winning typed classification: the attempt-1 slot fault
    // classifies as an Item corruption and stops workflow memory before any
    // goal statement runs, so the goal-criteria fault never surfaces.
    assert!(matches!(
        refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Item,
            }
        ))
    ));
    let unobserved = store(root.path()).read(deadline(), |snapshot| snapshot.current_graph());
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    let slot_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("value_json IS NOT NULL"))
        .map(|(_, counts)| counts.executes)
        .sum();
    let goal_criteria_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("criterion_ordinal, statement FROM v2_goal_criteria"))
        .map(|(_, counts)| counts.executes)
        .sum();
    assert_eq!(slot_executes, 1);
    assert_eq!(report.retention.slot_scopes, 2);
    assert_eq!(goal_criteria_executes, 0);

    // Goal-stage fault alone: `v2_goal` corrupt() classifies as Session
    // (the same record kind as session-row corruption), so the same-kind
    // stage distinction is carried by the probe-order registration below —
    // READS-002 bulk acquisition changes the counts but must preserve both
    // the Session classification and this ordering evidence.
    let (root, direct, _state) = reads001_fixture(&recipe_medium(), 125, 225);
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_goal_criteria SET statement=?1 WHERE goal_revision=1 AND criterion_ordinal=0",
            rusqlite::params!["x".repeat(5_000)],
        )
        .unwrap();
    drop(connection);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold.read(deadline(), |snapshot| snapshot.current_graph());
    let report = accountant.finish();
    assert!(matches!(
        refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    let unobserved = store(root.path()).read(deadline(), |snapshot| snapshot.current_graph());
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    let slot_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("value_json IS NOT NULL"))
        .map(|(_, counts)| counts.executes)
        .sum();
    let goal_criteria_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("criterion_ordinal, statement FROM v2_goal_criteria"))
        .map(|(_, counts)| counts.executes)
        .sum();
    // Workflow memory completed fully; the goal stage started and failed on
    // its first criteria statement.
    assert_eq!(slot_executes, 1);
    assert_eq!(
        report.retention.slot_scopes,
        attempt_total(&recipe_medium())
    );
    assert_eq!(goal_criteria_executes, 1);
}

/// A failed second graph reconstruction releases only its own decoded bytes:
/// a healthy graph A retained in the same accounting window keeps its
/// balance, and only graph B's committed partial bytes are booked as
/// released. Slot-scope rollback of B's failing attempt has already run.
#[test]
fn failed_second_graph_releases_only_its_own_decoded_bytes() {
    let recipe = recipe_small();
    let (root_a, _direct_a, state_a) = reads001_fixture(&recipe, 130, 230);
    let (root_b, direct_b, state_b) = reads001_fixture(&recipe, 131, 231);
    // Corrupt a later attempt's slot so graph B commits its first four
    // attempts (positive partial decode) before failing.
    let changed = corrupt_recorded_slot_value(
        direct_b.layout().database(),
        &attempt_uuid(131, 4),
        "item-0",
        UNKNOWN_KIND,
    )
    .unwrap();
    assert_eq!(changed, 1);
    let cold_a = store(root_a.path());
    let cold_b = store(root_b.path());
    let accountant = QueryAccountant::start();
    let graph_a = cold_a.read(deadline(), |snapshot| snapshot.current_graph());
    let refused_b = cold_b.read(deadline(), |snapshot| snapshot.current_graph());
    let report = accountant.finish();
    assert!(refused_b.is_err());
    let a_bytes = encoded_payload_bytes(&state_a);
    let b_partial: u64 = state_b
        .workflow_memory()
        .attempts()
        .iter()
        .take(4)
        .flat_map(|attempt| attempt.item_slots())
        .filter_map(|slot| {
            slot.value()
                .map(podway_core::recorded_value_encoded_size_v2)
        })
        .map(|size| size.map(|size| size as u64).unwrap_or_default())
        .sum();
    assert!(a_bytes > 0);
    assert!(b_partial > 0);
    assert_eq!(report.retention.graph_scopes, 2);
    assert_eq!(
        report.retention.fresh_payloads as u64,
        encoded_slot_count(&state_a) + encoded_slot_count_partial(&state_b),
        "fresh payloads cover A's slots plus B's committed attempts"
    );
    assert_eq!(
        report.retention.fresh_bytes_cumulative,
        a_bytes + b_partial,
        "cumulative fresh bytes cover A plus B's committed attempts"
    );
    assert_eq!(
        report.retention.decoded_retained_in_results, a_bytes,
        "the healthy retained graph A must keep its balance"
    );
    assert_eq!(
        report.retention.decoded_released_at_scope_end, b_partial,
        "only graph B's own contribution is booked as released"
    );
    // Graph A stays live through the assertions.
    let graph_a = graph_a.unwrap();
    assert_eq!(graph_a.as_ref(), Some(&state_a));
}

fn encoded_slot_count(state: &podway_store::GraphSessionStateV2) -> u64 {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .flat_map(|attempt| attempt.item_slots())
        .filter(|slot| slot.value().is_some())
        .count() as u64
}

fn encoded_slot_count_partial(state: &podway_store::GraphSessionStateV2) -> u64 {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .take(4)
        .flat_map(|attempt| attempt.item_slots())
        .filter(|slot| slot.value().is_some())
        .count() as u64
}

/// Builds the shared STATE-001 fixture: one terminal current session holding
/// both a stored delegation receipt (for its accepting attempt) and a
/// recorded terminal disposition.
fn disposed_delegated_session(direct: &DirectStoreV1) -> podway_store::GraphSessionStateV2 {
    use super::int_v2sdg004_receipt as receipts;
    let current = receipts::session_state();
    seed(direct, &current);
    let original = receipts::receipt(&current);
    let accepted = current
        .accept_delegation_successor_v1(current.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    let accept = request("negative-accept-delegation");
    direct
        .mutate(
            &accept,
            deadline(),
            || UnixMillis::new(200),
            |transaction| {
                transaction.now();
                transaction.accept_delegation(
                    current.workspace_revision(),
                    current.trace().revision(),
                    &accepted,
                    &receipts::digest('d'),
                    &original,
                )?;
                Ok(success(
                    &accept,
                    serde_json::json!({"accepted":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let completed = accepted
        .complete_active_action_v2(
            accepted.trace().revision(),
            accepted.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(300),
        )
        .unwrap()
        .into_state();
    let dispose = request("negative-record-disposition");
    direct
        .mutate(
            &dispose,
            deadline(),
            || UnixMillis::new(301),
            |transaction| {
                transaction.now();
                transaction.replace_graph(
                    accepted.workspace_revision(),
                    accepted.trace().revision(),
                    &completed,
                )?;
                transaction.record_disposition(
                    &podway_core::TerminalDispositionV2::not_required(
                        completed.trace().session_id().clone(),
                        completed.trace().revision(),
                        "No handoff required",
                        None,
                        UnixMillis::new(301),
                    )
                    .unwrap(),
                )?;
                Ok(success(
                    &dispose,
                    serde_json::json!({"disposed":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    completed
}

/// Disposition admission runs inside the graph loader before the view's
/// receipt acquisition and before the loader's in-memory cursor comparison.
/// Both faults classify as Session corruption, so the winning stage is
/// proven by registered probe order: the disposition SELECT must have
/// executed, and the receipt window must not have run.
#[test]
fn state001_disposition_admission_precedes_receipt_and_cursor_checks() {
    // (i) Disposition corruption paired with a forged receipt: the view must
    // refuse with the disposition's Session classification while the receipt
    // window statement never executes.
    let (root, direct) = fixture(&super::int_v2sdg004_receipt::identity());
    let completed = disposed_delegated_session(&direct);
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    let beyond_session_revision = completed.trace().revision().get() as i64 + 1;
    let changed = connection
        .execute(
            "UPDATE v2_terminal_dispositions SET terminal_session_revision=?1 WHERE session_id=?2",
            rusqlite::params![
                beyond_session_revision,
                completed.trace().session_id().as_str()
            ],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let (attempt_id, session_id, receipt_id, receipt_json): (String, String, String, String) =
        connection
            .query_row(
                "SELECT attempt_id,session_id,receipt_id,receipt_json FROM v2_delegation_receipts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
    let mut forged: serde_json::Value = serde_json::from_str(&receipt_json).unwrap();
    forged["workspace_uuid"] = serde_json::json!("00000000-0000-4000-8000-000000000499");
    let stored = podway_store::DelegationReceiptV1::new(forged)
        .unwrap()
        .canonical_json()
        .to_owned();
    connection
        .execute("DELETE FROM v2_delegation_receipts", [])
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_delegation_receipts(attempt_id,session_id,receipt_id,receipt_json) VALUES (?1,?2,?3,?4)",
            rusqlite::params![attempt_id, session_id, receipt_id, stored],
        )
        .unwrap();
    drop(connection);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold.read(deadline(), |snapshot| snapshot.view(UnixMillis::new(400)));
    let report = accountant.finish();
    assert!(matches!(
        refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    let disposition_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("FROM v2_terminal_dispositions"))
        .map(|(_, counts)| counts.executes)
        .sum();
    let window_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("ORDER BY a.trace_sequence DESC"))
        .map(|(_, counts)| counts.executes)
        .sum();
    assert_eq!(
        disposition_executes, 1,
        "exactly the loader's disposition admission must run before failing"
    );
    assert_eq!(
        window_executes, 0,
        "the receipt window must not run while disposition admission fails"
    );
    drop(root);

    // (ii) Disposition corruption paired with a corrupted persisted cursor
    // (v2_task_sessions.latest_trace_sequence incremented past the trace's
    // real latest sequence, the same column the supported corruption helper
    // uses): the cursor comparison at the end of the loader is in-memory, so
    // the ordering evidence is the executed disposition SELECT itself. If
    // the cursor check ever ran first, the disposition statement would not
    // appear in the refused read's inventory.
    let (root, direct) = fixture(&super::int_v2sdg004_receipt::identity());
    let completed = disposed_delegated_session(&direct);
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    let beyond_session_revision = completed.trace().revision().get() as i64 + 1;
    let changed = connection
        .execute(
            "UPDATE v2_terminal_dispositions SET terminal_session_revision=?1 WHERE session_id=?2",
            rusqlite::params![
                beyond_session_revision,
                completed.trace().session_id().as_str()
            ],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let cursor_changed = connection
        .execute(
            "UPDATE v2_task_sessions SET latest_trace_sequence = latest_trace_sequence + 1 \
             WHERE activity='current'",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(cursor_changed, 1);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold.read(deadline(), |snapshot| snapshot.view(UnixMillis::new(401)));
    let report = accountant.finish();
    assert!(matches!(
        refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    let disposition_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("FROM v2_terminal_dispositions"))
        .map(|(_, counts)| counts.executes)
        .sum();
    assert_eq!(
        disposition_executes, 1,
        "disposition admission must still run exactly once before the cursor comparison"
    );
}
