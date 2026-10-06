//! Native lease, marker, deadline, identity and retention boundary regressions.

use podway_core::{UnixMillis, canonicalize_json_v1};
use podway_store::{
    CanonicalExecutionJsonV1,
    direct_store::{
        DirectMaintenanceMarkerKindV1, DirectMaintenanceMarkerV1, DirectStoreClassificationV1,
        DirectStoreErrorV1, ExclusiveMaintenanceLeaseV1, SharedMaintenanceLeaseV1,
        classify_direct_store_v1,
    },
};
use serde_json::json;
use std::{
    fs,
    fs::TryLockError,
    sync::{Arc, Barrier, mpsc},
    time::{Duration, Instant},
};

use super::int_v2dfc005_direct_store::{deadline, digest, fixture, request, store, success};

fn marker(kind: DirectMaintenanceMarkerKindV1, phase: &str) -> DirectMaintenanceMarkerV1 {
    let (schema, command) = match kind {
        DirectMaintenanceMarkerKindV1::Intent => (
            "podway.workspace-maintenance-intent-marker/v1",
            "workspace.init",
        ),
        DirectMaintenanceMarkerKindV1::Reset => {
            ("podway.workspace-reset-marker/v1", "workspace.reset_all")
        }
        DirectMaintenanceMarkerKindV1::Removal => {
            ("podway.workspace-removal-marker/v2", "workspace.remove")
        }
    };
    let mut document = json!({"schema":schema,"command":command,"operation_id":"00000000-0000-4000-8000-000000000003",
        "response_request_id":"00000000-0000-4000-8000-000000000002","workspace_uuid":null,
        "idempotency_key":"maintenance","request_digest":digest('d'),"contract_manifest_digest":digest('c'),
        "created_at":"1970-01-01T00:00:00.010Z","root_identity":{"root_path_bytes_base64url":"L3RtcA",
            "common_dir_identity":digest('a'),"worktree_admin_identity":digest('b')}});
    if kind != DirectMaintenanceMarkerKindV1::Intent {
        document["phase"] = json!(phase);
    }
    DirectMaintenanceMarkerV1::new(
        kind,
        CanonicalExecutionJsonV1::new(canonicalize_json_v1(&document).unwrap()).unwrap(),
    )
    .unwrap()
}

#[test]
fn markers_are_no_clobber_content_fenced_and_never_resumed_by_reads() {
    for kind in [
        DirectMaintenanceMarkerKindV1::Intent,
        DirectMaintenanceMarkerKindV1::Reset,
        DirectMaintenanceMarkerKindV1::Removal,
    ] {
        let (_root, direct) = fixture();
        let lease =
            ExclusiveMaintenanceLeaseV1::acquire(direct.layout(), deadline(), false).unwrap();
        let initial = marker(kind, "prepared");
        lease.write_marker(direct.layout(), &initial, None).unwrap();
        assert_eq!(
            lease.read_marker(direct.layout(), kind).unwrap(),
            Some(initial.clone())
        );
        assert!(lease.write_marker(direct.layout(), &initial, None).is_err());
        assert_eq!(
            classify_direct_store_v1(direct.layout()).unwrap(),
            DirectStoreClassificationV1::Maintenance
        );
        assert!(matches!(
            direct.read::<()>(deadline(), |_| panic!("read must not resume maintenance")),
            Err(DirectStoreErrorV1::Maintenance)
        ));
        if kind != DirectMaintenanceMarkerKindV1::Intent {
            let next = marker(kind, "runtime_removed");
            lease
                .write_marker(direct.layout(), &next, Some(&initial))
                .unwrap();
            assert!(lease.remove_marker(direct.layout(), &initial).is_err());
            assert_eq!(
                lease.read_marker(direct.layout(), kind).unwrap(),
                Some(next.clone())
            );
            lease.remove_marker(direct.layout(), &next).unwrap();
        } else {
            lease.remove_marker(direct.layout(), &initial).unwrap();
        }
        assert!(lease.read_marker(direct.layout(), kind).unwrap().is_none());
    }
}

#[test]
fn replaced_lock_or_runtime_inode_invalidates_a_held_maintenance_lease() {
    for replaced in ["lock", "runtime"] {
        let (root, direct) = fixture();
        let lease =
            ExclusiveMaintenanceLeaseV1::acquire(direct.layout(), deadline(), false).unwrap();
        if replaced == "lock" {
            fs::rename(
                direct.layout().maintenance_lock(),
                root.path().join("retained-lock"),
            )
            .unwrap();
            fs::write(direct.layout().maintenance_lock(), b"").unwrap();
        } else {
            fs::rename(
                direct.layout().runtime_directory(),
                root.path().join("retained-runtime"),
            )
            .unwrap();
            fs::create_dir(direct.layout().runtime_directory()).unwrap();
        }
        assert!(matches!(
            lease.write_marker(
                direct.layout(),
                &marker(DirectMaintenanceMarkerKindV1::Intent, ""),
                None
            ),
            Err(DirectStoreErrorV1::Maintenance)
        ));
        assert!(!direct.layout().intent_marker().exists());
    }
}

#[test]
fn exclusive_intent_blocks_new_readers_while_existing_readers_drain() {
    let (_root, direct) = fixture();
    let shared = SharedMaintenanceLeaseV1::acquire(direct.layout(), deadline()).unwrap();
    let layout = direct.layout().clone();
    let begin = Arc::new(Barrier::new(2));
    let started = Arc::clone(&begin);
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        started.wait();
        let lease = ExclusiveMaintenanceLeaseV1::acquire(&layout, deadline(), false).unwrap();
        acquired_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(lease);
    });
    begin.wait();
    let limit = deadline();
    loop {
        let file = fs::File::open(direct.layout().intent_lock()).unwrap();
        if matches!(file.try_lock_shared(), Err(TryLockError::WouldBlock)) {
            break;
        }
        assert!(Instant::now() < limit);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(
        SharedMaintenanceLeaseV1::acquire(
            direct.layout(),
            Instant::now() + Duration::from_millis(20)
        ),
        Err(DirectStoreErrorV1::Maintenance)
    ));
    drop(shared);
    acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        SharedMaintenanceLeaseV1::acquire(
            direct.layout(),
            Instant::now() + Duration::from_millis(20)
        ),
        Err(DirectStoreErrorV1::Maintenance)
    ));
    release_tx.send(()).unwrap();
    writer.join().unwrap();
    SharedMaintenanceLeaseV1::acquire(direct.layout(), deadline()).unwrap();
}

#[test]
fn moved_root_is_rebound_only_by_a_new_successful_transaction() {
    let (original_root, direct) = fixture();
    let parent = tempfile::TempDir::new().unwrap();
    let moved = parent.path().join("moved-%?&#");
    fs::rename(original_root.path(), &moved).unwrap();
    let moved_store = store(&moved);
    let old = direct.layout().root().clone();
    moved_store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.binding().last_validated_root(), &old);
            Ok(())
        })
        .unwrap();
    let replay = moved_store.mutate(
        &request("init"),
        deadline(),
        || panic!("replay must not rebind"),
        |_| panic!("replay callback"),
    );
    assert!(replay.unwrap().replayed());
    let reject = request("reject-moved");
    assert!(
        moved_store
            .mutate(
                &reject,
                deadline(),
                || UnixMillis::new(20),
                |transaction| {
                    transaction.now();
                    Ok(CanonicalExecutionJsonV1::new("{}").unwrap())
                }
            )
            .is_err()
    );
    moved_store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.binding().last_validated_root(), &old);
            Ok(())
        })
        .unwrap();
    let change = request("accept-moved");
    moved_store
        .mutate(
            &change,
            deadline(),
            || UnixMillis::new(30),
            |transaction| {
                transaction.now();
                Ok(success(
                    &change,
                    json!({"repaired":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    moved_store
        .read(deadline(), |snapshot| {
            assert_eq!(
                snapshot.binding().last_validated_root(),
                moved_store.layout().root()
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn cold_foreign_writer_is_rejected_without_creating_an_index() {
    let (_root, direct) = fixture();
    fs::remove_file(direct.layout().shm()).unwrap();
    let main = fs::read(direct.layout().database()).unwrap();
    let wal = fs::read(direct.layout().wal()).unwrap();
    let foreign = podway_store::direct_store::DirectStoreV1::new(
        direct.layout().clone(),
        digest('f'),
        digest('a'),
        digest('b'),
    );
    assert!(matches!(
        foreign.mutate(
            &request("foreign-cold"),
            deadline(),
            || panic!("foreign clock"),
            |_| panic!("foreign effect")
        ),
        Err(DirectStoreErrorV1::WriterMismatch { .. })
    ));
    assert_eq!(fs::read(direct.layout().database()).unwrap(), main);
    assert_eq!(fs::read(direct.layout().wal()).unwrap(), wal);
    assert!(!direct.layout().shm().exists());
    let recover = request("cold-success");
    direct
        .mutate(
            &recover,
            deadline(),
            || UnixMillis::new(20),
            |transaction| {
                transaction.now();
                Ok(success(
                    &recover,
                    json!({"repaired":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    assert!(direct.layout().shm().is_file());
}

#[test]
fn workspace_outcomes_age_and_count_prune_without_pruning_session_history() {
    use podway_store::{
        IdempotencyKeyV1,
        direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1},
    };
    let (_root, direct) = fixture();
    let graph = super::int_v2dfc005_direct_store::grown_graph(2, 0);
    let seed = request("retention-seed");
    direct
        .mutate(
            &seed,
            deadline(),
            || UnixMillis::new(25),
            |transaction| {
                transaction.now();
                transaction.create_graph(&graph)?;
                Ok(success(&seed, json!({"seeded":true}), transaction.now()))
            },
        )
        .unwrap();
    let session = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("retained-session").unwrap(),
        digest('d'),
        "item.set",
        "00000000-0000-4000-8000-000000000002",
        DirectOutcomeScopeV1::Session(graph.trace().session_id().clone()),
    )
    .unwrap();
    direct
        .mutate(
            &session,
            deadline(),
            || UnixMillis::new(30),
            |transaction| {
                transaction.now();
                Ok(success(
                    &session,
                    json!({"changed":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    for index in 0..210 {
        let request = request(&format!("workspace-{index}"));
        direct
            .mutate(
                &request,
                deadline(),
                || UnixMillis::new(40 + index),
                |transaction| {
                    transaction.now();
                    Ok(success(
                        &request,
                        json!({"repaired":true}),
                        transaction.now(),
                    ))
                },
            )
            .unwrap();
    }
    direct
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(request("workspace-109").key())?.is_none());
            assert!(snapshot.outcome(request("workspace-110").key())?.is_some());
            assert!(snapshot.outcome(session.key())?.is_some());
            Ok(())
        })
        .unwrap();
    let later = request("later");
    direct
        .mutate(
            &later,
            deadline(),
            || UnixMillis::new(31 * 24 * 60 * 60 * 1000),
            |transaction| {
                transaction.now();
                Ok(success(&later, json!({"repaired":true}), transaction.now()))
            },
        )
        .unwrap();
    direct
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(request("workspace-209").key())?.is_none());
            assert!(snapshot.outcome(session.key())?.is_some());
            Ok(())
        })
        .unwrap();
    let connection = rusqlite::Connection::open_with_flags(
        direct.layout().database(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count: u64 = connection
        .query_row("SELECT count(*) FROM operational_journal", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 200);
}
