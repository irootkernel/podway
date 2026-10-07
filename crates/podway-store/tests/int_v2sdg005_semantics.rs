//! Current Procedure v2 semantics and direct persistence regression evidence.

use super::int_v2sdg004_receipt::session_state;
use podway_core::{
    BlockerId, CanonicalProcedureJsonV1, ItemId, ProcedureSnapshotId, ProcedureSourceLabelV1,
    Revision, Sha256Digest, UnixMillis, canonicalize_json_v1,
};
use podway_store::{ActiveItemMutationV2, GraphSessionStateV2};
use serde_json::json;
use sha2::{Digest, Sha256};

fn session_state_with_items() -> GraphSessionStateV2 {
    let base = session_state();
    let mut document: serde_json::Value =
        serde_json::from_str(base.snapshot().canonical_json().as_str()).unwrap();
    document["node_definitions"]["work"]["items"] = json!([
        {"id":"result","type":"text","prompt":"Final result","required":true},
        {"id":"note","type":"text","prompt":"Optional note","required":false}
    ]);
    document["manual_rework"] = json!({"allowed_targets":["work"]});
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = podway_store::ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000452").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("delegated-items.yaml").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap();
    GraphSessionStateV2::prepared(
        Revision::new(1),
        "Delegated item task",
        snapshot,
        base.trace().session_id().clone(),
        UnixMillis::new(3),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        base.trace().active_attempt().unwrap().attempt_id().clone(),
        None,
        UnixMillis::new(4),
    )
    .unwrap()
    .into_state()
}

#[test]
fn v2sdg005_acceptance_requires_final_items_and_clear_blockers() {
    assert_eq!(
        podway_store::v2_delegation::accepted_items_digest_v1(&session_state())
            .unwrap()
            .as_str(),
        "sha256:9c18719049a235bff24e61323e212ce320777ee717e2ac34fe660769b1e14b57"
    );
    let state = session_state_with_items();
    let attempt_id = state.trace().active_attempt().unwrap().attempt_id().clone();
    assert!(matches!(
        state.accept_delegation_successor_v1(&attempt_id),
        Err(podway_store::GraphMutationErrorV2::RequiredItemsMissing { .. })
    ));
    let ready = state
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("result").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "Final output".to_owned(),
            },
            UnixMillis::new(5),
        )
        .unwrap()
        .into_state();
    assert_eq!(
        podway_store::v2_delegation::accepted_items_digest_v1(&ready)
            .unwrap()
            .as_str(),
        "sha256:56eb5c46c1d1dbf3a181f723d19c8347ec5a7c92ca1ce8b2af90bba693d3c3eb"
    );
    let blocked = ready
        .block_active_attempt_v2(
            ready.trace().revision(),
            &attempt_id,
            BlockerId::new("00000000-0000-4000-8000-000000000453").unwrap(),
            "Awaiting supervisor",
            UnixMillis::new(6),
        )
        .unwrap()
        .into_state();
    assert!(matches!(
        blocked.accept_delegation_successor_v1(&attempt_id),
        Err(podway_store::GraphMutationErrorV2::BlockersPresent)
    ));
    let unblocked = blocked
        .unblock_active_attempt_v2(
            blocked.trace().revision(),
            &attempt_id,
            None,
            true,
            UnixMillis::new(7),
        )
        .unwrap()
        .into_state();
    assert!(
        unblocked
            .accept_delegation_successor_v1(&attempt_id)
            .is_ok()
    );
}
