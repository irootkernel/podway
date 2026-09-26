use podway_core::{
    AttemptId, BlockerId, CanonicalProcedureJsonV1, CriterionId, DomainCommand, DomainError,
    DomainResult, GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2, GoalRevisionReasonV2,
    GoalStatementV2, GraphNodeId, ItemId, ProcedureSnapshotId, ProcedureSourceLabelV1, ReasonV2,
    Revision, Sha256Digest, UnixMillis, canonicalize_json_v1,
};
use podway_store::{
    ActiveItemMutationV2, AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1,
    CanonicalExecutionJsonV1, DelegationReceiptV1, GraphInitialGoalV2, GraphSessionStateV2,
    IdempotencyKeyV1, PersistedGraphMutationFailureV2, PersistedGraphTerminalOperationV2,
    PersistedResponseContextV1, RevisionAttemptItemPreconditionsV1, SqliteStoreV1, StoreContractV1,
    StoreErrorV1, StoreGraphMutationContractV2, StoreGraphReadContractV2,
    StoreGraphStateContractV2, StoreIdempotencyReadContractV1, StoreReadContractV1,
    TerminalResultV1, WorkerIdV1,
};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::int_v2sdg004_receipt::{
    admit_and_claim, admit_and_claim_with, declaration, digest, identity, open, receipt,
    session_state,
};

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

fn direct_session_state() -> GraphSessionStateV2 {
    let base = session_state();
    let mut document: serde_json::Value =
        serde_json::from_str(base.snapshot().canonical_json().as_str()).unwrap();
    document["graph"]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("delegation");
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = podway_store::ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000466").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("direct.yaml").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap();
    GraphSessionStateV2::prepared(
        Revision::new(1),
        "Direct task",
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

fn session_state_with_goal() -> GraphSessionStateV2 {
    let base = session_state();
    let mut document: serde_json::Value =
        serde_json::from_str(base.snapshot().canonical_json().as_str()).unwrap();
    document["goal_tracking"] = json!(true);
    document["node_definitions"]["assess"] = json!({
        "type":"decision",
        "title":"Assess",
        "objective":"Assess the revised goal.",
        "prompt":"What is the goal outcome?",
        "options":[
            {"id":"achieved","label":"Achieved"},
            {"id":"not-achieved","label":"Not achieved"},
            {"id":"superseded","label":"Superseded"}
        ],
        "assessment":{
            "target":"session_goal",
            "outcomes":{
                "achieved":"achieved",
                "not-achieved":"not_achieved",
                "superseded":"superseded"
            }
        },
        "reason":{"required":true}
    });
    document["node_definitions"]["finish"] = json!({
        "type":"action","title":"Finish","intent":"Finish the goal."
    });
    document["graph"]["nodes"] = json!([
        {"id":"work","use":"work","next":"assess","delegation":declaration()},
        {"id":"assess","use":"assess","routes":{
            "achieved":{"to":"finish","effect":"advance"},
            "not-achieved":{"to":"finish","effect":"advance"},
            "superseded":{"to":"finish","effect":"advance"}
        }},
        {"id":"finish","use":"finish","terminal":true}
    ]);
    document["manual_rework"] = json!({"allowed_targets":["work"]});
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = podway_store::ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000460").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("delegated-goal.yaml").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap();
    let initial_goal = GraphInitialGoalV2::new(
        GoalStatementV2::new("Accept the delegated result").unwrap(),
        GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(CriterionId::new("reviewed").unwrap(), "Result reviewed.")
                .unwrap(),
        ])
        .unwrap(),
        None,
    );
    GraphSessionStateV2::prepared(
        Revision::new(1),
        "Delegated goal task",
        snapshot,
        base.trace().session_id().clone(),
        UnixMillis::new(3),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        base.trace().active_attempt().unwrap().attempt_id().clone(),
        Some(initial_goal),
        UnixMillis::new(4),
    )
    .unwrap()
    .into_state()
}

fn admit_complete_and_claim(
    store: &SqliteStoreV1,
    state: &GraphSessionStateV2,
    key: &str,
    job_number: u64,
) -> podway_store::ClaimedJobV1 {
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "session.complete",
            "execution_version": 6
        }))
        .unwrap(),
    )
    .unwrap();
    let request = AdmitRequestV1::new_with_canonical_execution(
        DomainCommand::SessionComplete,
        IdempotencyKeyV1::new(key).unwrap(),
        podway_core::JobId::new(format!("00000000-0000-4000-9000-{job_number:012x}")).unwrap(),
        RevisionAttemptItemPreconditionsV1::new(
            Some(state.trace().revision()),
            Some(state.trace().active_attempt().unwrap().attempt_id().clone()),
            None,
            None,
        )
        .unwrap(),
        digest('8'),
        UnixMillis::new(20),
        execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        state.trace().session_id().clone(),
    ))
    .with_response_context(
        PersistedResponseContextV1::new(
            format!("00000000-0000-4000-8000-{:012x}", job_number + 1),
            "session.complete",
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
            UnixMillis::new(21),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn v2sdg007_acceptance_revision_race_persists_a_terminal_conflict() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let initial = session_state_with_items();
    let attempt_id = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let ready = initial
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("result").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "Final result".to_owned(),
            },
            UnixMillis::new(5),
        )
        .unwrap()
        .into_state();
    store
        .create_graph_session_v2(&identity(), ready.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &ready);
    let changed = ready
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("note").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "New evidence".to_owned(),
            },
            UnixMillis::new(7),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            ready.workspace_revision(),
            ready.trace().revision(),
            changed.clone(),
        )
        .unwrap();

    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            changed.workspace_revision(),
            changed.trace().revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            PersistedGraphTerminalOperationV2::failure(
                PersistedGraphMutationFailureV2::SessionRevisionConflict {
                    expected: ready.trace().revision(),
                    actual: changed.trace().revision(),
                },
            )
            .unwrap(),
            UnixMillis::new(8),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap().unwrap(),
        changed
    );
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

#[test]
fn v2sdg005_acceptance_rejects_package_and_item_state_drift_atomically() {
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
    let mut wrong_summary = receipt(&state).value().clone();
    wrong_summary["summary"] = json!("Different summary");
    assert!(matches!(
        store.commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision().checked_next().unwrap(),
            next.clone(),
            digest('d'),
            DelegationReceiptV1::new(wrong_summary.clone()).unwrap(),
            UnixMillis::new(7),
        ),
        Err(StoreErrorV1::PreconditionConflictV1 { .. })
    ));
    assert!(matches!(
        store.commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            next.clone(),
            digest('d'),
            DelegationReceiptV1::new(wrong_summary).unwrap(),
            UnixMillis::new(7),
        ),
        Err(StoreErrorV1::InternalInvariantViolationV1 { .. })
    ));
    assert!(matches!(
        store.commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            next.clone(),
            digest('8'),
            receipt(&state),
            UnixMillis::new(7),
        ),
        Err(StoreErrorV1::InvalidStateV1(_))
    ));
    let mut wrong_items = receipt(&state).value().clone();
    wrong_items["accepted_items_digest"] = json!(digest('8'));
    assert!(matches!(
        store.commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            next,
            digest('d'),
            DelegationReceiptV1::new(wrong_items).unwrap(),
            UnixMillis::new(7),
        ),
        Err(StoreErrorV1::InvalidStateV1(_))
    ));
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(state)
    );
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        podway_store::v2_delegation::DelegationReadinessV1::ReceiptMissing
    );
    assert!(
        store
            .read_job(&identity(), claimed.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
}

#[test]
fn v2sdg005_completion_failure_and_success_are_validated_in_store_transaction() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let initial = session_state();
    store
        .create_graph_session_v2(&identity(), initial.clone())
        .unwrap();
    let attempt_id = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let completed = initial
        .complete_active_action_v2(
            initial.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(22),
        )
        .unwrap()
        .into_state();
    let first = admit_complete_and_claim(&store, &initial, "complete-before-accept", 0x455);
    let success = TerminalResultV1::Success(DomainResult::SessionChanged {
        session_id: initial.trace().session_id().clone(),
        revision_before: initial.trace().revision(),
        revision_after: completed.trace().revision(),
        changed: true,
    });
    let complete_operation = PersistedGraphTerminalOperationV2::complete(
        initial
            .trace()
            .active_attempt()
            .unwrap()
            .graph_node_id()
            .clone(),
        attempt_id.clone(),
        None,
        None,
    )
    .unwrap();
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                first.claim().clone(),
                initial.workspace_revision(),
                initial.trace().revision(),
                Some(completed),
                success,
                complete_operation,
                UnixMillis::new(22),
            )
            .is_err()
    );
    assert!(
        store
            .read_job(&identity(), first.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    let failure = TerminalResultV1::Failure(DomainError::InvalidState {
        reason: "Procedure v2 graph mutation failed",
    });
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                first.claim().clone(),
                initial.workspace_revision(),
                initial.trace().revision(),
                None,
                failure.clone(),
                PersistedGraphTerminalOperationV2::failure(
                    PersistedGraphMutationFailureV2::DelegationPackageChanged,
                )
                .unwrap(),
                UnixMillis::new(22),
            )
            .is_err()
    );
    assert!(
        store
            .read_job(&identity(), first.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    store
        .commit_graph_mutation_terminal_v2(
            first.claim().clone(),
            initial.workspace_revision(),
            initial.trace().revision(),
            None,
            failure,
            PersistedGraphTerminalOperationV2::failure(
                PersistedGraphMutationFailureV2::DelegationReceiptRequired,
            )
            .unwrap(),
            UnixMillis::new(22),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(initial.clone())
    );

    let accepted_job = admit_and_claim_with(&store, &initial, "accept-after-failure", 0x456);
    let accepted = initial.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            accepted_job.claim().clone(),
            initial.workspace_revision(),
            initial.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&initial),
            UnixMillis::new(23),
        )
        .unwrap();
    let completed = accepted
        .complete_active_action_v2(
            accepted.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(24),
        )
        .unwrap()
        .into_state();
    let second = admit_complete_and_claim(&store, &accepted, "complete-after-accept", 0x457);
    store
        .commit_graph_mutation_terminal_v2(
            second.claim().clone(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            Some(completed.clone()),
            TerminalResultV1::Success(DomainResult::SessionChanged {
                session_id: accepted.trace().session_id().clone(),
                revision_before: accepted.trace().revision(),
                revision_after: completed.trace().revision(),
                changed: true,
            }),
            PersistedGraphTerminalOperationV2::complete(
                accepted
                    .trace()
                    .active_attempt()
                    .unwrap()
                    .graph_node_id()
                    .clone(),
                attempt_id,
                None,
                None,
            )
            .unwrap(),
            UnixMillis::new(24),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(completed)
    );
    assert!(
        store
            .read_job(&identity(), second.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_some()
    );
}

#[test]
fn v2sdg005_receipt_stales_only_after_committed_item_change() {
    use podway_store::v2_delegation::{DelegationReadinessV1, accepted_items_digest_v1};

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let initial = session_state_with_items();
    let attempt_id = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let ready = initial
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
    store
        .create_graph_session_v2(&identity(), ready.clone())
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptMissing
    );
    let premature = ready
        .complete_active_action_v2(
            ready.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(6),
        )
        .unwrap()
        .into_state();
    assert!(
        store
            .replace_graph_session_v2(
                &identity(),
                ready.workspace_revision(),
                ready.trace().revision(),
                premature,
            )
            .is_err()
    );
    let no_op = ready
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("result").unwrap(),
            Revision::new(1),
            ActiveItemMutationV2::Set {
                value: "Final output".to_owned(),
            },
            UnixMillis::new(6),
        )
        .unwrap();
    assert!(!no_op.changed());
    assert_eq!(no_op.state(), &ready);

    let claimed = admit_and_claim(&store, &ready);
    let accepted = ready.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            ready.workspace_revision(),
            ready.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&ready),
            UnixMillis::new(7),
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptCurrent
    );
    assert_eq!(
        accepted_items_digest_v1(&ready).unwrap(),
        accepted_items_digest_v1(&accepted).unwrap()
    );
    let blocked = accepted
        .block_active_attempt_v2(
            accepted.trace().revision(),
            &attempt_id,
            BlockerId::new("00000000-0000-4000-8000-000000000454").unwrap(),
            "Pause completion",
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            blocked.clone(),
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptCurrent
    );
    assert!(matches!(
        blocked.complete_active_action_v2(
            blocked.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(9)
        ),
        Err(podway_store::GraphMutationErrorV2::BlockersPresent)
    ));
    let unblocked = blocked
        .unblock_active_attempt_v2(
            blocked.trace().revision(),
            &attempt_id,
            None,
            true,
            UnixMillis::new(9),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            blocked.workspace_revision(),
            blocked.trace().revision(),
            unblocked.clone(),
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptCurrent
    );
    let historic_acceptance = store
        .read_idempotency_lookup(
            &identity(),
            &IdempotencyKeyV1::new("delegation-receipt-key").unwrap(),
        )
        .unwrap()
        .unwrap();
    let optional = unblocked
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("note").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "Cosmetic note".to_owned(),
            },
            UnixMillis::new(10),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            unblocked.workspace_revision(),
            unblocked.trace().revision(),
            optional.clone(),
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ItemsChanged
    );
    let observed = store.read_graph_workspace_view_v2(&identity()).unwrap();
    assert_eq!(
        observed.delegation_receipts().readiness(),
        DelegationReadinessV1::ItemsChanged
    );
    assert_eq!(observed.delegation_receipts().total(), 1);
    assert_eq!(
        store
            .read_idempotency_lookup(
                &identity(),
                &IdempotencyKeyV1::new("delegation-receipt-key").unwrap(),
            )
            .unwrap()
            .unwrap(),
        historic_acceptance
    );
    let replay_request = |request_digest: Sha256Digest, job_number: u64| {
        AdmitRequestV1::new_with_canonical_execution(
            DomainCommand::DelegationAccept,
            IdempotencyKeyV1::new("delegation-receipt-key").unwrap(),
            podway_core::JobId::new(format!("00000000-0000-4000-9000-{job_number:012x}")).unwrap(),
            RevisionAttemptItemPreconditionsV1::new(
                Some(ready.trace().revision()),
                Some(attempt_id.clone()),
                None,
                None,
            )
            .unwrap(),
            request_digest,
            UnixMillis::new(10),
            claimed.execution().canonical_execution().clone(),
        )
        .with_procedure_v2_execution()
        .with_session_identity(AdmissionSessionIdentityV1::Exact(
            ready.trace().session_id().clone(),
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
        )
    };
    assert!(matches!(
        store.admit(&identity(), replay_request(digest('9'), 0x458)),
        Ok(AdmitOutcomeV1::Existing(_))
    ));
    assert!(matches!(
        store.admit(&identity(), replay_request(digest('8'), 0x459)),
        Err(StoreErrorV1::IdempotencyDigestConflictV1 { .. })
    ));
    let cleared = optional
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("note").unwrap(),
            Revision::new(1),
            ActiveItemMutationV2::Clear,
            UnixMillis::new(11),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            optional.workspace_revision(),
            optional.trace().revision(),
            cleared.clone(),
        )
        .unwrap();
    assert_ne!(
        accepted_items_digest_v1(&ready).unwrap(),
        accepted_items_digest_v1(&cleared).unwrap()
    );
    let completed = cleared
        .complete_active_action_v2(
            cleared.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(12),
        )
        .unwrap()
        .into_state();
    assert!(
        store
            .replace_graph_session_v2(
                &identity(),
                cleared.workspace_revision(),
                cleared.trace().revision(),
                completed,
            )
            .is_err()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(cleared.clone())
    );
    let retried = cleared
        .retry_active_attempt_v2(
            cleared.trace().revision(),
            &attempt_id,
            AttemptId::new("00000000-0000-4000-8000-000000000455").unwrap(),
            ReasonV2::new("Revalidate after edits").unwrap(),
            UnixMillis::new(13),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            cleared.workspace_revision(),
            cleared.trace().revision(),
            retried,
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptMissing
    );
}

#[test]
fn v2sdg005_no_op_and_rolled_back_edit_preserve_acceptance() {
    use podway_store::StoreFailpointV1;
    use podway_store::v2_delegation::DelegationReadinessV1;

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let initial = session_state_with_items();
    let attempt_id = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let ready = initial
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
    store
        .create_graph_session_v2(&identity(), ready.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &ready);
    let accepted = ready.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            ready.workspace_revision(),
            ready.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&ready),
            UnixMillis::new(6),
        )
        .unwrap();
    let no_op = accepted
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("result").unwrap(),
            Revision::new(1),
            ActiveItemMutationV2::Set {
                value: "Final output".to_owned(),
            },
            UnixMillis::new(7),
        )
        .unwrap();
    assert!(!no_op.changed());
    assert_eq!(no_op.state(), &accepted);
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptCurrent
    );
    drop(store);

    let interrupted = open(&temporary, Some(StoreFailpointV1::V2GraphStateBeforeCommit));
    let changed = accepted
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("note").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "Correction".to_owned(),
            },
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    assert!(
        interrupted
            .replace_graph_session_v2(
                &identity(),
                accepted.workspace_revision(),
                accepted.trace().revision(),
                changed,
            )
            .is_err()
    );
    assert_eq!(
        interrupted.read_graph_session_v2(&identity()).unwrap(),
        Some(accepted.clone())
    );
    assert_eq!(
        interrupted
            .read_delegation_readiness_v1(&identity())
            .unwrap(),
        DelegationReadinessV1::ReceiptCurrent
    );
    drop(interrupted);

    let reopened = open(&temporary, None);
    let completed = accepted
        .complete_active_action_v2(
            accepted.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(9),
        )
        .unwrap()
        .into_state();
    reopened
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            completed,
        )
        .unwrap();
}

#[test]
fn v2sdg005_manual_rework_keeps_old_receipt_historical() {
    use podway_store::v2_delegation::DelegationReadinessV1;

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let initial = session_state_with_items();
    let attempt_id = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let ready = initial
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("result").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "Reviewed output".to_owned(),
            },
            UnixMillis::new(5),
        )
        .unwrap()
        .into_state();
    store
        .create_graph_session_v2(&identity(), ready.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &ready);
    let accepted = ready.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            ready.workspace_revision(),
            ready.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&ready),
            UnixMillis::new(6),
        )
        .unwrap();
    let reworked = accepted
        .manual_rework_v2(
            accepted.trace().revision(),
            Some(&attempt_id),
            GraphNodeId::new("work").unwrap(),
            AttemptId::new("00000000-0000-4000-8000-000000000456").unwrap(),
            ReasonV2::new("Revalidate the accepted output").unwrap(),
            None,
            UnixMillis::new(7),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            reworked,
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptMissing
    );
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let historical_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(historical_count, 1);
}

#[test]
fn v2sdg005_goal_revision_cannot_relabel_the_old_receipt() {
    use podway_store::v2_delegation::DelegationReadinessV1;

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state_with_goal();
    let attempt_id = state.trace().active_attempt().unwrap().attempt_id().clone();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &state);
    let accepted = state.accept_delegation_successor_v1(&attempt_id).unwrap();
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
    let revised = accepted
        .revise_goal_v2(
            accepted.trace().revision(),
            Some(&attempt_id),
            GoalRevisionNumberV2::FIRST,
            GoalStatementV2::new("Revalidate the delegated result").unwrap(),
            GoalDefinitionV2::new(vec![
                GoalCriterionV2::new(
                    CriterionId::new("reviewed").unwrap(),
                    "Revised result reviewed.",
                )
                .unwrap(),
            ])
            .unwrap(),
            GraphNodeId::new("work").unwrap(),
            AttemptId::new("00000000-0000-4000-8000-000000000461").unwrap(),
            GoalRevisionReasonV2::new("Scope changed").unwrap(),
            None,
            false,
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            revised,
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ReceiptMissing
    );
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let historical_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(historical_count, 1);
}

#[test]
fn v2sdg005_direct_completion_remains_ungated() {
    use podway_store::v2_delegation::DelegationReadinessV1;

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let direct = direct_session_state();
    let attempt_id = direct
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    store
        .create_graph_session_v2(&identity(), direct.clone())
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::Direct
    );
    let completed = direct
        .complete_active_action_v2(
            direct.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(5),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            direct.workspace_revision(),
            direct.trace().revision(),
            completed.clone(),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(completed)
    );
}

#[test]
fn v2sdg005_restoring_required_item_text_keeps_the_receipt_stale() {
    use podway_store::v2_delegation::{DelegationReadinessV1, accepted_items_digest_v1};

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let initial = session_state_with_items();
    let attempt_id = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let ready = initial
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
    store
        .create_graph_session_v2(&identity(), ready.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &ready);
    let accepted = ready.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            claimed.claim().clone(),
            ready.workspace_revision(),
            ready.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&ready),
            UnixMillis::new(6),
        )
        .unwrap();
    let revised = accepted
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("result").unwrap(),
            Revision::new(1),
            ActiveItemMutationV2::Set {
                value: "Revised output".to_owned(),
            },
            UnixMillis::new(7),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            revised.clone(),
        )
        .unwrap();
    let restored = revised
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("result").unwrap(),
            Revision::new(2),
            ActiveItemMutationV2::Set {
                value: "Final output".to_owned(),
            },
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            revised.workspace_revision(),
            revised.trace().revision(),
            restored.clone(),
        )
        .unwrap();
    assert_ne!(
        accepted_items_digest_v1(&accepted).unwrap(),
        accepted_items_digest_v1(&restored).unwrap()
    );
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ItemsChanged
    );
    let completed = restored
        .complete_active_action_v2(
            restored.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(9),
        )
        .unwrap()
        .into_state();
    assert!(
        store
            .replace_graph_session_v2(
                &identity(),
                restored.workspace_revision(),
                restored.trace().revision(),
                completed,
            )
            .is_err()
    );
}

#[test]
fn v2sdg005_package_stale_completion_requires_the_matching_failure() {
    use podway_store::v2_delegation::DelegationReadinessV1;

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    let attempt_id = state.trace().active_attempt().unwrap().attempt_id().clone();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let accepted_job = admit_and_claim(&store, &state);
    let accepted = state.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            accepted_job.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&state),
            UnixMillis::new(6),
        )
        .unwrap();

    let mut mismatched = receipt(&state).value().clone();
    mismatched["delegation"]["max_rounds"] = json!(2);
    let mismatched = DelegationReceiptV1::new(mismatched).unwrap();
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    connection
        .execute_batch("DROP TRIGGER v2_delegation_receipts_immutable;")
        .unwrap();
    connection
        .execute(
            "UPDATE v2_delegation_receipts SET receipt_json = ?1 WHERE attempt_id = ?2",
            (
                mismatched.canonical_json(),
                accepted
                    .trace()
                    .active_attempt()
                    .unwrap()
                    .attempt_id()
                    .as_str(),
            ),
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::PackageChanged
    );

    let completed = accepted
        .complete_active_action_v2(
            accepted.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(7),
        )
        .unwrap()
        .into_state();
    assert!(
        store
            .replace_graph_session_v2(
                &identity(),
                accepted.workspace_revision(),
                accepted.trace().revision(),
                completed.clone(),
            )
            .is_err()
    );
    let claimed = admit_complete_and_claim(&store, &accepted, "complete-package-stale", 0x467);
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                accepted.workspace_revision(),
                accepted.trace().revision(),
                Some(completed),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: accepted.trace().session_id().clone(),
                    revision_before: accepted.trace().revision(),
                    revision_after: accepted.trace().revision().checked_next().unwrap(),
                    changed: true,
                }),
                PersistedGraphTerminalOperationV2::complete(
                    accepted
                        .trace()
                        .active_attempt()
                        .unwrap()
                        .graph_node_id()
                        .clone(),
                    attempt_id,
                    None,
                    None,
                )
                .unwrap(),
                UnixMillis::new(22),
            )
            .is_err()
    );
    assert!(
        store
            .read_job(&identity(), claimed.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    let wrong_failure = store.commit_graph_mutation_terminal_v2(
        claimed.claim().clone(),
        accepted.workspace_revision(),
        accepted.trace().revision(),
        None,
        TerminalResultV1::Failure(DomainError::InvalidState {
            reason: "Procedure v2 graph mutation failed",
        }),
        PersistedGraphTerminalOperationV2::failure(
            PersistedGraphMutationFailureV2::DelegationItemsChanged,
        )
        .unwrap(),
        UnixMillis::new(22),
    );
    assert!(wrong_failure.is_err());
    assert!(
        store
            .read_job(&identity(), claimed.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            PersistedGraphTerminalOperationV2::failure(
                PersistedGraphMutationFailureV2::DelegationPackageChanged,
            )
            .unwrap(),
            UnixMillis::new(22),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(accepted)
    );
}

#[test]
fn v2sdg005_malformed_receipt_fails_closed_on_live_completion() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    let attempt_id = state.trace().active_attempt().unwrap().attempt_id().clone();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let accepted_job = admit_and_claim(&store, &state);
    let accepted = state.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            accepted_job.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&state),
            UnixMillis::new(6),
        )
        .unwrap();

    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    connection
        .execute_batch("DROP TRIGGER v2_delegation_receipts_immutable;")
        .unwrap();
    connection
        .execute(
            "UPDATE v2_delegation_receipts SET receipt_json = ?1 WHERE attempt_id = ?2",
            (
                format!("{} ", receipt(&state).canonical_json()),
                attempt_id.as_str(),
            ),
        )
        .unwrap();
    drop(connection);
    assert!(store.read_delegation_readiness_v1(&identity()).is_err());

    let completed = accepted
        .complete_active_action_v2(
            accepted.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(7),
        )
        .unwrap()
        .into_state();
    assert!(
        store
            .replace_graph_session_v2(
                &identity(),
                accepted.workspace_revision(),
                accepted.trace().revision(),
                completed.clone(),
            )
            .is_err()
    );
    let claimed = admit_complete_and_claim(&store, &accepted, "complete-malformed-receipt", 0x468);
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                claimed.claim().clone(),
                accepted.workspace_revision(),
                accepted.trace().revision(),
                Some(completed),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: accepted.trace().session_id().clone(),
                    revision_before: accepted.trace().revision(),
                    revision_after: accepted.trace().revision().checked_next().unwrap(),
                    changed: true,
                }),
                PersistedGraphTerminalOperationV2::complete(
                    accepted
                        .trace()
                        .active_attempt()
                        .unwrap()
                        .graph_node_id()
                        .clone(),
                    attempt_id,
                    None,
                    None,
                )
                .unwrap(),
                UnixMillis::new(22),
            )
            .is_err()
    );
    assert!(
        store
            .read_job(&identity(), claimed.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
}

#[test]
fn v2sdg005_stale_completion_commits_typed_failure_after_current_fences() {
    use podway_store::v2_delegation::DelegationReadinessV1;

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let initial = session_state_with_items();
    let attempt_id = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let ready = initial
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
    store
        .create_graph_session_v2(&identity(), ready.clone())
        .unwrap();
    let accepted_job = admit_and_claim(&store, &ready);
    let accepted = ready.accept_delegation_successor_v1(&attempt_id).unwrap();
    store
        .commit_graph_delegation_accept_terminal_v1(
            accepted_job.claim().clone(),
            ready.workspace_revision(),
            ready.trace().revision(),
            accepted.clone(),
            digest('d'),
            receipt(&ready),
            UnixMillis::new(6),
        )
        .unwrap();
    let stale_fence = admit_complete_and_claim(
        &store,
        &accepted,
        "complete-with-stale-session-fence",
        0x465,
    );
    let changed = accepted
        .mutate_active_item_v2(
            &attempt_id,
            &ItemId::new("note").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "Correction after acceptance".to_owned(),
            },
            UnixMillis::new(7),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            changed.clone(),
        )
        .unwrap();
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ItemsChanged
    );
    let completed = changed
        .complete_active_action_v2(
            changed.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    store
        .commit_graph_mutation_terminal_v2(
            stale_fence.claim().clone(),
            changed.workspace_revision(),
            changed.trace().revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            PersistedGraphTerminalOperationV2::failure(
                PersistedGraphMutationFailureV2::SessionRevisionConflict {
                    expected: accepted.trace().revision(),
                    actual: changed.trace().revision(),
                },
            )
            .unwrap(),
            UnixMillis::new(22),
        )
        .unwrap();
    let stale_complete =
        admit_complete_and_claim(&store, &changed, "complete-after-item-change", 0x464);
    assert_eq!(
        stale_complete.claim().job_id().as_str(),
        "00000000-0000-4000-9000-000000000464"
    );
    assert_eq!(
        stale_complete.execution().command(),
        &DomainCommand::SessionComplete
    );
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                stale_complete.claim().clone(),
                changed.workspace_revision(),
                changed.trace().revision(),
                Some(completed),
                TerminalResultV1::Success(DomainResult::SessionChanged {
                    session_id: changed.trace().session_id().clone(),
                    revision_before: changed.trace().revision(),
                    revision_after: changed.trace().revision().checked_next().unwrap(),
                    changed: true,
                }),
                PersistedGraphTerminalOperationV2::complete(
                    changed
                        .trace()
                        .active_attempt()
                        .unwrap()
                        .graph_node_id()
                        .clone(),
                    attempt_id,
                    None,
                    None,
                )
                .unwrap(),
                UnixMillis::new(23),
            )
            .is_err()
    );
    assert!(
        store
            .read_job(&identity(), stale_complete.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    assert!(
        store
            .commit_graph_mutation_terminal_v2(
                stale_complete.claim().clone(),
                changed.workspace_revision(),
                changed.trace().revision(),
                None,
                TerminalResultV1::Failure(DomainError::InvalidState {
                    reason: "Procedure v2 graph mutation failed",
                }),
                PersistedGraphTerminalOperationV2::failure(
                    PersistedGraphMutationFailureV2::DelegationReceiptRequired,
                )
                .unwrap(),
                UnixMillis::new(23),
            )
            .is_err()
    );
    assert!(
        store
            .read_job(&identity(), stale_complete.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_none()
    );
    store
        .commit_graph_mutation_terminal_v2(
            stale_complete.claim().clone(),
            changed.workspace_revision(),
            changed.trace().revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            PersistedGraphTerminalOperationV2::failure(
                PersistedGraphMutationFailureV2::DelegationItemsChanged,
            )
            .unwrap(),
            UnixMillis::new(23),
        )
        .unwrap();
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(changed.clone())
    );

    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::ItemsChanged
    );
}

#[test]
fn v2sdg005_store_rechecks_acceptance_items_and_blockers() {
    fn assert_rejected(state: GraphSessionStateV2) {
        let temporary = TempDir::new().unwrap();
        let store = open(&temporary, None);
        store
            .create_graph_session_v2(&identity(), state.clone())
            .unwrap();
        let claimed = admit_and_claim(&store, &state);
        assert!(matches!(
            store.commit_graph_delegation_accept_terminal_v1(
                claimed.claim().clone(),
                state.workspace_revision(),
                state.trace().revision(),
                state.clone(),
                digest('d'),
                receipt(&state),
                UnixMillis::new(9),
            ),
            Err(StoreErrorV1::InvalidStateV1(_))
        ));
        assert_eq!(
            store.read_graph_session_v2(&identity()).unwrap(),
            Some(state)
        );
        assert!(
            store
                .read_job(&identity(), claimed.claim().job_id())
                .unwrap()
                .unwrap()
                .terminal_receipt()
                .is_none()
        );
        let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
        let receipt_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(receipt_count, 0);
    }

    let missing_item = session_state_with_items();
    let attempt_id = missing_item
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    assert_rejected(missing_item.clone());

    let ready = missing_item
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
    let blocked = ready
        .block_active_attempt_v2(
            ready.trace().revision(),
            &attempt_id,
            BlockerId::new("00000000-0000-4000-8000-000000000462").unwrap(),
            "Supervisor review pending",
            UnixMillis::new(6),
        )
        .unwrap()
        .into_state();
    assert_rejected(blocked);
}

#[test]
fn v2sdg005_required_items_precede_the_delegation_completion_gate() {
    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state_with_items();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_complete_and_claim(&store, &state, "complete-before-required-item", 0x463);
    store
        .commit_graph_mutation_terminal_v2(
            claimed.claim().clone(),
            state.workspace_revision(),
            state.trace().revision(),
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "Procedure v2 graph mutation failed",
            }),
            PersistedGraphTerminalOperationV2::failure(
                PersistedGraphMutationFailureV2::RequiredItemsMissing {
                    item_ids: vec![ItemId::new("result").unwrap()],
                },
            )
            .unwrap(),
            UnixMillis::new(22),
        )
        .unwrap();
    assert!(
        store
            .read_job(&identity(), claimed.claim().job_id())
            .unwrap()
            .unwrap()
            .terminal_receipt()
            .is_some()
    );
    assert_eq!(
        store.read_graph_session_v2(&identity()).unwrap(),
        Some(state)
    );
}

#[test]
fn v2sdg005_cancellation_keeps_the_receipt_historical() {
    use podway_store::v2_delegation::DelegationReadinessV1;

    let temporary = TempDir::new().unwrap();
    let store = open(&temporary, None);
    let state = session_state();
    let attempt_id = state.trace().active_attempt().unwrap().attempt_id().clone();
    store
        .create_graph_session_v2(&identity(), state.clone())
        .unwrap();
    let claimed = admit_and_claim(&store, &state);
    let accepted = state.accept_delegation_successor_v1(&attempt_id).unwrap();
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
    let cancelled = accepted
        .cancel_active_session_v2(
            accepted.trace().revision(),
            &attempt_id,
            ReasonV2::new("Delegated work is no longer needed").unwrap(),
            UnixMillis::new(8),
        )
        .unwrap()
        .into_state();
    store
        .replace_graph_session_v2(
            &identity(),
            accepted.workspace_revision(),
            accepted.trace().revision(),
            cancelled.clone(),
        )
        .unwrap();
    assert!(cancelled.trace().active_attempt().is_none());
    assert_eq!(
        store.read_delegation_readiness_v1(&identity()).unwrap(),
        DelegationReadinessV1::Direct
    );
    let connection = Connection::open(temporary.path().join("state.sqlite3")).unwrap();
    let historical_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(historical_count, 1);
}
