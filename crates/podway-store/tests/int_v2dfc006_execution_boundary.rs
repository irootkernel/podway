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
