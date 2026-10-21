//! Sorted batches share one unpublished attempt draft.
//!
//! Legal 1/16/128 shapes, mixed no-ops, input-order permutations, and
//! first/middle/last failures must publish one revision or none.

use podway_core::{
    ArtifactValueV1, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, BlockerId,
    BlockerState, CanonicalProcedureJsonV1, CheckResultExecutorV2, CheckResultInputBasisV2,
    CheckResultOutcomeV2, CheckResultValueV2, GraphNodeId, ItemId, ItemTypeV1, OperationId,
    ProcedureSnapshotId, ProcedureSourceLabelV1, RecordedItemValueV2, Revision, SessionAttemptV2,
    SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis,
    canonicalize_json_v1,
};
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, AttemptMetadataV2, AttemptWorkflowMemoryV2,
    BlockerStateV2, GraphItemsMutationOutcomeV2, GraphMutationErrorV2, GraphNodeCounterV2,
    GraphSessionStateV2, ItemSlotStateV2, ProcedureSnapshotV2, WorkflowMemoryStateV2,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const NOW: u64 = 1_700_000_000_000;

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

fn attempt_id() -> AttemptId {
    AttemptId::new("00000000-0000-4000-8000-000000000001").unwrap()
}

fn item_id(index: usize) -> ItemId {
    ItemId::new(format!("n{index:03}")).unwrap()
}

fn snapshot_from(document: Value, snapshot_id: &str) -> ProcedureSnapshotV2 {
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new(snapshot_id).unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("optim-draft.json").unwrap(),
        UnixMillis::new(NOW),
    )
    .unwrap()
}

fn running_graph(
    snapshot: ProcedureSnapshotV2,
    slots: Vec<ItemSlotStateV2>,
    blockers: Vec<BlockerStateV2>,
) -> GraphSessionStateV2 {
    let attempt = SessionAttemptV2::new(
        attempt_id(),
        GraphNodeId::new("work").unwrap(),
        AttemptNumberV2::new(1),
        TraceSequenceV2::new(1),
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        None,
    )
    .unwrap();
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        "Optim draft",
        snapshot,
        SessionTraceV2::from_parts(
            SessionId::new("00000000-0000-4000-8000-000000002311").unwrap(),
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
                    blockers,
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

fn text_slots(count: usize) -> Vec<ItemSlotStateV2> {
    (0..count)
        .map(|index| {
            ItemSlotStateV2::new(
                attempt_id(),
                item_id(index),
                ItemTypeV1::Text,
                Revision::ZERO,
                None,
                UnixMillis::new(NOW),
                UnixMillis::new(NOW),
            )
            .unwrap()
        })
        .collect()
}

fn shape_state(count: usize) -> GraphSessionStateV2 {
    let items = (0..count)
        .map(|index| {
            json!({
                "id": format!("n{index:03}"),
                "type": "text",
                "prompt": "Value",
                "required": false
            })
        })
        .collect::<Vec<_>>();
    let snapshot = snapshot_from(
        json!({
            "schema": "podway.procedure/v2",
            "id": "optim-draft",
            "version": "1",
            "name": "Optim draft",
            "purpose": "Apply one unpublished draft to a sorted item batch.",
            "node_definitions": {
                "work": {
                    "type": "action",
                    "title": "Work",
                    "intent": "Record text.",
                    "items": items
                }
            },
            "graph": {
                "entry": "work",
                "nodes": [{"id": "work", "use": "work", "terminal": true}]
            }
        }),
        "00000000-0000-4000-8000-000000002303",
    );
    let blocker = BlockerStateV2::new(
        BlockerId::new("00000000-0000-4000-8000-000000000099").unwrap(),
        attempt_id(),
        "held",
        BlockerState::Open,
        UnixMillis::new(NOW),
        None,
    )
    .unwrap();
    running_graph(snapshot, text_slots(count), vec![blocker])
}

fn apply(
    state: &GraphSessionStateV2,
    requests: &[ActiveItemMutationRequestV2],
) -> Result<GraphItemsMutationOutcomeV2, GraphMutationErrorV2> {
    state.mutate_active_items_v2(&attempt_id(), requests, UnixMillis::new(NOW + 40))
}

fn mixed_requests(count: usize) -> Vec<ActiveItemMutationRequestV2> {
    (0..count)
        .map(|index| {
            let mutation = if index % 2 == 0 {
                ActiveItemMutationV2::Set {
                    value: "v".to_owned(),
                }
            } else {
                ActiveItemMutationV2::Clear
            };
            ActiveItemMutationRequestV2::new(item_id(index), Revision::ZERO, mutation)
        })
        .collect()
}

fn assert_mixed(outcome: &GraphItemsMutationOutcomeV2, count: usize) {
    assert!(outcome.changed());
    assert_eq!(outcome.state().workspace_revision(), Revision::new(2));
    assert_eq!(outcome.state().trace().revision(), Revision::new(2));
    assert_eq!(outcome.items().len(), count);
    let text = RecordedItemValueV2::text("v").unwrap();
    for (index, entry) in outcome.items().iter().enumerate() {
        let changed = index % 2 == 0;
        assert_eq!(entry.item_id(), &item_id(index));
        assert_eq!(entry.changed(), changed);
        assert_eq!(
            entry.item_revision(),
            if changed {
                Revision::new(1)
            } else {
                Revision::ZERO
            }
        );
        let slot = &outcome.state().workflow_memory().attempts()[0].item_slots()[index];
        if changed {
            assert_eq!(slot.value(), Some(&text));
        } else {
            assert!(slot.value().is_none());
        }
    }
    assert_eq!(
        outcome.state().workflow_memory().attempts()[0].blockers()[0]
            .blocker_id()
            .as_str(),
        "00000000-0000-4000-8000-000000000099"
    );
}

fn failure_positions(count: usize) -> Vec<usize> {
    let mut positions = vec![0];
    if count > 1 {
        positions.push(count / 2);
        positions.push(count - 1);
    }
    positions.sort_unstable();
    positions.dedup();
    positions
}

#[test]
fn optim003_batch_shapes_publish_one_revision_for_mixed_noops() {
    for count in [1_usize, 16, 128] {
        let state = shape_state(count);
        let requests = mixed_requests(count);
        let forward = apply(&state, &requests).unwrap();
        assert_mixed(&forward, count);

        let mut reversed = requests.clone();
        reversed.reverse();
        let backward = apply(&state, &reversed).unwrap();
        assert_eq!(backward.state(), forward.state());
        assert_eq!(backward.items(), forward.items());

        let no_ops = (0..count)
            .map(|index| {
                ActiveItemMutationRequestV2::new(
                    item_id(index),
                    Revision::ZERO,
                    ActiveItemMutationV2::Clear,
                )
            })
            .collect::<Vec<_>>();
        let unchanged = apply(&state, &no_ops).unwrap();
        assert!(!unchanged.changed());
        assert_eq!(unchanged.state(), &state);
        assert!(unchanged.items().iter().all(|entry| !entry.changed()));
    }

    let state = shape_state(1);
    let duplicate =
        ActiveItemMutationRequestV2::new(item_id(0), Revision::ZERO, ActiveItemMutationV2::Clear);
    assert!(matches!(
        apply(&state, &[duplicate.clone(), duplicate]),
        Err(GraphMutationErrorV2::DuplicateItemMutation { .. })
    ));
    let excessive = (0..=128)
        .map(|index| {
            ActiveItemMutationRequestV2::new(
                ItemId::new(format!("n{index:03}")).unwrap(),
                Revision::ZERO,
                ActiveItemMutationV2::Clear,
            )
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        apply(&state, &excessive),
        Err(GraphMutationErrorV2::TooManyItemMutations { maximum: 128 })
    ));
    assert_eq!(state.workspace_revision(), Revision::new(1));
}

#[test]
fn optim003_sorted_position_failures_publish_nothing() {
    for count in [1_usize, 16, 128] {
        let state = shape_state(count);
        for position in failure_positions(count) {
            for stale in [true, false] {
                let mut requests = (0..count)
                    .map(|index| {
                        if index == position && stale {
                            ActiveItemMutationRequestV2::new(
                                item_id(index),
                                Revision::new(1_000 + index as u64),
                                ActiveItemMutationV2::Set {
                                    value: "v".to_owned(),
                                },
                            )
                        } else if index == position {
                            ActiveItemMutationRequestV2::new(
                                item_id(index),
                                Revision::ZERO,
                                ActiveItemMutationV2::Check,
                            )
                        } else {
                            ActiveItemMutationRequestV2::new(
                                item_id(index),
                                Revision::ZERO,
                                ActiveItemMutationV2::Set {
                                    value: "v".to_owned(),
                                },
                            )
                        }
                    })
                    .collect::<Vec<_>>();
                requests.reverse();
                let error = apply(&state, &requests).unwrap_err();
                if stale {
                    assert_eq!(
                        error,
                        GraphMutationErrorV2::ItemRevisionConflict {
                            expected: Revision::new(1_000 + position as u64),
                            actual: Revision::ZERO,
                        }
                    );
                } else {
                    assert_eq!(error, GraphMutationErrorV2::ItemTypeMismatch);
                }
                assert_eq!(state.workspace_revision(), Revision::new(1));
                assert!(
                    state.workflow_memory().attempts()[0]
                        .item_slots()
                        .iter()
                        .all(|slot| slot.revision() == Revision::ZERO && slot.value().is_none())
                );
            }
        }
    }
}

fn forms_state() -> GraphSessionStateV2 {
    let items = json!([
        {"id":"artifact","type":"artifact","prompt":"Artifact","required":false,"allowed_media_types":["text/plain"]},
        {"id":"check","type":"check_result","prompt":"Check","required":false,"operation_id":"make-test","operation_digest":digest('a'),"accepted_outcomes":["pass"]},
        {"id":"choice","type":"choice","prompt":"Choice","required":false,"choices":["green","red"]},
        {"id":"confirm","type":"confirm","prompt":"Confirm","required":false},
        {"id":"integer","type":"integer","prompt":"Integer","required":false},
        {"id":"list-add","type":"list","prompt":"Add","required":false},
        {"id":"list-replace","type":"list","prompt":"Replace","required":false},
        {"id":"note","type":"text","prompt":"Note","required":false},
        {"id":"text","type":"text","prompt":"Text","required":false}
    ]);
    let snapshot = snapshot_from(
        json!({
            "schema": "podway.procedure/v2",
            "id": "optim-forms",
            "version": "1",
            "name": "Optim forms",
            "purpose": "Apply every supported item form through one draft.",
            "node_definitions": {
                "work": {
                    "type": "action",
                    "title": "Work",
                    "intent": "Record every form.",
                    "items": items
                }
            },
            "graph": {
                "entry": "work",
                "nodes": [{"id": "work", "use": "work", "terminal": true}]
            }
        }),
        "00000000-0000-4000-8000-000000002304",
    );
    let specs = [
        ("artifact", ItemTypeV1::Artifact),
        ("check", ItemTypeV1::CheckResult),
        ("choice", ItemTypeV1::Choice),
        ("confirm", ItemTypeV1::Confirm),
        ("integer", ItemTypeV1::Integer),
        ("list-add", ItemTypeV1::List),
        ("list-replace", ItemTypeV1::List),
        ("note", ItemTypeV1::Text),
        ("text", ItemTypeV1::Text),
    ];
    let slots = specs
        .into_iter()
        .map(|(id, item_type)| {
            ItemSlotStateV2::new(
                attempt_id(),
                ItemId::new(id).unwrap(),
                item_type,
                Revision::ZERO,
                None,
                UnixMillis::new(NOW),
                UnixMillis::new(NOW),
            )
            .unwrap()
        })
        .collect();
    running_graph(snapshot, slots, Vec::new())
}

fn form_requests() -> Vec<ActiveItemMutationRequestV2> {
    let check = CheckResultValueV2::new(
        OperationId::new("make-test").unwrap(),
        digest('a'),
        CheckResultInputBasisV2::new("fixture inputs", digest('b')).unwrap(),
        CheckResultExecutorV2::new("fixture", "1").unwrap(),
        CheckResultOutcomeV2::Pass,
        "Passed.",
        digest('c'),
    )
    .unwrap();
    let mut requests = vec![
        ActiveItemMutationRequestV2::new(
            ItemId::new("text").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "ready".to_owned(),
            },
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("note").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Clear,
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("list-replace").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::ReplaceList {
                values: vec!["beta".to_owned()],
            },
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("list-add").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Add {
                value: "locked".to_owned(),
            },
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("integer").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "-7".to_owned(),
            },
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("confirm").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Check,
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("choice").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Set {
                value: "green".to_owned(),
            },
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("check").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::RecordCheckResult { value: check },
        ),
        ActiveItemMutationRequestV2::new(
            ItemId::new("artifact").unwrap(),
            Revision::ZERO,
            ActiveItemMutationV2::Attach {
                value: ArtifactValueV1::local_path(
                    "reports/result.txt",
                    digest('c'),
                    42,
                    "text/plain",
                )
                .unwrap(),
            },
        ),
    ];
    requests.reverse();
    requests
}

#[test]
fn optim003_supported_forms_follow_sorted_item_order() {
    let state = forms_state();
    let requests = form_requests();
    let forward = apply(&state, &requests).unwrap();
    let mut reversed = requests;
    reversed.reverse();
    let backward = apply(&state, &reversed).unwrap();
    assert_eq!(forward.state(), backward.state());
    assert_eq!(forward.items(), backward.items());
    assert_eq!(
        forward
            .items()
            .iter()
            .map(|entry| (entry.item_id().as_str(), entry.changed()))
            .collect::<Vec<_>>(),
        vec![
            ("artifact", true),
            ("check", true),
            ("choice", true),
            ("confirm", true),
            ("integer", true),
            ("list-add", true),
            ("list-replace", true),
            ("note", false),
            ("text", true),
        ]
    );
    assert_eq!(
        forward
            .items()
            .iter()
            .find(|entry| entry.item_id().as_str() == "artifact")
            .unwrap()
            .value_digest(),
        Some(&digest('c'))
    );
    assert_eq!(forward.state().workspace_revision(), Revision::new(2));

    let recorded = forward.state();
    let follow = apply(
        recorded,
        &[
            ActiveItemMutationRequestV2::new(
                ItemId::new("integer").unwrap(),
                Revision::new(1),
                ActiveItemMutationV2::Set {
                    value: "-7".to_owned(),
                },
            ),
            ActiveItemMutationRequestV2::new(
                ItemId::new("text").unwrap(),
                Revision::new(1),
                ActiveItemMutationV2::Clear,
            ),
            ActiveItemMutationRequestV2::new(
                ItemId::new("list-add").unwrap(),
                Revision::new(1),
                ActiveItemMutationV2::Remove {
                    value: "locked".to_owned(),
                    ignore_missing: false,
                },
            ),
            ActiveItemMutationRequestV2::new(
                ItemId::new("confirm").unwrap(),
                Revision::new(1),
                ActiveItemMutationV2::Uncheck,
            ),
        ],
    )
    .unwrap();
    assert!(follow.changed());
    assert_eq!(follow.state().workspace_revision(), Revision::new(3));
    assert_eq!(follow.state().trace().revision(), Revision::new(3));
    let by_id = |name: &str| {
        follow.state().workflow_memory().attempts()[0]
            .item_slots()
            .iter()
            .find(|slot| slot.item_id().as_str() == name)
            .unwrap()
    };
    assert!(by_id("confirm").value().is_none());
    assert_eq!(by_id("confirm").revision(), Revision::new(2));
    assert!(by_id("text").value().is_none());
    assert_eq!(by_id("text").revision(), Revision::new(2));
    assert!(by_id("list-add").value().is_none());
    assert_eq!(by_id("list-add").revision(), Revision::new(2));
    assert_eq!(
        by_id("integer").value(),
        Some(&RecordedItemValueV2::integer(-7))
    );
    assert_eq!(by_id("integer").revision(), Revision::new(1));
    assert!(
        !follow
            .items()
            .iter()
            .find(|entry| entry.item_id().as_str() == "integer")
            .unwrap()
            .changed()
    );
}
