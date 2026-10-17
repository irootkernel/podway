//! Behavior of a preloaded attempt-content rejection.
//!
//! An earlier sorted item that crosses the scalar ceiling is rejected even when
//! a later item would reduce the final total. The rejected request leaves no
//! revision change and no success outcome.

use std::time::{Duration, Instant};

use podway_core::{
    AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, CanonicalProcedureJsonV1,
    GraphNodeId, ItemId, ItemTypeV1, MAX_ATTEMPT_CONTENT_SCALARS_V2, ProcedureSnapshotId,
    ProcedureSourceLabelV1, Revision, SessionAttemptV2, SessionId, SessionLifecycle,
    SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, AttemptMetadataV2, AttemptWorkflowMemoryV2,
    CanonicalExecutionJsonV1, GraphMutationErrorV2, GraphNodeCounterV2, GraphSessionStateV2,
    IdempotencyKeyV1, ItemSlotStateV2, ProcedureSnapshotV2, ValidatedWorkspaceRootV1,
    WorkflowMemoryStateV2,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1, DirectStoreV1,
        DirectWorkspaceLayoutV1, ExclusiveMaintenanceLeaseV1,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const INPUT_CAP_BYTES: usize = 1_048_576;
const NOW: u64 = 1_700_000_000_000;

fn snapshot_from(document: Value, snapshot_id: &str) -> ProcedureSnapshotV2 {
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new(snapshot_id).unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("optim-batch.json").unwrap(),
        UnixMillis::new(NOW),
    )
    .unwrap()
}

fn list_snapshot() -> ProcedureSnapshotV2 {
    let mut items = vec![json!({
        "id": "a-early",
        "type": "list",
        "prompt": "Early list.",
        "required": false,
        "max_items": 1000,
        "max_item_length": 1000,
        "max_total_length": 1_000_000,
        "unique": false
    })];
    for index in 0..15 {
        items.push(json!({
            "id": format!("b{index:02}"),
            "type": "list",
            "prompt": "Filled list.",
            "required": false,
            "max_items": 1000,
            "max_item_length": 1000,
            "max_total_length": 1_000_000,
            "unique": false
        }));
    }
    items.push(json!({
        "id": "z-later",
        "type": "list",
        "prompt": "Later list.",
        "required": false,
        "max_items": 1000,
        "max_item_length": 1000,
        "max_total_length": 1_000_000,
        "unique": false
    }));
    snapshot_from(
        json!({
            "schema": "podway.procedure/v2",
            "id": "optim-overflow",
            "version": "1",
            "name": "Optim overflow",
            "purpose": "Reject an earlier sorted item that crosses the attempt scalar ceiling.",
            "node_definitions": {
                "work": {
                    "type": "action",
                    "title": "Work",
                    "intent": "Record lists.",
                    "items": items
                }
            },
            "graph": {
                "entry": "work",
                "nodes": [{"id": "work", "use": "work", "terminal": true}]
            }
        }),
        "00000000-0000-4000-8000-000000002202",
    )
}

fn attempt(number: u64, node_name: &str, lifecycle: AttemptLifecycle) -> SessionAttemptV2 {
    SessionAttemptV2::new(
        AttemptId::new(format!("00000000-0000-4000-8000-{number:012}")).unwrap(),
        GraphNodeId::new(node_name).unwrap(),
        AttemptNumberV2::new(number),
        TraceSequenceV2::new(number),
        lifecycle,
        AttemptValidityV2::Valid,
        None,
    )
    .unwrap()
}

fn empty_slots(
    attempt_id: &AttemptId,
    ids: &[ItemId],
    item_type: ItemTypeV1,
) -> Vec<ItemSlotStateV2> {
    ids.iter()
        .map(|id| {
            ItemSlotStateV2::new(
                attempt_id.clone(),
                id.clone(),
                item_type,
                Revision::ZERO,
                None,
                UnixMillis::new(NOW),
                UnixMillis::new(NOW),
            )
            .unwrap()
        })
        .collect()
}

fn list_values(entries: usize, width: usize) -> Vec<String> {
    let values = vec!["a".repeat(width); entries];
    let encoded = serde_json::to_vec(&values).unwrap();
    assert!(encoded.len() <= INPUT_CAP_BYTES);
    values
}

fn empty_overflow_state() -> GraphSessionStateV2 {
    let snapshot = list_snapshot();
    let ids = snapshot_item_ids();
    let attempt = attempt(1, "work", AttemptLifecycle::Active);
    let slots = empty_slots(attempt.attempt_id(), &ids, ItemTypeV1::List);
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        "Optim overflow",
        snapshot,
        SessionTraceV2::from_parts(
            SessionId::new("00000000-0000-4000-8000-000000002211").unwrap(),
            SessionLifecycle::Running,
            Revision::new(1),
            vec![attempt.clone()],
        )
        .unwrap(),
        vec![GraphNodeCounterV2::new(
            GraphNodeId::new("work").unwrap(),
            1,
            0,
        )],
        vec![
            AttemptMetadataV2::new(
                attempt.attempt_id().clone(),
                UnixMillis::new(NOW),
                None,
                None,
            )
            .unwrap(),
        ],
        WorkflowMemoryStateV2::new(
            vec![
                AttemptWorkflowMemoryV2::new(
                    attempt.attempt_id().clone(),
                    slots,
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap(),
            ],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        UnixMillis::new(NOW),
        None,
        None,
        None,
    )
    .unwrap()
}

fn overflow_state() -> GraphSessionStateV2 {
    let ids = snapshot_item_ids();
    let mut seeded = empty_overflow_state();
    let full = list_values(1000, 1000);
    for (offset, id) in ids.iter().filter(|id| id.as_str() != "a-early").enumerate() {
        let values = full.clone();
        let attempt_id = seeded
            .trace()
            .active_attempt()
            .unwrap()
            .attempt_id()
            .clone();
        seeded = seeded
            .mutate_active_items_v2(
                &attempt_id,
                &[ActiveItemMutationRequestV2::new(
                    id.clone(),
                    Revision::ZERO,
                    ActiveItemMutationV2::ReplaceList { values },
                )],
                UnixMillis::new(NOW + 2 + offset as u64),
            )
            .unwrap()
            .into_state();
    }
    seeded
}

fn snapshot_item_ids() -> Vec<ItemId> {
    let mut ids = vec![ItemId::new("a-early").unwrap()];
    ids.extend((0..15).map(|index| ItemId::new(format!("b{index:02}")).unwrap()));
    ids.push(ItemId::new("z-later").unwrap());
    ids
}

#[test]
fn optim001_preloaded_attempt_rejects_earlier_overflow_before_later_reduction() {
    let seeded = overflow_state();
    let preimage = seeded.clone();
    let mut exact = list_values(777, 1000);
    exact.push("a".repeat(216));
    assert_eq!(
        exact
            .iter()
            .map(|value| value.chars().count())
            .sum::<usize>(),
        777_216
    );
    let legal = seeded
        .clone()
        .mutate_active_items_v2(
            seeded.trace().active_attempt().unwrap().attempt_id(),
            &[ActiveItemMutationRequestV2::new(
                ItemId::new("a-early").unwrap(),
                Revision::ZERO,
                ActiveItemMutationV2::ReplaceList { values: exact },
            )],
            UnixMillis::new(NOW + 30),
        )
        .unwrap()
        .into_state();
    assert_eq!(
        legal
            .workflow_memory()
            .attempts()
            .iter()
            .map(AttemptWorkflowMemoryV2::content_scalars)
            .sum::<u64>(),
        MAX_ATTEMPT_CONTENT_SCALARS_V2
    );
    let mut over = list_values(777, 1000);
    over.push("a".repeat(217));
    let early = ActiveItemMutationRequestV2::new(
        ItemId::new("a-early").unwrap(),
        Revision::ZERO,
        ActiveItemMutationV2::ReplaceList { values: over },
    );
    let later = ActiveItemMutationRequestV2::new(
        ItemId::new("z-later").unwrap(),
        Revision::new(1),
        ActiveItemMutationV2::Clear,
    );
    let attempt_id = seeded
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    for requests in [vec![later.clone(), early.clone()], vec![early, later]] {
        let error = seeded
            .mutate_active_items_v2(&attempt_id, &requests, UnixMillis::new(NOW + 40))
            .unwrap_err();
        let GraphMutationErrorV2::Domain(podway_core::DomainError::BoundExceeded {
            field,
            actual,
            maximum,
            ..
        }) = error
        else {
            panic!("overflow returned {error:?}");
        };
        assert_eq!(field, "attempt recorded content");
        assert_eq!(maximum, MAX_ATTEMPT_CONTENT_SCALARS_V2);
        assert_eq!(actual, 16_777_217);
        assert_ne!(actual, 16_777_232);
        assert_eq!(seeded, preimage);
        assert_eq!(seeded.workspace_revision(), preimage.workspace_revision());
        assert_eq!(seeded.trace().revision(), preimage.trace().revision());
    }
}

fn digest_nibble(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn store_deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn mutation_request(key: &str) -> DirectMutationRequestV1 {
    DirectMutationRequestV1::new(
        IdempotencyKeyV1::new(key).unwrap(),
        digest_nibble('d'),
        "workspace.repair",
        "00000000-0000-4000-8000-000000000221",
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap()
}

fn recorded_success(request: &DirectMutationRequestV1) -> CanonicalExecutionJsonV1 {
    CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "schema": "podway.output/v4",
            "request_id": request.response_id(),
            "command": request.command(),
            "generated_at": "2023-11-14T22:13:20.000Z",
            "result": {"recorded": true},
            "warnings": []
        }))
        .unwrap(),
    )
    .unwrap()
}

fn owned_store() -> (TempDir, DirectStoreV1) {
    let root = TempDir::new().unwrap();
    let canonical = std::fs::canonicalize(root.path()).unwrap();
    let store = DirectStoreV1::new(
        DirectWorkspaceLayoutV1::new(ValidatedWorkspaceRootV1::from_path(&canonical).unwrap())
            .unwrap(),
        digest_nibble('c'),
        digest_nibble('a'),
        digest_nibble('b'),
    );
    let lease =
        ExclusiveMaintenanceLeaseV1::acquire(store.layout(), store_deadline(), true).unwrap();
    let request = mutation_request("init");
    let success = recorded_success(&request);
    store
        .bootstrap(
            &lease,
            WorkspaceId::new("00000000-0000-4000-8000-000000000221").unwrap(),
            "generation-one",
            &request,
            store_deadline(),
            || UnixMillis::new(NOW + 80),
            move |transaction| {
                let _ = transaction.now();
                Ok(success)
            },
        )
        .unwrap();
    drop(lease);
    (root, store)
}

fn content_scalars(state: &GraphSessionStateV2) -> u64 {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .map(AttemptWorkflowMemoryV2::content_scalars)
        .sum()
}

fn slot_revision_pairs(state: &GraphSessionStateV2) -> Vec<(String, u64)> {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .flat_map(|attempt| {
            attempt.item_slots().iter().map(|slot| {
                (
                    format!("{}:{}", slot.attempt_id(), slot.item_id()),
                    slot.revision().get(),
                )
            })
        })
        .collect()
}

fn slot_revision(state: &GraphSessionStateV2, item: &str) -> Revision {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .flat_map(|attempt| attempt.item_slots().iter())
        .find(|slot| slot.item_id().as_str() == item)
        .unwrap()
        .revision()
}

fn load_graph(store: &DirectStoreV1) -> GraphSessionStateV2 {
    store
        .read(store_deadline(), |snapshot| {
            snapshot
                .current_graph()?
                .ok_or(DirectStoreErrorV1::InvalidInput {
                    reason: "missing graph",
                })
        })
        .unwrap()
}

fn outcome_recorded(store: &DirectStoreV1, key: &str) -> bool {
    let key = IdempotencyKeyV1::new(key).unwrap();
    store
        .read(store_deadline(), |snapshot| snapshot.outcome(&key))
        .unwrap()
        .is_some()
}

fn commit_seed(store: &DirectStoreV1, key: &str, values: Vec<String>, item: ItemId) {
    let request = mutation_request(key);
    let success = recorded_success(&request);
    let mut failure = None;
    let committed = store.mutate(
        &request,
        store_deadline(),
        || UnixMillis::new(NOW + 100),
        |transaction| {
            let _ = transaction.now();
            let prior = transaction
                .current_graph()?
                .ok_or(DirectStoreErrorV1::InvalidInput {
                    reason: "missing graph",
                })?
                .clone();
            let attempt_id = prior.trace().active_attempt().unwrap().attempt_id().clone();
            let revision = slot_revision(&prior, item.as_str());
            let next = match prior.mutate_active_items_v2(
                &attempt_id,
                &[ActiveItemMutationRequestV2::new(
                    item,
                    revision,
                    ActiveItemMutationV2::ReplaceList { values },
                )],
                UnixMillis::new(NOW + 2),
            ) {
                Ok(outcome) => outcome.into_state(),
                Err(error) => {
                    failure = Some(format!("{error:?}"));
                    return Err(DirectStoreErrorV1::InvalidInput {
                        reason: "seed rejected",
                    });
                }
            };
            transaction.replace_graph(
                prior.workspace_revision(),
                prior.trace().revision(),
                &next,
            )?;
            Ok(success)
        },
    );
    assert!(failure.is_none(), "seed {key} rejected: {failure:?}");
    committed.expect("seed commit");
}

fn reject_overflow(
    store: &DirectStoreV1,
    key: &str,
    requests: Vec<ActiveItemMutationRequestV2>,
) -> GraphMutationErrorV2 {
    let request = mutation_request(key);
    let mut captured = None;
    let rejected = store.mutate(
        &request,
        store_deadline(),
        || UnixMillis::new(NOW + 120),
        |transaction| {
            let _ = transaction.now();
            let prior = transaction
                .current_graph()?
                .ok_or(DirectStoreErrorV1::InvalidInput {
                    reason: "missing graph",
                })?
                .clone();
            let attempt_id = prior.trace().active_attempt().unwrap().attempt_id().clone();
            match prior.mutate_active_items_v2(&attempt_id, &requests, UnixMillis::new(NOW + 40)) {
                Err(error) => {
                    captured = Some(error);
                    Err(DirectStoreErrorV1::InvalidInput {
                        reason: "attempt recorded content rejected",
                    })
                }
                Ok(_) => Err(DirectStoreErrorV1::InvalidInput {
                    reason: "overflow must reject",
                }),
            }
        },
    );
    assert!(rejected.is_err(), "rejected key {key} committed an outcome");
    captured.expect("domain error")
}

#[test]
fn optim001_persisted_store_rejects_attempt_content_without_outcome() {
    let (_root, store) = owned_store();
    let empty = empty_overflow_state();
    let create = mutation_request("create-empty");
    let success = recorded_success(&create);
    store
        .mutate(
            &create,
            store_deadline(),
            || UnixMillis::new(NOW + 90),
            |transaction| {
                let _ = transaction.now();
                transaction.create_graph(&empty)?;
                Ok(success)
            },
        )
        .unwrap();
    let full = list_values(1000, 1000);
    for id in snapshot_item_ids()
        .into_iter()
        .filter(|id| id.as_str() != "a-early")
    {
        let values = full.clone();
        assert!(serde_json::to_vec(&values).unwrap().len() <= INPUT_CAP_BYTES);
        commit_seed(&store, &format!("seed-{}", id.as_str()), values, id);
    }
    assert!(outcome_recorded(&store, "seed-b00"));
    let preimage = load_graph(&store);
    let seeded_scalars = content_scalars(&preimage);
    assert_eq!(seeded_scalars, 16_000_000);
    assert!(MAX_ATTEMPT_CONTENT_SCALARS_V2 - seeded_scalars <= 1_000_000);
    let early_revision = slot_revision(&preimage, "a-early");
    let later_revision = slot_revision(&preimage, "z-later");
    assert_eq!(early_revision, Revision::ZERO);
    let mut over = list_values(777, 1000);
    over.push("a".repeat(217));
    assert!(serde_json::to_vec(&over).unwrap().len() <= INPUT_CAP_BYTES);
    for (key, later_first) in [("reject-later-early", true), ("reject-early-later", false)] {
        let early = ActiveItemMutationRequestV2::new(
            ItemId::new("a-early").unwrap(),
            early_revision,
            ActiveItemMutationV2::ReplaceList {
                values: over.clone(),
            },
        );
        let later = ActiveItemMutationRequestV2::new(
            ItemId::new("z-later").unwrap(),
            later_revision,
            ActiveItemMutationV2::Clear,
        );
        let requests = if later_first {
            vec![later, early]
        } else {
            vec![early, later]
        };
        let error = reject_overflow(&store, key, requests);
        let GraphMutationErrorV2::Domain(podway_core::DomainError::BoundExceeded {
            field,
            actual,
            maximum,
            ..
        }) = error
        else {
            panic!("persisted overflow returned {error:?}");
        };
        assert_eq!(field, "attempt recorded content");
        assert_eq!(maximum, MAX_ATTEMPT_CONTENT_SCALARS_V2);
        assert_eq!(actual, 16_777_217);
        let after = load_graph(&store);
        assert_eq!(slot_revision_pairs(&after), slot_revision_pairs(&preimage));
        assert_eq!(after.workspace_revision(), preimage.workspace_revision());
        assert_eq!(after.trace().revision(), preimage.trace().revision());
        assert_eq!(after, preimage);
        assert!(!outcome_recorded(&store, key));
    }
}
