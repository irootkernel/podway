use std::sync::atomic::{AtomicUsize, Ordering};

use podway_core::UnixMillis;
use podway_store::direct_store::DirectStoreErrorV1;
use serde_json::json;

use super::int_v2dfc005_direct_store::{deadline, fixture, request, success};

#[test]
fn reserved_success_sequence_matches_commit_replay_and_rollback_without_reading_clock() {
    let (_root, store) = fixture();
    let clock_reads = AtomicUsize::new(0);
    let key = request("sequence-success");
    let result = store
        .mutate(
            &key,
            deadline(),
            || {
                clock_reads.fetch_add(1, Ordering::SeqCst);
                UnixMillis::new(40)
            },
            |transaction| {
                assert_eq!(transaction.next_workspace_sequence()?, 2);
                assert_eq!(transaction.next_workspace_sequence()?, 2);
                assert_eq!(clock_reads.load(Ordering::SeqCst), 0);
                let now = transaction.now();
                Ok(success(
                    &key,
                    json!({"sequence":transaction.next_workspace_sequence()?}),
                    now,
                ))
            },
        )
        .unwrap();
    assert_eq!(result.outcome().sequence(), 2);
    assert_eq!(clock_reads.load(Ordering::SeqCst), 1);
    let replay = store
        .mutate(
            &key,
            deadline(),
            || panic!("replay reads no clock"),
            |_| panic!("replay invokes no execution callback"),
        )
        .unwrap();
    assert!(replay.replayed());
    assert_eq!(replay.outcome().sequence(), 2);
    let rejected = request("sequence-rejected");
    let error = store
        .mutate(
            &rejected,
            deadline(),
            || panic!("rejected preparation reads no clock"),
            |transaction| {
                assert_eq!(transaction.next_workspace_sequence()?, 3);
                Err(DirectStoreErrorV1::InvalidInput {
                    reason: "rejected prepared input",
                })
            },
        )
        .unwrap_err();
    assert!(matches!(error, DirectStoreErrorV1::InvalidInput { .. }));
    store
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(rejected.key())?.is_none());
            assert_eq!(snapshot.committed_sequence()?, 2);
            Ok(())
        })
        .unwrap();
    store
        .mutate(
            &rejected,
            deadline(),
            || UnixMillis::new(50),
            |transaction| {
                assert_eq!(transaction.next_workspace_sequence()?, 3);
                let now = transaction.now();
                Ok(success(&rejected, json!({"sequence":3}), now))
            },
        )
        .unwrap();
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.committed_sequence()?, 3);
            Ok(())
        })
        .unwrap();
}

#[test]
fn agent003_post_replace_graph_error_rolls_back_state_and_outcome_atomically() {
    use super::int_v2_goal_state::{assessment_state, attempt_id, criterion, identity, item, node};
    use super::int_v2dfc005_direct_store::store as cold_store;
    use super::int_v2dfc008_persistence::{fixture as v2_fixture, seed};
    use podway_core::{
        ActorAttributionV2, CriterionAssessmentBatchEntryV2, CriterionCitationV2,
        GoalRevisionNumberV2, Revision,
    };

    let (_root, store) = v2_fixture(&identity());
    let root_path = store.layout().root().to_path_buf();
    let initial = assessment_state(5, true, Vec::new());
    seed(&store, &initial);

    let after_batch = initial
        .assess_goal_criteria_v2(
            Revision::new(5),
            &attempt_id(2),
            GoalRevisionNumberV2::FIRST,
            vec![
                CriterionAssessmentBatchEntryV2::new(
                    criterion("z-proof"),
                    "satisfied",
                    "First entry before post-replace error.",
                    vec![
                        CriterionCitationV2::Evidence(node("clarify")),
                        CriterionCitationV2::Item(item("assessment-note")),
                    ],
                ),
                CriterionAssessmentBatchEntryV2::new(
                    criterion("a-safety"),
                    "satisfied",
                    "Second entry before post-replace error.",
                    Vec::new(),
                ),
            ],
            Some(ActorAttributionV2::new("rollback-actor").unwrap()),
            UnixMillis::new(75),
        )
        .unwrap()
        .into_state();

    let key = request("agent003-post-state-rollback");
    let before_seq = store
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    let err = store
        .mutate(
            &key,
            deadline(),
            || UnixMillis::new(75),
            |transaction| {
                let _ = transaction.now();
                let _ = transaction.next_workspace_sequence()?;
                transaction.replace_graph(
                    initial.workspace_revision(),
                    initial.trace().revision(),
                    &after_batch,
                )?;
                Err(DirectStoreErrorV1::InvalidInput {
                    reason: "post-state envelope validation failure",
                })
            },
        )
        .unwrap_err();
    assert!(matches!(err, DirectStoreErrorV1::InvalidInput { .. }));
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.current_graph()?, Some(initial.clone()));
            assert_eq!(snapshot.committed_sequence()?, before_seq);
            assert!(snapshot.outcome(key.key())?.is_none());
            Ok(())
        })
        .unwrap();
    drop(store);
    let reopened = cold_store(root_path.as_path());
    reopened
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.current_graph()?, Some(initial.clone()));
            assert_eq!(snapshot.committed_sequence()?, before_seq);
            assert!(snapshot.outcome(key.key())?.is_none());
            Ok(())
        })
        .unwrap();
    reopened.verify_integrity(deadline()).unwrap();
}
