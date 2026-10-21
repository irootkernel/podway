//! READS-002 validation-order and read-only persistence regressions.
//!
//! The pre-change characterization pins typed winners, validator stages,
//! citation routing and collection boundaries. Bulk acquisition may change
//! SQL executions and scalar-lookahead rows; fallback duplicates remain
//! counted honestly. Canonical-schema SQL injections use real Store reads,
//! observed/unobserved parity, preimage and no-sidecar assertions.

use podway_core::UnixMillis;
use podway_store::direct_store::DirectStoreV1;
use podway_store::test_support::query_account::QueryAccountant;
use serde_json::json;

use super::int_reads001_query_inventory::{
    attempt_uuid, reads001_fixture, reads001_session, reads001_snapshot, recipe_small,
};
use super::int_v2dfc005_direct_store::{deadline, digest, store};
use super::int_v2dfc008_persistence::{archive, image, seed};

fn sidecar_exists(direct: &DirectStoreV1) -> bool {
    direct.layout().shm().exists()
}

fn cold_current_graph(
    direct: &DirectStoreV1,
) -> Result<Option<podway_store::GraphSessionStateV2>, podway_store::direct_store::DirectStoreErrorV1>
{
    direct.read(deadline(), |snapshot| snapshot.current_graph())
}

fn executes_of(
    report: &podway_store::test_support::query_account::QueryReport,
    needle: &str,
) -> u64 {
    report
        .sql
        .iter()
        .filter(|(key, _)| key.contains(needle))
        .map(|(_, counts)| counts.executes)
        .sum()
}

/// The pinned row-limit concern on the actual OLD Store path, at the exact
/// COLD boundary position: a legal oversized `item_id` (injected under the
/// raw connection's default length limit; canonical DDL has no id length
/// CHECK) that is the FIRST row of the IMMEDIATE successor attempt
/// (index 1) in PK/BINARY delivery order (leading `!` byte sorts before every
/// `item-*` id), plus a deterministic blocker fault on attempt 0. A future
/// cold streaming statement would peek attempt 1's oversized first row at
/// attempt 0's collection boundary — before attempt 0's blocker stage. The
/// old per-attempt loader keeps the blocker winner: attempt 1's slot
/// statement never runs.
#[test]
fn oversize_later_item_id_keeps_earlier_blocker_winner_old_store() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 150, 250);
    // Healthy control: the unmodified fixture loads successfully.
    let healthy = store(root.path());
    assert!(cold_current_graph(&healthy).unwrap().is_some());
    drop(healthy);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_item_slots SET item_id=?2 WHERE attempt_id=?1 AND item_id='item-0'",
                rusqlite::params![
                    attempt_uuid(150, 1).as_str(),
                    format!("{}{}", "!", "a".repeat(2_000_000)),
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE v2_blockers SET reason=?2 WHERE attempt_id=?1",
                rusqlite::params![attempt_uuid(150, 0).as_str(), "z".repeat(5_000)],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Blocker,
            }
        ))
    ));
    // Bulk acquisition steps the successor's oversized row, retires, and
    // re-reads attempt 0 inside the same validation scope. Its blocker
    // fault still wins before attempt 1's own validation turn.
    assert_eq!(executes_of(&report, "value_json IS NOT NULL"), 2);
    assert_eq!(executes_of(&report, "FROM v2_blockers"), 1);
    assert_eq!(report.retention.slot_scopes, 1);
    assert_eq!(
        executes_of(
            &report,
            "criterion_ordinal, statement FROM v2_goal_criteria"
        ),
        0
    );
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// Healthy-control side of the row-limit concern: the same oversized
/// first-in-attempt `item_id` on the immediate successor (attempt 1)
/// surfaces at its OWN attempt's slot statement as a deterministic
/// data-length failure. SQLITE_TOOBIG is not in the persisted-data code
/// list, so the old path maps it through the default arm:
/// `StorageUnavailableV1 { reason: StorageIo }` — the actual typed variant
/// is preserved, not remapped to a corruption kind.
#[test]
fn oversize_item_id_alone_surfaces_at_own_attempt_statement() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 151, 251);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_item_slots SET item_id=?2 WHERE attempt_id=?1 AND item_id='item-0'",
                rusqlite::params![
                    attempt_uuid(151, 1).as_str(),
                    format!("{}{}", "!", "a".repeat(2_000_000)),
                ],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::StorageUnavailableV1 {
                reason: podway_store::StoreUnavailableReasonV1::StorageIo,
            }
        ))
    ));
    // Attempt 0 loaded; attempt 1's own slot statement initiated and failed
    // on its first (oversized) row. Attempt 0's scope entered and
    // committed; attempt 1's scope releases its partial state.
    assert_eq!(executes_of(&report, "value_json IS NOT NULL"), 3);
    assert_eq!(report.retention.slot_scopes, 2);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// Orphan criterion at absent revision 0 (inserted with foreign keys off on
/// the injection connection; canonical schema unchanged): the old loader
/// selects criteria per loaded revision, so the orphan is invisible until
/// the terminal COUNT cross-check refuses AFTER criteria and goal
/// assessments.
#[test]
fn orphan_revision0_criterion_refuses_at_terminal_count_after_assessments() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 152, 252);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        // Injection connection: canonical schema unchanged; foreign-key
        // enforcement is disabled exactly as the row-limit/orphan concern
        // describes (rusqlite enables it by default).
        connection
            .execute_batch("PRAGMA foreign_keys=OFF;")
            .unwrap();
        let session: String = connection
            .query_row(
                "SELECT session_id FROM v2_task_sessions WHERE activity='current'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let changed = connection
            .execute(
                "INSERT INTO v2_goal_criteria (session_id, goal_revision, criterion_id, criterion_ordinal, statement) \
                 VALUES (?1, 0, 'crit-orphan', 0, 'orphaned criterion')",
                [&session],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    // Stage evidence: both revisions' criteria, all results and their
    // citations, and the goal assessments all loaded; the terminal COUNT
    // ran once and refused. (Goal assessments are one session-scoped
    // statement delivering both assessed attempts' rows.)
    assert_eq!(
        executes_of(
            &report,
            "criterion_ordinal, statement FROM v2_goal_criteria"
        ),
        1
    );
    assert_eq!(
        executes_of(&report, "v2_criterion_assessment_results r JOIN"),
        1
    );
    assert_eq!(executes_of(&report, "citation_ordinal, citation_kind"), 1);
    assert_eq!(
        executes_of(&report, "record_digest FROM v2_goal_assessments"),
        1
    );
    assert_eq!(executes_of(&report, "AS citation"), 1);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// The same orphan plus an earlier citation domain fault: the citation
/// fault keeps the original winner and the terminal COUNT never runs.
#[test]
fn orphan_criterion_with_earlier_citation_fault_keeps_assessment_winner() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 153, 253);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys=OFF;")
            .unwrap();
        let session: String = connection
            .query_row(
                "SELECT session_id FROM v2_task_sessions WHERE activity='current'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO v2_goal_criteria (session_id, goal_revision, criterion_id, criterion_ordinal, statement) \
                 VALUES (?1, 0, 'crit-orphan', 0, 'orphaned criterion')",
                [&session],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE v2_criterion_citations SET citation_ordinal=5 \
                 WHERE attempt_id=?1 AND criterion_id='crit-0' AND citation_ordinal=0",
                [attempt_uuid(153, 1).as_str()],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    // The first result's citation query ran and its ordinal check failed;
    // the terminal COUNT never started.
    assert_eq!(executes_of(&report, "citation_ordinal, citation_kind"), 1);
    assert_eq!(executes_of(&report, "AS citation"), 0);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// Mixed-revision ordinal collision: one attempt holds a revision-1 and a
/// revision-2 result that tie on criterion_ordinal 0. Both results' own
/// pre-citation checks, citations and post checks complete before the
/// grouped mixed-revision rejection fires.
#[test]
fn mixed_revision_ordinal_collision_runs_both_results_citations_before_rejection() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 154, 254);
    let healthy = store(root.path());
    assert!(cold_current_graph(&healthy).unwrap().is_some());
    drop(healthy);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        // Swap revision 2's ordinals through an unused temporary ordinal;
        // each individual UPDATE respects the canonical UNIQUE constraint.
        connection
            .execute_batch(
                "UPDATE v2_goal_criteria SET criterion_ordinal=2 \
                 WHERE goal_revision=2 AND criterion_id='crit-0'; \
                 UPDATE v2_goal_criteria SET criterion_ordinal=0 \
                 WHERE goal_revision=2 AND criterion_id='crit-1'; \
                 UPDATE v2_goal_criteria SET criterion_ordinal=1 \
                 WHERE goal_revision=2 AND criterion_id='crit-0';",
            )
            .unwrap();
        let changed = connection
            .execute(
                "UPDATE v2_criterion_assessment_results SET goal_revision=2 \
                 WHERE attempt_id=?1 AND criterion_id='crit-1'",
                [attempt_uuid(154, 1).as_str()],
            )
            .unwrap();
        assert_eq!(changed, 1);
        let mut statement = connection
            .prepare(
                "SELECT a.trace_sequence, c.criterion_ordinal, r.goal_revision, r.rowid \
                 FROM v2_criterion_assessment_results r \
                 JOIN v2_attempts a ON a.attempt_id=r.attempt_id \
                 JOIN v2_goal_criteria c ON c.session_id=r.session_id \
                   AND c.goal_revision=r.goal_revision AND c.criterion_id=r.criterion_id \
                 WHERE r.attempt_id=?1 ORDER BY r.rowid",
            )
            .unwrap();
        let tied = statement
            .query_map([attempt_uuid(154, 1).as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tied.len(), 2);
        assert_eq!((tied[0].0, tied[0].1), (tied[1].0, tied[1].1));
        assert_eq!(tied[0].1, 0);
        assert_ne!(tied[0].2, tied[1].2);
        assert_ne!(tied[0].3, tied[1].3);
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    // Stage evidence: both tied results' citation validators completed before
    // the grouped rejection; the terminal COUNT never started. The
    // validator-stage entries pin the same boundary: two pulls and two
    // post-citation-check entries completed before the rejection.
    assert_eq!(executes_of(&report, "citation_ordinal, citation_kind"), 1);
    assert_eq!(report.retention.citation_pull_entries, 2);
    assert_eq!(report.retention.citation_post_check_entries, 2);
    assert_eq!(executes_of(&report, "AS citation"), 0);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// A later result row's TEXT conversion (collection stage) precedes an
/// earlier result's citation fault: invalid UTF-8 in a later row's reason
/// refuses during result collection, before ANY citation statement runs.
#[test]
fn later_result_row_conversion_precedes_earlier_result_citation_fault() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 155, 255);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_criterion_citations SET citation_ordinal=5 \
                 WHERE attempt_id=?1 AND criterion_id='crit-0' AND citation_ordinal=0",
                [attempt_uuid(155, 1).as_str()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE v2_criterion_assessment_results SET reason=CAST(x'22ff22' AS TEXT) \
                 WHERE attempt_id=?1",
                [attempt_uuid(155, 3).as_str()],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    // No citation statement ran: the collection-stage conversion won. The
    // validator-stage entries agree: zero citation-pull entries.
    assert_eq!(executes_of(&report, "citation_ordinal, citation_kind"), 0);
    assert_eq!(report.retention.citation_pull_entries, 0);
    assert_eq!(report.retention.citation_post_check_entries, 0);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// A result's post-citation domain fault (negative recorded_at) precedes
/// the NEXT result's citation validator: only the faulty result's own
/// citation Strings convert before the refusal.
#[test]
fn same_result_post_checks_precede_next_group_citations() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 156, 256);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_criterion_assessment_results SET recorded_at_ms=-1 \
                 WHERE attempt_id=?1 AND criterion_id='crit-0'",
                [attempt_uuid(156, 1).as_str()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE v2_criterion_citations SET citation_ordinal=5 \
                 WHERE attempt_id=?1 AND criterion_id='crit-1' AND citation_ordinal=0",
                [attempt_uuid(156, 1).as_str()],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    // Only crit-0's citation validator ran; the bulk boundary peek does
    // not convert crit-1's citation Strings.
    // Validator-stage entries: one pull and one post-citation-check entry
    // (the recorded_at domain fault fires after the post entry). An eager
    // next-group boundary String conversion would leave the post entry at
    // zero.
    assert_eq!(executes_of(&report, "citation_ordinal, citation_kind"), 1);
    assert_eq!(report.retention.citation_pull_entries, 1);
    assert_eq!(report.retention.citation_post_check_entries, 1);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// Cross-session result tie: a result row whose session_id is the current
/// session while its attempt_id references an archived work-chain session's
/// attempt sharing trace_sequence 2, tied with the current session's own
/// ordinal-0 revision-1 result. The foreign attempt carries no results of
/// its own, so the injected row is insertable under the global results PK.
/// Pins the characterized visible set, grouping and refusal stage with
/// formal validator entries, independently of the reduced SQL count.
#[test]
fn cross_session_result_row_tie_characterizes_actual_visibility() {
    let identity = podway_store::DurableWorktreeIdentityV1::new(
        digest('a'),
        podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000000157").unwrap(),
        digest('b'),
    );
    let (root, direct) = super::int_v2dfc008_persistence::fixture(&identity);
    // Archived foreign work-chain session A: attempts at traces 1-2, no
    // criterion results of its own.
    let chain = [2usize, 2];
    let snapshot_a = super::int_reads002_baseline::workchain_snapshot(&chain, 9107);
    let running_a = super::int_reads002_baseline::workchain_session(&chain, &snapshot_a, 9007, 24);
    let active_a = running_a
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let completed_a = running_a
        .complete_active_action_v2(
            running_a.trace().revision(),
            &active_a,
            None,
            podway_core::UnixMillis::new(500),
        )
        .unwrap()
        .into_state();
    seed(&direct, &completed_a);
    archive(&direct, &completed_a, podway_core::UnixMillis::new(600));
    // Current session B with assessed attempts and citations.
    let recipe = recipe_small();
    let snapshot_b = reads001_snapshot(&recipe, 9108);
    let state_b = reads001_session(&recipe, &snapshot_b, 9008, 0, false);
    seed(&direct, &state_b);
    let foreign_attempt = super::int_reads002_baseline::chain_attempt_uuid(9007, 1);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys=OFF;")
            .unwrap();
        let session_b: String = connection
            .query_row(
                "SELECT session_id FROM v2_task_sessions WHERE activity='current'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let changed = connection
            .execute(
                "INSERT INTO v2_criterion_assessment_results \
                 (attempt_id, session_id, goal_revision, criterion_id, status, mode, reason, actor, recorded_at_ms) \
                 VALUES (?1, ?2, 1, 'crit-0', 'satisfied', 'assessment', 'Injected cross-session tie.', NULL, 101)",
                rusqlite::params![foreign_attempt.as_str(), session_b],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
    // Characterized actual behavior: the foreign-tied row IS visible to the
    // results query; all five result rows' citations loaded (four current
    // results plus the injected tie) before the load refused, and the
    // terminal COUNT never ran. The validator-stage entries pin all five
    // citation stages AND all five post-citation-check entries before the
    // later refusal.
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    assert_eq!(
        executes_of(&report, "v2_criterion_assessment_results r JOIN"),
        1
    );
    assert_eq!(executes_of(&report, "citation_ordinal, citation_kind"), 1);
    assert_eq!(report.retention.citation_pull_entries, 5);
    assert_eq!(report.retention.citation_post_check_entries, 5);
    assert_eq!(executes_of(&report, "AS citation"), 0);
    println!(
        "READS002_BASELINE|cross-session-tie|outcome=refused|results_executes=1|citation_executes=1|count_executes=0"
    );
}

/// Cold UTF-8 collection order: an earlier row's bad payload beats a later
/// row's bad item_id — the old mapper converts whole rows in delivery
/// (item_id-ASC) order, so item-0's invalid-UTF-8 value_json wins over
/// item-1's invalid-UTF-8 id.
#[test]
fn cold_slot_earlier_bad_payload_beats_later_bad_item_id() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 158, 258);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_item_slots SET value_json=CAST(x'22ff22' AS TEXT) \
                 WHERE attempt_id=?1 AND item_id='item-0'",
                [attempt_uuid(158, 1).as_str()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE v2_item_slots SET item_id=CAST(x'ff' AS TEXT) \
                 WHERE attempt_id=?1 AND item_id='item-1'",
                [attempt_uuid(158, 1).as_str()],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Item,
            }
        ))
    ));
    // Window-cumulative decode attempts: attempt 0's two payloads decoded
    // before attempt 1's collection faulted on item-0's invalid UTF-8
    // value; the corrupt payload itself never reaches a decode attempt.
    assert_eq!(report.retention.decode_attempts, 2);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// An invalid-UTF-8 item_id alone refuses at its own row during
/// collection, before any payload decode of that attempt.
#[test]
fn cold_slot_bad_item_id_refuses_at_its_own_row() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 159, 259);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_item_slots SET item_id=CAST(x'ff' AS TEXT) \
                 WHERE attempt_id=?1 AND item_id='item-1'",
                [attempt_uuid(159, 1).as_str()],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Item,
            }
        ))
    ));
    // Attempt 0's two payloads decoded; attempt 1's collection faulted at
    // its own row before any of its payloads decoded.
    assert_eq!(report.retention.decode_attempts, 2);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// Intra-result collection boundary made observable (C2): one result's
/// citations are [ordinal 1 (missing 0), ordinal 2 with an invalid-UTF-8
/// item_id]. The old per-result loader collects BOTH rows through the
/// mapper before any ordinal check runs, so the citation statement
/// delivers two rows and the invalid-UTF-8 conversion wins. An
/// implementation rejecting the ordinal gap after the first row would
/// deliver only one row — the delivered-row count is the discriminator.
/// The first baseline result (attempt 1, crit-0, the first group in
/// (trace, ordinal) order) is chosen so no earlier citation statement
/// pollutes the count.
#[test]
fn citation_collection_boundary_collects_both_rows_before_ordinal_checks() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 161, 261);
    // Healthy control: the unmodified fixture loads successfully.
    let healthy = store(root.path());
    assert!(cold_current_graph(&healthy).unwrap().is_some());
    drop(healthy);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        // Attempt 1 / crit-0 is the first baseline result and carries one
        // item citation at ordinal 0. First citation: ordinal 0 -> 1 (gap
        // at 0), valid TEXT. Second citation: a new ordinal-2 row with an
        // invalid-UTF-8 item_id in the valid item-kind shape.
        connection
            .execute(
                "UPDATE v2_criterion_citations SET citation_ordinal=1 \
                 WHERE attempt_id=?1 AND criterion_id='crit-0' AND citation_ordinal=0",
                [attempt_uuid(161, 1).as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO v2_criterion_citations \
                 (attempt_id, criterion_id, citation_ordinal, citation_kind, source_graph_node_id, item_id) \
                 VALUES (?1, 'crit-0', 2, 'item', NULL, CAST(x'ff' AS TEXT))",
                [attempt_uuid(161, 1).as_str()],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Session,
            }
        ))
    ));
    // The discriminator: one citation execution delivered BOTH rows before
    // any ordinal check could reject; the terminal COUNT never ran. The
    // validator-stage entries agree: one pull entry, and zero post-check
    // entries because the refusal fires inside the citation collection.
    let citation_executes = executes_of(&report, "citation_ordinal, citation_kind");
    assert_eq!(citation_executes, 1);
    assert_eq!(report.retention.citation_pull_entries, 1);
    assert_eq!(report.retention.citation_post_check_entries, 0);
    let citation_rows: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("citation_ordinal, citation_kind"))
        .map(|(_, counts)| counts.rows)
        .sum();
    assert_eq!(citation_rows, 2, "collection reaches both rows first");
    assert_eq!(executes_of(&report, "AS citation"), 0);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// Invalid UTF-8 inside the evidence family's selected-items JSON (SQL
/// injectable under `json_valid` on the pinned SQLite) refuses at the
/// evidence stage of the owning attempt, after that attempt's slots and
/// blockers loaded.
#[test]
fn evidence_selected_ids_invalid_utf8_refuses_at_evidence_stage() {
    let recipe = recipe_small();
    let (root, direct, _state) = reads001_fixture(&recipe, 160, 260);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_resolved_evidence_references SET selected_item_ids_json=CAST(x'5b22ff225d' AS TEXT) \
                 WHERE attempt_id=?1",
                [attempt_uuid(160, 3).as_str()],
            )
            .unwrap();
    }
    let after_injection = image(&direct);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Attempt,
            }
        ))
    ));
    // One bulk cursor per family; the fourth slot validation scope
    // completes collection before attempt 3's evidence conversion fails.
    assert_eq!(executes_of(&report, "value_json IS NOT NULL"), 1);
    assert_eq!(
        executes_of(&report, "FROM v2_resolved_evidence_references"),
        1
    );
    assert_eq!(report.retention.slot_scopes, 4);
    let unobserved = store(root.path());
    let unobserved = cold_current_graph(&unobserved);
    assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
    assert_eq!(image(&cold), after_injection);
    assert!(!sidecar_exists(&cold));
}

/// A bulk sorter or boundary step can encounter a later owner's TooBig
/// before the current owner's domain check. Retiring acquisition must
/// restore that check's original priority, including citation postchecks.
#[test]
fn bulk_length_failures_preserve_current_owner_validation() {
    for case in 0..3u64 {
        let (root, direct, _state) = reads001_fixture(&recipe_small(), 170 + case, 270 + case);
        {
            let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
            match case {
                0 => {
                    connection
                        .execute(
                            "UPDATE v2_blockers SET reason=?1 WHERE attempt_id=?2",
                            rusqlite::params!["x".repeat(5_000), attempt_uuid(170, 0).as_str()],
                        )
                        .unwrap();
                    connection.execute(
                        "INSERT INTO v2_blockers (blocker_id,attempt_id,reason,state,created_at_ms,resolved_at_ms) \
                         VALUES ('00000000-0000-4000-a000-000000000999',?1,?2,'open',101,NULL)",
                        rusqlite::params![attempt_uuid(170, 1).as_str(), "x".repeat(2_000_000)],
                    ).unwrap();
                }
                1 => {
                    connection.execute(
                        "UPDATE v2_goal_criteria SET statement=?1 WHERE goal_revision=1 AND criterion_ordinal=0",
                        ["x".repeat(5_000)],
                    ).unwrap();
                    connection.execute(
                        "UPDATE v2_goal_criteria SET statement=?1 WHERE goal_revision=2 AND criterion_ordinal=0",
                        ["x".repeat(2_000_000)],
                    ).unwrap();
                }
                2 => {
                    connection
                        .execute(
                            "UPDATE v2_criterion_assessment_results SET recorded_at_ms=-1 \
                         WHERE attempt_id=?1 AND criterion_id='crit-0'",
                            [attempt_uuid(172, 1).as_str()],
                        )
                        .unwrap();
                    connection
                        .execute(
                            "UPDATE v2_criterion_citations SET item_id=?1 \
                         WHERE attempt_id=?2 AND criterion_id='crit-1'",
                            rusqlite::params!["x".repeat(2_000_000), attempt_uuid(172, 1).as_str()],
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
        }
        let after_injection = image(&direct);
        let cold = store(root.path());
        let accountant = QueryAccountant::start();
        let refused = cold_current_graph(&cold);
        let report = accountant.finish();
        let expected = if case == 0 {
            podway_store::StoreRecordKindV1::Blocker
        } else {
            podway_store::StoreRecordKindV1::Session
        };
        assert!(
            matches!(
                &refused,
                Err(podway_store::direct_store::DirectStoreErrorV1::State(
                    podway_store::StoreErrorV1::CorruptStateV1 { record }
                )) if record == &expected
            ),
            "case {case}: {refused:?}"
        );
        let needle = match case {
            0 => "FROM v2_blockers",
            1 => "criterion_ordinal, statement FROM v2_goal_criteria",
            2 => "citation_ordinal, citation_kind",
            _ => unreachable!(),
        };
        assert_eq!(executes_of(&report, needle), 2, "bulk plus owner fallback");
        if case == 2 {
            assert_eq!(report.retention.citation_pull_entries, 1);
            assert_eq!(report.retention.citation_post_check_entries, 1);
        }
        let unobserved = cold_current_graph(&store(root.path()));
        assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
        assert_eq!(image(&cold), after_injection);
        assert!(!sidecar_exists(&cold));
        println!("READS002_FALLBACK|case={case}|family_executes=2|winner={expected:?}");
    }
}

/// Defined citation stages on real tied results, independent of whichever
/// tied result the original SQL chose first. Acquired boundary rows remain
/// visible in the SQL counters; they are not owner-converted rows.
#[test]
fn tied_result_multifaults_preserve_collection_postchecks_and_future_boundaries() {
    for case in 0..3u64 {
        let session = 190 + case;
        let (root, direct, _state) = reads001_fixture(&recipe_small(), session, 290 + case);
        assert!(cold_current_graph(&store(root.path())).unwrap().is_some());
        {
            let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
            let attempt = attempt_uuid(session, 1);
            let ids = if case < 2 {
                connection.execute_batch(
                    "UPDATE v2_goal_criteria SET criterion_ordinal=2 WHERE goal_revision=2 AND criterion_id='crit-0'; \
                     UPDATE v2_goal_criteria SET criterion_ordinal=0 WHERE goal_revision=2 AND criterion_id='crit-1'; \
                     UPDATE v2_goal_criteria SET criterion_ordinal=1 WHERE goal_revision=2 AND criterion_id='crit-0';"
                ).unwrap();
                assert_eq!(connection.execute(
                    "UPDATE v2_criterion_assessment_results SET goal_revision=2 WHERE attempt_id=?1 AND criterion_id='crit-1'",
                    [attempt.as_str()],
                ).unwrap(), 1);
                if case == 0 {
                    assert_eq!(connection.execute(
                        "UPDATE v2_criterion_citations SET citation_ordinal=1 WHERE attempt_id=?1",
                        [attempt.as_str()],
                    ).unwrap(), 2);
                    assert_eq!(connection.execute(
                        "INSERT INTO v2_criterion_citations \
                         (attempt_id,criterion_id,citation_ordinal,citation_kind,source_graph_node_id,item_id) \
                         SELECT attempt_id,criterion_id,2,'item',NULL,CAST(x'ff' AS TEXT) \
                         FROM v2_criterion_assessment_results WHERE attempt_id=?1",
                        [attempt.as_str()],
                    ).unwrap(), 2);
                } else {
                    assert_eq!(connection.execute(
                        "UPDATE v2_criterion_assessment_results SET recorded_at_ms=-1 WHERE attempt_id=?1",
                        [attempt.as_str()],
                    ).unwrap(), 2);
                }
                ["crit-0", "crit-1"]
            } else {
                // A strictly earlier ordinal-0 result must finish its own
                // postcheck before either future ordinal-1 tied group's
                // first citation String is converted.
                connection.execute(
                    "UPDATE v2_goal_criteria SET criterion_ordinal=2 WHERE goal_revision=2 AND criterion_id='crit-1'",
                    [],
                ).unwrap();
                connection.execute(
                    "INSERT INTO v2_goal_criteria (session_id,goal_revision,criterion_id,criterion_ordinal,statement) \
                     SELECT session_id,2,'crit-2',1,'Future tied criterion.' FROM v2_task_sessions WHERE activity='current'",
                    [],
                ).unwrap();
                connection.execute(
                    "INSERT INTO v2_criterion_assessment_results \
                     (attempt_id,session_id,goal_revision,criterion_id,status,mode,reason,actor,recorded_at_ms) \
                     SELECT ?1,session_id,2,'crit-2','satisfied','assessment','Future tied result.',NULL,101 \
                     FROM v2_task_sessions WHERE activity='current'",
                    [attempt.as_str()],
                ).unwrap();
                connection.execute(
                    "INSERT INTO v2_criterion_citations \
                     (attempt_id,criterion_id,citation_ordinal,citation_kind,source_graph_node_id,item_id) \
                     VALUES (?1,'crit-2',0,'item',NULL,CAST(x'ff' AS TEXT))",
                    [attempt.as_str()],
                ).unwrap();
                assert_eq!(
                    connection
                        .execute(
                            "UPDATE v2_criterion_citations SET item_id=CAST(x'ff' AS TEXT) \
                     WHERE attempt_id=?1 AND criterion_id='crit-1' AND citation_ordinal=0",
                            [attempt.as_str()],
                        )
                        .unwrap(),
                    1
                );
                connection
                    .execute(
                        "UPDATE v2_criterion_assessment_results SET recorded_at_ms=-1 \
                     WHERE attempt_id=?1 AND criterion_id='crit-0'",
                        [attempt.as_str()],
                    )
                    .unwrap();
                ["crit-1", "crit-2"]
            };
            let tie: (i64, i64, i64, i64, i64, i64) = connection.query_row(
                "SELECT COUNT(*),COUNT(DISTINCT r.goal_revision),COUNT(DISTINCT r.rowid), \
                 MIN(c.criterion_ordinal),MAX(c.criterion_ordinal),COUNT(DISTINCT a.trace_sequence) \
                 FROM v2_criterion_assessment_results r JOIN v2_attempts a ON a.attempt_id=r.attempt_id \
                 JOIN v2_goal_criteria c ON c.session_id=r.session_id AND c.goal_revision=r.goal_revision \
                   AND c.criterion_id=r.criterion_id \
                 WHERE r.attempt_id=?1 AND r.criterion_id IN (?2,?3)",
                rusqlite::params![attempt.as_str(), ids[0], ids[1]],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
            ).unwrap();
            let ordinal = if case == 2 { 1 } else { 0 };
            assert_eq!(tie, (2, 2, 2, ordinal, ordinal, 1));
        }
        let after_injection = image(&direct);
        let cold = store(root.path());
        let accountant = QueryAccountant::start();
        let refused = cold_current_graph(&cold);
        let report = accountant.finish();
        assert!(
            matches!(
                &refused,
                Err(podway_store::direct_store::DirectStoreErrorV1::State(
                    podway_store::StoreErrorV1::CorruptStateV1 {
                        record: podway_store::StoreRecordKindV1::Session
                    }
                ))
            ),
            "case {case}: {refused:?}"
        );
        let expected_post = u64::from(case != 0);
        assert_eq!(report.retention.citation_pull_entries, 1);
        assert_eq!(report.retention.citation_post_check_entries, expected_post);
        assert_eq!(report.retention.slot_scopes, 6);
        let rows: u64 = report
            .sql
            .iter()
            .filter(|(sql, _)| sql.contains("citation_ordinal, citation_kind"))
            .map(|(_, count)| count.rows)
            .sum();
        let bytes: u64 = report
            .sql
            .iter()
            .filter(|(sql, _)| sql.contains("citation_ordinal, citation_kind"))
            .map(|(_, count)| count.column_bytes)
            .sum();
        if case == 0 {
            assert_eq!(rows, 2, "both own rows precede ordinal validation");
            assert_eq!(bytes, 15);
        } else {
            assert!(
                (1..=2).contains(&rows),
                "a scalar-only boundary row is allowed"
            );
            assert_eq!(
                bytes,
                if rows == 1 {
                    10
                } else if case == 1 {
                    20
                } else {
                    15
                }
            );
        }
        let unobserved = cold_current_graph(&store(root.path()));
        assert_eq!(format!("{refused:?}"), format!("{unobserved:?}"));
        assert_eq!(image(&cold), after_injection);
        assert!(!sidecar_exists(&cold));
        println!(
            "READS002_TIE_ORACLE|case={case}|pull=1|post={expected_post}|citation_rows={rows}|column_bytes={bytes}|winner=Session"
        );
    }
}

/// Compatible incremental List-Add construction near the 1M-scalar domain
/// bound: one list grows to its 1,000,000-scalar limit through 125 real
/// `Add` mutations, each an acknowledged domain-success mutation from an
/// empty seeded state. The prepared graph predates all growth, so the mixed
/// reload revalidates the near-cap serialized value through one header
/// execution and one padded chunk — the real-Store
/// complement to the SQL-only allocator diagnostic.
#[test]
fn mixed_reload_survives_incremental_list_add_near_domain_cap() {
    use super::int_v2dfc008_persistence::{fixture, seed};
    use podway_core::workflow_v2::{ActiveItemMutationRequestV2, ActiveItemMutationV2};
    use podway_core::{
        AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, CanonicalProcedureJsonV1,
        GraphNodeId, ItemId, ItemTypeV1, ProcedureSnapshotId, ProcedureSourceLabelV1, Revision,
        SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis,
        WorkspaceId, canonicalize_json_v1,
    };
    use podway_store::{
        AttemptMetadataV2, AttemptWorkflowMemoryV2, DurableWorktreeIdentityV1, GraphNodeCounterV2,
        GraphSessionStateV2, ItemSlotStateV2, WorkflowMemoryStateV2,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    const ITEMS: usize = 1;
    const WAVES: usize = 125;
    const ENTRY_CHARS: usize = 8_000;
    let identity = DurableWorktreeIdentityV1::new(
        super::int_v2dfc005_direct_store::digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000162").unwrap(),
        super::int_v2dfc005_direct_store::digest('b'),
    );
    let (root, direct) = fixture(&identity);
    let list_items: Vec<_> = (0..ITEMS)
        .map(|index| {
            json!({
                "id": format!("item-{index}"),
                "type": "list",
                "prompt": "Incremental growth.",
                "required": false,
                "max_items": 1_000,
                "max_item_length": 8_192,
                "max_total_length": 1_000_000
            })
        })
        .collect();
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "reads002-listadd",
        "version": "1",
        "name": "READS-002 list-add",
        "purpose": "Compatible incremental List-Add growth.",
        "node_definitions": {
            "work-0": {"type": "action", "title": "W0", "intent": "W", "items": list_items},
            "work-1": {"type": "action", "title": "W1", "intent": "W", "items": list_items}
        },
        "graph": {"entry": "n0", "nodes": [
            {"id": "n0", "use": "work-0", "next": "n1"},
            {"id": "n1", "use": "work-1", "terminal": true}
        ]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest_value =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000262").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest_value,
        ProcedureSourceLabelV1::file("listadd.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap();
    // Seeded state: attempt 0 ACTIVE on n0 with every list slot empty
    // (revision 0, no value). The prepared graph predates all growth.
    let session = 162usize;
    let attempt = |index: usize| {
        let value = ((session as u64) << 24) | (index as u64 & 0xff_ffff);
        AttemptId::new(format!("00000000-0000-4000-8000-{value:012x}")).unwrap()
    };
    let node = GraphNodeId::new("n0").unwrap();
    let slots: Vec<_> = (0..ITEMS)
        .map(|index| {
            ItemSlotStateV2::new(
                attempt(0),
                ItemId::new(format!("item-{index}")).unwrap(),
                ItemTypeV1::List,
                Revision::ZERO,
                None,
                UnixMillis::new(100),
                UnixMillis::new(100),
            )
            .unwrap()
        })
        .collect();
    let state = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        "READS-002 list-add",
        snapshot.clone(),
        SessionTraceV2::from_parts(
            SessionId::new(format!("00000000-0000-4000-8000-{session:012x}")).unwrap(),
            SessionLifecycle::Running,
            Revision::new(1),
            vec![
                podway_core::SessionAttemptV2::new(
                    attempt(0),
                    node.clone(),
                    AttemptNumberV2::FIRST,
                    TraceSequenceV2::new(1),
                    AttemptLifecycle::Active,
                    AttemptValidityV2::Valid,
                    None,
                )
                .unwrap(),
            ],
        )
        .unwrap(),
        vec![
            GraphNodeCounterV2::new(node, 1, 0),
            GraphNodeCounterV2::new(GraphNodeId::new("n1").unwrap(), 0, 0),
        ],
        vec![AttemptMetadataV2::new(attempt(0), UnixMillis::new(100), None, None).unwrap()],
        WorkflowMemoryStateV2::new(
            vec![AttemptWorkflowMemoryV2::new(attempt(0), slots, Vec::new(), Vec::new()).unwrap()],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        UnixMillis::new(90),
        None,
        None,
        None,
    )
    .unwrap();
    seed(&direct, &state);
    let accountant = QueryAccountant::start();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    accountant.finish();
    // Compatible incremental growth: WAVES domain mutations, each a batch
    // of <=128 single-entry Add mutations (one per item, duplicates
    // rejected by the domain API), persisted through replace_graph. Every
    // wave is an acknowledged success.
    let mut current = state.clone();
    for wave in 0..WAVES {
        let mutations: Vec<ActiveItemMutationRequestV2> = (0..ITEMS)
            .map(|index| {
                let slot = current
                    .workflow_memory()
                    .attempts()
                    .iter()
                    .find(|memory| memory.attempt_id() == &attempt(0))
                    .unwrap()
                    .item_slots()
                    .iter()
                    .find(|slot| slot.item_id().as_str() == format!("item-{index}"))
                    .unwrap();
                ActiveItemMutationRequestV2::new(
                    ItemId::new(format!("item-{index}")).unwrap(),
                    slot.revision(),
                    ActiveItemMutationV2::Add {
                        value: format!("{wave:03}{}", "e".repeat(ENTRY_CHARS - 3)),
                    },
                )
            })
            .collect();
        let next = current
            .mutate_active_items_v2(&attempt(0), &mutations, UnixMillis::new(200 + wave as u64))
            .unwrap()
            .into_state();
        let mutation = super::int_v2dfc005_direct_store::request(&format!("listadd-w{wave}"));
        direct
            .mutate(
                &mutation,
                deadline(),
                || UnixMillis::new(200 + wave as u64),
                |transaction| {
                    transaction.now();
                    transaction.replace_graph(
                        current.workspace_revision(),
                        current.trace().revision(),
                        &next,
                    )?;
                    Ok(super::int_v2dfc005_direct_store::success(
                        &mutation,
                        json!({"wave": wave}),
                        transaction.now(),
                    ))
                },
            )
            .unwrap();
        current = next;
    }
    let aggregate_scalars = ITEMS * WAVES * ENTRY_CHARS;
    assert_eq!(
        aggregate_scalars,
        podway_core::MAX_LIST_TOTAL_SCALARS_V2 as usize
    );
    // The complete serialized record, rather than a sum over many small
    // records, is near the SQLite row limit on the real Store connection.
    let cold = store(root.path());
    assert_eq!(cold_current_graph(&cold).unwrap(), Some(current.clone()));
    drop(cold);
    let accountant = QueryAccountant::start();
    let revalidate = super::int_v2dfc005_direct_store::request("listadd-revalidate");
    let refused = direct.mutate(
        &revalidate,
        deadline(),
        || UnixMillis::new(950),
        |transaction| {
            transaction.use_prepared_graph(Some(&prepared))?;
            Ok(super::int_v2dfc005_direct_store::success(
                &revalidate,
                json!({"revalidated": true}),
                transaction.now(),
            ))
        },
    );
    let report = accountant.finish();
    assert!(
        refused.is_ok(),
        "near-cap incremental list reload stays healthy"
    );
    let header_executes = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("typeof(value_json) != 'null'"))
        .map(|(_, counts)| counts.executes)
        .sum::<u64>();
    let chunk_executes = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("WITH wanted"))
        .map(|(_, counts)| counts.executes)
        .sum::<u64>();
    assert_eq!(header_executes, 1);
    assert_eq!(chunk_executes, 1);
    assert_eq!(report.retention.raw_released_at_scope_end, 0);
    println!(
        "READS002_BASELINE|incremental-list-add-near-cap|header_executes=1|chunk_executes=1|items={ITEMS}|waves={WAVES}|aggregate_scalars={aggregate_scalars}"
    );
    drop(direct);
    drop(root);
}

/// Real SQLite TooBig retirement after successful payload delivery. Every
/// fixture has more queued chunks than may execute; cached owners keep
/// already decoded slots, while a NEW owner discards its partial raw map
/// and repeats the original whole-owner collection before spec validation.
#[test]
fn mixed_chunk_late_too_big_preserves_cached_and_new_collection_modes() {
    use super::int_reads002_baseline::{
        append_wave, chain_attempt_uuid, record_wave, workchain_fixture,
    };
    use super::int_v2dfc005_direct_store::{request, success};
    use podway_store::direct_store::DirectStoreErrorV1;
    use podway_store::{StoreErrorV1, StoreUnavailableReasonV1};
    for (case, new_owner, fault_position) in [
        (0, false, 5usize),
        (1, false, 133),
        (2, true, 7),
        (3, true, 129),
    ] {
        let session = 170 + case;
        let chain = [128usize, 128, 128, 128, 1];
        let (root, direct, initial) = workchain_fixture(&chain, session, 270 + case, 24);
        let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
        let mut current = initial;
        if new_owner {
            current = record_wave(
                &direct,
                "cached-two",
                &current,
                &[(0, "b".repeat(24)), (1, "b".repeat(24))],
                890,
            );
        }
        current = append_wave(
            &direct,
            "append-two",
            &current,
            chain_attempt_uuid(session, 2),
            900,
        );
        if new_owner {
            let sets = (0..128).map(|i| (i, "c".repeat(24))).collect::<Vec<_>>();
            current = record_wave(&direct, "new-record", &current, &sets, 910);
        }
        let current = append_wave(
            &direct,
            "append-three",
            &current,
            chain_attempt_uuid(session, 3),
            920,
        );
        assert_eq!(
            cold_current_graph(&store(root.path())).unwrap(),
            Some(current)
        );
        // Keep this fixture-owned injection connection open: an existing
        // WAL/SHM must remain byte-identical across the rejected mutation
        // and its read-only parity check, with no absent-sidecar ambiguity.
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        let (owner, item) = if new_owner {
            let mut ids = (0..128).map(|i| format!("item-{i}")).collect::<Vec<_>>();
            ids.sort();
            (2, ids[fault_position - 2].clone())
        } else {
            assert_eq!(connection.execute(
                "UPDATE v2_item_slots SET item_revision=item_revision+1 WHERE attempt_id IN (?1,?2)",
                rusqlite::params![chain_attempt_uuid(session, 0).as_str(), chain_attempt_uuid(session, 1).as_str()],
            ).unwrap(), 256);
            (
                fault_position / 128,
                format!("item-{}", fault_position % 128),
            )
        };
        assert_eq!(
            connection
                .execute(
                    "UPDATE v2_item_slots SET value_json=?3 WHERE attempt_id=?1 AND item_id=?2",
                    rusqlite::params![
                        chain_attempt_uuid(session, owner).as_str(),
                        item,
                        format!(
                            "{{\"kind\":\"text\",\"value\":\"{}\"}}",
                            "x".repeat(2_000_000)
                        )
                    ],
                )
                .unwrap(),
            1
        );
        let before = image(&direct);
        let shm = sidecar_exists(&direct);
        let mutation = request("late-too-big");
        let accountant = QueryAccountant::start();
        let refused = direct.mutate(
            &mutation,
            deadline(),
            || UnixMillis::new(950),
            |transaction| {
                transaction.use_prepared_graph(Some(&prepared))?;
                Ok(success(
                    &mutation,
                    json!({"unexpected":true}),
                    transaction.now(),
                ))
            },
        );
        let report = accountant.finish();
        assert!(
            matches!(
                &refused,
                Err(DirectStoreErrorV1::State(
                    StoreErrorV1::StorageUnavailableV1 {
                        reason: StoreUnavailableReasonV1::StorageIo
                    }
                ))
            ),
            "case {case}: {refused:?}"
        );
        let chunks = report
            .sql
            .iter()
            .filter(|(sql, _)| sql.contains("WITH wanted"))
            .map(|(_, n)| n.executes)
            .sum::<u64>();
        let rows = report
            .sql
            .iter()
            .filter(|(sql, _)| sql.contains("WITH wanted"))
            .map(|(_, n)| n.rows)
            .sum::<u64>();
        assert_eq!(
            chunks,
            (fault_position / 128 + 1) as u64,
            "case {case}: no later chunk may arm"
        );
        assert_eq!(
            rows, fault_position as u64,
            "case {case}: earlier payloads really delivered"
        );
        assert_eq!(
            executes_of(
                &report,
                "SELECT value_json, created_at_ms, updated_at_ms FROM v2_item_slots WHERE"
            ),
            u64::from(!new_owner)
        );
        assert_eq!(
            executes_of(
                &report,
                "created_at_ms, updated_at_ms, value_json FROM v2_item_slots WHERE"
            ),
            u64::from(new_owner)
        );
        assert_eq!(
            report.retention.decode_attempts,
            if new_owner { 2 } else { fault_position as u64 },
            "cached payloads are not replayed; NEW collection precedes decoding"
        );
        assert_eq!(report.retention.discovery_scratch_current, 0);
        let parity = direct.read(deadline(), |snapshot| {
            assert!(snapshot.outcome(mutation.key())?.is_none());
            snapshot.current_graph()
        });
        assert_eq!(
            format!("{:?}", refused.err()),
            format!("{:?}", parity.err())
        );
        assert_eq!(image(&direct), before);
        assert_eq!(sidecar_exists(&direct), shm);
        drop(connection);
    }
}

/// Deletion is visible during discovery in the same immutable snapshot;
/// this is a spec-winner regression, not a claim that a queued row can
/// disappear between discovery and chunk consumption.
#[test]
fn mixed_missing_earlier_spec_wins_before_later_oversized_payload() {
    use super::int_reads002_baseline::{chain_attempt_uuid, record_wave, workchain_fixture};
    use super::int_v2dfc005_direct_store::{request, success};
    let (root, direct, initial) = workchain_fixture(&[3, 3, 1], 180, 280, 24);
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    // Acknowledged recording advances the owning revisions, so this is a
    // real mixed reload rather than the compatible unchanged fast path.
    record_wave(
        &direct,
        "advance-before-delete",
        &initial,
        &[(0, "b".repeat(24))],
        890,
    );
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    assert_eq!(
        connection
            .execute(
                "DELETE FROM v2_item_slots WHERE attempt_id=?1 AND item_id='item-0'",
                [chain_attempt_uuid(180, 0).as_str()]
            )
            .unwrap(),
        1
    );
    assert_eq!(connection.execute("UPDATE v2_item_slots SET item_revision=item_revision+1, value_json=?2 WHERE attempt_id=?1 AND item_id='item-1'",
        rusqlite::params![chain_attempt_uuid(180, 0).as_str(), format!("{{\"kind\":\"text\",\"value\":\"{}\"}}", "x".repeat(2_000_000))]).unwrap(), 1);
    let before = image(&direct);
    let shm = sidecar_exists(&direct);
    let mutation = request("missing-before-oversize");
    let accountant = QueryAccountant::start();
    let refused = direct.mutate(
        &mutation,
        deadline(),
        || UnixMillis::new(950),
        |transaction| {
            transaction.use_prepared_graph(Some(&prepared))?;
            Ok(success(
                &mutation,
                json!({"unexpected":true}),
                transaction.now(),
            ))
        },
    );
    let report = accountant.finish();
    assert!(
        matches!(
            &refused,
            Err(podway_store::direct_store::DirectStoreErrorV1::State(
                podway_store::StoreErrorV1::CorruptStateV1 {
                    record: podway_store::StoreRecordKindV1::Item
                }
            ))
        ),
        "{refused:?}"
    );
    assert_eq!(executes_of(&report, "WITH wanted"), 0);
    assert_eq!(
        executes_of(
            &report,
            "SELECT value_json, created_at_ms, updated_at_ms FROM v2_item_slots WHERE"
        ),
        0
    );
    assert_eq!(report.retention.decode_attempts, 0);
    direct
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(mutation.key())?.is_none());
            Ok(())
        })
        .unwrap();
    assert_eq!(image(&direct), before);
    assert_eq!(sidecar_exists(&direct), shm);
    drop(connection);
    drop(root);
}
