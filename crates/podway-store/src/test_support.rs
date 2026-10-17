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
