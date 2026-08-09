//! Phase 2 schema and internal codec foundation contracts.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use podway_core::{
    AttemptId, DomainCommand, DomainCommandKind, DomainError, DomainResult, ItemId, JobId,
    Revision, SessionId, SessionLifecycle, Sha256Digest, UnixMillis, WorkspaceId,
    canonicalize_json_v1,
};
use podway_store::codec::{
    PersistedDomainCommandKindV1, PersistedDomainCommandV1, PersistedDomainErrorV1,
    PersistedDomainResultV1, PersistedResponseContextV1, PersistedSessionLifecycleV1,
    PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1, PersistedTerminalReceiptV1,
    PersistedTerminalResultV1, PersistedTerminalSessionProjectionV1, STORE_COMMAND_SCHEMA_V1,
    STORE_COMMAND_SCHEMA_V2, STORE_TERMINAL_SCHEMA_V0, STORE_TERMINAL_SCHEMA_V1,
    STORE_TERMINAL_SCHEMA_V2, STORE_TERMINAL_SCHEMA_V3, StoreCodecErrorV1, decode_command_v1,
    decode_terminal_receipt_v1, encode_command_v1, encode_persisted_terminal_receipt_v1,
    encode_terminal_receipt_v1,
};
use podway_store::schema::{
    SQLITE_INITIAL_MIGRATION_NAME_V1, SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3,
    SQLITE_SCHEMA_VERSION_CURRENT, SQLITE_SCHEMA_VERSION_V1, open_or_initialize_v1, sqlite_v1_ddl,
    sqlite_v3_ddl_checksum, verify_connection_pragmas_v1,
};
use podway_store::{
    ClaimTokenV1, ClaimedExecutionV1, ClaimedJobV1, DurableWorktreeIdentityV1, JobReceiptV1,
    PersistedSessionMutationV1, PersistedStartIdentityV1, RevisionAttemptItemPreconditionsV1,
    SqliteStoreOptionsV1, StateTransitionV1, StoreErrorV1, StoreFailpointV1, StoreIntegrityCheckV1,
    StoreUnavailableReasonV1, TerminalReceiptV1, TerminalResultV1, ValidatedWorkspaceRootV1,
    WorkerIdV1,
};
use rusqlite::{Connection, params, types::ValueRef};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
const FROZEN_SQLITE_V1_DDL: &str = include_str!("../../../assets/specifications/sqlite-v1.sql");
const EXPECTED_SQLITE_V1_MIGRATION_SHA256: &str =
    "sha256:20ea04d9635b8e1632e6d3aa5f3a888eaca49307b43ade9b9991363b30607423";
const EXPECTED_SQLITE_V3_MIGRATION_SHA256: &str =
    "sha256:45548f0dfa89ec404b6b85f441b3aa7ba4f8adbfb9d34cd26a6f79c691d2156f";

fn digest(hex_digit: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", hex_digit.to_string().repeat(64)))
        .expect("fixture digest must be valid")
}

#[test]
fn terminal_v3_codec_round_trips_the_canonical_response_context_and_rejects_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let session_id = session_id();
    let receipt = PersistedTerminalReceiptV1::new_with_projections(
        receipt(102),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
            session_id: session_id.clone(),
            revision_before: Revision::new(4),
            revision_after: Revision::new(5),
            changed: true,
        }),
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Succeeded,
            UnixMillis::new(30),
            Some(UnixMillis::new(31)),
            UnixMillis::new(32),
        )?,
        Some(PersistedTerminalSessionProjectionV1::new(
            session_id,
            "Response-loss-safe task".to_owned(),
            PersistedSessionLifecycleV1::Running,
            Revision::new(4),
            Revision::new(5),
        )?),
    )?
    .with_lookup_command(PersistedDomainCommandV1::SessionComplete)?
    .with_response_context(PersistedResponseContextV1::new(
        "00000000-0000-4000-8000-000000000102",
        "session.complete",
        workspace_id(),
        "/safe/worktree",
        102,
    )?)?;

    let encoded = encode_persisted_terminal_receipt_v1(&receipt)?;
    assert!(encoded.contains(STORE_TERMINAL_SCHEMA_V3));
    assert!(encoded.contains(r#""request_id":"00000000-0000-4000-8000-000000000102""#));
    assert_eq!(decode_terminal_receipt_v1(&encoded)?, receipt);

    assert!(
        decode_terminal_receipt_v1(&encoded.replacen(
            r#""command":"session.complete""#,
            r#""command":"session.start""#,
            1,
        ))
        .is_err()
    );
    assert!(
        decode_terminal_receipt_v1(&encoded.replacen(
            r#""workspace_sequence":102"#,
            r#""workspace_sequence":102,"unknown":true"#,
            1,
        ))
        .is_err()
    );
    Ok(())
}

#[test]
fn terminal_v4_codec_round_trips_the_frozen_public_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let receipt = PersistedTerminalReceiptV1::new_with_projections(
        receipt(103),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::WorkspaceInitialized {
            workspace_id: workspace_id(),
            revision: Revision::ZERO,
        }),
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Succeeded,
            UnixMillis::new(30),
            Some(UnixMillis::new(31)),
            UnixMillis::new(32),
        )?,
        None,
    )?
    .with_lookup_command(PersistedDomainCommandV1::WorkspaceInitialize)?
    .with_response_context(PersistedResponseContextV1::new(
        "00000000-0000-4000-8000-000000000103",
        "workspace.init",
        workspace_id(),
        "/safe/worktree",
        103,
    )?)?
    .with_public_terminal_envelope(serde_json::json!({
        "schema": "podway.output/v1",
        "request_id": "00000000-0000-4000-8000-000000000103",
        "command": "workspace.init"
    }))?;

    let encoded = encode_persisted_terminal_receipt_v1(&receipt)?;
    assert!(encoded.contains("podway.store-terminal/v4"));
    assert_eq!(decode_terminal_receipt_v1(&encoded)?, receipt);
    assert!(
        decode_terminal_receipt_v1(&encoded.replacen(
            r#""schema":"podway.output/v1""#,
            r#""schema":"podway.unknown/v1""#,
            1,
        ))
        .is_err()
    );

    let v2_receipt = receipt.with_public_terminal_envelope(serde_json::json!({
        "schema": "podway.output/v2",
        "request_id": "00000000-0000-4000-8000-000000000103",
        "command": "workspace.init"
    }))?;
    assert_eq!(
        decode_terminal_receipt_v1(&encode_persisted_terminal_receipt_v1(&v2_receipt)?)?,
        v2_receipt
    );
    Ok(())
}

fn remove_parallel_v2_tables(connection: &Connection) -> rusqlite::Result<()> {
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    let tables = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name GLOB 'v2_*' ORDER BY name DESC",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for table in tables {
        assert!(
            table.chars().all(|character| {
                character == '_' || character.is_ascii_lowercase() || character.is_ascii_digit()
            }),
            "canonical v2 table names are safe SQL identifiers"
        );
        connection.execute_batch(&format!("DROP TABLE {table};"))?;
    }
    connection.pragma_update(None, "foreign_keys", "ON")
}

fn restore_schema_v1_shape(connection: &Connection) -> rusqlite::Result<()> {
    remove_parallel_v2_tables(connection)?;
    connection.execute_batch(
        "ALTER TABLE jobs DROP COLUMN response_context_json;\
         DELETE FROM schema_migrations WHERE version IN (2, 3);\
         PRAGMA user_version = 1;",
    )
}

fn restore_schema_v2_shape(connection: &Connection) -> rusqlite::Result<()> {
    remove_parallel_v2_tables(connection)?;
    connection.execute_batch(
        "DELETE FROM schema_migrations WHERE version = 3;\
         PRAGMA user_version = 2;",
    )
}

fn logical_database_state(connection: &Connection) -> rusqlite::Result<Vec<String>> {
    let user_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let mut state = vec![format!("user_version:{user_version}")];
    let schema_objects = {
        let mut statement = connection.prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name, tbl_name, sql",
        )?;
        statement
            .query_map([], |row| {
                Ok(format!(
                    "schema:{:?}",
                    (
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    )
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    state.extend(schema_objects);

    let tables = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for table in tables {
        let quoted_table = table.replace('"', "\"\"");
        let mut statement = connection.prepare(&format!("SELECT * FROM \"{quoted_table}\""))?;
        let column_count = statement.column_count();
        let mut rows = statement.query([])?;
        let mut encoded_rows = Vec::new();
        while let Some(row) = rows.next()? {
            let mut encoded = String::new();
            for index in 0..column_count {
                if index != 0 {
                    encoded.push('|');
                }
                match row.get_ref(index)? {
                    ValueRef::Null => encoded.push('n'),
                    ValueRef::Integer(value) => encoded.push_str(&format!("i{value}")),
                    ValueRef::Real(value) => {
                        encoded.push_str(&format!("r{:016x}", value.to_bits()))
                    }
                    ValueRef::Text(value) => {
                        encoded.push('t');
                        for byte in value {
                            encoded.push_str(&format!("{byte:02x}"));
                        }
                    }
                    ValueRef::Blob(value) => {
                        encoded.push('b');
                        for byte in value {
                            encoded.push_str(&format!("{byte:02x}"));
                        }
                    }
                }
            }
            encoded_rows.push(encoded);
        }
        encoded_rows.sort();
        state.push(format!("table:{table}:{encoded_rows:?}"));
    }
    Ok(state)
}

fn insert_retained_v1_session(connection: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    const SNAPSHOT_ID: &str = "00000000-0000-4000-8000-000000000104";
    const SESSION_ID: &str = "00000000-0000-4000-8000-000000000105";
    const ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000106";
    let canonical_json = canonicalize_json_v1(&serde_json::json!({
        "schema": "podway.procedure/v1",
        "id": "migration-fixture",
        "version": "1",
        "name": "Migration fixture",
        "stages": [{
            "id": "first",
            "title": "First",
            "instructions": [],
            "items": [{"type": "confirm", "id": "done", "prompt": "Done", "required": false}]
        }],
        "rework": {"allow_return_to": "any_previous"}
    }))?;
    let snapshot_digest = format!("sha256:{:x}", Sha256::digest(canonical_json.as_bytes()));
    connection.execute(
        "INSERT INTO procedure_snapshots (
             snapshot_id, schema_id, procedure_id, procedure_version, name, digest,
             canonical_json, source_kind, source_label, created_at_ms
         ) VALUES (?1, 'podway.procedure/v1', 'migration-fixture', '1', 'Migration fixture',
                   ?2, ?3, 'preset', 'migration-fixture', 1)",
        params![SNAPSHOT_ID, snapshot_digest, canonical_json],
    )?;
    connection.execute(
        "INSERT INTO task_sessions (
             singleton, session_id, task_title, procedure_snapshot_id, lifecycle,
             session_revision, active_stage_id, active_attempt_id, created_at_ms
         ) VALUES (1, ?1, 'Retained v1 task', ?2, 'running', 1, 'first', ?3, 1)",
        params![SESSION_ID, SNAPSHOT_ID, ATTEMPT_ID],
    )?;
    connection.execute(
        "INSERT INTO stage_progress (
             session_id, stage_id, stage_index, progress_state, latest_attempt_number,
             latest_attempt_id
         ) VALUES (?1, 'first', 0, 'current', 1, ?2)",
        params![SESSION_ID, ATTEMPT_ID],
    )?;
    connection.execute(
        "INSERT INTO attempts (
             attempt_id, session_id, stage_id, attempt_number, lifecycle, started_at_ms
         ) VALUES (?1, ?2, 'first', 1, 'active', 1)",
        params![ATTEMPT_ID, SESSION_ID],
    )?;
    connection.execute(
        "INSERT INTO item_slots (
             attempt_id, item_id, item_type, item_revision, value_json, created_at_ms, updated_at_ms
         ) VALUES (?1, 'done', 'confirm', 0, NULL, 1, 1)",
        [ATTEMPT_ID],
    )?;
    Ok(())
}

#[test]
fn canonical_v1_database_migrates_once_through_v3_with_response_context_and_v2_state()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let connection = open_temp_database(&temporary, &root(), &identity())?;
    insert_retained_v1_session(&connection)?;
    restore_schema_v1_shape(&connection)?;
    drop(connection);

    let migrated = open_temp_database(&temporary, &root(), &identity())?;
    let version: i64 = migrated.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let migrations: i64 =
        migrated.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    let context_column: i64 = migrated.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('jobs') WHERE name = 'response_context_json'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(version, i64::from(SQLITE_SCHEMA_VERSION_CURRENT));
    assert_eq!(migrations, 3);
    assert_eq!(context_column, 1);
    let v2_table_count: i64 = migrated.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name GLOB 'v2_*'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(v2_table_count, 16);
    let retained_v1: (String, String, String, String, i64) = migrated.query_row(
        "SELECT s.session_id, s.task_title, s.active_stage_id, a.attempt_id,
                (SELECT COUNT(*) FROM item_slots WHERE attempt_id = a.attempt_id)
         FROM task_sessions AS s
         JOIN attempts AS a ON a.attempt_id = s.active_attempt_id
         WHERE s.singleton = 1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    assert_eq!(
        retained_v1,
        (
            "00000000-0000-4000-8000-000000000105".to_owned(),
            "Retained v1 task".to_owned(),
            "first".to_owned(),
            "00000000-0000-4000-8000-000000000106".to_owned(),
            1,
        )
    );
    let v3_migration: (String, String) = migrated.query_row(
        "SELECT name, checksum FROM schema_migrations WHERE version = 3",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(
        v3_migration,
        (
            SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3.to_owned(),
            sqlite_v3_ddl_checksum(),
        )
    );
    drop(migrated);

    let reopened = open_temp_database(&temporary, &root(), &identity())?;
    let migrations: i64 =
        reopened.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    assert_eq!(migrations, 3, "reopen must not duplicate migrations");
    Ok(())
}

#[test]
fn v1_to_v3_upgrade_rolls_back_every_migration_step_when_commit_fails()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let connection = open_temp_database(&temporary, &root(), &identity())?;
    insert_retained_v1_session(&connection)?;
    restore_schema_v1_shape(&connection)?;
    let predecessor_state = logical_database_state(&connection)?;
    drop(connection);

    let failing_options = options().with_failpoint(Some(StoreFailpointV1::SchemaBeforeCommit));
    assert!(matches!(
        open_temp_database_with_options(&temporary, &root(), &identity(), &failing_options),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));

    let raw = Connection::open(temporary.path().join("state.sqlite3"))?;
    assert_eq!(logical_database_state(&raw)?, predecessor_state);
    let version: i64 = raw.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let migration_count: i64 =
        raw.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    let response_context_columns: i64 = raw.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('jobs') WHERE name = 'response_context_json'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(version, 1);
    assert_eq!(migration_count, 1);
    assert_eq!(response_context_columns, 0);
    Ok(())
}

#[test]
fn corrupt_v1_predecessor_is_rejected_without_advancing_the_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let connection = open_temp_database(&temporary, &root(), &identity())?;
    restore_schema_v1_shape(&connection)?;
    connection.execute(
        "UPDATE schema_migrations SET checksum = \
         'sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff' \
         WHERE version = 1",
        [],
    )?;
    let predecessor_state = logical_database_state(&connection)?;
    drop(connection);

    assert!(matches!(
        opened_error(&temporary, &root(), &identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::MigrationChecksum
        }
    ));

    let raw = Connection::open(temporary.path().join("state.sqlite3"))?;
    assert_eq!(logical_database_state(&raw)?, predecessor_state);
    let version: i64 = raw.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let migration_count: i64 =
        raw.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    let response_context_columns: i64 = raw.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('jobs') WHERE name = 'response_context_json'",
        [],
        |row| row.get(0),
    )?;
    let v2_table_count: i64 = raw.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name GLOB 'v2_*'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(version, 1);
    assert_eq!(migration_count, 1);
    assert_eq!(response_context_columns, 0);
    assert_eq!(v2_table_count, 0);

    let unexpected_object = TempDir::new()?;
    let connection = open_temp_database(&unexpected_object, &root(), &identity())?;
    restore_schema_v1_shape(&connection)?;
    connection.execute_batch("CREATE TABLE unexpected_predecessor (value INTEGER) STRICT;")?;
    let predecessor_state = logical_database_state(&connection)?;
    drop(connection);
    assert!(matches!(
        opened_error(&unexpected_object, &root(), &identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::RequiredSchemaObjects
        }
    ));
    let raw = Connection::open(unexpected_object.path().join("state.sqlite3"))?;
    assert_eq!(logical_database_state(&raw)?, predecessor_state);
    let version: i64 = raw.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let migration_count: i64 =
        raw.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    assert_eq!(version, 1);
    assert_eq!(migration_count, 1);
    Ok(())
}

#[test]
fn migration_rejects_mismatched_identity_and_logical_corruption_before_commit()
-> Result<(), Box<dyn std::error::Error>> {
    for restore_predecessor in [
        restore_schema_v1_shape as fn(&Connection) -> rusqlite::Result<()>,
        restore_schema_v2_shape,
    ] {
        let mismatch = TempDir::new()?;
        let connection = open_temp_database(&mismatch, &root(), &identity())?;
        restore_predecessor(&connection)?;
        let predecessor_state = logical_database_state(&connection)?;
        drop(connection);

        assert!(matches!(
            opened_error(&mismatch, &root(), &other_identity()),
            StoreErrorV1::StorageIntegrityV1 {
                check: StoreIntegrityCheckV1::WorkspaceIdentity
            }
        ));
        let raw = Connection::open(mismatch.path().join("state.sqlite3"))?;
        assert_eq!(logical_database_state(&raw)?, predecessor_state);

        let corrupt = TempDir::new()?;
        let connection = open_temp_database(&corrupt, &root(), &identity())?;
        insert_retained_v1_session(&connection)?;
        restore_predecessor(&connection)?;
        connection.execute(
            "UPDATE procedure_snapshots SET digest = \
             'sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
            [],
        )?;
        let predecessor_state = logical_database_state(&connection)?;
        drop(connection);

        assert!(matches!(
            opened_error(&corrupt, &root(), &identity()),
            StoreErrorV1::StorageIntegrityV1 {
                check: StoreIntegrityCheckV1::SnapshotDigest
            }
        ));
        let raw = Connection::open(corrupt.path().join("state.sqlite3"))?;
        assert_eq!(logical_database_state(&raw)?, predecessor_state);
    }
    Ok(())
}

#[test]
fn v2_to_v3_upgrade_rolls_back_and_rejects_corrupt_predecessors_without_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let rollback = TempDir::new()?;
    let connection = open_temp_database(&rollback, &root(), &identity())?;
    restore_schema_v2_shape(&connection)?;
    let predecessor_state = logical_database_state(&connection)?;
    drop(connection);

    let failing_options = options().with_failpoint(Some(StoreFailpointV1::SchemaBeforeCommit));
    assert!(matches!(
        open_temp_database_with_options(&rollback, &root(), &identity(), &failing_options),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));
    let raw = Connection::open(rollback.path().join("state.sqlite3"))?;
    assert_eq!(logical_database_state(&raw)?, predecessor_state);
    drop(raw);
    let migrated = open_temp_database(&rollback, &root(), &identity())?;
    let version: i64 = migrated.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    assert_eq!(version, 3);

    let corrupt = TempDir::new()?;
    let connection = open_temp_database(&corrupt, &root(), &identity())?;
    restore_schema_v2_shape(&connection)?;
    connection.execute(
        "UPDATE schema_migrations SET checksum = \
         'sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff' \
         WHERE version = 2",
        [],
    )?;
    let predecessor_state = logical_database_state(&connection)?;
    drop(connection);
    assert!(matches!(
        opened_error(&corrupt, &root(), &identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::MigrationChecksum
        }
    ));
    let raw = Connection::open(corrupt.path().join("state.sqlite3"))?;
    assert_eq!(logical_database_state(&raw)?, predecessor_state);
    Ok(())
}

#[test]
fn sqlite_v3_exposes_the_complete_parallel_state_inventory_and_cursor_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let connection = open_temp_database(&temporary, &root(), &identity())?;
    assert_eq!(
        sqlite_v3_ddl_checksum(),
        EXPECTED_SQLITE_V3_MIGRATION_SHA256
    );
    let tables = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name GLOB 'v2_*' ORDER BY name",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    assert_eq!(
        tables,
        [
            "v2_attempts",
            "v2_blockers",
            "v2_criterion_assessment_results",
            "v2_criterion_citations",
            "v2_decision_records",
            "v2_goal_assessments",
            "v2_goal_criteria",
            "v2_goal_revisions",
            "v2_graph_node_counters",
            "v2_graph_nodes",
            "v2_item_slots",
            "v2_procedure_snapshots",
            "v2_resolved_evidence_references",
            "v2_rework_records",
            "v2_task_sessions",
            "v2_workspace_state",
        ]
    );

    connection.execute(
        "INSERT INTO v2_procedure_snapshots (
             snapshot_id, schema_id, procedure_id, procedure_version, name, purpose, digest,
             canonical_json, source_kind, source_label, goal_tracking, created_at_ms
         ) VALUES (?1, 'podway.procedure/v2', 'workflow', '1', 'Workflow', 'Test', ?2,
                   '{}', 'file', 'procedure.yaml', 0, 1)",
        params!["snapshot", digest('9').as_str()],
    )?;
    for (index, node) in ["first", "second"].iter().enumerate() {
        connection.execute(
            "INSERT INTO v2_graph_nodes (
                 snapshot_id, graph_node_id, node_definition_id, placement_index, node_type,
                 goal_assessment, canonical_placement_json
             ) VALUES ('snapshot', ?1, ?1, ?2, 'action', 0, '{}')",
            params![node, index as i64],
        )?;
    }
    connection.execute(
        "INSERT INTO v2_task_sessions (
             singleton, session_id, task_title, procedure_snapshot_id, lifecycle,
             session_revision, latest_trace_sequence, active_graph_node_id, active_attempt_id,
             active_trace_sequence, goal_tracking, created_at_ms
         ) VALUES (1, 'session', 'Task', 'snapshot', 'running', 1, 1, 'first', 'attempt-1', 1, 0, 1)",
        [],
    )?;
    connection.execute(
        "INSERT INTO v2_attempts (
             attempt_id, session_id, snapshot_id, graph_node_id, node_definition_id,
             attempt_number, trace_sequence, lifecycle, validity, started_at_ms
         ) VALUES ('attempt-1', 'session', 'snapshot', 'first', 'first', 1, 1, 'active', 'valid', 1)",
        [],
    )?;
    assert!(
        connection
            .execute(
                "INSERT INTO v2_attempts (
                     attempt_id, session_id, snapshot_id, graph_node_id, node_definition_id,
                     attempt_number, trace_sequence, lifecycle, validity, started_at_ms
                 ) VALUES ('attempt-2', 'session', 'snapshot', 'second', 'second', 1, 2, 'active', 'valid', 1)",
                [],
            )
            .is_err(),
        "the schema must reject a second active v2 attempt"
    );
    assert!(
        connection
            .execute(
                "INSERT INTO v2_attempts (
                     attempt_id, session_id, snapshot_id, graph_node_id, node_definition_id,
                     attempt_number, trace_sequence, lifecycle, validity, started_at_ms, ended_at_ms
                 ) VALUES ('attempt-3', 'session', 'snapshot', 'first', 'first', 2, 2, 'completed', 'valid', 1, 2)",
                [],
            )
            .is_err(),
        "the schema must reject two valid attempts for one graph node"
    );
    Ok(())
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::new("00000000-0000-4000-8000-000000000001")
        .expect("fixture workspace ID must be valid")
}

fn job_id() -> JobId {
    JobId::new("00000000-0000-4000-8000-000000000003").expect("fixture job ID must be valid")
}

fn session_id() -> podway_core::SessionId {
    podway_core::SessionId::new("00000000-0000-4000-8000-000000000004")
        .expect("fixture session ID must be valid")
}

fn attempt_id() -> AttemptId {
    AttemptId::new("00000000-0000-4000-8000-000000000005")
        .expect("fixture attempt ID must be valid")
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(digest('a'), workspace_id(), digest('b'))
}

fn other_identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(digest('c'), workspace_id(), digest('b'))
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-store-phase2"))
        .expect("fixture root must be valid")
}

fn options() -> SqliteStoreOptionsV1 {
    SqliteStoreOptionsV1::new(8).expect("fixture options must be valid")
}

fn receipt(sequence: u64) -> podway_store::JobReceiptV1 {
    podway_store::JobReceiptV1::new(sequence, job_id(), digest('d'))
}

fn open_temp_database(
    temporary: &TempDir,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
) -> Result<Connection, StoreErrorV1> {
    let options = options();
    open_temp_database_with_options(temporary, root, identity, &options)
}

fn open_temp_database_with_options(
    temporary: &TempDir,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
) -> Result<Connection, StoreErrorV1> {
    open_or_initialize_v1(
        temporary.path().join("state.sqlite3"),
        root,
        identity,
        options,
        UnixMillis::new(1234),
    )
}

fn opened_error(
    temporary: &TempDir,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
) -> StoreErrorV1 {
    match open_temp_database(temporary, root, identity) {
        Ok(_) => panic!("open must fail"),
        Err(error) => error,
    }
}
fn assert_codec_error<T>(result: Result<T, StoreCodecErrorV1>, expected: StoreCodecErrorV1) {
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(_) => panic!("unknown field must be rejected"),
    }
}

fn make_database_private(temporary: &TempDir) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    fs::set_permissions(
        temporary.path().join("state.sqlite3"),
        fs::Permissions::from_mode(0o600),
    )?;
    Ok(())
}

fn assert_uninitialized_schema0(
    temporary: &TempDir,
    application_id: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::open(temporary.path().join("state.sqlite3"))?;
    let user_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let user_object_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    let initialization_object_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('schema_migrations', 'workspace_state')",
        [],
        |row| row.get(0),
    )?;
    let actual_application_id: i64 =
        connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;

    assert_eq!(user_version, 0);
    assert_eq!(user_object_count, 0);
    assert_eq!(initialization_object_count, 0);
    assert_eq!(actual_application_id, application_id);
    Ok(())
}

fn assert_schema_initialization_failpoint_recovers(
    failpoint: StoreFailpointV1,
) -> Result<(), Box<dyn std::error::Error>> {
    const APPLICATION_ID: i64 = 0x504f_4457;

    let temporary = TempDir::new()?;
    let root = root();
    let identity = identity();
    let raw = Connection::open(temporary.path().join("state.sqlite3"))?;
    raw.pragma_update(None, "application_id", APPLICATION_ID)?;
    drop(raw);
    make_database_private(&temporary)?;

    let failing_options = options().with_failpoint(Some(failpoint));
    assert!(matches!(
        open_temp_database_with_options(&temporary, &root, &identity, &failing_options),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));
    assert_uninitialized_schema0(&temporary, APPLICATION_ID)?;

    let connection = open_temp_database(&temporary, &root, &identity)?;
    let migration_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    let identity_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM workspace_state", [], |row| row.get(0))?;
    assert_eq!(migration_count, 3);
    assert_eq!(identity_count, 1);
    drop(connection);

    open_temp_database(&temporary, &root, &identity)?;
    Ok(())
}

#[test]
fn schema0_initializes_and_reopens_with_exact_pragmas_and_migration_checksum()
-> Result<(), Box<dyn std::error::Error>> {
    const BUSY_TIMEOUT_MS: u32 = 4_321;

    let temporary = TempDir::new()?;
    let root = root();
    let identity = identity();
    let options = options().with_busy_timeout_ms(BUSY_TIMEOUT_MS)?;
    let connection = open_temp_database_with_options(&temporary, &root, &identity, &options)?;

    verify_connection_pragmas_v1(&connection, BUSY_TIMEOUT_MS)?;
    let user_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    let busy_timeout: i64 = connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
    let trusted_schema: i64 =
        connection.query_row("PRAGMA trusted_schema", [], |row| row.get(0))?;
    assert_eq!(user_version, i64::from(SQLITE_SCHEMA_VERSION_CURRENT));
    assert_eq!(foreign_keys, 1);
    assert_eq!(journal_mode, "wal");
    assert_eq!(synchronous, 2);
    assert_eq!(busy_timeout, i64::from(BUSY_TIMEOUT_MS));
    assert_eq!(trusted_schema, 0);
    let ddl = sqlite_v1_ddl();
    assert_eq!(ddl.as_bytes(), FROZEN_SQLITE_V1_DDL.as_bytes());
    assert_eq!(
        format!(
            "sha256:{:x}",
            Sha256::digest(FROZEN_SQLITE_V1_DDL.as_bytes())
        ),
        EXPECTED_SQLITE_V1_MIGRATION_SHA256
    );
    assert_eq!(
        format!("sha256:{:x}", Sha256::digest(ddl.as_bytes())),
        EXPECTED_SQLITE_V1_MIGRATION_SHA256
    );

    let migration: (String, String) = connection.query_row(
        "SELECT name, checksum FROM schema_migrations WHERE version = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(migration.0, SQLITE_INITIAL_MIGRATION_NAME_V1);
    assert_eq!(migration.1, EXPECTED_SQLITE_V1_MIGRATION_SHA256);

    let workspace: (String, String, i64, i64) = connection.query_row(
        "SELECT workspace_uuid, last_validated_root, next_workspace_sequence, created_at_ms \
         FROM workspace_state WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(workspace.0, identity.workspace_uuid().as_str());
    assert_eq!(workspace.1, root.as_encoded());
    assert_eq!(workspace.2, 0);
    assert_eq!(workspace.3, 1234);
    drop(connection);

    let moved_root = ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-store-moved"))?;
    let connection = open_temp_database_with_options(&temporary, &moved_root, &identity, &options)?;
    verify_connection_pragmas_v1(&connection, BUSY_TIMEOUT_MS)?;
    let reopened_foreign_keys: i64 =
        connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    let reopened_journal_mode: String =
        connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let reopened_synchronous: i64 =
        connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    let reopened_busy_timeout: i64 =
        connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
    let reopened_trusted_schema: i64 =
        connection.query_row("PRAGMA trusted_schema", [], |row| row.get(0))?;
    assert_eq!(reopened_foreign_keys, 1);
    assert_eq!(reopened_journal_mode, "wal");
    assert_eq!(reopened_synchronous, 2);
    assert_eq!(reopened_busy_timeout, i64::from(BUSY_TIMEOUT_MS));
    assert_eq!(reopened_trusted_schema, 0);

    let stored_root: String = connection.query_row(
        "SELECT last_validated_root FROM workspace_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(stored_root, moved_root.as_encoded());
    Ok(())
}

#[test]

fn pac_040_schema0_pragmas_transactional_initialization_preserves_task_state_without_duplicate_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    const APPLICATION_ID: i64 = 0x504f_4457;
    const PAGE_SIZE: i64 = 8_192;
    const AUTO_VACUUM_FULL: i64 = 1;
    const BUSY_TIMEOUT_MS: u32 = 4_321;
    const TASK_STATE_TABLES: &[&str] = &[
        "procedure_snapshots",
        "task_sessions",
        "stage_progress",
        "attempts",
        "item_slots",
        "blockers",
        "jobs",
        "idempotency_records",
        "operational_journal",
    ];

    let temporary = TempDir::new()?;
    let worktree = temporary.path().join("worktree");
    let runtime = worktree.join(".podway/runtime");
    let external_global = temporary.path().join("global-state.sqlite3");
    fs::create_dir_all(&runtime)?;
    #[cfg(unix)]
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;

    let predecessor = Connection::open(runtime.join("state.sqlite3"))?;
    predecessor.pragma_update(None, "application_id", APPLICATION_ID)?;
    predecessor.pragma_update(None, "page_size", PAGE_SIZE)?;
    predecessor.pragma_update(None, "auto_vacuum", "FULL")?;
    predecessor.execute_batch("VACUUM")?;
    let seeded_predecessor: (i64, i64, i64, i64) = predecessor.query_row(
        "SELECT \
         (SELECT application_id FROM pragma_application_id), \
         (SELECT page_size FROM pragma_page_size), \
         (SELECT auto_vacuum FROM pragma_auto_vacuum), \
         (SELECT user_version FROM pragma_user_version)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(
        seeded_predecessor,
        (APPLICATION_ID, PAGE_SIZE, AUTO_VACUUM_FULL, 0),
        "schema-0 predecessor metadata must be seeded exactly"
    );
    let predecessor_rows: i64 = predecessor.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        predecessor_rows, 0,
        "schema-0 predecessor must not invent task or application tables"
    );
    drop(predecessor);
    let path = runtime.join("state.sqlite3");
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    let global = Connection::open(&external_global)?;
    global.execute_batch(
        "CREATE TABLE global_metadata (scope TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO global_metadata (scope, value) VALUES ('owner', 'external-global');",
    )?;
    drop(global);
    #[cfg(unix)]
    fs::set_permissions(&external_global, fs::Permissions::from_mode(0o600))?;
    let seeded_external_bytes = fs::read(&external_global)?;

    let worktree_root = ValidatedWorkspaceRootV1::from_path(&worktree)?;
    let options = options().with_busy_timeout_ms(BUSY_TIMEOUT_MS)?;
    let connection = open_or_initialize_v1(
        &path,
        &worktree_root,
        &identity(),
        &options,
        UnixMillis::new(1234),
    )?;
    verify_connection_pragmas_v1(&connection, BUSY_TIMEOUT_MS)?;
    let preserved_predecessor: (i64, i64, i64, i64) = connection.query_row(
        "SELECT \
         (SELECT application_id FROM pragma_application_id), \
         (SELECT page_size FROM pragma_page_size), \
         (SELECT auto_vacuum FROM pragma_auto_vacuum), \
         (SELECT user_version FROM pragma_user_version)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(
        preserved_predecessor,
        (
            APPLICATION_ID,
            PAGE_SIZE,
            AUTO_VACUUM_FULL,
            i64::from(SQLITE_SCHEMA_VERSION_CURRENT)
        ),
        "initialization must preserve schema-0 header metadata while advancing only user_version"
    );
    let migration: (i64, String, String) = connection.query_row(
        "SELECT version, name, checksum FROM schema_migrations",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(
        migration,
        (
            i64::from(SQLITE_SCHEMA_VERSION_V1),
            SQLITE_INITIAL_MIGRATION_NAME_V1.to_owned(),
            EXPECTED_SQLITE_V1_MIGRATION_SHA256.to_owned(),
        )
    );
    let migration_rows: i64 =
        connection.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    assert_eq!(
        migration_rows, 3,
        "initialization must record all canonical migrations"
    );
    for table in TASK_STATE_TABLES {
        let rows: i64 =
            connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
        assert_eq!(rows, 0, "schema-0 initialization copied rows into {table}");
    }
    let workspace_rows: i64 =
        connection.query_row("SELECT COUNT(*) FROM workspace_state", [], |row| row.get(0))?;
    assert_eq!(workspace_rows, 1);
    drop(connection);

    assert_eq!(
        fs::read(&external_global)?,
        seeded_external_bytes,
        "initialization must not mutate an external global location"
    );
    let global = Connection::open(&external_global)?;
    let global_metadata: (String, String) =
        global.query_row("SELECT scope, value FROM global_metadata", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    assert_eq!(
        global_metadata,
        ("owner".to_owned(), "external-global".to_owned()),
        "initialization must not copy task state into or alter the external global database"
    );
    let global_task_tables: i64 = global.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name IN (
             'procedure_snapshots', 'task_sessions', 'stage_progress', 'attempts', 'item_slots',
             'blockers', 'jobs', 'idempotency_records', 'operational_journal'
         )",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(global_task_tables, 0);
    drop(global);

    let worktree_entries = fs::read_dir(&worktree)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(worktree_entries, vec![std::ffi::OsString::from(".podway")]);
    let mut temporary_entries = fs::read_dir(temporary.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    temporary_entries.sort();
    assert_eq!(
        temporary_entries,
        vec![
            std::ffi::OsString::from("global-state.sqlite3"),
            std::ffi::OsString::from("worktree"),
        ],
        "task state must remain under the worktree rather than creating an external durable copy"
    );

    let reopened = open_or_initialize_v1(
        &path,
        &worktree_root,
        &identity(),
        &options,
        UnixMillis::new(1235),
    )?;
    verify_connection_pragmas_v1(&reopened, BUSY_TIMEOUT_MS)?;
    let reopened_migration_rows: i64 =
        reopened.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    assert_eq!(
        reopened_migration_rows, 3,
        "reopen must not append duplicate schema migrations"
    );
    let reopened_predecessor: (i64, i64, i64, i64) = reopened.query_row(
        "SELECT \
         (SELECT application_id FROM pragma_application_id), \
         (SELECT page_size FROM pragma_page_size), \
         (SELECT auto_vacuum FROM pragma_auto_vacuum), \
         (SELECT user_version FROM pragma_user_version)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(
        reopened_predecessor, preserved_predecessor,
        "reopen must preserve initialized predecessor metadata exactly"
    );
    for table in TASK_STATE_TABLES {
        let rows: i64 =
            reopened.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
        assert_eq!(rows, 0, "reopen invented task state in {table}");
    }
    let rollback = TempDir::new()?;
    let rollback_raw = Connection::open(rollback.path().join("state.sqlite3"))?;
    rollback_raw.pragma_update(None, "application_id", APPLICATION_ID)?;
    drop(rollback_raw);
    make_database_private(&rollback)?;
    let failing_options = SqliteStoreOptionsV1::new(8)?
        .with_busy_timeout_ms(BUSY_TIMEOUT_MS)?
        .with_failpoint(Some(StoreFailpointV1::SchemaBeforeCommit));
    assert!(matches!(
        open_temp_database_with_options(&rollback, &worktree_root, &identity(), &failing_options,),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));
    assert_uninitialized_schema0(&rollback, APPLICATION_ID)?;
    let recovered =
        open_temp_database_with_options(&rollback, &worktree_root, &identity(), &options)?;
    let recovered_migration_rows: i64 =
        recovered.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    assert_eq!(recovered_migration_rows, 3);
    drop(recovered);
    drop(reopened);
    assert_eq!(
        fs::read(&external_global)?,
        seeded_external_bytes,
        "reopen must not mutate an external global location"
    );
    Ok(())
}

#[test]
fn schema_after_pragmas_failure_leaves_schema0_unchanged_and_retry_initializes()
-> Result<(), Box<dyn std::error::Error>> {
    assert_schema_initialization_failpoint_recovers(StoreFailpointV1::SchemaAfterPragmas)
}

#[test]
fn schema_before_commit_failure_rolls_back_and_retry_initializes()
-> Result<(), Box<dyn std::error::Error>> {
    assert_schema_initialization_failpoint_recovers(StoreFailpointV1::SchemaBeforeCommit)
}

#[test]
fn pac_041_migration_checksum_validation_and_transactional_rollback_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let partial = TempDir::new()?;
    Connection::open(partial.path().join("state.sqlite3"))?
        .execute_batch("CREATE TABLE unexpected_schema0 (value INTEGER) STRICT;")?;
    make_database_private(&partial)?;
    assert!(matches!(
        opened_error(&partial, &root(), &identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::RequiredSchemaObjects
        }
    ));

    let newer = TempDir::new()?;
    let raw = Connection::open(newer.path().join("state.sqlite3"))?;
    raw.pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION_CURRENT + 1)?;
    drop(raw);
    make_database_private(&newer)?;
    assert!(matches!(
        opened_error(&newer, &root(), &identity()),
        StoreErrorV1::NewerStateV1 {
            found_schema_version: 4,
            supported_schema_version: SQLITE_SCHEMA_VERSION_CURRENT
        }
    ));

    let checksum = TempDir::new()?;
    let connection = open_temp_database(&checksum, &root(), &identity())?;
    connection.execute(
        "UPDATE schema_migrations SET checksum = 'sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
        [],
    )?;
    drop(connection);
    assert!(matches!(
        opened_error(&checksum, &root(), &identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::MigrationChecksum
        }
    ));

    let missing_migration = TempDir::new()?;
    let connection = open_temp_database(&missing_migration, &root(), &identity())?;
    connection.execute("DELETE FROM schema_migrations", [])?;
    drop(connection);
    assert!(matches!(
        opened_error(&missing_migration, &root(), &identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::MigrationChecksum
        }
    ));

    let missing_object = TempDir::new()?;
    let connection = open_temp_database(&missing_object, &root(), &identity())?;
    connection.execute_batch("DROP INDEX ix_jobs_state_sequence;")?;
    drop(connection);
    assert!(matches!(
        opened_error(&missing_object, &root(), &identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::RequiredSchemaObjects
        }
    ));

    let mismatch = TempDir::new()?;
    let connection = open_temp_database(&mismatch, &root(), &identity())?;
    drop(connection);
    assert!(matches!(
        opened_error(&mismatch, &root(), &other_identity()),
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::WorkspaceIdentity
        }
    ));
    let rollback = TempDir::new()?;
    let raw = Connection::open(rollback.path().join("state.sqlite3"))?;
    raw.pragma_update(None, "application_id", 0x504f_4457i64)?;
    drop(raw);
    make_database_private(&rollback)?;
    let failing_options = options().with_failpoint(Some(StoreFailpointV1::SchemaBeforeCommit));
    assert!(matches!(
        open_temp_database_with_options(&rollback, &root(), &identity(), &failing_options),
        Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));
    assert_uninitialized_schema0(&rollback, 0x504f_4457)?;
    let initialized = open_temp_database(&rollback, &root(), &identity())?;
    let checksum: String = initialized.query_row(
        "SELECT checksum FROM schema_migrations WHERE version = 1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(checksum, EXPECTED_SQLITE_V1_MIGRATION_SHA256);
    drop(initialized);
    Ok(())
}

#[cfg(unix)]
#[test]
fn workspace_root_text_is_lossless_for_non_utf8_unix_paths()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::ffi::OsStrExt;

    const ENCODED_NON_UTF8_ROOT: &str = "podway.unix-path/v1:2f746d702f706f647761792dff2d726f6f74";

    let bytes = b"/tmp/podway-\xff-root".to_vec();
    let root = ValidatedWorkspaceRootV1::from_unix_bytes(bytes.clone())?;
    assert_eq!(root.unix_bytes(), bytes.as_slice());
    assert_eq!(root.to_path_buf().as_os_str().as_bytes(), bytes.as_slice());
    assert_eq!(root.as_encoded(), ENCODED_NON_UTF8_ROOT);
    assert_eq!(
        ValidatedWorkspaceRootV1::from_encoded(root.as_encoded())?,
        root
    );

    let temporary = TempDir::new()?;
    let identity = identity();
    let connection = open_temp_database(&temporary, &root, &identity)?;
    let stored_encoded: String = connection.query_row(
        "SELECT last_validated_root FROM workspace_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(stored_encoded, ENCODED_NON_UTF8_ROOT);
    let stored_root = ValidatedWorkspaceRootV1::from_encoded(stored_encoded.clone())?;
    assert_eq!(stored_root.unix_bytes(), bytes.as_slice());
    assert_eq!(
        stored_root.to_path_buf().as_os_str().as_bytes(),
        bytes.as_slice()
    );
    connection.close().map_err(|(_, error)| error)?;

    let connection = open_temp_database(&temporary, &root, &identity)?;
    let reopened_encoded: String = connection.query_row(
        "SELECT last_validated_root FROM workspace_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(reopened_encoded, ENCODED_NON_UTF8_ROOT);
    let reopened_root = ValidatedWorkspaceRootV1::from_encoded(reopened_encoded)?;
    assert_eq!(reopened_root.unix_bytes(), bytes.as_slice());
    assert_eq!(
        reopened_root.to_path_buf().as_os_str().as_bytes(),
        bytes.as_slice()
    );
    connection.close().map_err(|(_, error)| error)?;

    assert!(ValidatedWorkspaceRootV1::from_encoded("podway.unix-path/v1:FF").is_err());
    Ok(())
}

fn fully_specified_preconditions(
    item: &ItemId,
) -> Result<RevisionAttemptItemPreconditionsV1, Box<dyn std::error::Error>> {
    Ok(RevisionAttemptItemPreconditionsV1::new(
        Some(Revision::new(11)),
        Some(attempt_id()),
        Some(item.clone()),
        Some(Revision::new(7)),
    )?)
}

fn assert_command_fields(actual: &DomainCommand, expected: &DomainCommand) {
    match actual {
        DomainCommand::WorkspaceInitialize => {
            assert!(matches!(expected, DomainCommand::WorkspaceInitialize));
        }
        DomainCommand::WorkspaceResetAll => {
            assert!(matches!(expected, DomainCommand::WorkspaceResetAll));
        }
        DomainCommand::SessionStart => {
            assert!(matches!(expected, DomainCommand::SessionStart));
        }
        DomainCommand::SessionStartReplace => {
            assert!(matches!(expected, DomainCommand::SessionStartReplace));
        }
        DomainCommand::SessionComplete => {
            assert!(matches!(expected, DomainCommand::SessionComplete));
        }
        DomainCommand::SessionSkip => {
            assert!(matches!(expected, DomainCommand::SessionSkip));
        }
        DomainCommand::SessionRetry => {
            assert!(matches!(expected, DomainCommand::SessionRetry));
        }
        DomainCommand::SessionReturn => {
            assert!(matches!(expected, DomainCommand::SessionReturn));
        }
        DomainCommand::SessionBlock => {
            assert!(matches!(expected, DomainCommand::SessionBlock));
        }
        DomainCommand::SessionUnblock => {
            assert!(matches!(expected, DomainCommand::SessionUnblock));
        }
        DomainCommand::SessionCancel => {
            assert!(matches!(expected, DomainCommand::SessionCancel));
        }
        DomainCommand::SessionReopen => {
            assert!(matches!(expected, DomainCommand::SessionReopen));
        }
        DomainCommand::SessionReset => {
            assert!(matches!(expected, DomainCommand::SessionReset));
        }
        DomainCommand::ItemCheck { item_id } => match expected {
            DomainCommand::ItemCheck {
                item_id: expected_item_id,
            } => assert_eq!(item_id, expected_item_id),
            _ => panic!("decoded command kind differs from the literal golden"),
        },
        DomainCommand::ItemUncheck { item_id } => match expected {
            DomainCommand::ItemUncheck {
                item_id: expected_item_id,
            } => assert_eq!(item_id, expected_item_id),
            _ => panic!("decoded command kind differs from the literal golden"),
        },
        DomainCommand::ItemSet { item_id } => match expected {
            DomainCommand::ItemSet {
                item_id: expected_item_id,
            } => assert_eq!(item_id, expected_item_id),
            _ => panic!("decoded command kind differs from the literal golden"),
        },
        DomainCommand::ItemAdd { item_id } => match expected {
            DomainCommand::ItemAdd {
                item_id: expected_item_id,
            } => assert_eq!(item_id, expected_item_id),
            _ => panic!("decoded command kind differs from the literal golden"),
        },
        DomainCommand::ItemRemove { item_id } => match expected {
            DomainCommand::ItemRemove {
                item_id: expected_item_id,
            } => assert_eq!(item_id, expected_item_id),
            _ => panic!("decoded command kind differs from the literal golden"),
        },
        DomainCommand::ItemAttach { item_id } => match expected {
            DomainCommand::ItemAttach {
                item_id: expected_item_id,
            } => assert_eq!(item_id, expected_item_id),
            _ => panic!("decoded command kind differs from the literal golden"),
        },
        DomainCommand::ItemClear { item_id } => match expected {
            DomainCommand::ItemClear {
                item_id: expected_item_id,
            } => assert_eq!(item_id, expected_item_id),
            _ => panic!("decoded command kind differs from the literal golden"),
        },
    }
}

fn assert_command_execution_fields(actual: &ClaimedExecutionV1, expected: &ClaimedExecutionV1) {
    assert_command_fields(actual.command(), expected.command());
    assert_eq!(
        actual.preconditions().expected_session_revision(),
        expected.preconditions().expected_session_revision()
    );
    assert_eq!(
        actual.preconditions().expected_attempt_id(),
        expected.preconditions().expected_attempt_id()
    );
    assert_eq!(
        actual.preconditions().expected_item_id(),
        expected.preconditions().expected_item_id()
    );
    assert_eq!(
        actual.preconditions().expected_item_revision(),
        expected.preconditions().expected_item_revision()
    );
    assert_eq!(
        actual.has_complete_execution_document(),
        expected.has_complete_execution_document()
    );
}

fn assert_command_golden(
    execution: &ClaimedExecutionV1,
    golden: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(encode_command_v1(execution)?, golden);
    assert!(!execution.has_complete_execution_document());
    let decoded = decode_command_v1(golden)?;
    assert!(!decoded.has_complete_execution_document());
    assert_command_execution_fields(&decoded, execution);
    Ok(())
}

fn command_golden_v1(command: &DomainCommand) -> &'static str {
    match command {
        DomainCommand::WorkspaceInitialize => {
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::WorkspaceResetAll => {
            r#"{"command":{"kind":"workspace_reset_all"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionStart => {
            r#"{"command":{"kind":"session_start"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionStartReplace => {
            r#"{"command":{"kind":"session_start_replace"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionComplete => {
            r#"{"command":{"kind":"session_complete"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionSkip => {
            r#"{"command":{"kind":"session_skip"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionRetry => {
            r#"{"command":{"kind":"session_retry"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionReturn => {
            r#"{"command":{"kind":"session_return"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionBlock => {
            r#"{"command":{"kind":"session_block"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionUnblock => {
            r#"{"command":{"kind":"session_unblock"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionCancel => {
            r#"{"command":{"kind":"session_cancel"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionReopen => {
            r#"{"command":{"kind":"session_reopen"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::SessionReset => {
            r#"{"command":{"kind":"session_reset"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::ItemCheck { .. } => {
            r#"{"command":{"item_id":"selected-item","kind":"item_check"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::ItemUncheck { .. } => {
            r#"{"command":{"item_id":"selected-item","kind":"item_uncheck"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::ItemSet { .. } => {
            r#"{"command":{"item_id":"selected-item","kind":"item_set"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::ItemAdd { .. } => {
            r#"{"command":{"item_id":"selected-item","kind":"item_add"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::ItemRemove { .. } => {
            r#"{"command":{"item_id":"selected-item","kind":"item_remove"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::ItemAttach { .. } => {
            r#"{"command":{"item_id":"selected-item","kind":"item_attach"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
        DomainCommand::ItemClear { .. } => {
            r#"{"command":{"item_id":"selected-item","kind":"item_clear"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#
        }
    }
}

fn assert_terminal_result_variant_coverage(result: &PersistedTerminalResultV1) {
    match result {
        PersistedTerminalResultV1::Success(_) => {}
        PersistedTerminalResultV1::Failure(_) => {}
        PersistedTerminalResultV1::Cancelled => {}
    }
}

fn assert_persisted_result_variant_coverage(result: &PersistedDomainResultV1) {
    match result {
        PersistedDomainResultV1::WorkspaceInitialized { .. } => {}
        PersistedDomainResultV1::WorkspaceReset { .. } => {}
        PersistedDomainResultV1::SessionChanged { .. } => {}
        PersistedDomainResultV1::ItemChanged { .. } => {}
    }
}

fn assert_persisted_error_variant_coverage(error: &PersistedDomainErrorV1) {
    match error {
        PersistedDomainErrorV1::EmptyValue { .. } => {}
        PersistedDomainErrorV1::ValueTooLong { .. } => {}
        PersistedDomainErrorV1::InvalidUuid { .. } => {}
        PersistedDomainErrorV1::InvalidIdentifier { .. } => {}
        PersistedDomainErrorV1::InvalidSha256Digest => {}
        PersistedDomainErrorV1::RevisionOverflow { .. } => {}
        PersistedDomainErrorV1::InvalidState { .. } => {}
        PersistedDomainErrorV1::RequiredItemsMissing => {}
        PersistedDomainErrorV1::BlockersPresent => {}
        PersistedDomainErrorV1::BlockerLimitReached { .. } => {}
        PersistedDomainErrorV1::ArtifactChanged => {}
        PersistedDomainErrorV1::InvalidTransition { .. } => {}
        PersistedDomainErrorV1::PreconditionFailed { .. } => {}
        PersistedDomainErrorV1::ItemNotFound { .. } => {}
        PersistedDomainErrorV1::BlockerNotCurrent => {}
        PersistedDomainErrorV1::SessionIdentityMismatch { .. } => {}
        PersistedDomainErrorV1::AttemptNotCurrent { .. } => {}
    }
}

fn assert_receipt_fields(actual: &JobReceiptV1, expected: &JobReceiptV1) {
    assert_eq!(actual.identity_sequence(), expected.identity_sequence());
    assert_eq!(actual.job_id(), expected.job_id());
    assert_eq!(actual.request_digest(), expected.request_digest());
}

fn assert_success_fields(actual: &PersistedDomainResultV1, expected: &DomainResult) {
    assert_persisted_result_variant_coverage(actual);
    match expected {
        DomainResult::WorkspaceInitialized {
            workspace_id,
            revision,
        } => match actual {
            PersistedDomainResultV1::WorkspaceInitialized {
                workspace_id: actual_workspace_id,
                revision: actual_revision,
            } => {
                assert_eq!(actual_workspace_id, workspace_id);
                assert_eq!(actual_revision, revision);
            }
            _ => panic!("decoded result kind differs from the literal golden"),
        },
        DomainResult::WorkspaceReset {
            workspace_id,
            revision,
        } => match actual {
            PersistedDomainResultV1::WorkspaceReset {
                workspace_id: actual_workspace_id,
                revision: actual_revision,
            } => {
                assert_eq!(actual_workspace_id, workspace_id);
                assert_eq!(actual_revision, revision);
            }
            _ => panic!("decoded result kind differs from the literal golden"),
        },
        DomainResult::SessionChanged {
            session_id,
            revision_before,
            revision_after,
            changed,
        } => match actual {
            PersistedDomainResultV1::SessionChanged {
                session_id: actual_session_id,
                revision_before: actual_revision_before,
                revision_after: actual_revision_after,
                changed: actual_changed,
            } => {
                assert_eq!(actual_session_id, session_id);
                assert_eq!(actual_revision_before, revision_before);
                assert_eq!(actual_revision_after, revision_after);
                assert_eq!(actual_changed, changed);
            }
            _ => panic!("decoded result kind differs from the literal golden"),
        },
        DomainResult::ItemChanged {
            session_id,
            item_id,
            revision_before,
            revision_after,
            changed,
        } => match actual {
            PersistedDomainResultV1::ItemChanged {
                session_id: actual_session_id,
                item_id: actual_item_id,
                revision_before: actual_revision_before,
                revision_after: actual_revision_after,
                changed: actual_changed,
            } => {
                assert_eq!(actual_session_id, session_id);
                assert_eq!(actual_item_id, item_id);
                assert_eq!(actual_revision_before, revision_before);
                assert_eq!(actual_revision_after, revision_after);
                assert_eq!(actual_changed, changed);
            }
            _ => panic!("decoded result kind differs from the literal golden"),
        },
    }
}

fn expected_persisted_command_kind(kind: DomainCommandKind) -> PersistedDomainCommandKindV1 {
    match kind {
        DomainCommandKind::WorkspaceInitialize => PersistedDomainCommandKindV1::WorkspaceInitialize,
        DomainCommandKind::WorkspaceResetAll => PersistedDomainCommandKindV1::WorkspaceResetAll,
        DomainCommandKind::SessionStart => PersistedDomainCommandKindV1::SessionStart,
        DomainCommandKind::SessionStartReplace => PersistedDomainCommandKindV1::SessionStartReplace,
        DomainCommandKind::SessionComplete => PersistedDomainCommandKindV1::SessionComplete,
        DomainCommandKind::SessionSkip => PersistedDomainCommandKindV1::SessionSkip,
        DomainCommandKind::SessionRetry => PersistedDomainCommandKindV1::SessionRetry,
        DomainCommandKind::SessionReturn => PersistedDomainCommandKindV1::SessionReturn,
        DomainCommandKind::SessionBlock => PersistedDomainCommandKindV1::SessionBlock,
        DomainCommandKind::SessionUnblock => PersistedDomainCommandKindV1::SessionUnblock,
        DomainCommandKind::SessionCancel => PersistedDomainCommandKindV1::SessionCancel,
        DomainCommandKind::SessionReopen => PersistedDomainCommandKindV1::SessionReopen,
        DomainCommandKind::SessionReset => PersistedDomainCommandKindV1::SessionReset,
        DomainCommandKind::ItemCheck => PersistedDomainCommandKindV1::ItemCheck,
        DomainCommandKind::ItemUncheck => PersistedDomainCommandKindV1::ItemUncheck,
        DomainCommandKind::ItemSet => PersistedDomainCommandKindV1::ItemSet,
        DomainCommandKind::ItemAdd => PersistedDomainCommandKindV1::ItemAdd,
        DomainCommandKind::ItemRemove => PersistedDomainCommandKindV1::ItemRemove,
        DomainCommandKind::ItemAttach => PersistedDomainCommandKindV1::ItemAttach,
        DomainCommandKind::ItemClear => PersistedDomainCommandKindV1::ItemClear,
    }
}

fn expected_persisted_lifecycle(state: SessionLifecycle) -> PersistedSessionLifecycleV1 {
    match state {
        SessionLifecycle::Running => PersistedSessionLifecycleV1::Running,
        SessionLifecycle::Completed => PersistedSessionLifecycleV1::Completed,
        SessionLifecycle::Cancelled => PersistedSessionLifecycleV1::Cancelled,
    }
}

fn assert_failure_fields(actual: &PersistedDomainErrorV1, expected: &DomainError) {
    assert_persisted_error_variant_coverage(actual);
    match expected {
        DomainError::EmptyValue { field } => match actual {
            PersistedDomainErrorV1::EmptyValue {
                field: actual_field,
            } => assert_eq!(actual_field.as_str(), *field),
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::ValueTooLong {
            field,
            maximum,
            actual: expected_actual,
        } => match actual {
            PersistedDomainErrorV1::ValueTooLong {
                field: actual_field,
                maximum: actual_maximum,
                actual: actual_actual,
            } => {
                assert_eq!(actual_field.as_str(), *field);
                assert_eq!(*actual_maximum, u64::try_from(*maximum).unwrap());
                assert_eq!(*actual_actual, u64::try_from(*expected_actual).unwrap());
            }
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::InvalidUuid { field } => match actual {
            PersistedDomainErrorV1::InvalidUuid {
                field: actual_field,
            } => assert_eq!(actual_field.as_str(), *field),
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::InvalidIdentifier { field } => match actual {
            PersistedDomainErrorV1::InvalidIdentifier {
                field: actual_field,
            } => assert_eq!(actual_field.as_str(), *field),
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::InvalidSha256Digest => {
            assert!(matches!(
                actual,
                PersistedDomainErrorV1::InvalidSha256Digest
            ));
        }
        DomainError::RevisionOverflow { revision } => match actual {
            PersistedDomainErrorV1::RevisionOverflow {
                revision: actual_revision,
            } => assert_eq!(actual_revision, revision),
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::InvalidState { reason } => match actual {
            PersistedDomainErrorV1::InvalidState {
                reason: actual_reason,
            } => assert_eq!(actual_reason.as_str(), *reason),
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::RequiredItemsMissing => {
            assert!(matches!(
                actual,
                PersistedDomainErrorV1::RequiredItemsMissing
            ));
        }
        DomainError::BlockersPresent => {
            assert!(matches!(actual, PersistedDomainErrorV1::BlockersPresent));
        }
        DomainError::BlockerLimitReached {
            maximum_open_blockers,
        } => match actual {
            PersistedDomainErrorV1::BlockerLimitReached {
                maximum_open_blockers: actual_maximum,
            } => assert_eq!(
                *actual_maximum,
                u64::try_from(*maximum_open_blockers).unwrap()
            ),
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::ArtifactChanged => {
            assert!(matches!(actual, PersistedDomainErrorV1::ArtifactChanged));
        }
        DomainError::InvalidTransition { command, state } => match actual {
            PersistedDomainErrorV1::InvalidTransition {
                command: actual_command,
                state: actual_state,
            } => {
                assert_eq!(*actual_command, expected_persisted_command_kind(*command));
                assert_eq!(*actual_state, expected_persisted_lifecycle(*state));
            }
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::PreconditionFailed {
            expected,
            actual: expected_actual,
        } => match actual {
            PersistedDomainErrorV1::PreconditionFailed {
                expected: actual_expected,
                actual: actual_actual,
            } => {
                assert_eq!(actual_expected, expected);
                assert_eq!(actual_actual, expected_actual);
            }
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::ItemNotFound { item_id } => match actual {
            PersistedDomainErrorV1::ItemNotFound {
                item_id: actual_item_id,
            } => assert_eq!(actual_item_id, item_id),
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::BlockerNotCurrent => {
            assert!(matches!(actual, PersistedDomainErrorV1::BlockerNotCurrent));
        }
        DomainError::SessionIdentityMismatch {
            expected,
            actual: expected_actual,
        } => match actual {
            PersistedDomainErrorV1::SessionIdentityMismatch {
                expected: actual_expected,
                actual: actual_actual,
            } => {
                assert_eq!(actual_expected, expected);
                assert_eq!(actual_actual, expected_actual);
            }
            _ => panic!("decoded error kind differs from the literal golden"),
        },
        DomainError::AttemptNotCurrent {
            expected,
            actual: expected_actual,
        } => match actual {
            PersistedDomainErrorV1::AttemptNotCurrent {
                expected: actual_expected,
                actual: actual_actual,
            } => {
                assert_eq!(actual_expected, expected);
                assert_eq!(actual_actual, expected_actual);
            }
            _ => panic!("decoded error kind differs from the literal golden"),
        },
    }
}

fn assert_success_terminal_golden(
    terminal: &TerminalReceiptV1,
    expected_result: &DomainResult,
    golden: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let legacy_golden = golden.replacen(STORE_TERMINAL_SCHEMA_V1, STORE_TERMINAL_SCHEMA_V0, 1);
    assert_eq!(encode_terminal_receipt_v1(terminal)?, legacy_golden);
    let decoded = decode_terminal_receipt_v1(&legacy_golden)?;
    assert_receipt_fields(decoded.job(), terminal.job());
    assert_terminal_result_variant_coverage(decoded.result());
    match decoded.result() {
        PersistedTerminalResultV1::Success(result) => {
            assert_success_fields(result, expected_result)
        }
        _ => panic!("decoded terminal result kind differs from the literal golden"),
    }
    Ok(())
}

fn assert_failure_terminal_golden(
    terminal: &TerminalReceiptV1,
    expected_error: &DomainError,
    golden: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let legacy_golden = golden.replacen(STORE_TERMINAL_SCHEMA_V1, STORE_TERMINAL_SCHEMA_V0, 1);
    assert_eq!(encode_terminal_receipt_v1(terminal)?, legacy_golden);
    let decoded = decode_terminal_receipt_v1(&legacy_golden)?;
    assert_receipt_fields(decoded.job(), terminal.job());
    assert_terminal_result_variant_coverage(decoded.result());
    match decoded.result() {
        PersistedTerminalResultV1::Failure(error) => assert_failure_fields(error, expected_error),
        _ => panic!("decoded terminal result kind differs from the literal golden"),
    }
    Ok(())
}

fn success_golden_v1(result: &DomainResult) -> &'static str {
    match result {
        DomainResult::WorkspaceInitialized { .. } => {
            r#"{"job":{"identity_sequence":1,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"success","payload":{"kind":"workspace_initialized","revision":1,"workspace_id":"00000000-0000-4000-8000-000000000001"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainResult::WorkspaceReset { .. } => {
            r#"{"job":{"identity_sequence":2,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"success","payload":{"kind":"workspace_reset","revision":2,"workspace_id":"00000000-0000-4000-8000-000000000001"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainResult::SessionChanged { .. } => {
            r#"{"job":{"identity_sequence":3,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"success","payload":{"changed":true,"kind":"session_changed","revision_after":3,"revision_before":2,"session_id":"00000000-0000-4000-8000-000000000004"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainResult::ItemChanged { .. } => {
            r#"{"job":{"identity_sequence":4,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"success","payload":{"changed":false,"item_id":"selected-item","kind":"item_changed","revision_after":4,"revision_before":3,"session_id":"00000000-0000-4000-8000-000000000004"}},"schema":"podway.store-terminal/v1"}"#
        }
    }
}

fn failure_golden_v1(error: &DomainError) -> &'static str {
    match error {
        DomainError::EmptyValue { .. } => {
            r#"{"job":{"identity_sequence":10,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"field":"field","kind":"empty_value"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::ValueTooLong { .. } => {
            r#"{"job":{"identity_sequence":11,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"actual":2,"field":"field","kind":"value_too_long","maximum":1}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::InvalidUuid { .. } => {
            r#"{"job":{"identity_sequence":12,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"field":"field","kind":"invalid_uuid"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::InvalidIdentifier { .. } => {
            r#"{"job":{"identity_sequence":13,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"field":"field","kind":"invalid_identifier"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::InvalidSha256Digest => {
            r#"{"job":{"identity_sequence":14,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"invalid_sha256_digest"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::RevisionOverflow { .. } => {
            r#"{"job":{"identity_sequence":15,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"revision_overflow","revision":9}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::InvalidState { .. } => {
            r#"{"job":{"identity_sequence":16,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"invalid_state","reason":"fixture"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::InvalidTransition { .. } => {
            r#"{"job":{"identity_sequence":17,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"command":"item_check","kind":"invalid_transition","state":"completed"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::PreconditionFailed { .. } => {
            r#"{"job":{"identity_sequence":18,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"actual":11,"expected":10,"kind":"precondition_failed"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::ItemNotFound { .. } => {
            r#"{"job":{"identity_sequence":19,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"item_id":"missing-item","kind":"item_not_found"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::BlockerNotCurrent => {
            r#"{"job":{"identity_sequence":20,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"blocker_not_current"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::RequiredItemsMissing => {
            r#"{"job":{"identity_sequence":21,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"required_items_missing"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::BlockersPresent => {
            r#"{"job":{"identity_sequence":22,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"blockers_present"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::ArtifactChanged => {
            r#"{"job":{"identity_sequence":23,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"artifact_changed"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::SessionIdentityMismatch { .. } => {
            r#"{"job":{"identity_sequence":24,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"actual":"00000000-0000-4000-8000-000000000005","expected":"00000000-0000-4000-8000-000000000004","kind":"session_identity_mismatch"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::AttemptNotCurrent { .. } => {
            r#"{"job":{"identity_sequence":25,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"actual":"00000000-0000-4000-8000-000000000007","expected":"00000000-0000-4000-8000-000000000006","kind":"attempt_not_current"}},"schema":"podway.store-terminal/v1"}"#
        }
        DomainError::BlockerLimitReached { .. } => {
            r#"{"job":{"identity_sequence":26,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"failure","payload":{"kind":"blocker_limit_reached","maximum_open_blockers":1024}},"schema":"podway.store-terminal/v1"}"#
        }
    }
}

#[test]
fn command_codec_matches_independent_literal_goldens_for_every_variant_and_precondition_shape()
-> Result<(), Box<dyn std::error::Error>> {
    let item = ItemId::new("selected-item")?;
    let preconditions = fully_specified_preconditions(&item)?;
    let commands = vec![
        DomainCommand::WorkspaceInitialize,
        DomainCommand::WorkspaceResetAll,
        DomainCommand::SessionStart,
        DomainCommand::SessionStartReplace,
        DomainCommand::SessionComplete,
        DomainCommand::SessionSkip,
        DomainCommand::SessionRetry,
        DomainCommand::SessionReturn,
        DomainCommand::SessionBlock,
        DomainCommand::SessionUnblock,
        DomainCommand::SessionCancel,
        DomainCommand::SessionReopen,
        DomainCommand::SessionReset,
        DomainCommand::ItemCheck {
            item_id: item.clone(),
        },
        DomainCommand::ItemUncheck {
            item_id: item.clone(),
        },
        DomainCommand::ItemSet {
            item_id: item.clone(),
        },
        DomainCommand::ItemAdd {
            item_id: item.clone(),
        },
        DomainCommand::ItemRemove {
            item_id: item.clone(),
        },
        DomainCommand::ItemAttach {
            item_id: item.clone(),
        },
        DomainCommand::ItemClear {
            item_id: item.clone(),
        },
    ];
    for command in commands {
        let execution = ClaimedExecutionV1::new(command, preconditions.clone());
        assert_command_golden(&execution, command_golden_v1(execution.command()))?;
    }

    let optional_shapes = vec![
        (
            None,
            None,
            None,
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        ),
        (
            Some(Revision::new(11)),
            None,
            None,
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
        ),
        (
            None,
            Some(attempt_id()),
            None,
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        ),
        (
            Some(Revision::new(11)),
            Some(attempt_id()),
            None,
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":null,"expected_item_revision":null,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
        ),
        (
            None,
            None,
            Some(item.clone()),
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":"selected-item","expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        ),
        (
            Some(Revision::new(11)),
            None,
            Some(item.clone()),
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":"selected-item","expected_item_revision":null,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
        ),
        (
            None,
            Some(attempt_id()),
            Some(item.clone()),
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        ),
        (
            Some(Revision::new(11)),
            Some(attempt_id()),
            Some(item.clone()),
            None,
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":null,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
        ),
        (
            None,
            None,
            Some(item.clone()),
            Some(Revision::new(7)),
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        ),
        (
            Some(Revision::new(11)),
            None,
            Some(item.clone()),
            Some(Revision::new(7)),
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
        ),
        (
            None,
            Some(attempt_id()),
            Some(item.clone()),
            Some(Revision::new(7)),
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        ),
        (
            Some(Revision::new(11)),
            Some(attempt_id()),
            Some(item.clone()),
            Some(Revision::new(7)),
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":"selected-item","expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
        ),
    ];
    for (session_revision, attempt, item_id, item_revision, golden) in optional_shapes {
        let preconditions = RevisionAttemptItemPreconditionsV1::new(
            session_revision,
            attempt,
            item_id,
            item_revision,
        )?;
        assert_command_golden(
            &ClaimedExecutionV1::new(DomainCommand::WorkspaceInitialize, preconditions),
            golden,
        )?;
    }

    for malformed_optional_shape in [
        r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":7,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":null,"expected_item_revision":7,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
        r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000005","expected_item_id":null,"expected_item_revision":7,"expected_session_revision":11},"schema":"podway.store-command/v1"}"#,
    ] {
        assert!(decode_command_v1(malformed_optional_shape).is_err());
    }

    let encoded = command_golden_v1(&DomainCommand::WorkspaceInitialize);
    assert!(
        decode_command_v1(
            &encoded.replacen(STORE_COMMAND_SCHEMA_V1, "podway.store-command/v2", 1,)
        )
        .is_err()
    );
    assert_codec_error(
        decode_command_v1(&encoded.replacen(STORE_COMMAND_SCHEMA_V1, "podway.store-command/v3", 1)),
        StoreCodecErrorV1::UnsupportedSchema {
            expected: STORE_COMMAND_SCHEMA_V2,
            found: "podway.store-command/v3".to_owned(),
        },
    );
    assert!(
        decode_command_v1(&encoded.replacen("\"schema\"", "\"unexpected\":true,\"schema\"", 1,))
            .is_err()
    );
    assert!(decode_command_v1(&encoded.replacen("workspace_initialize", "unknown", 1)).is_err());
    assert!(decode_command_v1(&encoded.replacen("selected-item", "Invalid", 1)).is_err());
    Ok(())
}

#[test]
fn terminal_codec_matches_independent_literal_goldens_for_results_errors_and_cancellation()
-> Result<(), Box<dyn std::error::Error>> {
    let results = vec![
        DomainResult::WorkspaceInitialized {
            workspace_id: workspace_id(),
            revision: Revision::new(1),
        },
        DomainResult::WorkspaceReset {
            workspace_id: workspace_id(),
            revision: Revision::new(2),
        },
        DomainResult::SessionChanged {
            session_id: session_id(),
            revision_before: Revision::new(2),
            revision_after: Revision::new(3),
            changed: true,
        },
        DomainResult::ItemChanged {
            session_id: session_id(),
            item_id: ItemId::new("selected-item")?,
            revision_before: Revision::new(3),
            revision_after: Revision::new(4),
            changed: false,
        },
    ];
    for (index, result) in results.into_iter().enumerate() {
        let sequence = u64::try_from(index + 1)?;
        let terminal =
            TerminalReceiptV1::new(receipt(sequence), TerminalResultV1::Success(result.clone()));
        assert_success_terminal_golden(&terminal, &result, success_golden_v1(&result))?;
    }

    let errors = vec![
        DomainError::EmptyValue { field: "field" },
        DomainError::ValueTooLong {
            field: "field",
            maximum: 1,
            actual: 2,
        },
        DomainError::InvalidUuid { field: "field" },
        DomainError::InvalidIdentifier { field: "field" },
        DomainError::InvalidSha256Digest,
        DomainError::RevisionOverflow {
            revision: Revision::new(9),
        },
        DomainError::InvalidState { reason: "fixture" },
        DomainError::InvalidTransition {
            command: DomainCommandKind::ItemCheck,
            state: SessionLifecycle::Completed,
        },
        DomainError::PreconditionFailed {
            expected: Revision::new(10),
            actual: Revision::new(11),
        },
        DomainError::ItemNotFound {
            item_id: ItemId::new("missing-item")?,
        },
        DomainError::BlockerNotCurrent,
        DomainError::RequiredItemsMissing,
        DomainError::BlockersPresent,
        DomainError::ArtifactChanged,
        DomainError::SessionIdentityMismatch {
            expected: SessionId::new("00000000-0000-4000-8000-000000000004")?,
            actual: Some(SessionId::new("00000000-0000-4000-8000-000000000005")?),
        },
        DomainError::AttemptNotCurrent {
            expected: AttemptId::new("00000000-0000-4000-8000-000000000006")?,
            actual: Some(AttemptId::new("00000000-0000-4000-8000-000000000007")?),
        },
        DomainError::BlockerLimitReached {
            maximum_open_blockers: 1_024,
        },
    ];
    for (index, error) in errors.into_iter().enumerate() {
        let sequence = u64::try_from(index + 10)?;
        let terminal =
            TerminalReceiptV1::new(receipt(sequence), TerminalResultV1::Failure(error.clone()));
        assert_failure_terminal_golden(&terminal, &error, failure_golden_v1(&error))?;
    }

    let cancelled = PersistedTerminalReceiptV1::cancelled(receipt(99));
    let cancelled_golden = r#"{"job":{"identity_sequence":99,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"cancelled"},"schema":"podway.store-terminal/v0"}"#;
    assert_eq!(
        encode_persisted_terminal_receipt_v1(&cancelled)?,
        cancelled_golden
    );
    let decoded = decode_terminal_receipt_v1(cancelled_golden)?;
    assert_receipt_fields(decoded.job(), cancelled.job());
    assert_terminal_result_variant_coverage(decoded.result());
    assert!(matches!(
        decoded.result(),
        PersistedTerminalResultV1::Cancelled
    ));
    assert!(decoded.job_projection().is_none());
    assert!(decoded.session_projection().is_none());
    assert!(
        decode_terminal_receipt_v1(&cancelled_golden.replacen(
            STORE_TERMINAL_SCHEMA_V0,
            "podway.store-terminal/v2",
            1,
        ))
        .is_err()
    );
    assert!(
        decode_terminal_receipt_v1(&cancelled_golden.replacen(
            "\"schema\"",
            "\"unexpected\":true,\"schema\"",
            1,
        ))
        .is_err()
    );
    assert!(
        decode_terminal_receipt_v1(&cancelled_golden.replacen("\"cancelled\"", "\"unknown\"", 1))
            .is_err()
    );
    let invalid_digest = format!("sha256:{}", "z".repeat(64));
    assert!(
        decode_terminal_receipt_v1(&cancelled_golden.replacen(
            digest('d').as_str(),
            &invalid_digest,
            1,
        ))
        .is_err()
    );
    Ok(())
}
#[test]
fn terminal_codec_preserves_legacy_v0_literals_and_requires_v1_replay_projections()
-> Result<(), Box<dyn std::error::Error>> {
    let legacy = r#"{"job":{"identity_sequence":99,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"cancelled"},"schema":"podway.store-terminal/v0"}"#;
    let legacy_receipt = decode_terminal_receipt_v1(legacy)?;
    assert!(legacy_receipt.job_projection().is_none());
    assert!(legacy_receipt.session_projection().is_none());
    assert_eq!(
        encode_persisted_terminal_receipt_v1(&legacy_receipt)?,
        legacy
    );
    assert_codec_error(
        decode_terminal_receipt_v1(&legacy.replacen(
            STORE_TERMINAL_SCHEMA_V0,
            STORE_TERMINAL_SCHEMA_V1,
            1,
        )),
        StoreCodecErrorV1::InvalidJson,
    );

    let session_id = session_id();
    let enriched = PersistedTerminalReceiptV1::new_with_projections(
        receipt(100),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
            session_id: session_id.clone(),
            revision_before: Revision::new(2),
            revision_after: Revision::new(3),
            changed: true,
        }),
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Succeeded,
            UnixMillis::new(10),
            Some(UnixMillis::new(11)),
            UnixMillis::new(12),
        )?,
        Some(PersistedTerminalSessionProjectionV1::new(
            session_id.clone(),
            "Fixture task".to_owned(),
            PersistedSessionLifecycleV1::Completed,
            Revision::new(2),
            Revision::new(3),
        )?),
    )?;
    let golden = r#"{"job":{"identity_sequence":100,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"job_projection":{"claimed_at":11,"finished_at":12,"state":"succeeded","submitted_at":10},"result":{"kind":"success","payload":{"changed":true,"kind":"session_changed","revision_after":3,"revision_before":2,"session_id":"00000000-0000-4000-8000-000000000004"}},"schema":"podway.store-terminal/v1","session_projection":{"lifecycle":"completed","revision_after":3,"revision_before":2,"session_id":"00000000-0000-4000-8000-000000000004","task_title":"Fixture task"}}"#;
    assert_eq!(encode_persisted_terminal_receipt_v1(&enriched)?, golden);
    let decoded = decode_terminal_receipt_v1(golden)?;
    assert_eq!(decoded, enriched);
    assert!(decoded.start_identity().is_none());
    let start_identity = PersistedStartIdentityV1::new(5, digest('e'))?;
    let retained = enriched.clone().with_start_identity(start_identity.clone());
    let retained_json = encode_persisted_terminal_receipt_v1(&retained)?;
    let retained_decoded = decode_terminal_receipt_v1(&retained_json)?;
    assert_eq!(retained_decoded, retained);
    assert_eq!(retained_decoded.start_identity(), Some(&start_identity));
    let job_projection = decoded.job_projection().expect("projection is present");
    assert_eq!(
        job_projection.state(),
        PersistedTerminalJobStateV1::Succeeded
    );
    assert_eq!(job_projection.submitted_at(), UnixMillis::new(10));
    assert_eq!(job_projection.claimed_at(), Some(UnixMillis::new(11)));
    assert_eq!(job_projection.finished_at(), UnixMillis::new(12));
    let session_projection = decoded
        .session_projection()
        .expect("session projection is present");
    assert_eq!(session_projection.session_id(), &session_id);
    assert_eq!(session_projection.task_title(), "Fixture task");
    assert_eq!(
        session_projection.lifecycle(),
        PersistedSessionLifecycleV1::Completed
    );
    assert_eq!(session_projection.revision_before(), Revision::new(2));
    assert_eq!(session_projection.revision_after(), Revision::new(3));
    assert_codec_error(
        PersistedTerminalReceiptV1::new_with_projections(
            receipt(101),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                session_id: session_id.clone(),
                revision_before: Revision::new(2),
                revision_after: Revision::new(3),
                changed: true,
            }),
            PersistedTerminalJobProjectionV1::new(
                PersistedTerminalJobStateV1::Succeeded,
                UnixMillis::new(10),
                Some(UnixMillis::new(11)),
                UnixMillis::new(12),
            )?,
            None,
        ),
        StoreCodecErrorV1::InvalidValue {
            field: "terminal session projection",
        },
    );
    assert_codec_error(
        decode_terminal_receipt_v1(&golden.replacen("\"changed\":true", "\"changed\":false", 1)),
        StoreCodecErrorV1::InvalidValue {
            field: "terminal session projection",
        },
    );
    assert_codec_error(
        decode_terminal_receipt_v1(&golden.replace("\"revision_after\":3", "\"revision_after\":2")),
        StoreCodecErrorV1::InvalidValue {
            field: "terminal session projection",
        },
    );

    assert!(
        decode_terminal_receipt_v1(&golden.replacen("\"claimed_at\":11", "\"claimed_at\":13", 1,))
            .is_err()
    );
    assert!(
        decode_terminal_receipt_v1(&golden.replacen(
            "\"task_title\":\"Fixture task\"",
            "\"task_title\":\" \"",
            1,
        ))
        .is_err()
    );
    assert!(
        decode_terminal_receipt_v1(&golden.replacen(
            "\"session_projection\":{\"lifecycle\":\"completed\",\"revision_after\":3",
            "\"session_projection\":{\"lifecycle\":\"completed\",\"revision_after\":1",
            1,
        ))
        .is_err()
    );
    assert!(
        decode_terminal_receipt_v1(&golden.replacen(
            "\"job_projection\":{",
            "\"job_projection\":{\"unexpected\":true,",
            1,
        ))
        .is_err()
    );
    Ok(())
}

#[test]
fn terminal_v2_codec_round_trips_lookup_command_and_preserves_legacy_literals()
-> Result<(), Box<dyn std::error::Error>> {
    let legacy = r#"{"job":{"identity_sequence":99,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"result":{"kind":"cancelled"},"schema":"podway.store-terminal/v0"}"#;
    assert_eq!(
        encode_persisted_terminal_receipt_v1(&decode_terminal_receipt_v1(legacy)?)?,
        legacy
    );

    let session_id = session_id();
    let receipt = PersistedTerminalReceiptV1::new_with_projections(
        receipt(101),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
            session_id: session_id.clone(),
            revision_before: Revision::new(3),
            revision_after: Revision::new(4),
            changed: true,
        }),
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Succeeded,
            UnixMillis::new(20),
            Some(UnixMillis::new(21)),
            UnixMillis::new(22),
        )?,
        Some(PersistedTerminalSessionProjectionV1::new(
            session_id,
            "Lookup-safe task".to_owned(),
            PersistedSessionLifecycleV1::Running,
            Revision::new(3),
            Revision::new(4),
        )?),
    )?
    .with_lookup_command(PersistedDomainCommandV1::SessionComplete)?;
    let encoded = encode_persisted_terminal_receipt_v1(&receipt)?;
    assert!(encoded.contains(STORE_TERMINAL_SCHEMA_V2));
    assert!(encoded.contains(r#""command":{"kind":"session_complete"}"#));
    assert!(!encoded.contains("canonical_request"));
    assert_eq!(decode_terminal_receipt_v1(&encoded)?, receipt);

    let missing_command = encoded.replacen(r#""command":{"kind":"session_complete"},"#, "", 1);
    assert!(decode_terminal_receipt_v1(&missing_command).is_err());
    let mismatched_command = encoded.replacen(
        r#""command":{"kind":"session_complete"}"#,
        r#""command":{"kind":"workspace_initialize"}"#,
        1,
    );
    assert!(decode_terminal_receipt_v1(&mismatched_command).is_err());
    let unknown_field = encoded.replacen(
        r#""command":{"kind":"session_complete"}"#,
        r#""command":{"kind":"session_complete","unknown":true}"#,
        1,
    );
    assert!(decode_terminal_receipt_v1(&unknown_field).is_err());
    Ok(())
}
#[test]
fn terminal_v1_codec_rejects_inconsistent_job_and_session_projections() {
    let timestamp_error = StoreCodecErrorV1::InvalidValue {
        field: "terminal job timestamps",
    };
    for state in [
        PersistedTerminalJobStateV1::Succeeded,
        PersistedTerminalJobStateV1::Failed,
    ] {
        assert!(
            PersistedTerminalJobProjectionV1::new(
                state,
                UnixMillis::new(10),
                None,
                UnixMillis::new(12),
            )
            .is_ok(),
            "successful and failed administrative terminals may complete without a claim"
        );
    }
    assert_codec_error(
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Cancelled,
            UnixMillis::new(10),
            Some(UnixMillis::new(11)),
            UnixMillis::new(12),
        ),
        timestamp_error.clone(),
    );
    assert_codec_error(
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Succeeded,
            UnixMillis::new(12),
            Some(UnixMillis::new(11)),
            UnixMillis::new(10),
        ),
        timestamp_error.clone(),
    );

    let terminal_v1 = |state: &str, claimed_at: &str, result: &str| {
        format!(
            r#"{{"job":{{"identity_sequence":101,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}},"job_projection":{{{claimed_at}"finished_at":12,"state":"{state}","submitted_at":10}},"result":{result},"schema":"podway.store-terminal/v1"}}"#
        )
    };
    let cleared_session = terminal_v1(
        "succeeded",
        r#""claimed_at":11,"#,
        r#"{"kind":"success","payload":{"changed":true,"kind":"session_changed","revision_after":0,"revision_before":2,"session_id":"00000000-0000-4000-8000-000000000004"}}"#,
    );
    let decoded_cleared_session = decode_terminal_receipt_v1(&cleared_session)
        .expect("session-clearing terminal receipt must decode");
    assert!(decoded_cleared_session.session_projection().is_none());

    assert_codec_error(
        decode_terminal_receipt_v1(&terminal_v1(
            "cancelled",
            r#""claimed_at":11,"#,
            r#"{"kind":"cancelled"}"#,
        )),
        StoreCodecErrorV1::InvalidValue {
            field: "terminal job timestamps",
        },
    );

    for malformed in [
        terminal_v1(
            "succeeded",
            r#""claimed_at":11,"#,
            r#"{"kind":"success","payload":{"changed":true,"item_id":"selected-item","kind":"item_changed","revision_after":0,"revision_before":2,"session_id":"00000000-0000-4000-8000-000000000004"}}"#,
        ),
        terminal_v1(
            "succeeded",
            r#""claimed_at":11,"#,
            r#"{"kind":"success","payload":{"changed":false,"kind":"session_changed","revision_after":0,"revision_before":2,"session_id":"00000000-0000-4000-8000-000000000004"}}"#,
        ),
        terminal_v1(
            "succeeded",
            r#""claimed_at":11,"#,
            r#"{"kind":"success","payload":{"changed":true,"kind":"session_changed","revision_after":0,"revision_before":0,"session_id":"00000000-0000-4000-8000-000000000004"}}"#,
        ),
    ] {
        assert_codec_error(
            decode_terminal_receipt_v1(&malformed),
            StoreCodecErrorV1::InvalidValue {
                field: "terminal session projection",
            },
        );
    }
}
#[test]
fn codecs_reject_unknown_fields_in_nested_v1_objects_with_exact_classification() {
    assert_codec_error(
        decode_command_v1(
            r#"{"command":{"kind":"workspace_initialize","unexpected":true},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
        ),
        StoreCodecErrorV1::InvalidValue {
            field: "canonical command",
        },
    );
    assert_codec_error(
        decode_command_v1(
            r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null,"unexpected":true},"schema":"podway.store-command/v1"}"#,
        ),
        StoreCodecErrorV1::InvalidJson,
    );
    assert_codec_error(
        decode_terminal_receipt_v1(
            r#"{"job":{"identity_sequence":1,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","unexpected":true},"job_projection":{"finished_at":12,"state":"cancelled","submitted_at":10},"result":{"kind":"cancelled"},"schema":"podway.store-terminal/v1"}"#,
        ),
        StoreCodecErrorV1::InvalidJson,
    );
    assert_codec_error(
        decode_terminal_receipt_v1(
            r#"{"job":{"identity_sequence":1,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"job_projection":{"finished_at":12,"state":"cancelled","submitted_at":10},"result":{"kind":"cancelled","unexpected":true},"schema":"podway.store-terminal/v1"}"#,
        ),
        StoreCodecErrorV1::InvalidJson,
    );
    assert_codec_error(
        decode_terminal_receipt_v1(
            r#"{"job":{"identity_sequence":1,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"job_projection":{"claimed_at":11,"finished_at":12,"state":"succeeded","submitted_at":10},"result":{"kind":"success","payload":{"kind":"workspace_initialized","revision":1,"unexpected":true,"workspace_id":"00000000-0000-4000-8000-000000000001"}},"schema":"podway.store-terminal/v1"}"#,
        ),
        StoreCodecErrorV1::InvalidJson,
    );
    assert_codec_error(
        decode_terminal_receipt_v1(
            r#"{"job":{"identity_sequence":10,"job_id":"00000000-0000-4000-8000-000000000003","request_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"job_projection":{"claimed_at":11,"finished_at":12,"state":"failed","submitted_at":10},"result":{"kind":"failure","payload":{"field":"field","kind":"empty_value","unexpected":true}},"schema":"podway.store-terminal/v1"}"#,
        ),
        StoreCodecErrorV1::InvalidJson,
    );
}

#[test]
fn additive_production_apis_preserve_phase0_construction_and_session_revision_invariant()
-> Result<(), Box<dyn std::error::Error>> {
    let preconditions = RevisionAttemptItemPreconditionsV1::new(None, None, None, None)?;
    assert_eq!(options().busy_timeout_ms(), 5_000);
    assert!(options().with_busy_timeout_ms(5_001).is_err());
    let claim = ClaimTokenV1::new(
        identity(),
        job_id(),
        Revision::new(1),
        WorkerIdV1::new("worker")?,
    );
    let persisted = ClaimedJobV1::new_persisted(
        claim,
        receipt(1),
        ClaimedExecutionV1::new(DomainCommand::WorkspaceInitialize, preconditions),
        None,
    );
    assert_eq!(
        persisted.execution().command(),
        &DomainCommand::WorkspaceInitialize
    );
    assert!(persisted.current_session().is_none());

    let unchanged_transition = StateTransitionV1::new_persisted(
        None,
        Revision::new(1),
        Revision::new(1),
        PersistedSessionMutationV1::Unchanged,
    )?;
    assert_eq!(
        unchanged_transition.persisted_session_mutation(),
        &PersistedSessionMutationV1::Unchanged
    );
    let clear_transition = StateTransitionV1::new_persisted(
        None,
        Revision::new(4),
        Revision::ZERO,
        PersistedSessionMutationV1::Clear,
    )?;
    assert_eq!(
        clear_transition.resulting_workspace_revision(),
        Revision::ZERO
    );
    assert!(clear_transition.session_id().is_none());
    assert!(matches!(
        clear_transition.persisted_session_mutation(),
        PersistedSessionMutationV1::Clear
    ));
    Ok(())
}
