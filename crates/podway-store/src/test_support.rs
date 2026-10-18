//! Test fixtures for current direct persistence and rejected daemon-era inputs.

pub mod legacy_fixture;

use crate::{AttemptMetadataV2, GraphNodeCounterV2, GraphSessionStateV2};
use podway_core::{
    AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, CanonicalProcedureJsonV1,
    GraphNodeId, ProcedureSnapshotId, ProcedureSourceLabelV1, Revision, SessionAttemptV2,
    SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis,
    canonicalize_json_v1,
};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Native EXPLAIN output from the real, configured Store snapshot connection.
#[derive(Debug)]
pub struct ReadQueryOpcode {
    pub opcode: String,
    pub p1: i64,
    pub p2: i64,
    pub p3: i64,
    pub p4: Option<String>,
}

pub fn explain_read_query(
    snapshot: &crate::direct_store::DirectReadSnapshotV1<'_>,
    sql: &str,
) -> Result<Vec<ReadQueryOpcode>, crate::direct_store::DirectStoreErrorV1> {
    crate::direct_store::explain_read_query(snapshot, sql)
}

/// Parameter count and unchanged read settings on the real snapshot connection.
pub fn read_query_settings(
    snapshot: &crate::direct_store::DirectReadSnapshotV1<'_>,
    sql: &str,
) -> Result<(usize, bool, i32), crate::direct_store::DirectStoreErrorV1> {
    crate::direct_store::read_query_settings(snapshot, sql)
}

/// The production mixed-path session header statement text.
pub fn mixed_header_sql() -> &'static str {
    crate::v2_memory::SLOTS_HEADER_BULK_SQL
}

/// The production fixed-arity changed-slot chunk statement text.
pub fn mixed_chunk_sql() -> &'static str {
    crate::read_family::CHUNK_SQL
}

/// Actual production sizes of the compact mixed-routing records: the
/// per-row header stamp and the global pair entry.
pub fn mixed_routing_type_sizes() -> (usize, usize) {
    (
        std::mem::size_of::<crate::v2_memory::RowStamp>(),
        std::mem::size_of::<crate::v2_memory::PairEntry>(),
    )
}

/// Creates an impossible current-session cursor for read-only recovery-path tests.
pub fn corrupt_current_session_cursor(database_path: &Path) -> Result<(), String> {
    let connection = Connection::open(database_path).map_err(|error| error.to_string())?;
    let changed = connection
        .execute(
            "UPDATE v2_task_sessions SET latest_trace_sequence = latest_trace_sequence + 1 \
             WHERE activity = 'current'",
            [],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(format!("expected one current session, changed {changed}"));
    }
    Ok(())
}

/// Replaces the persisted Git directory fingerprints without changing the workspace UUID or root.
pub fn detach_git_identity(database_path: &Path) -> Result<(), String> {
    let connection = Connection::open(database_path).map_err(|error| error.to_string())?;
    let changed = connection
        .execute(
            "UPDATE workspace_state SET git_common_fingerprint = ?1, \
             git_worktree_fingerprint = ?2 WHERE singleton = 1",
            params![
                format!("sha256:{}", "1".repeat(64)),
                format!("sha256:{}", "2".repeat(64)),
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(format!("expected one workspace binding, changed {changed}"));
    }
    Ok(())
}

/// Replaces exactly one recorded slot value with arbitrary bytes for READS
/// corruption oracles, returning the changed row count (zero or one).
pub fn corrupt_recorded_slot_value<P: AsRef<std::path::Path>>(
    database_path: P,
    attempt_id: &podway_core::AttemptId,
    item_id: &str,
    replacement: &str,
) -> Result<usize, String> {
    let connection = Connection::open(database_path.as_ref()).map_err(|error| error.to_string())?;
    let changed = connection
        .execute(
            "UPDATE v2_item_slots SET value_json=?3 WHERE attempt_id=?1 AND item_id=?2",
            params![attempt_id.as_str(), item_id, replacement],
        )
        .map_err(|error| error.to_string())?;
    if changed > 1 {
        return Err(format!(
            "expected at most one recorded slot row, changed {changed}"
        ));
    }
    Ok(changed)
}

/// Running one-node session used by archive-window integration tests.
pub fn running_archive_fixture_session_v2(
    session_number: u64,
    snapshot_number: u64,
    created_at: u64,
) -> GraphSessionStateV2 {
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "archive-window",
        "version": "1",
        "name": "Archive window",
        "purpose": "Plant inactive sessions for list-window tests.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Work",
                "intent": "Work.",
                "items": [{
                    "id": "done",
                    "type": "confirm",
                    "prompt": "Done",
                    "required": false
                }]
            }
        },
        "graph": {
            "entry": "work",
            "nodes": [{
                "id": "work",
                "use": "work",
                "skip": {"allowed": true, "reason_required": true},
                "terminal": true
            }]
        }
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let uuid = |number: u64| format!("00000000-0000-4000-8000-{number:012x}");
    let snapshot = crate::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new(uuid(snapshot_number)).unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("procedure.yaml").unwrap(),
        UnixMillis::new(created_at),
    )
    .unwrap();
    let node = GraphNodeId::new("work").unwrap();
    let attempt_id = AttemptId::new(uuid(session_number + 1)).unwrap();
    let attempt = SessionAttemptV2::new(
        attempt_id.clone(),
        node.clone(),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        None,
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(uuid(session_number)).unwrap(),
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt],
    )
    .unwrap();
    GraphSessionStateV2::new(
        Revision::new(1),
        "Archive window",
        snapshot,
        trace,
        vec![GraphNodeCounterV2::new(node, 1, 0)],
        vec![AttemptMetadataV2::new(attempt_id, UnixMillis::new(created_at), None, None).unwrap()],
        UnixMillis::new(created_at),
        None,
        None,
        None,
    )
    .unwrap()
}

/// READS data-access characterization surface: scoped SQL prepare/execute/row
/// counting and logical slot-payload retention over the registered read,
/// preflight and prepared-admission call sites. These are scoped
/// measurements, not a total SQL counter; the measured call sites and the
/// registered uncovered reads are documented in the READS qualification
/// guidance.
pub mod query_account {
    use std::collections::BTreeMap;

    /// Handle for one accounting window. Created after fixture setup so
    /// setup queries stay outside the counters.
    pub struct QueryAccountant;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SqlCounts {
        pub prepares: u64,
        pub executes: u64,
        pub rows: u64,
        /// Variable-length (TEXT/BLOB) column bytes delivered through row
        /// mappers on this SQL text, over the registered call sites only.
        pub column_bytes: u64,
    }

    /// Logical retention units; byte counts are serialized payload lengths,
    /// not allocator measurements.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RetentionReport {
        pub slot_scopes: u64,
        pub raw_scope_peak: u64,
        pub raw_window_peak: u64,
        /// Nonzero only when slot scopes ended on error paths with raw rows
        /// not yet consumed; those bytes are released at scope end.
        pub raw_released_at_scope_end: u64,
        /// Cumulative fresh serialized payload bytes decoded in the window.
        pub fresh_bytes_cumulative: u64,
        /// Fresh decoded bytes held in graphs this window successfully
        /// reconstructed; failed-scope bytes are excluded. Caller-side drops
        /// of returned graphs after reconstruction are not observed.
        pub decoded_retained_in_results: u64,
        /// Whole-graph reconstruction guards opened in this window.
        pub graph_scopes: u64,
        /// Peak simultaneously live fresh decoded bytes.
        pub decoded_live_peak: u64,
        /// Fresh bytes dropped with failed scopes' partial results.
        pub decoded_released_at_scope_end: u64,
        /// Slot payload decode attempts, successful or failed.
        pub decode_attempts: u64,
        pub fresh_payloads: u64,
        pub reused_payloads: u64,
        pub reused_bytes: u64,
        pub unknown_reuse_payloads: u64,
        pub container_copies: u64,
        pub cached_input_bytes: u64,
        pub combined_peak: u64,
        /// Identity-text retention units (item id + item type String bytes
        /// materialized by the slot-family loaders); logical bytes and row
        /// counts, excluded from the combined payload peak.
        pub identity_rows: u64,
        pub identity_current: u64,
        pub identity_scope_peak: u64,
        pub identity_window_peak: u64,
        pub identity_released_at_scope_end: u64,
        /// Validator-stage entries in the criterion-assessment result loop:
        /// (a) citation-pull entries (prechecks done, pull beginning) and
        /// (b) post-citation-check entries (citations returned, reason/
        /// result/actor/time construction not yet begun).
        pub citation_pull_entries: u64,
        pub citation_post_check_entries: u64,
        /// Discovery-phase raw header scratch bytes (one owner at a time),
        /// held separately from the scope-owned identity balance so
        /// slot-scope exits never release a future failing owner's bytes.
        pub discovery_scratch_current: u64,
        pub discovery_scratch_peak: u64,
        /// Scratch never consumed by a validation turn (error unwind).
        pub discovery_scratch_released_at_abort: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct QueryReport {
        /// Per exact SQL text, over the registered call sites only.
        pub sql: BTreeMap<String, SqlCounts>,
        pub totals: SqlCounts,
        pub retention: RetentionReport,
    }

    impl QueryAccountant {
        pub fn start() -> Self {
            crate::query_accounting::ledger::start();
            Self
        }

        pub fn finish(self) -> QueryReport {
            let (sql, totals, retention) = crate::query_accounting::ledger::finish()
                .expect("query accountant window must be finished once");
            QueryReport {
                sql: sql
                    .into_iter()
                    .map(|(key, counts)| {
                        (
                            key,
                            SqlCounts {
                                prepares: counts.prepares,
                                executes: counts.executes,
                                rows: counts.rows,
                                column_bytes: counts.column_bytes,
                            },
                        )
                    })
                    .collect(),
                totals: SqlCounts {
                    prepares: totals.prepares,
                    executes: totals.executes,
                    rows: totals.rows,
                    column_bytes: totals.column_bytes,
                },
                retention: RetentionReport {
                    slot_scopes: retention.slot_scopes,
                    raw_scope_peak: retention.raw_scope_peak,
                    raw_window_peak: retention.raw_window_peak,
                    raw_released_at_scope_end: retention.raw_released_at_scope_end,
                    fresh_bytes_cumulative: retention.fresh_bytes_cumulative,
                    decoded_retained_in_results: retention.decoded_live,
                    graph_scopes: retention.graph_scopes,
                    decoded_live_peak: retention.decoded_live_peak,
                    decoded_released_at_scope_end: retention.decoded_released_at_scope_end,
                    decode_attempts: retention.decode_attempts,
                    fresh_payloads: retention.fresh_payloads,
                    reused_payloads: retention.reused_payloads,
                    reused_bytes: retention.reused_bytes,
                    unknown_reuse_payloads: retention.unknown_reuse_payloads,
                    container_copies: retention.container_copies,
                    cached_input_bytes: retention.cached_input_bytes,
                    combined_peak: retention.combined_peak,
                    identity_rows: retention.identity_rows,
                    identity_current: retention.identity_current,
                    identity_scope_peak: retention.identity_scope_peak,
                    identity_window_peak: retention.identity_window_peak,
                    identity_released_at_scope_end: retention.identity_released_at_scope_end,
                    citation_pull_entries: retention.citation_pull_entries,
                    citation_post_check_entries: retention.citation_post_check_entries,
                    discovery_scratch_current: retention.discovery_scratch_current,
                    discovery_scratch_peak: retention.discovery_scratch_peak,
                    discovery_scratch_released_at_abort: retention
                        .discovery_scratch_released_at_abort,
                },
            }
        }
    }

    /// Declares the measured payload bytes held by a caller-supplied cached
    /// input graph for the next measured window.
    pub fn declare_cached_input_bytes(bytes: u64) {
        crate::query_accounting::ledger::declare_cached_input_bytes(bytes);
    }

    /// Returns the logical serialized size recorded for a slot payload
    /// pointer, proving the input cache accounting stays complete.
    pub fn payload_bytes_of(slot: &crate::ItemSlotStateV2) -> Option<usize> {
        crate::query_accounting::ledger::payload_pointer(slot)
            .and_then(crate::query_accounting::ledger::lookup_payload_bytes)
    }

    /// Stable identity of a slot payload for pointer-sharing proofs.
    pub fn payload_pointer(slot: &crate::ItemSlotStateV2) -> Option<usize> {
        crate::query_accounting::ledger::payload_pointer(slot)
    }

    /// Drops registered payload identities; call between independent
    /// measurement scenarios so reuse provenance never depends on dangling
    /// pointers from dropped fixture graphs.
    pub fn clear_payload_registry() {
        crate::query_accounting::ledger::clear_payload_registry();
    }
}
