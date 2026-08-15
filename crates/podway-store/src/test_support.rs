//! Test-only compatibility fixtures for Store integration consumers.

use std::path::Path;

use podway_core::JobId;
use rusqlite::{Connection, params};
use serde_json::Value;

/// Restores the schema-v3 legacy tables and inserts one Procedure v1 snapshot.
pub fn downgrade_to_schema_v3_with_legacy_snapshot(
    database_path: &Path,
    legacy_schema_id: &str,
) -> Result<(), String> {
    let connection = Connection::open(database_path).map_err(|error| error.to_string())?;
    let reference = Connection::open_in_memory().map_err(|error| error.to_string())?;
    reference
        .execute_batch(crate::schema::sqlite_v1_ddl())
        .map_err(|error| error.to_string())?;
    let mut statement = reference
        .prepare(
            "SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL AND (\
             (type = 'table' AND name IN ('procedure_snapshots', 'task_sessions', \
             'stage_progress', 'attempts', 'item_slots', 'blockers')) OR \
             (type = 'index' AND name IN ('ux_stage_progress_one_current', \
             'ux_attempts_one_active', 'ix_attempts_stage', 'ix_blockers_attempt_state'))) \
             ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, name",
        )
        .map_err(|error| error.to_string())?;
    let objects = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for sql in objects {
        connection
            .execute_batch(&sql)
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute("DELETE FROM schema_migrations WHERE version = 4", [])
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "user_version", 3)
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO procedure_snapshots (
                snapshot_id, schema_id, procedure_id, procedure_version, name, digest,
                canonical_json, source_kind, source_label, created_at_ms
             ) VALUES ('legacy-reset-fixture', ?1, 'legacy-reset', '1',
                       'Legacy reset fixture', ?2, '{}', 'file', 'legacy.yaml', 1)",
            params![legacy_schema_id, format!("sha256:{}", "c".repeat(64))],
        )
        .map_err(|error| error.to_string())?;
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

/// Replaces the persisted workspace root with another valid encoded absolute path.
pub fn detach_workspace_root(database_path: &Path) -> Result<(), String> {
    let connection = Connection::open(database_path).map_err(|error| error.to_string())?;
    let changed = connection
        .execute(
            "UPDATE workspace_state SET last_validated_root = 'podway.unix-path/v1:2f746d70' \
             WHERE singleton = 1",
            [],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(format!("expected one workspace binding, changed {changed}"));
    }
    Ok(())
}

/// Rewrites one start terminal receipt to its pre-PSTRT shape in both durable copies.
pub fn rewrite_start_terminal_as_legacy(
    database_path: &Path,
    job_id: &JobId,
    legacy_v0: bool,
) -> Result<(), String> {
    let mut connection = Connection::open(database_path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let encoded: String = transaction
        .query_row(
            "SELECT terminal_response_json FROM jobs WHERE job_id = ?1",
            [job_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut legacy: Value = serde_json::from_str(&encoded).map_err(|error| error.to_string())?;
    let object = legacy
        .as_object_mut()
        .ok_or_else(|| "terminal receipt must be an object".to_owned())?;
    object.remove("start_identity");
    if legacy_v0 {
        object.insert(
            "schema".to_owned(),
            Value::String("podway.store-terminal/v0".to_owned()),
        );
        object.remove("command");
        object.remove("job_projection");
        object.remove("session_projection");
    } else {
        object
            .get_mut("session_projection")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "start terminal receipt must have a session projection".to_owned())?
            .remove("procedure_digest");
    }
    let legacy = serde_json::to_string(&legacy).map_err(|error| error.to_string())?;
    for table in ["jobs", "idempotency_records"] {
        let statement = format!("UPDATE {table} SET terminal_response_json = ?1 WHERE job_id = ?2");
        let changed = transaction
            .execute(&statement, params![legacy.as_str(), job_id.as_str()])
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err(format!(
                "expected one {table} terminal receipt, changed {changed}"
            ));
        }
    }
    transaction.commit().map_err(|error| error.to_string())
}
