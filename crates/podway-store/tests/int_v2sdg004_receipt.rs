//! Current Procedure v2 semantics and direct persistence regression evidence.

use podway_core::{
    AttemptId, CanonicalProcedureJsonV1, ProcedureSnapshotId, ProcedureSourceLabelV1, Revision,
    SessionId, Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{DelegationReceiptV1, DurableWorktreeIdentityV1, GraphSessionStateV2};
use serde_json::json;
use sha2::{Digest, Sha256};

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
    let snapshot = podway_store::admit_procedure_snapshot_v2(
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
        podway_store::admit_procedure_snapshot_v2(
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
    let snapshot = podway_store::admit_procedure_snapshot_v2(
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
fn v2sdg006_receipt_snapshot_rejects_incoherent_window_and_readiness() {
    use podway_store::v2_delegation::{DelegationReadinessV1, DelegationReceiptSnapshotV1};

    let receipt = receipt(&session_state());
    let summary = receipt.summary();
    assert!(
        DelegationReceiptSnapshotV1::new(DelegationReadinessV1::ReceiptCurrent, None, vec![], 0)
            .is_err()
    );
    // STATE-001: a receipt-requiring readiness with no admitted receipt is
    // the snapshot-path fallback incoherence; it resolves to InvalidStateV1.
    assert!(
        DelegationReceiptSnapshotV1::new(DelegationReadinessV1::PackageChanged, None, vec![], 0)
            .is_err()
    );
    assert!(
        DelegationReceiptSnapshotV1::new(DelegationReadinessV1::ItemsChanged, None, vec![], 0)
            .is_err()
    );
    assert!(
        DelegationReceiptSnapshotV1::new(
            DelegationReadinessV1::Direct,
            Some(receipt.clone()),
            vec![summary.clone()],
            1,
        )
        .is_err()
    );
    assert!(
        DelegationReceiptSnapshotV1::new(
            DelegationReadinessV1::ReceiptCurrent,
            Some(receipt.clone()),
            vec![summary.clone(); 33],
            33,
        )
        .is_err()
    );
    assert!(
        DelegationReceiptSnapshotV1::new(
            DelegationReadinessV1::ReceiptCurrent,
            Some(receipt.clone()),
            vec![summary.clone(), summary.clone()],
            1,
        )
        .is_err()
    );
    let mut other = summary.clone();
    other["receipt_id"] = json!("00000000-0000-4000-8000-000000000499");
    assert!(
        DelegationReceiptSnapshotV1::new(
            DelegationReadinessV1::ReceiptCurrent,
            Some(receipt),
            vec![other],
            1,
        )
        .is_err()
    );
}

use super::int_v2dfc005_direct_store::{deadline, request, success};
use super::int_v2dfc008_persistence::{fixture, image, reject_corruption, seed};
use rusqlite::{Connection, params};

fn accepted_fixture() -> (
    tempfile::TempDir,
    podway_store::direct_store::DirectStoreV1,
    GraphSessionStateV2,
    DelegationReceiptV1,
) {
    let (temporary, store) = fixture(&identity());
    let current = session_state();
    seed(&store, &current);
    let original = receipt(&current);
    let accepted = current
        .accept_delegation_successor_v1(current.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    let accept = request("accept-current-delegation");
    store
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
                    &digest('d'),
                    &original,
                )?;
                Ok(success(
                    &accept,
                    json!({"accepted":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    store.verify_integrity(deadline()).unwrap();
    (temporary, store, accepted, original)
}

#[test]
fn v2sdg006_live_receipt_reads_reject_row_identity_mismatch() {
    for field in ["session_id", "workspace_uuid", "attempt_id", "receipt_id"] {
        let (_temporary, store, _state, original) = accepted_fixture();
        let mut changed = original.value().clone();
        changed[field] = json!("00000000-0000-4000-8000-000000000499");
        let changed = DelegationReceiptV1::new(changed).unwrap();
        let connection = Connection::open(store.layout().database()).unwrap();
        connection
            .execute("DELETE FROM v2_delegation_receipts", [])
            .unwrap();
        connection.execute(
            "INSERT INTO v2_delegation_receipts(attempt_id,session_id,receipt_id,receipt_json) VALUES (?1,?2,?3,?4)",
            params![original.attempt_id().as_str(), original.session_id().as_str(), original.receipt_id().as_str(), changed.canonical_json()],
        ).unwrap();
        drop(connection);
        assert!(
            store
                .read(deadline(), |snapshot| snapshot.delegation_receipt(
                    &original.session_id(),
                    &original.attempt_id()
                ))
                .is_err()
        );
        reject_corruption(&store, true);
    }
}

#[test]
fn v2sdg004_orphan_receipt_is_rejected_by_cold_doctor() {
    let (_temporary, store) = fixture(&identity());
    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    let bogus = "00000000-0000-4000-8000-000000000499";
    connection.execute(
        "INSERT INTO v2_delegation_receipts(attempt_id,session_id,receipt_id,receipt_json) VALUES (?1,?1,?1,'{}')",
        [bogus],
    ).unwrap();
    drop(connection);
    reject_corruption(&store, false);
}

#[test]
fn v2sdg004_cold_reads_reject_well_formed_mismatched_and_noncanonical_receipts() {
    for variant in [
        "workspace_uuid",
        "procedure_digest",
        "placement_id",
        "delegation",
        "noncanonical",
        "archived_procedure_digest",
        "archived_receipt_id",
    ] {
        let (_temporary, store, accepted, original) = accepted_fixture();
        let archived = variant.starts_with("archived_");
        if archived {
            let completed = accepted
                .complete_active_action_v2(
                    accepted.trace().revision(),
                    accepted.trace().active_attempt().unwrap().attempt_id(),
                    None,
                    UnixMillis::new(8),
                )
                .unwrap()
                .into_state();
            let archive = request("archive-current-delegation");
            store
                .mutate(
                    &archive,
                    deadline(),
                    || UnixMillis::new(300),
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
                                UnixMillis::new(9),
                            )
                            .unwrap(),
                        )?;
                        transaction.archive_graph(
                            completed.workspace_revision(),
                            completed.trace().revision(),
                        )?;
                        Ok(success(
                            &archive,
                            json!({"archived":true}),
                            transaction.now(),
                        ))
                    },
                )
                .unwrap();
            store.verify_integrity(deadline()).unwrap();
        }
        let mut value = original.value().clone();
        match variant {
            "workspace_uuid" => {
                value["workspace_uuid"] = json!("00000000-0000-4000-8000-000000000499")
            }
            "procedure_digest" | "archived_procedure_digest" => {
                value["procedure_digest"] = json!(digest('9'))
            }
            "archived_receipt_id" => {
                value["receipt_id"] = json!("00000000-0000-4000-8000-000000000499")
            }
            "placement_id" => value["placement_id"] = json!("other-work"),
            "delegation" => {
                value["delegation"]["role_ref"] = json!("other-role");
                value["execution"]["role_ref"] = json!("other-role");
            }
            "noncanonical" => {}
            _ => unreachable!(),
        }
        let stored = if variant == "noncanonical" {
            format!("{} ", original.canonical_json())
        } else {
            DelegationReceiptV1::new(value)
                .unwrap()
                .canonical_json()
                .to_owned()
        };
        let connection = Connection::open(store.layout().database()).unwrap();
        connection
            .execute("DELETE FROM v2_delegation_receipts", [])
            .unwrap();
        connection.execute(
            "INSERT INTO v2_delegation_receipts(attempt_id,session_id,receipt_id,receipt_json) VALUES (?1,?2,?3,?4)",
            params![original.attempt_id().as_str(), original.session_id().as_str(), original.receipt_id().as_str(), stored],
        ).unwrap();
        drop(connection);
        if archived {
            assert!(
                store
                    .read(deadline(), |snapshot| snapshot
                        .archived_view(&original.session_id(), UnixMillis::new(400)))
                    .is_err()
            );
        }
        reject_corruption(&store, !archived);
    }
}

fn reject_acceptance(
    store: &podway_store::direct_store::DirectStoreV1,
    current: &GraphSessionStateV2,
    next: &GraphSessionStateV2,
    receipt: &DelegationReceiptV1,
) {
    let key = request("reject-delegation-acceptance");
    let sequence = store
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    let connection = Connection::open(store.layout().database()).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    drop(connection);
    assert!(
        store
            .mutate(
                &key,
                deadline(),
                || UnixMillis::new(400),
                |transaction| {
                    transaction.now();
                    transaction.accept_delegation(
                        current.workspace_revision(),
                        current.trace().revision(),
                        next,
                        &digest('d'),
                        receipt,
                    )?;
                    Ok(success(&key, json!({"accepted":true}), transaction.now()))
                }
            )
            .is_err()
    );
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.current_graph()?, Some(current.clone()));
            assert_eq!(snapshot.committed_sequence()?, sequence);
            assert!(snapshot.outcome(key.key())?.is_none());
            Ok(())
        })
        .unwrap();
    let connection = Connection::open(store.layout().database()).unwrap();
    let after: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(after, count);
    drop(connection);
    store.verify_integrity(deadline()).unwrap();
}

#[test]
fn v2sdg004_receipt_rejects_wrong_parent_without_state_or_outcome_commit() {
    for field in [
        "workspace_uuid",
        "session_id",
        "attempt_id",
        "placement_id",
        "procedure_digest",
        "delegation",
        "package_digest",
        "accepted_items_digest",
    ] {
        let (_temporary, store) = fixture(&identity());
        let current = session_state();
        seed(&store, &current);
        let next = current
            .accept_delegation_successor_v1(current.trace().active_attempt().unwrap().attempt_id())
            .unwrap();
        let mut value = receipt(&current).value().clone();
        match field {
            "workspace_uuid" | "session_id" | "attempt_id" => {
                value[field] = json!("00000000-0000-4000-8000-000000000498")
            }
            "placement_id" => value[field] = json!("other"),
            "procedure_digest" | "package_digest" | "accepted_items_digest" => {
                value[field] = json!(digest('8'))
            }
            "delegation" => {
                value["delegation"]["role_ref"] = json!("other-role");
                value["execution"]["role_ref"] = json!("other-role");
            }
            _ => unreachable!(),
        }
        reject_acceptance(
            &store,
            &current,
            &next,
            &DelegationReceiptV1::new(value).unwrap(),
        );
    }
}

#[test]
fn v2sdg004_second_acceptance_of_same_attempt_rolls_back() {
    let (_temporary, store, current, _original) = accepted_fixture();
    let next = current
        .accept_delegation_successor_v1(current.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    let mut value = receipt(&current).value().clone();
    value["receipt_id"] = json!("00000000-0000-4000-8000-000000000409");
    reject_acceptance(
        &store,
        &current,
        &next,
        &DelegationReceiptV1::new(value).unwrap(),
    );
}

#[test]
fn v2sdg004_duplicate_receipt_id_across_attempts_rolls_back() {
    let (_temporary, store, current, original) = accepted_fixture();
    let retried = current
        .retry_active_attempt_v2(
            current.trace().revision(),
            current.trace().active_attempt().unwrap().attempt_id(),
            AttemptId::new("00000000-0000-4000-8000-00000000040a").unwrap(),
            podway_core::ReasonV2::new("Retry delegated work").unwrap(),
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    let retry = request("retry-delegation");
    store
        .mutate(
            &retry,
            deadline(),
            || UnixMillis::new(300),
            |transaction| {
                transaction.now();
                transaction.replace_graph(
                    current.workspace_revision(),
                    current.trace().revision(),
                    &retried,
                )?;
                Ok(success(&retry, json!({"retried":true}), transaction.now()))
            },
        )
        .unwrap();
    let next = retried
        .accept_delegation_successor_v1(retried.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    reject_acceptance(&store, &retried, &next, &receipt(&retried));
    let connection = Connection::open(store.layout().database()).unwrap();
    let rows: Vec<(String, String)> = connection
        .prepare("SELECT attempt_id,receipt_id FROM v2_delegation_receipts")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![(
            original.attempt_id().as_str().to_owned(),
            original.receipt_id().as_str().to_owned()
        )]
    );
}

#[test]
fn v2sdg006_receipt_history_reports_exact_total_and_newest_bounded_window() {
    let (_temporary, store) = fixture(&identity());
    let mut state = session_state();
    seed(&store, &state);
    let mut receipt_ids = Vec::new();
    for index in 0..33_u64 {
        let mut value = receipt(&state).value().clone();
        value["receipt_id"] = json!(format!("00000000-0000-4000-8000-{:012x}", 0x600 + index));
        let original = DelegationReceiptV1::new(value).unwrap();
        receipt_ids.push(original.receipt_id().as_str().to_owned());
        let accepted = state
            .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
            .unwrap();
        let accept = request(&format!("receipt-window-{index}"));
        store
            .mutate(
                &accept,
                deadline(),
                || UnixMillis::new(10 + index * 3),
                |transaction| {
                    transaction.now();
                    transaction.accept_delegation(
                        state.workspace_revision(),
                        state.trace().revision(),
                        &accepted,
                        &digest('d'),
                        &original,
                    )?;
                    Ok(success(
                        &accept,
                        json!({"accepted":true}),
                        transaction.now(),
                    ))
                },
            )
            .unwrap();
        state = accepted;
        if index < 32 {
            let retried = state
                .retry_active_attempt_v2(
                    state.trace().revision(),
                    state.trace().active_attempt().unwrap().attempt_id(),
                    AttemptId::new(format!("00000000-0000-4000-8000-{:012x}", 0x700 + index))
                        .unwrap(),
                    podway_core::ReasonV2::new("Inspect the next accepted attempt").unwrap(),
                    UnixMillis::new(11 + index * 3),
                )
                .unwrap()
                .into_state();
            let retry = request(&format!("retry-receipt-window-{index}"));
            store
                .mutate(
                    &retry,
                    deadline(),
                    || UnixMillis::new(11 + index * 3),
                    |transaction| {
                        transaction.now();
                        transaction.replace_graph(
                            state.workspace_revision(),
                            state.trace().revision(),
                            &retried,
                        )?;
                        Ok(success(&retry, json!({"retried":true}), transaction.now()))
                    },
                )
                .unwrap();
            state = retried;
            if index == 0 {
                let view = store
                    .read(deadline(), |snapshot| snapshot.view(UnixMillis::new(12)))
                    .unwrap();
                let receipts = view.delegation_receipts();
                assert_eq!(
                    receipts.readiness(),
                    podway_store::v2_delegation::DelegationReadinessV1::ReceiptMissing
                );
                assert!(receipts.current_receipt().is_none());
                assert_eq!(receipts.total(), 1);
                assert_eq!(receipts.summaries().len(), 1);
                assert!(!receipts.truncated());
            }
        }
    }
    let view = store
        .read(deadline(), |snapshot| snapshot.view(UnixMillis::new(109)))
        .unwrap();
    let receipts = view.delegation_receipts();
    assert_eq!(receipts.total(), 33);
    assert_eq!(receipts.summaries().len(), 32);
    assert!(receipts.truncated());
    assert_eq!(
        receipts.current_receipt().unwrap().summary(),
        receipts.summaries()[0]
    );
    let selected: Vec<_> = receipts
        .summaries()
        .iter()
        .map(|summary| summary["receipt_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        selected,
        receipt_ids[1..]
            .iter()
            .rev()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    let completed = state
        .complete_active_action_v2(
            state.trace().revision(),
            state.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(110),
        )
        .unwrap()
        .into_state();
    let complete = request("complete-receipt-window");
    store
        .mutate(
            &complete,
            deadline(),
            || UnixMillis::new(110),
            |transaction| {
                transaction.now();
                transaction.replace_graph(
                    state.workspace_revision(),
                    state.trace().revision(),
                    &completed,
                )?;
                Ok(success(
                    &complete,
                    json!({"completed":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    super::int_v2dfc008_persistence::archive(&store, &completed, UnixMillis::new(111));
    let cold =
        super::int_v2dfc005_direct_store::store(store.layout().root().to_path_buf().as_path());
    let archived = cold
        .read(deadline(), |snapshot| {
            snapshot.archived_view(completed.trace().session_id(), UnixMillis::new(112))
        })
        .unwrap()
        .unwrap();
    assert_eq!(archived.delegation_receipts().total(), 33);
    assert_eq!(
        archived.delegation_receipts().summaries(),
        receipts.summaries()
    );
    cold.verify_integrity(deadline()).unwrap();
}

/// STATE-001 outcome pin: with a truncated receipt history and an active
/// attempt that holds no receipt, the view reports `ReceiptMissing`. This
/// pins the observable outcome only — window-derived absence and the point
/// read are indistinguishable on this path; the prepared-Arc write-path
/// test below separates the two acquisitions.
#[test]
fn state001_truncated_history_reports_point_readiness_outcome_for_absent_active_receipt() {
    let (_temporary, store) = fixture(&identity());
    let mut state = session_state();
    seed(&store, &state);
    for index in 0..33_u64 {
        let mut value = receipt(&state).value().clone();
        value["receipt_id"] = json!(format!("00000000-0000-4000-8000-{:012x}", 0x610 + index));
        let original = DelegationReceiptV1::new(value).unwrap();
        let accepted = state
            .accept_delegation_successor_v1(state.trace().active_attempt().unwrap().attempt_id())
            .unwrap();
        let accept = request(&format!("truncated-accept-{index}"));
        store
            .mutate(
                &accept,
                deadline(),
                || UnixMillis::new(10 + index * 3),
                |transaction| {
                    transaction.now();
                    transaction.accept_delegation(
                        state.workspace_revision(),
                        state.trace().revision(),
                        &accepted,
                        &digest('d'),
                        &original,
                    )?;
                    Ok(success(
                        &accept,
                        json!({"accepted":true}),
                        transaction.now(),
                    ))
                },
            )
            .unwrap();
        state = accepted;
        let retried = state
            .retry_active_attempt_v2(
                state.trace().revision(),
                state.trace().active_attempt().unwrap().attempt_id(),
                AttemptId::new(format!("00000000-0000-4000-8000-{:012x}", 0x710 + index)).unwrap(),
                podway_core::ReasonV2::new("Move past the accepted attempt").unwrap(),
                UnixMillis::new(11 + index * 3),
            )
            .unwrap()
            .into_state();
        let retry = request(&format!("truncated-retry-{index}"));
        store
            .mutate(
                &retry,
                deadline(),
                || UnixMillis::new(11 + index * 3),
                |transaction| {
                    transaction.now();
                    transaction.replace_graph(
                        state.workspace_revision(),
                        state.trace().revision(),
                        &retried,
                    )?;
                    Ok(success(&retry, json!({"retried":true}), transaction.now()))
                },
            )
            .unwrap();
        state = retried;
    }
    // 33 stored receipts, the 32 newest in the window, and the final retry's
    // active attempt holds none of them.
    let view = store
        .read(deadline(), |snapshot| snapshot.view(UnixMillis::new(200)))
        .unwrap();
    let receipts = view.delegation_receipts();
    assert_eq!(receipts.total(), 33);
    assert!(receipts.truncated());
    assert_eq!(receipts.summaries().len(), 32);
    assert!(receipts.current_receipt().is_none());
    assert_eq!(
        receipts.readiness(),
        podway_store::v2_delegation::DelegationReadinessV1::ReceiptMissing
    );
}

/// STATE-001: whole-window history validation runs before readiness, so a
/// malformed older entry refuses the view even though the active attempt's
/// receipt is present and valid.
#[test]
fn state001_older_receipt_window_corruption_refuses_with_valid_active_receipt() {
    let (_temporary, store) = fixture(&identity());
    let current = session_state();
    seed(&store, &current);
    let first = receipt(&current);
    let accepted = current
        .accept_delegation_successor_v1(current.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    let first_accept = request("older-window-first-accept");
    store
        .mutate(
            &first_accept,
            deadline(),
            || UnixMillis::new(200),
            |transaction| {
                transaction.now();
                transaction.accept_delegation(
                    current.workspace_revision(),
                    current.trace().revision(),
                    &accepted,
                    &digest('d'),
                    &first,
                )?;
                Ok(success(
                    &first_accept,
                    json!({"accepted":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let retried = accepted
        .retry_active_attempt_v2(
            accepted.trace().revision(),
            accepted.trace().active_attempt().unwrap().attempt_id(),
            AttemptId::new("00000000-0000-4000-8000-000000000406").unwrap(),
            podway_core::ReasonV2::new("Retry delegated work").unwrap(),
            UnixMillis::new(201),
        )
        .unwrap()
        .into_state();
    let retry = request("older-window-retry");
    store
        .mutate(
            &retry,
            deadline(),
            || UnixMillis::new(202),
            |transaction| {
                transaction.now();
                transaction.replace_graph(
                    accepted.workspace_revision(),
                    accepted.trace().revision(),
                    &retried,
                )?;
                Ok(success(&retry, json!({"retried":true}), transaction.now()))
            },
        )
        .unwrap();
    let mut value = receipt(&retried).value().clone();
    value["receipt_id"] = json!("00000000-0000-4000-8000-000000000407");
    let second = DelegationReceiptV1::new(value).unwrap();
    let accepted_second = retried
        .accept_delegation_successor_v1(retried.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    let second_accept = request("older-window-second-accept");
    store
        .mutate(
            &second_accept,
            deadline(),
            || UnixMillis::new(203),
            |transaction| {
                transaction.now();
                transaction.accept_delegation(
                    retried.workspace_revision(),
                    retried.trace().revision(),
                    &accepted_second,
                    &digest('d'),
                    &second,
                )?;
                Ok(success(
                    &second_accept,
                    json!({"accepted":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    // Control: the active attempt's receipt is valid, newest and current.
    let healthy = store
        .read(deadline(), |snapshot| {
            let view = snapshot.view(UnixMillis::new(204))?;
            Ok((
                view.delegation_receipts().readiness(),
                view.delegation_receipts().current_receipt().is_some(),
                view.delegation_receipts().total(),
            ))
        })
        .unwrap();
    assert_eq!(
        healthy,
        (
            podway_store::v2_delegation::DelegationReadinessV1::ReceiptCurrent,
            true,
            2
        )
    );
    // Forge only the older accepting attempt's stored receipt.
    let mut forged = first.value().clone();
    forged["workspace_uuid"] = json!("00000000-0000-4000-8000-000000000499");
    let stored = DelegationReceiptV1::new(forged)
        .unwrap()
        .canonical_json()
        .to_owned();
    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute(
            "DELETE FROM v2_delegation_receipts WHERE attempt_id=?1",
            [first.attempt_id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_delegation_receipts(attempt_id,session_id,receipt_id,receipt_json) VALUES (?1,?2,?3,?4)",
            params![
                first.attempt_id().as_str(),
                first.session_id().as_str(),
                first.receipt_id().as_str(),
                stored
            ],
        )
        .unwrap();
    drop(connection);
    let cold =
        super::int_v2dfc005_direct_store::store(store.layout().root().to_path_buf().as_path());
    assert!(matches!(
        cold.read(deadline(), |snapshot| snapshot.view(UnixMillis::new(205))),
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::DelegationReceipt,
            }
        ))
    ));
    reject_corruption(&cold, true);
}

/// STATE-001: completing a delegated attempt without an admitted receipt
/// fails the write-time guard inside replace_direct_graph_session_transaction_v2.
#[test]
fn state001_completing_delegated_attempt_without_admitted_receipt_fails_write_time_guard() {
    let (_temporary, store) = fixture(&identity());
    let current = session_state();
    seed(&store, &current);
    let completed = current
        .complete_active_action_v2(
            current.trace().revision(),
            current.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(300),
        )
        .unwrap()
        .into_state();
    let sequence = store
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    let complete = request("complete-without-receipt");
    let refusal = store.mutate(
        &complete,
        deadline(),
        || UnixMillis::new(301),
        |transaction| {
            transaction.now();
            transaction.replace_graph(
                current.workspace_revision(),
                current.trace().revision(),
                &completed,
            )?;
            Ok(success(
                &complete,
                json!({"completed":true}),
                transaction.now(),
            ))
        },
    );
    assert!(matches!(
        refusal,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::InvalidStateV1(
                podway_store::StoreValueErrorV1::InvalidProcedureV2State {
                    reason: "delegation receipt is missing or stale",
                }
            )
        ))
    ));
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.current_graph()?, Some(current.clone()));
            assert_eq!(snapshot.committed_sequence()?, sequence);
            assert!(snapshot.outcome(complete.key())?.is_none());
            Ok(())
        })
        .unwrap();
    store.verify_integrity(deadline()).unwrap();
}

/// Deterministic dump of every user table's committed rows through a plain
/// connection: tables in name order, rows in rowid order, typed values. The
/// dump is insensitive to WAL and checkpoint file lifecycle, so it captures
/// the committed logical content rather than physical bytes. It assumes
/// rowid tables: the v15 schema has no `WITHOUT ROWID` table, and such a
/// table would fail loudly at the `ORDER BY rowid` prepare.
fn committed_logical_dump(database: &std::path::Path) -> String {
    let connection = Connection::open(database).unwrap();
    let mut tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(0))
                .map(|rows| rows.collect::<Result<Vec<String>, _>>())
        })
        .unwrap()
        .unwrap();
    tables.retain(|table| !table.starts_with("sqlite_"));
    let mut dump = String::new();
    for table in tables {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let mut fields = Vec::with_capacity(columns);
            for index in 0..columns {
                fields.push(format!(
                    "{:?}",
                    row.get::<_, rusqlite::types::Value>(index).unwrap()
                ));
            }
            dump.push_str(&table);
            dump.push('|');
            dump.push_str(&fields.join("\u{1f}"));
            dump.push('\n');
        }
    }
    dump
}

/// STATE-001 write-path boundary: a prepared-Arc transaction's receipt
/// snapshot keeps the point-query acquisition. After the active attempt's
/// row is made unjoinable to v2_attempts out of band (with no sequence or
/// stamp change), the count and window miss the stored receipt while the
/// point query still finds it, and today's read refuses through snapshot
/// coherence instead of deriving absence from the window.
#[test]
fn state001_prepared_arc_write_path_receipt_snapshot_refuses_unjoinable_active_receipt() {
    let (_temporary, store, accepted, _original) = accepted_fixture();
    let active = accepted
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let prepared = store.prepare_graph(deadline()).unwrap().unwrap();
    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    let deleted = connection
        .execute(
            "DELETE FROM v2_attempts WHERE attempt_id=?1",
            [active.as_str()],
        )
        .unwrap();
    drop(connection);
    assert_eq!(deleted, 1, "the out-of-band delete removes the active row");
    // The delete itself changes bytes and content; the refused write below
    // must change neither the committed logical content nor the committed
    // main database file.
    let after_delete = image(&store);
    let after_delete_dump = committed_logical_dump(&store.layout().database());
    let outcomes_after_delete: i64 = Connection::open(store.layout().database())
        .and_then(|connection| {
            connection.query_row("SELECT COUNT(*) FROM mutation_outcomes", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    let sequence = store
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    let key = request("prepared-arc-receipts");
    let refusal = store.mutate(
        &key,
        deadline(),
        || UnixMillis::new(400),
        |transaction| {
            transaction.use_prepared_graph(Some(&prepared))?;
            transaction.delegation_receipts()?;
            Ok(success(&key, json!({"read":true}), transaction.now()))
        },
    );
    assert!(matches!(
        refusal,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::InvalidStateV1(
                podway_store::StoreValueErrorV1::InvalidProcedureV2State {
                    reason: "delegation receipt is invalid",
                }
            )
        ))
    ));
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.committed_sequence()?, sequence);
            assert!(snapshot.outcome(key.key())?.is_none());
            Ok(())
        })
        .unwrap();
    let outcomes_after_refusal: i64 = Connection::open(store.layout().database())
        .and_then(|connection| {
            connection.query_row("SELECT COUNT(*) FROM mutation_outcomes", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(
        outcomes_after_refusal, outcomes_after_delete,
        "the refused write must not record any outcome row"
    );
    // The committed logical content is identical, proving the refused write
    // committed nothing anywhere (a WAL-confined commit would surface here).
    assert_eq!(
        committed_logical_dump(&store.layout().database()),
        after_delete_dump,
        "the refused write must not change any committed table content"
    );
    // The main database file also keeps its exact bytes. The WAL file's
    // lifecycle bytes may differ around the plain out-of-band connection,
    // so the physical comparison covers the main database file only.
    assert_eq!(
        image(&store)[0],
        after_delete[0],
        "the refused write must not change the committed database bytes"
    );
}
