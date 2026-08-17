use std::path::Path;
#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};

use podway_core::{
    DomainCommand, JobId, Revision, SessionId, Sha256Digest, UnixMillis, WorkspaceId,
    canonicalize_json_v1,
};
use podway_store::codec::{
    PersistedDomainCommandV1, PersistedDomainResultV1, PersistedSessionLifecycleV1,
    PersistedTerminalResultV1, encode_command_v1, encode_persisted_terminal_receipt_v1,
};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitRequestV1, CancelOutcomeV1, CanonicalExecutionJsonV1,
    ClaimedExecutionV1, DurableExecutionFlavorV1, DurableWorktreeIdentityV1, EpochMillisV1,
    GraphSessionStateV2, IdempotencyKeyV1, JobReceiptV1, JobStateV1,
    PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1, PersistedTerminalReceiptV1,
    PersistedTerminalSessionProjectionV1, RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1,
    SqliteStoreV1, StoreContractV1, StoreErrorV1, StoreFailpointV1, StoreGraphStateContractV2,
    StoreIdempotencyReadContractV1, StoreReadContractV1, StoreUnavailableReasonV1,
    ValidatedWorkspaceRootV1,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use tempfile::TempDir;

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-v2-only-schema")).unwrap()
}

fn uuid(number: u64) -> String {
    format!("00000000-0000-4000-8000-{number:012x}")
}

fn populated_graph_state() -> GraphSessionStateV2 {
    crate::int_v2_goal_state::rich_v2_state_for_schema_migration()
}

fn restore_schema_v3_shape(connection: &Connection) {
    let reference = Connection::open_in_memory().unwrap();
    reference
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v1.sql"))
        .unwrap();
    let mut statement = reference
        .prepare(
            "SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL AND (\
             (type = 'table' AND name IN ('procedure_snapshots', 'task_sessions', \
             'stage_progress', 'attempts', 'item_slots', 'blockers')) OR \
             (type = 'index' AND name IN ('ux_stage_progress_one_current', \
             'ux_attempts_one_active', 'ix_attempts_stage', 'ix_blockers_attempt_state'))) \
             ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, name",
        )
        .unwrap();
    let objects = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for sql in objects {
        connection.execute_batch(&sql).unwrap();
    }
    connection
        .execute("DELETE FROM schema_migrations WHERE version = 4", [])
        .unwrap();
    connection.pragma_update(None, "user_version", 3).unwrap();
}

fn schema_objects(connection: &Connection) -> Vec<(String, String, String, Option<String>)> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name, tbl_name, sql",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn assert_schema_v3_shape(connection: &Connection) {
    let reference = Connection::open_in_memory().unwrap();
    reference
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v1.sql"))
        .unwrap();
    reference
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v2.sql"))
        .unwrap();
    reference
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v3.sql"))
        .unwrap();
    let actual = schema_objects(connection);
    let expected = schema_objects(&reference);
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(actual, expected, "schema object {} differs", expected.1);
    }
}

fn canonical_schema_v4(lifecycle: &str) -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    for ddl in [
        include_str!("../../../assets/specifications/sqlite-v1.sql"),
        include_str!("../../../assets/specifications/sqlite-v2.sql"),
        include_str!("../../../assets/specifications/sqlite-v3.sql"),
        include_str!("../../../assets/specifications/sqlite-v4.sql"),
    ] {
        connection.execute_batch(ddl).unwrap();
    }
    connection
        .execute(
            "INSERT INTO v2_procedure_snapshots (
                snapshot_id, schema_id, procedure_id, procedure_version, name, purpose,
                digest, canonical_json, source_kind, source_label, goal_tracking,
                created_at_ms
             ) VALUES ('snapshot', 'podway.procedure/v2', 'workflow', '1', 'Workflow',
                       'Migration fixture', ?1, '{}', 'file', 'workflow.yaml', 0, 1)",
            [digest('a').as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_graph_nodes (
                snapshot_id, graph_node_id, node_definition_id, placement_index,
                node_type, goal_assessment, canonical_placement_json
             ) VALUES ('snapshot', 'work', 'work', 0, 'action', 0, '{}')",
            [],
        )
        .unwrap();

    let (active_graph_node_id, active_attempt_id, active_trace_sequence) = if lifecycle == "running"
    {
        (Some("work"), Some("attempt"), Some(1_i64))
    } else {
        (None, None, None)
    };
    let completed_at_ms = (lifecycle == "completed").then_some(2_i64);
    let cancelled_at_ms = (lifecycle == "cancelled").then_some(2_i64);
    let cancel_reason = (lifecycle == "cancelled").then_some("cancelled");
    connection
        .execute(
            "INSERT INTO v2_task_sessions (
                singleton, session_id, task_title, procedure_snapshot_id, lifecycle,
                session_revision, latest_trace_sequence, active_graph_node_id,
                active_attempt_id, active_trace_sequence, goal_tracking,
                current_goal_revision, created_at_ms, completed_at_ms, cancelled_at_ms,
                cancel_reason
             ) VALUES (1, ?1, 'Migration fixture', 'snapshot', ?2, 7, 1, ?3, ?4, ?5,
                       0, NULL, 1, ?6, ?7, ?8)",
            params![
                uuid(700),
                lifecycle,
                active_graph_node_id,
                active_attempt_id,
                active_trace_sequence,
                completed_at_ms,
                cancelled_at_ms,
                cancel_reason,
            ],
        )
        .unwrap();
    if lifecycle == "running" {
        connection
            .execute(
                "INSERT INTO v2_attempts (
                    attempt_id, session_id, snapshot_id, graph_node_id, node_definition_id,
                    attempt_number, trace_sequence, lifecycle, validity, goal_revision,
                    started_at_ms, ended_at_ms, terminal_reason
                 ) VALUES ('attempt', ?1, 'snapshot', 'work', 'work', 1, 1, 'active',
                           'valid', NULL, 1, NULL, NULL)",
                [uuid(700)],
            )
            .unwrap();
    }
    connection
}

fn apply_reserved_schema_v5(connection: &mut Connection) {
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v5.sql"))
        .unwrap();
    let foreign_key_violations: i64 = transaction
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(foreign_key_violations, 0);
    transaction.commit().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
}

#[test]
fn v2lif002_sqlite_v5_contract_preserves_v4_sessions_and_reserves_prepared_storage() {
    for lifecycle in ["running", "completed", "cancelled"] {
        let mut connection = canonical_schema_v4(lifecycle);
        let before: (
            String,
            i64,
            i64,
            Option<String>,
            Option<String>,
            Option<i64>,
        ) = connection
            .query_row(
                "SELECT lifecycle, session_revision, latest_trace_sequence,
                        active_graph_node_id, active_attempt_id, active_trace_sequence
                 FROM v2_task_sessions",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();

        apply_reserved_schema_v5(&mut connection);

        let after: (
            String,
            i64,
            i64,
            Option<String>,
            Option<String>,
            Option<i64>,
        ) = connection
            .query_row(
                "SELECT lifecycle, session_revision, latest_trace_sequence,
                        active_graph_node_id, active_attempt_id, active_trace_sequence
                 FROM v2_task_sessions",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        let (version, attempts, foreign_key_violations): (i64, i64, i64) = connection
            .query_row(
                "SELECT (SELECT user_version FROM pragma_user_version),
                        (SELECT COUNT(*) FROM v2_attempts),
                        (SELECT COUNT(*) FROM pragma_foreign_key_check)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(after, before, "v5 changed the {lifecycle} v4 session");
        assert_eq!(version, 5);
        assert_eq!(attempts, i64::from(lifecycle == "running"));
        assert_eq!(foreign_key_violations, 0);
    }

    let mut connection = canonical_schema_v4("completed");
    apply_reserved_schema_v5(&mut connection);
    connection
        .execute("DELETE FROM v2_task_sessions", [])
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_task_sessions (
                singleton, session_id, task_title, procedure_snapshot_id, lifecycle,
                session_revision, latest_trace_sequence, active_graph_node_id,
                active_attempt_id, active_trace_sequence, goal_tracking,
                current_goal_revision, created_at_ms, completed_at_ms, cancelled_at_ms,
                cancel_reason
             ) VALUES (1, ?1, 'Prepared fixture', 'snapshot', 'prepared', 0, 0,
                       NULL, NULL, NULL, 1, NULL, 1, NULL, NULL, NULL)",
            [uuid(701)],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE v2_task_sessions SET session_revision = 1 WHERE singleton = 1",
                [],
            )
            .is_err(),
        "prepared storage must reject a nonzero revision"
    );
    for mutation in [
        "UPDATE v2_task_sessions SET latest_trace_sequence = 1 WHERE singleton = 1",
        "UPDATE v2_task_sessions SET active_graph_node_id = 'work' WHERE singleton = 1",
        "UPDATE v2_task_sessions SET completed_at_ms = 2 WHERE singleton = 1",
    ] {
        assert!(
            connection.execute(mutation, []).is_err(),
            "prepared storage accepted forbidden state: {mutation}"
        );
    }

    connection
        .execute(
            "UPDATE v2_task_sessions
             SET lifecycle = 'completed', session_revision = 8, completed_at_ms = 2
             WHERE singleton = 1",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_terminal_dispositions (
                session_id, terminal_session_revision, kind, summary,
                stable_reference, reason, actor, recorded_at_ms
             ) VALUES (?1, 8, 'handed_off', 'Delivered', 'commit:abc', NULL,
                       'master', 3)",
            [uuid(701)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_terminal_dispositions (
                session_id, terminal_session_revision, kind, summary,
                stable_reference, reason, actor, recorded_at_ms
             ) VALUES (?1, 9, 'not_required', NULL, NULL, 'No handoff needed',
                       NULL, 4)",
            [uuid(701)],
        )
        .unwrap();
    let oversized_reason = "x".repeat(4001);
    let oversized_actor = "x".repeat(257);
    for (revision, kind, summary, reference, reason, actor) in [
        (
            0_i64,
            "handed_off",
            Some("Delivered"),
            Some("commit:abc"),
            None,
            None,
        ),
        (10, "handed_off", Some("Delivered"), None, None, None),
        (
            11,
            "not_required",
            None,
            None,
            Some(oversized_reason.as_str()),
            None,
        ),
        (
            12,
            "handed_off",
            Some("Delivered"),
            Some("commit:abc"),
            None,
            Some(oversized_actor.as_str()),
        ),
    ] {
        assert!(
            connection
                .execute(
                    "INSERT INTO v2_terminal_dispositions (
                        session_id, terminal_session_revision, kind, summary,
                        stable_reference, reason, actor, recorded_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 5)",
                    params![uuid(701), revision, kind, summary, reference, reason, actor],
                )
                .is_err(),
            "terminal disposition storage accepted an invalid row"
        );
    }
    assert!(
        connection
            .execute(
                "UPDATE v2_task_sessions
                 SET lifecycle = 'running', session_revision = 0,
                     active_graph_node_id = 'work', active_attempt_id = 'attempt',
                     active_trace_sequence = 1, completed_at_ms = NULL
                 WHERE singleton = 1",
                [],
            )
            .is_err(),
        "running storage must reject revision zero"
    );
    connection
        .execute("DELETE FROM v2_task_sessions", [])
        .unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM v2_terminal_dispositions", [], |row| {
                row.get::<_, i64>(0)
            },)
            .unwrap(),
        0,
        "reset must cascade terminal disposition history"
    );
}

fn seed_populated_v2_schema_v3(
    temporary: &TempDir,
) -> (GraphSessionStateV2, JobId, PersistedTerminalReceiptV1) {
    let path = temporary.path().join("state.sqlite3");
    let store = SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(1),
    )
    .unwrap();
    let state = populated_graph_state();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "session.start",
            "execution_version": 6,
            "procedure": {"canonical": true}
        }))
        .unwrap(),
    )
    .unwrap();
    let mut terminal_job_id = None;
    for sequence in 1..state.workspace_revision().get() {
        let job_id = JobId::new(uuid(404 + sequence)).unwrap();
        let request = AdmitRequestV1::new_with_canonical_execution(
            DomainCommand::SessionStart,
            IdempotencyKeyV1::new(format!("schema-v4-preservation-{sequence}")).unwrap(),
            job_id.clone(),
            RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
            Sha256Digest::new(format!("sha256:{sequence:064x}")).unwrap(),
            UnixMillis::new(20 + sequence),
            execution.clone(),
        )
        .with_procedure_v2_execution()
        .with_session_identity(AdmissionSessionIdentityV1::Absent);
        store.admit(&identity(), request).unwrap();
        assert!(matches!(
            store
                .cancel_before_claim(
                    &identity(),
                    job_id.clone(),
                    Revision::new(sequence),
                    UnixMillis::new(100 + sequence),
                )
                .unwrap(),
            CancelOutcomeV1::Cancelled(_)
        ));
        terminal_job_id = Some(job_id);
    }
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let job_id = terminal_job_id.unwrap();
    let receipt = store
        .read_job(&identity(), &job_id)
        .unwrap()
        .unwrap()
        .terminal_receipt()
        .unwrap()
        .clone();
    drop(store);

    let connection = Connection::open(path).unwrap();
    restore_schema_v3_shape(&connection);
    assert_schema_v3_shape(&connection);
    drop(connection);
    (state, job_id, receipt)
}

fn seed_schema_v3(temporary: &TempDir, with_legacy_state: bool) {
    let path = temporary.path().join("state.sqlite3");
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v1.sql"))
        .unwrap();
    connection
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v2.sql"))
        .unwrap();
    connection
        .execute_batch(include_str!("../../../assets/specifications/sqlite-v3.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO workspace_state (
                singleton, workspace_uuid, git_common_fingerprint, git_worktree_fingerprint,
                last_validated_root, created_at_ms, updated_at_ms
             ) VALUES (1, ?1, ?2, ?3, ?4, 1, 1)",
            params![
                identity().workspace_uuid().as_str(),
                identity().common_dir_identity().as_str(),
                identity().worktree_admin_identity().as_str(),
                root().as_encoded(),
            ],
        )
        .unwrap();
    for (version, name, checksum) in [
        (
            1,
            podway_store::schema::SQLITE_INITIAL_MIGRATION_NAME_V1,
            podway_store::schema::sqlite_v1_ddl_checksum(),
        ),
        (
            2,
            podway_store::schema::SQLITE_RESPONSE_CONTEXT_MIGRATION_NAME_V2,
            podway_store::schema::sqlite_v2_ddl_checksum(),
        ),
        (
            3,
            podway_store::schema::SQLITE_PROCEDURE_V2_STATE_MIGRATION_NAME_V3,
            podway_store::schema::sqlite_v3_ddl_checksum(),
        ),
    ] {
        connection
            .execute(
                "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms)
                 VALUES (?1, ?2, ?3, 1)",
                params![version, name, checksum],
            )
            .unwrap();
    }
    if with_legacy_state {
        connection
            .execute(
                "INSERT INTO procedure_snapshots (
                    snapshot_id, schema_id, procedure_id, procedure_version, name, digest,
                    canonical_json, source_kind, source_label, created_at_ms
                 ) VALUES ('legacy', 'podway.procedure/v1', 'legacy', '1', 'Legacy', ?1,
                           '{}', 'file', 'legacy.yaml', 1)",
                [digest('c').as_str()],
            )
            .unwrap();
    }
    drop(connection);
    #[cfg(unix)]
    fs::set_permissions(
        temporary.path().join("state.sqlite3"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
}

#[test]
fn schema_v3_without_legacy_state_migrates_to_v2_only_schema_v4() {
    let temporary = TempDir::new().unwrap();
    seed_schema_v3(&temporary, false);
    let path = temporary.path().join("state.sqlite3");
    let predecessor = fs::read(&path).unwrap();

    let binding =
        SqliteStoreV1::inspect_workspace_binding(&path, &SqliteStoreOptionsV1::new(8).unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(binding.identity(), &identity());
    assert_eq!(fs::read(&path).unwrap(), predecessor);

    drop(
        SqliteStoreV1::open(
            &path,
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(2),
        )
        .unwrap(),
    );

    let connection = Connection::open(path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);
    let legacy_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN \
             ('procedure_snapshots', 'task_sessions', 'stage_progress', 'attempts', \
              'item_slots', 'blockers')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_tables, 0);
}

#[test]
fn schema_v3_v2_state_and_terminal_receipt_survive_v4_migration_and_reopen() {
    let temporary = TempDir::new().unwrap();
    let (expected_state, job_id, expected_receipt) = seed_populated_v2_schema_v3(&temporary);
    let path = temporary.path().join("state.sqlite3");

    let migrated = SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(30),
    )
    .unwrap();
    assert_eq!(
        migrated.read_graph_session_v2(&identity()).unwrap(),
        Some(expected_state.clone())
    );
    let migrated_job = migrated.read_job(&identity(), &job_id).unwrap().unwrap();
    assert_eq!(migrated_job.state(), JobStateV1::Cancelled);
    assert_eq!(migrated_job.terminal_receipt(), Some(&expected_receipt));
    drop(migrated);

    let reopened = SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(31),
    )
    .unwrap();
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(expected_state)
    );
    assert_eq!(
        reopened
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&expected_receipt)
    );
    drop(reopened);

    let connection = Connection::open(path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let migration: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 4",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let legacy_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN \
             ('procedure_snapshots', 'task_sessions', 'stage_progress', 'attempts', \
              'item_slots', 'blockers')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for table in [
        "v2_workspace_state",
        "v2_procedure_snapshots",
        "v2_graph_nodes",
        "v2_task_sessions",
        "v2_graph_node_counters",
        "v2_attempts",
        "v2_item_slots",
        "v2_blockers",
        "v2_resolved_evidence_references",
        "v2_decision_records",
        "v2_rework_records",
        "v2_goal_revisions",
        "v2_goal_criteria",
        "v2_criterion_assessment_results",
        "v2_criterion_citations",
        "v2_goal_assessments",
    ] {
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(rows > 0, "rich migration fixture did not populate {table}");
    }
    let receipts: (String, String) = connection
        .query_row(
            "SELECT jobs.terminal_response_json, idempotency_records.terminal_response_json \
             FROM jobs JOIN idempotency_records USING (job_id) WHERE jobs.job_id = ?1",
            [job_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((version, migration, legacy_tables), (4, 1, 0));
    assert_eq!(receipts.0, receipts.1);
}

#[test]
fn schema_v3_v2_only_migration_rolls_back_before_commit() {
    let temporary = TempDir::new().unwrap();
    let (expected_state, job_id, expected_receipt) = seed_populated_v2_schema_v3(&temporary);
    let path = temporary.path().join("state.sqlite3");
    let predecessor_receipts: (String, String) = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT jobs.terminal_response_json, idempotency_records.terminal_response_json \
             FROM jobs JOIN idempotency_records USING (job_id) WHERE jobs.job_id = ?1",
            [job_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let error = match SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(Some(StoreFailpointV1::SchemaBeforeCommit)),
        UnixMillis::new(30),
    ) {
        Ok(_) => panic!("schema-v4 migration must stop at the pre-commit failpoint"),
        Err(error) => error,
    };
    assert_eq!(
        error,
        StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        }
    );

    let connection = Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let v4_migrations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 4",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let legacy_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN \
             ('procedure_snapshots', 'task_sessions', 'stage_progress', 'attempts', \
              'item_slots', 'blockers')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let v2_rows: (i64, i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM v2_procedure_snapshots), \
                    (SELECT COUNT(*) FROM v2_task_sessions), \
                    (SELECT COUNT(*) FROM v2_attempts), \
                    (SELECT COUNT(*) FROM v2_item_slots)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let rolled_back_receipts: (String, String) = connection
        .query_row(
            "SELECT jobs.terminal_response_json, idempotency_records.terminal_response_json \
             FROM jobs JOIN idempotency_records USING (job_id) WHERE jobs.job_id = ?1",
            [job_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((version, v4_migrations, legacy_tables), (3, 0, 6));
    assert_eq!(v2_rows, (1, 1, 4, 3));
    assert_eq!(rolled_back_receipts, predecessor_receipts);
    drop(connection);

    let recovered = SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(31),
    )
    .unwrap();
    assert_eq!(
        recovered.read_graph_session_v2(&identity()).unwrap(),
        Some(expected_state)
    );
    assert_eq!(
        recovered
            .read_job(&identity(), &job_id)
            .unwrap()
            .unwrap()
            .terminal_receipt(),
        Some(&expected_receipt)
    );
}

#[test]
fn schema_v3_with_legacy_state_is_rejected_without_mutation() {
    let temporary = TempDir::new().unwrap();
    seed_schema_v3(&temporary, true);
    let path = temporary.path().join("state.sqlite3");

    let error = match SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(2),
    ) {
        Ok(_) => panic!("legacy Procedure state must not migrate implicitly"),
        Err(error) => error,
    };
    assert_eq!(error, StoreErrorV1::LegacyProcedureStateUnsupportedV1);

    let connection = Connection::open(path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM procedure_snapshots", [], |row| {
            row.get(0)
        })
        .unwrap();
    let migration: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 4",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((version, rows, migration), (3, 1, 0));
}

#[test]
fn schema_v3_workspace_scoped_v1_start_job_is_rejected_without_mutation() {
    let temporary = TempDir::new().unwrap();
    seed_schema_v3(&temporary, false);
    let path = temporary.path().join("state.sqlite3");
    let request = encode_command_v1(&ClaimedExecutionV1::new(
        DomainCommand::SessionStart,
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
    ))
    .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO jobs (
                job_id, workspace_sequence, idempotency_key, request_digest, command_name,
                canonical_request_json, state, session_id, submitted_at_ms
             ) VALUES (?1, 1, 'legacy-start', ?2, 'session.start', ?3, 'queued', NULL, 1)",
            params![
                "00000000-0000-4000-8000-000000000099",
                digest('d').as_str(),
                request,
            ],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(2),
    ) {
        Ok(_) => panic!("workspace-scoped Procedure v1 start must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error, StoreErrorV1::LegacyProcedureStateUnsupportedV1);

    let connection = Connection::open(path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let jobs: i64 = connection
        .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!((version, jobs), (3, 1));
}

fn seed_orphaned_v2_start_cancellation(temporary: &TempDir) -> String {
    let path = temporary.path().join("state.sqlite3");
    let store = SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(1),
    )
    .unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "session.start",
            "execution_version": 6,
            "procedure": {"canonical": true}
        }))
        .unwrap(),
    )
    .unwrap();

    let job_id = JobId::new(uuid(501)).unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::SessionStart,
        IdempotencyKeyV1::new("orphaned-v2-cancel").unwrap(),
        job_id.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest('f'),
        UnixMillis::new(1),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Absent);
    store.admit(&identity(), request).unwrap();
    assert!(matches!(
        store
            .cancel_before_claim(
                &identity(),
                job_id.clone(),
                Revision::new(1),
                UnixMillis::new(2)
            )
            .unwrap(),
        CancelOutcomeV1::Cancelled(_)
    ));
    drop(store);

    let connection = Connection::open(&path).unwrap();
    let encoded: String = connection
        .query_row(
            "SELECT terminal_response_json FROM idempotency_records \
             WHERE idempotency_key = 'orphaned-v2-cancel'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute("DELETE FROM jobs WHERE job_id = ?1", [job_id.as_str()])
        .unwrap();
    restore_schema_v3_shape(&connection);
    assert_schema_v3_shape(&connection);
    drop(connection);
    encoded
}

#[test]
fn schema_v3_orphaned_explicit_v2_start_cancellation_migrates_and_preserves_receipt() {
    let temporary = TempDir::new().unwrap();
    let encoded = seed_orphaned_v2_start_cancellation(&temporary);
    assert_eq!(
        serde_json::from_str::<Value>(&encoded).unwrap()["schema"],
        "podway.store-terminal/v5"
    );

    let migrated = SqliteStoreV1::open(
        temporary.path().join("state.sqlite3"),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(3),
    )
    .unwrap();
    let lookup = migrated
        .read_idempotency_lookup(
            &identity(),
            &IdempotencyKeyV1::new("orphaned-v2-cancel").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        lookup.terminal_receipt().unwrap().execution_flavor(),
        DurableExecutionFlavorV1::ProcedureV2
    );
}

#[test]
fn schema_v3_orphaned_ambiguous_start_cancellation_is_rejected_without_mutation() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("state.sqlite3");
    let encoded = seed_orphaned_v2_start_cancellation(&temporary);
    let connection = Connection::open(&path).unwrap();
    let mut released: Value = serde_json::from_str(&encoded).unwrap();
    released["schema"] = json!("podway.store-terminal/v2");
    released.as_object_mut().unwrap().remove("execution_flavor");
    let ambiguous = canonicalize_json_v1(&released).unwrap();
    connection
        .execute(
            "UPDATE idempotency_records SET terminal_response_json = ?1 \
             WHERE idempotency_key = 'orphaned-v2-cancel'",
            [&ambiguous],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(3),
    ) {
        Ok(_) => panic!("ambiguous orphaned start receipt must fail closed"),
        Err(error) => error,
    };
    assert_eq!(error, StoreErrorV1::LegacyProcedureStateUnsupportedV1);

    let connection = Connection::open(path).unwrap();
    let (version, encoded): (i64, String) = connection
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version), terminal_response_json \
             FROM idempotency_records WHERE idempotency_key = 'orphaned-v2-cancel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(version, 3);
    assert_eq!(encoded, ambiguous);
}

#[test]
fn schema_v3_orphaned_v1_idempotency_receipt_is_rejected_without_mutation() {
    let temporary = TempDir::new().unwrap();
    seed_schema_v3(&temporary, false);
    let path = temporary.path().join("state.sqlite3");
    let job_id = JobId::new("00000000-0000-4000-8000-000000000098").unwrap();
    let session_id = SessionId::new("00000000-0000-4000-8000-000000000097").unwrap();
    let receipt = PersistedTerminalReceiptV1::new_with_projections(
        JobReceiptV1::new(1, job_id.clone(), digest('e')),
        PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
            session_id: session_id.clone(),
            revision_before: Revision::ZERO,
            revision_after: Revision::new(1),
            changed: true,
        }),
        PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Succeeded,
            EpochMillisV1::new(1),
            Some(EpochMillisV1::new(1)),
            EpochMillisV1::new(2),
        )
        .unwrap(),
        Some(
            PersistedTerminalSessionProjectionV1::new(
                session_id,
                "Legacy start".to_owned(),
                PersistedSessionLifecycleV1::Running,
                Revision::ZERO,
                Revision::new(1),
            )
            .unwrap(),
        ),
    )
    .unwrap()
    .with_lookup_command(PersistedDomainCommandV1::SessionStart)
    .unwrap();
    let terminal = encode_persisted_terminal_receipt_v1(&receipt).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO idempotency_records (
                idempotency_key, request_digest, job_id, scope_kind, scope_session_id,
                terminal_response_json, created_at_ms, updated_at_ms
             ) VALUES ('orphaned-legacy', ?1, ?2, 'workspace', NULL, ?3, 1, 2)",
            params![digest('e').as_str(), job_id.as_str(), terminal],
        )
        .unwrap();
    drop(connection);

    let error = match SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(3),
    ) {
        Ok(_) => panic!("orphaned Procedure v1 receipt must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error, StoreErrorV1::LegacyProcedureStateUnsupportedV1);

    let connection = Connection::open(path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let receipts: i64 = connection
        .query_row("SELECT COUNT(*) FROM idempotency_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!((version, receipts), (3, 1));
}
