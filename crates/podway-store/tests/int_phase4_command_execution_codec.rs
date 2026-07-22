//! Durable full-execution command codec and restart contracts.

use std::fs;
use std::path::Path;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use podway_core::{DomainCommand, ItemId, JobId, Sha256Digest, UnixMillis, WorkspaceId};
use podway_store::codec::{
    STORE_COMMAND_SCHEMA_V1, STORE_COMMAND_SCHEMA_V2, decode_command_v1, encode_command_v1,
};
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, CanonicalExecutionJsonV1, ClaimedExecutionV1,
    DurableWorktreeIdentityV1, IdempotencyKeyV1, MAX_CANONICAL_EXECUTION_JSON_BYTES_V1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreValueErrorV1, ValidatedWorkspaceRootV1, WorkerIdV1,
};
use tempfile::TempDir;

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn execution_digest(document: &CanonicalExecutionJsonV1) -> Sha256Digest {
    Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(document.as_str().as_bytes())
    ))
    .unwrap()
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000041").unwrap(),
        digest('b'),
    )
}

fn job(number: u8) -> JobId {
    JobId::new(format!("00000000-0000-4000-8000-{:012x}", number)).unwrap()
}

fn preconditions() -> RevisionAttemptItemPreconditionsV1 {
    RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap()
}

fn canonical_execution(document: serde_json::Value) -> CanonicalExecutionJsonV1 {
    CanonicalExecutionJsonV1::new(podway_core::canonicalize_json_v1(&document).unwrap()).unwrap()
}

fn open_store(temporary: &TempDir, now: u64) -> SqliteStoreV1 {
    SqliteStoreV1::open(
        temporary.path().join("state.sqlite3"),
        &ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4-codec")).unwrap(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(now),
    )
    .unwrap()
}

#[test]
fn legacy_minimal_executions_preserve_the_v1_golden_bytes() {
    let execution = ClaimedExecutionV1::new(DomainCommand::WorkspaceInitialize, preconditions());
    let encoded = encode_command_v1(&execution).unwrap();
    assert_eq!(
        encoded,
        r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#
    );
    assert!(encoded.contains(STORE_COMMAND_SCHEMA_V1));
    assert_eq!(decode_command_v1(&encoded).unwrap(), execution);
}

#[test]
fn full_store_command_documents_preserve_the_v2_golden_bytes() {
    let canonical_execution = CanonicalExecutionJsonV1::new(
        r#"{"command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v1"}"#,
    )
    .unwrap();
    let execution = ClaimedExecutionV1::new_with_canonical_execution(
        DomainCommand::WorkspaceInitialize,
        preconditions(),
        canonical_execution,
    );

    assert_eq!(
        encode_command_v1(&execution).unwrap(),
        r#"{"canonical_execution_json":"{\"command\":{\"kind\":\"workspace_initialize\"},\"preconditions\":{\"expected_attempt_id\":null,\"expected_item_id\":null,\"expected_item_revision\":null,\"expected_session_revision\":null},\"schema\":\"podway.store-command/v1\"}","command":{"kind":"workspace_initialize"},"preconditions":{"expected_attempt_id":null,"expected_item_id":null,"expected_item_revision":null,"expected_session_revision":null},"schema":"podway.store-command/v2"}"#
    );
}

#[test]
fn full_semantic_execution_documents_round_trip_as_v2_without_interpretation() {
    let item_id = ItemId::new("semantic-item").unwrap();
    let cases = vec![
        (
            DomainCommand::SessionStart,
            serde_json::json!({
                "task": {"title": "start semantic session"},
                "procedure": {"version": 1, "id": "fixture"},
                "command": "session.start"
            }),
        ),
        (
            DomainCommand::ItemSet {
                item_id: item_id.clone(),
            },
            serde_json::json!({
                "value": {"checked": true, "text": "item semantic value"},
                "item_id": "semantic-item",
                "command": "item.set"
            }),
        ),
        (
            DomainCommand::SessionRetry,
            serde_json::json!({
                "reason": "retry semantic document",
                "command": "session.retry"
            }),
        ),
        (
            DomainCommand::SessionReturn,
            serde_json::json!({
                "target_stage": "previous-stage",
                "command": "session.return"
            }),
        ),
        (
            DomainCommand::SessionBlock,
            serde_json::json!({
                "blocker": {"message": "waiting for review", "id": "review"},
                "command": "session.block"
            }),
        ),
        (
            DomainCommand::SessionComplete,
            serde_json::json!({
                "summary": "complete semantic document",
                "command": "session.complete"
            }),
        ),
    ];

    for (command, semantic_document) in cases {
        let canonical_execution = canonical_execution(semantic_document);
        let execution = ClaimedExecutionV1::new_with_canonical_execution(
            command,
            preconditions(),
            canonical_execution.clone(),
        );
        let encoded = encode_command_v1(&execution).unwrap();
        assert!(encoded.contains(STORE_COMMAND_SCHEMA_V2));
        let decoded = decode_command_v1(&encoded).unwrap();
        assert_eq!(decoded.command(), execution.command());
        assert_eq!(decoded.preconditions(), execution.preconditions());
        assert_eq!(decoded.canonical_execution(), &canonical_execution);
        assert_eq!(encode_command_v1(&decoded).unwrap(), encoded);
    }
}

#[test]
fn canonical_execution_rejects_noncanonical_deep_and_oversized_documents() {
    assert_eq!(
        CanonicalExecutionJsonV1::new(r#"{"z":1,"a":2}"#),
        Err(StoreValueErrorV1::InvalidCanonicalExecutionJson)
    );
    assert_eq!(
        CanonicalExecutionJsonV1::new(r#"{"number":1.5}"#),
        Err(StoreValueErrorV1::InvalidCanonicalExecutionJson)
    );

    let mut too_deep = String::new();
    for _ in 0..65 {
        too_deep.push_str(r#"{"a":"#);
    }
    too_deep.push_str("null");
    for _ in 0..65 {
        too_deep.push('}');
    }
    assert!(matches!(
        CanonicalExecutionJsonV1::new(too_deep),
        Err(StoreValueErrorV1::CanonicalExecutionJsonDepthExceeded { maximum: 64 })
    ));

    let oversized = format!("\"{}\"", "x".repeat(MAX_CANONICAL_EXECUTION_JSON_BYTES_V1));
    assert!(matches!(
        CanonicalExecutionJsonV1::new(oversized),
        Err(StoreValueErrorV1::ValueTooLong {
            field: "canonical execution JSON",
            maximum_bytes: MAX_CANONICAL_EXECUTION_JSON_BYTES_V1,
        })
    ));
}

#[test]
fn full_execution_is_stored_in_canonical_request_json_and_survives_restart_claim() {
    let temporary = TempDir::new().unwrap();
    let database = temporary.path().join("state.sqlite3");
    let semantic_execution = canonical_execution(serde_json::json!({
        "command": "workspace.initialize",
        "metadata": {"attempt": 2, "source": "restart-test"},
        "workspace": {"root": "/tmp/podway-phase4-codec"}
    }));
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::WorkspaceInitialize,
        IdempotencyKeyV1::new("full-execution-restart").unwrap(),
        job(1),
        preconditions(),
        execution_digest(&semantic_execution),
        UnixMillis::new(2),
        semantic_execution.clone(),
    );

    let store = open_store(&temporary, 1);
    assert!(matches!(
        store.admit(&identity(), request),
        Ok(AdmitOutcomeV1::New(_))
    ));
    drop(store);

    let connection = Connection::open(&database).unwrap();
    let stored: String = connection
        .query_row(
            "SELECT canonical_request_json FROM jobs WHERE job_id = ?1",
            [job(1).as_str()],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert!(stored.contains(STORE_COMMAND_SCHEMA_V2));
    assert_eq!(
        decode_command_v1(&stored)
            .unwrap()
            .canonical_execution()
            .as_str(),
        semantic_execution.as_str()
    );

    let reopened = open_store(&temporary, 3);
    let claimed = reopened
        .claim_next(
            &identity(),
            WorkerIdV1::new("restart-worker").unwrap(),
            UnixMillis::new(4),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.execution().canonical_execution().as_str(),
        semantic_execution.as_str()
    );
    assert_eq!(
        claimed.job().request_digest(),
        &execution_digest(&semantic_execution)
    );
}

#[test]
fn workspace_binding_inspection_is_read_only_and_returns_none_only_for_a_missing_database() {
    let temporary = TempDir::new().unwrap();
    let database = temporary.path().join("state.sqlite3");
    let options = SqliteStoreOptionsV1::new(8).unwrap();

    assert_eq!(
        SqliteStoreV1::inspect_workspace_binding(&database, &options).unwrap(),
        None
    );
    assert!(!database.exists());

    let store = SqliteStoreV1::open(
        &database,
        &ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4-codec")).unwrap(),
        identity(),
        options.clone(),
        UnixMillis::new(1),
    )
    .unwrap();
    drop(store);

    let database_before = fs::read(&database).unwrap();
    let connection = Connection::open(&database).unwrap();
    let before: (String, String, String, String, i64) = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
             last_validated_root, updated_at_ms FROM workspace_state WHERE singleton = 1",
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
        )
        .unwrap();
    drop(connection);

    let binding = SqliteStoreV1::inspect_workspace_binding(&database, &options)
        .unwrap()
        .expect("an existing valid database must expose its binding");
    assert_eq!(binding.identity(), &identity());
    assert_eq!(
        binding.last_validated_root().as_encoded(),
        ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-phase4-codec"))
            .unwrap()
            .as_encoded()
    );
    assert_eq!(fs::read(&database).unwrap(), database_before);

    let connection = Connection::open(&database).unwrap();
    let after: (String, String, String, String, i64) = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
             last_validated_root, updated_at_ms FROM workspace_state WHERE singleton = 1",
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
        )
        .unwrap();
    assert_eq!(after, before);
}
