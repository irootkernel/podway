use std::path::Path;

use podway_core::{
    AttemptId, CanonicalProcedureJsonV1, DomainCommand, ProcedureSnapshotId,
    ProcedureSourceLabelV1, ReasonV2, Revision, SessionId, Sha256Digest, TerminalDispositionV2,
    UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitRequestV1, CanonicalExecutionJsonV1, DelegationReceiptV1,
    DurableWorktreeIdentityV1, GraphSessionStateV2, IdempotencyKeyV1, PersistedResponseContextV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreErrorV1, StoreFailpointV1, StoreGraphMutationContractV2, StoreGraphStateContractV2,
    StoreIdempotencyReadContractV1, StoreIntegrityCheckV1, StoreReadContractV1,
    StoreSessionArchiveContractV2, StoreTerminalDispositionContractV2, ValidatedWorkspaceRootV1,
    WorkerIdV1,
};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub(super) fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

pub(super) fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000401").unwrap(),
        digest('b'),
    )
}

fn root() -> ValidatedWorkspaceRootV1 {
    ValidatedWorkspaceRootV1::from_path(Path::new("/tmp/podway-v2sdg004-receipt")).unwrap()
}

pub(super) fn open(temporary: &TempDir, failpoint: Option<StoreFailpointV1>) -> SqliteStoreV1 {
    SqliteStoreV1::open(
        temporary.path().join("state.sqlite3"),
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(failpoint),
        UnixMillis::new(1),
    )
    .unwrap()
}

pub(super) fn declaration() -> serde_json::Value {
    json!({
        "binding_ref": "local-specialist",
        "role_ref": "reviewer",
        "role_snapshot_digest": digest('c'),
        "max_rounds": 3,
        "after_accept": "retain",
    })
}

pub(super) fn session_state() -> GraphSessionStateV2 {
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "delegated-receipt",
        "version": "1",
        "name": "Delegated receipt",
        "purpose": "Verify immutable receipt persistence.",
        "node_definitions": {
            "work": {"type": "action", "title": "Work", "intent": "Complete the work."}
        },
        "graph": {
            "entry": "work",
            "nodes": [{"id": "work", "use": "work", "terminal": true, "delegation": declaration()}]
        }
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = podway_store::ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000402").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("delegated.yaml").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap();
    let prepared = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Delegated task",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000000403").unwrap(),
        UnixMillis::new(3),
    )
    .unwrap();
    prepared
        .begin_v2(
            Revision::ZERO,
            AttemptId::new("00000000-0000-4000-8000-000000000404").unwrap(),
            None,
            UnixMillis::new(4),
        )
        .unwrap()
        .into_state()
}

pub(super) fn receipt(state: &GraphSessionStateV2) -> DelegationReceiptV1 {
    DelegationReceiptV1::new(json!({
        "schema": "podway.delegation-receipt/v1",
        "receipt_id": "00000000-0000-4000-8000-000000000405",
        "workspace_uuid": identity().workspace_uuid(),
        "procedure_digest": state.snapshot().digest(),
        "session_id": state.trace().session_id(),
        "placement_id": "work",
        "attempt_id": state.trace().active_attempt().unwrap().attempt_id(),
        "delegation": declaration(),
        "package_digest": digest('d'),
        "execution": {
            "binding_ref": "local-specialist",
            "adapter_id": "adapter",
            "adapter_version": "1",
            "primary_host": "external",
            "scope_kind": "session",
            "scope_ref": "scope-1",
            "supervisor_ref": "supervisor-1",
            "worker_ref": "worker-1",
            "task_ref": "task-1",
            "role_ref": "reviewer",
            "role_snapshot_digest": digest('c'),
            "agent_configuration_digest": digest('e'),
            "accepted_task_digest": digest('f'),
        },
        "rounds_used": 1,
        "result_digest": digest('1'),
        "summary": "Reviewed result",
        "worker_disposition": "idle",
        "accepted": true,
        "accepted_items_digest": podway_store::v2_delegation::accepted_items_digest_v1(state).unwrap(),
        "accepted_at": "2026-09-17T00:00:00.000Z",
    }))
    .unwrap()
}

pub(super) fn admit_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
) -> podway_store::ClaimedJobV1 {
    admit_and_claim_with(store, state, "delegation-receipt-key", 0x406)
}

pub(super) fn admit_and_claim_with(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let accepted = receipt(state);
    let accepted = accepted.value();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "attached_artifact": null,
            "command": "delegation.accept",
            "execution_version": 8,
            "fresh_attempt_id": null,
            "fresh_blocker_id": null,
            "payload": {
                "schema": "podway.delegation-accept-input/v1",
                "workspace_uuid": accepted["workspace_uuid"],
                "session_id": accepted["session_id"],
                "session_revision": state.trace().revision().get(),
                "attempt_id": accepted["attempt_id"],
                "idempotency_key": key,
                "package_digest": accepted["package_digest"],
                "execution": accepted["execution"],
                "rounds_used": accepted["rounds_used"],
                "result_digest": accepted["result_digest"],
                "summary": accepted["summary"],
                "worker_disposition": accepted["worker_disposition"],
                "accepted": true,
            },
            "preconditions": {},
            "selector": {},
            "workspace_id": identity().workspace_uuid().as_str(),
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::DelegationAccept,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(format!("00000000-0000-4000-9000-{job_number:012x}")).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(state.trace().active_attempt().unwrap().attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
        digest('9'),
        UnixMillis::new(5),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            format!("00000000-0000-4000-8000-{:012x}", job_number + 1),
            "delegation.accept",
            identity().workspace_uuid().clone(),
            "/tmp/podway-v2sdg004-receipt",
            0,
        )
        .unwrap(),
    );
    store.admit(&identity(), request).unwrap();
    store
        .claim_next(
            &identity(),
            WorkerIdV1::new("store-test-worker").unwrap(),
            UnixMillis::new(6),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn v2sdg004_receipt_codec_is_closed_and_canonical() {
    let state = session_state();
    let receipt = receipt(&state);
    assert_eq!(
        DelegationReceiptV1::decode(receipt.canonical_json()).unwrap(),
        receipt
    );
    let mut invalid = receipt.value().clone();
    invalid["unknown"] = json!(true);
    assert!(DelegationReceiptV1::new(invalid).is_err());
    assert!(DelegationReceiptV1::decode(&format!(" {}", receipt.canonical_json())).is_err());
}

#[test]
fn v2sdg004_receipt_codec_rejects_closed_contract_violations() {
    let base = receipt(&session_state()).value().clone();
    let mut invalid = Vec::new();
    let mut value = base.clone();
    value["rounds_used"] = json!(4);
    invalid.push(value);
    let mut value = base.clone();
    value["worker_disposition"] = json!("released");
    invalid.push(value);
    let mut value = base.clone();
    value["accepted"] = json!(false);
    invalid.push(value);
    let mut value = base.clone();
    value["accepted_at"] = json!("2026-09-17");
    invalid.push(value);
    let mut value = base.clone();
    value["accepted_at"] = json!("2026-02-30T00:00:00.000Z");
    invalid.push(value);
    let mut value = base.clone();
    value["execution"]["role_ref"] = json!("different-role");
    invalid.push(value);
    let mut value = base.clone();
    value["execution"]["primary_host"] = json!("unknown");
    invalid.push(value);
    let mut value = base.clone();
    value["package_digest"] = json!("sha256:BAD");
    invalid.push(value);
    let mut value = base.clone();
    value["summary"] = json!("x".repeat(4097));
    invalid.push(value);
    let mut value = base.clone();
    value["execution"]["unexpected"] = json!(true);
    invalid.push(value);
    for value in invalid {
        assert!(DelegationReceiptV1::new(value).is_err());
    }

    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/schemas/delegation-components-v1.schema.json"
    ))
    .unwrap();
    for (definition, value) in [
        ("receipt", &base),
        ("declaration", &base["delegation"]),
        ("execution", &base["execution"]),
    ] {
        assert_eq!(schema["$defs"][definition]["additionalProperties"], false);
        let mut required: Vec<_> = schema["$defs"][definition]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry.as_str().unwrap())
            .collect();
        let mut actual: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        required.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, required);
    }
    assert_eq!(schema["$defs"]["opaqueRef"]["x-podway-max-utf8-bytes"], 256);
    assert_eq!(
        schema["$defs"]["receipt"]["properties"]["summary"]["x-podway-max-utf8-bytes"],
        4096
    );
    let mut boundary = base.clone();
    boundary["delegation"]["role_ref"] = json!("é".repeat(128));
    boundary["execution"]["role_ref"] = json!("é".repeat(128));
    assert!(DelegationReceiptV1::new(boundary.clone()).is_ok());
    boundary["delegation"]["role_ref"] = json!("é".repeat(129));
    boundary["execution"]["role_ref"] = json!("é".repeat(129));
    assert!(DelegationReceiptV1::new(boundary).is_err());
}

#[test]
fn v2sdg004_delegated_placement_enforces_opaque_utf8_byte_limit() {
    let state = session_state();
    let mut document: serde_json::Value =
        serde_json::from_str(state.snapshot().canonical_json().as_str()).unwrap();
    document["graph"]["nodes"][0]["delegation"]["role_ref"] = json!("é".repeat(129));
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    assert!(
        podway_store::ProcedureSnapshotV2::new(
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000499").unwrap(),
            CanonicalProcedureJsonV1::new(canonical).unwrap(),
            digest,
            ProcedureSourceLabelV1::file("oversize-role.yaml").unwrap(),
            UnixMillis::new(2),
        )
        .is_err()
    );
}

#[test]
fn v2sdg004_acceptance_successor_rejects_stale_and_direct_attempts() {
    let delegated = session_state();
    assert!(
        delegated
            .accept_delegation_successor_v1(
                &AttemptId::new("00000000-0000-4000-8000-000000000499").unwrap()
            )
            .is_err()
    );

    let mut document: serde_json::Value =
        serde_json::from_str(delegated.snapshot().canonical_json().as_str()).unwrap();
    document["graph"]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("delegation");
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = podway_store::ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000497").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("direct.yaml").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap();
    let direct = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Direct task",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000000496").unwrap(),
        UnixMillis::new(3),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new("00000000-0000-4000-8000-000000000495").unwrap(),
        None,
        UnixMillis::new(4),
    )
    .unwrap()
    .into_state();
    assert!(
        direct
            .accept_delegation_successor_v1(direct.trace().active_attempt().unwrap().attempt_id())
            .is_err()
    );
}

#[test]
fn v2sdg004_receipt_rejects_wrong_parent_without_state_or_job_commit() {
    let state = session_state();
    let base = receipt(&state).value().clone();
    let mut mismatches = Vec::new();
    let mut value = base.clone();
    value["workspace_uuid"] = json!("00000000-0000-4000-8000-000000000498");
    mismatches.push(value);
    let mut value = base.clone();
    value["session_id"] = json!("00000000-0000-4000-8000-000000000498");
    mismatches.push(value);
    let mut value = base.clone();
    value["attempt_id"] = json!("00000000-0000-4000-8000-000000000498");
    mismatches.push(value);
    let mut value = base.clone();
    value["placement_id"] = json!("other");
    mismatches.push(value);
    let mut value = base.clone();
    value["procedure_digest"] = json!(digest('8'));
    mismatches.push(value);
    let mut value = base;
    value["delegation"]["role_ref"] = json!("other-role");
    value["execution"]["role_ref"] = json!("other-role");
    mismatches.push(value);

    for value in mismatches {
        let temporary = TempDir::new().unwrap();
        let store = open(&temporary, None);
        store
            .create_graph_session_v2(&identity(), state.clone())
            .unwrap();
        let claimed = admit_and_claim(&store, &state);
        let next = state
            .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
            .unwrap();
        let wrong = DelegationReceiptV1::new(value).unwrap();
        assert!(
            store
                .commit_graph_delegation_accept_terminal_v1(
                    claimed.claim().clone(),
                    state.workspace_revision(),
                    state.trace().revision(),
                    next,
                    digest('d'),
                    wrong,
                    UnixMillis::new(7)
                )
                .is_err()
        );
        assert_eq!(
            store.read_graph_session_v2(&identity()).unwrap(),
            Some(state.clone())
        );
        let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        assert!(
            store
                .read_job(&identity(), claimed.claim().job_id())
                .unwrap()
                .unwrap()
                .terminal_receipt()
                .is_none()
        );
    }
}

#[test]
fn v2sdg004_second_acceptance_of_same_attempt_rolls_back() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let first = admit_and_claim(&store, &state);
    let accepted = state
        .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            first.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&state),
            UnixMillis::new(7),
        )
        .unwrap();
    let second = admit_and_claim_with(&store, &accepted, "delegation-receipt-key-2", 0x408);
    let next = accepted
        .accept_delegation_successor_v1(accepted.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    let mut duplicate = receipt(&accepted).value().clone();
    duplicate["receipt_id"] = json!("00000000-0000-4000-8000-000000000409");
    assert!(
        store
            .commit_graph_delegation_accept_terminal_v1(
                second.claim().clone(),
                accepted.workspace_revision(),
                accepted.trace().revision(),
                next,
                digest('d'),
                DelegationReceiptV1::new(duplicate).unwrap(),
                UnixMillis::new(8)
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(accepted)
    );
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        store
            .read_job(&identity(), second.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
}

#[test]
fn v2sdg004_duplicate_receipt_id_across_attempts_rolls_back() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let first = admit_and_claim(&store, &state);
    let first_receipt = receipt(&state);
    let accepted = state
        .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            first.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            accepted.clone(),
            digest('d'),
            first_receipt.clone(),
            UnixMillis::new(7),
        )
        .unwrap();

    let retried = accepted
        .retry_active_attempt_v2(
            accepted.trace().revision(),
            accepted.trace().active_attempt().unwrap().attempt_id(),
            AttemptId::new("00000000-0000-4000-8000-00000000040a").unwrap(),
            ReasonV2::new("Retry delegated work").unwrap(),
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            retried.clone(),
        )
        .unwrap();
    let second = admit_and_claim_with(&store, &retried, "delegation-receipt-key-2", 0x40b);
    let next = retried
        .accept_delegation_successor_v1(retried.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    let duplicate = receipt(&retried);
    let result = store.commit_graph_delegation_accept_terminal_v1(
        second.claim().clone(),
        retried.workspace_revision(),
        retried.trace().revision(),
        next,
        digest('d'),
        duplicate,
        UnixMillis::new(9),
    );
    assert!(matches!(result, Err(StoreErrorV1::InvalidStateV1(_))));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(retried)
    );
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let rows: Vec<(String, String)> = connection
        .prepare("SELECT attempt_id, receipt_id FROM v2_delegation_receipts")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![(
            state
                .trace()
                .active_attempt()
                .unwrap()
                .attempt_id()
                .as_str()
                .to_owned(),
            first_receipt.receipt_id().as_str().to_owned(),
        )]
    );
    assert!(
        store
            .read_job(&identity(), second.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
}

#[test]
fn v2sdg004_receipt_state_and_job_commit_atomically() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &state);
    let next = state
        .accept_delegation_successor_v1(
            claimed
                .execution()
                .preconditions()
                .expected_attempt_id()
                .unwrap(),
        )
        .unwrap();
    let receipt = receipt(&state);
    let terminal = store
        .commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            next.clone(),
            digest('d'),
            receipt.clone(),
            UnixMillis::new(7),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(next.clone())
    );
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let stored: String = connection
        .query_row(
            "SELECT receipt_json FROM v2_delegation_receipts WHERE attempt_id = ?1",
            [receipt.attempt_id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, receipt.canonical_json());
    assert!(
        connection
            .execute(
                "UPDATE v2_delegation_receipts SET receipt_json = receipt_json",
                []
            )
            .is_err()
    );
    drop(connection);
    assert!(
        store
            .read_job(&identity(), terminal.job().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_some()
    );
    assert!(
        store
            .read_idempotency_lookup(
                &identity(),
                &IdempotencyKeyV1::new("delegation-receipt-key").unwrap()
            )
            .unwrap()
            .is_some()
    );
    drop(store);
    let reopened = open(&temporary, None);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
}

#[test]
fn v2sdg004_precommit_failure_rolls_back_receipt_state_and_terminal() {
    let temporary = TempDir::new().unwrap();
    let store = open(
        &temporary,
        Some(StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate),
    );
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &state);
    let next = state
        .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    assert!(
        store
            .commit_graph_delegation_accept_terminal_v1(
                claimed.claim().clone(),
                state.workspace_revision(),
                state.trace().revision(),
                next,
                digest('d'),
                receipt(&state),
                UnixMillis::new(7),
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(state)
    );
    drop(store);
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    let terminal: Option<String> = connection
        .query_row(
            "SELECT terminal_response_json FROM jobs WHERE job_id = ?1",
            [claimed.claim().job_id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(terminal.is_none());
}

#[test]
fn v2sdg004_populated_v11_migrates_without_changing_graph_bytes() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    drop(store);
    let path = temporary.path().join("state.sqlite3");
    let connection = Connection::open(&path).unwrap();
    let snapshot_before: String = connection
        .query_row(
            "SELECT canonical_json FROM v2_procedure_snapshots",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER v2_delegation_receipts_immutable;
         DROP TABLE v2_delegation_receipts;
         DELETE FROM schema_migrations WHERE version = 12;
         PRAGMA user_version = 11;",
        )
        .unwrap();
    drop(connection);

    let failed = SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8)
            .unwrap()
            .with_failpoint(Some(StoreFailpointV1::SchemaBeforeCommit)),
        UnixMillis::new(8),
    );
    assert!(failed.is_err());
    let connection = Connection::open(&path).unwrap();
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
    let table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'v2_delegation_receipts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_count, 0);
    drop(connection);

    let reopened = open(&temporary, None);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(state)
    );
    drop(reopened);
    let connection = Connection::open(&path).unwrap();
    let snapshot_after: String = connection
        .query_row(
            "SELECT canonical_json FROM v2_procedure_snapshots",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(snapshot_after, snapshot_before);
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 12);
}

#[test]
fn v2sdg004_newer_schema_is_rejected_without_rewrite() {
    let temporary = TempDir::new().unwrap();
    drop(open(&temporary, None));
    let path = temporary.path().join("state.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 13;")
        .unwrap();
    drop(connection);
    let result = SqliteStoreV1::open(
        &path,
        &root(),
        identity(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(9),
    );
    assert!(matches!(
        result,
        Err(podway_store::StoreErrorV1::NewerStateV1 {
            found_schema_version: 13,
            supported_schema_version: 12,
        })
    ));
    let connection = Connection::open(&path).unwrap();
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 13);
}

#[test]
fn v2sdg004_reset_removes_receipt_with_owning_attempt() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &state);
    let next = state
        .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            next.clone(),
            digest('d'),
            receipt(&state),
            UnixMillis::new(7),
        )
        .unwrap();
    store
        .clear_graph_session_v2(
            &identity(),
            next.workspace_revision(),
            next.trace().revision(),
        )
        .unwrap();
    assert_eq!(store.read_graph_session_v2(&identity()).unwrap(), None);
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn v2sdg004_postcommit_response_loss_replays_one_receipt() {
    let temporary = TempDir::new().unwrap();
    let store = open(
        &temporary,
        Some(StoreFailpointV1::TerminalAfterCommitBeforeResponse),
    );
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &state);
    let next = state
        .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    assert!(
        store
            .commit_graph_delegation_accept_terminal_v1(
                claimed.claim().clone(),
                state.workspace_revision(),
                state.trace().revision(),
                next.clone(),
                digest('d'),
                receipt(&state),
                UnixMillis::new(7)
            )
            .is_err()
    );
    drop(store);
    let reopened = open(&temporary, None);
    assert_eq!(
        reopened.read_graph_session_v2(&identity()).unwrap(),
        Some(next)
    );
    let job = reopened
        .read_job(&identity(), claimed.claim().job_id())
        .unwrap()
        .unwrap();
    assert!(job.terminal_receipt().is_some());
    let idempotent = reopened
        .read_idempotency_lookup(
            &identity(),
            &IdempotencyKeyV1::new("delegation-receipt-key").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(idempotent.terminal_receipt().is_some());
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn v2sdg004_orphan_receipt_is_rejected_on_reopen() {
    let temporary = TempDir::new().unwrap();
    drop(open(&temporary, None));
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let bogus = "00000000-0000-4000-8000-000000000499";
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_delegation_receipts (attempt_id, session_id, receipt_id, receipt_json)
         VALUES (?1, ?1, ?1, '{}')",
            [bogus],
        )
        .unwrap();
    drop(connection);
    assert!(
        SqliteStoreV1::open(
            temporary.path().join("state.sqlite3"),
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(9)
        )
        .is_err()
    );
}

#[test]
fn v2sdg004_reopen_rejects_well_formed_mismatched_and_noncanonical_receipts() {
    for variant in [
        "workspace_uuid",
        "procedure_digest",
        "placement_id",
        "delegation",
        "noncanonical",
        "archived_procedure_digest",
    ] {
        let temporary = TempDir::new().unwrap();
        let store = open(&temporary, None);
        let state = session_state();
        store
            .create_graph_session_v2(&identity(), state.clone())
            .unwrap();
        let claimed = admit_and_claim(&store, &state);
        let original = receipt(&state);
        let accepted = state
            .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
            .unwrap();
        store
            .commit_graph_delegation_accept_terminal_v1(
                claimed.claim().clone(),
                state.workspace_revision(),
                state.trace().revision(),
                accepted.clone(),
                digest('d'),
                original.clone(),
                UnixMillis::new(7),
            )
            .unwrap();
        if variant == "archived_procedure_digest" {
            let completed = accepted
                .complete_active_action_v2(
                    accepted.trace().revision(),
                    accepted.trace().active_attempt().unwrap().attempt_id(),
                    None,
                    UnixMillis::new(8),
                )
                .unwrap()
                .into_state();
            store
                .replace_graph_session_v2(
                    &identity(),
                    accepted.workspace_revision(),
                    accepted.trace().revision(),
                    completed.clone(),
                )
                .unwrap();
            store
                .record_terminal_disposition_v2(
                    &identity(),
                    TerminalDispositionV2::not_required(
                        completed.trace().session_id().clone(),
                        completed.trace().revision(),
                        "No handoff required",
                        None,
                        UnixMillis::new(9),
                    )
                    .unwrap(),
                )
                .unwrap();
            store
                .archive_current_session_v2(
                    &identity(),
                    completed.workspace_revision(),
                    completed.trace().revision(),
                    UnixMillis::new(10),
                )
                .unwrap();
        }
        drop(store);
        drop(open(&temporary, None));

        let mut value = original.value().clone();
        match variant {
            "workspace_uuid" => {
                value["workspace_uuid"] = json!("00000000-0000-4000-8000-000000000499")
            }
            "procedure_digest" | "archived_procedure_digest" => {
                value["procedure_digest"] = json!(digest('9'));
            }
            "placement_id" => value["placement_id"] = json!("other-work"),
            "delegation" => {
                value["delegation"]["role_ref"] = json!("other-role");
                value["execution"]["role_ref"] = json!("other-role");
            }
            "noncanonical" => {}
            _ => unreachable!(),
        }
        let stored_json = if variant == "noncanonical" {
            format!("{} ", original.canonical_json())
        } else {
            DelegationReceiptV1::new(value)
                .unwrap()
                .canonical_json()
                .to_owned()
        };
        let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
        connection
            .execute("DELETE FROM v2_delegation_receipts", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO v2_delegation_receipts (attempt_id, session_id, receipt_id, receipt_json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    original.attempt_id().as_str(),
                    original.session_id().as_str(),
                    original.receipt_id().as_str(),
                    stored_json,
                ],
            )
            .unwrap();
        drop(connection);
        let error = match SqliteStoreV1::open(
            temporary.path().join("state.sqlite3"),
            &root(),
            identity(),
            SqliteStoreOptionsV1::new(8).unwrap(),
            UnixMillis::new(11),
        ) {
            Ok(_) => panic!("{variant}: corrupted receipt reopened"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                StoreErrorV1::StorageIntegrityV1 {
                    check: StoreIntegrityCheckV1::SessionCursor,
                }
            ),
            "{variant}: {error:?}"
        );
    }
}

#[test]
fn v2sdg004_archived_receipt_survives_reopen_then_purges_with_session() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &state);
    let accepted = state
        .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&state),
            UnixMillis::new(7),
        )
        .unwrap();
    let completed = accepted
        .complete_active_action_v2(
            accepted.trace().revision(),
            accepted.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            completed.clone(),
        )
        .unwrap();
    store
        .record_terminal_disposition_v2(
            &identity(),
            TerminalDispositionV2::not_required(
                completed.trace().session_id().clone(),
                completed.trace().revision(),
                "No handoff required",
                None,
                UnixMillis::new(9),
            )
            .unwrap(),
        )
        .unwrap();
    store
        .archive_current_session_v2(
            &identity(),
            completed.workspace_revision(),
            completed.trace().revision(),
            UnixMillis::new(10),
        )
        .unwrap();
    drop(store);
    let reopened = open(&temporary, None);
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    drop(connection);
    reopened
        .purge_archived_session_v2(
            &identity(),
            completed.trace().session_id(),
            completed.trace().revision(),
        )
        .unwrap();
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}
