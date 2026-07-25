//! Test-only compatibility fixtures for Store integration consumers.

use std::path::Path;

use podway_core::JobId;
use rusqlite::{Connection, params};
use serde_json::Value;

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
