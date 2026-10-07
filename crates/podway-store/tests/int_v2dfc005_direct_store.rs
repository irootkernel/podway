//! Real direct Store persistence, read-only files and native process boundaries.

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, Instant},
};

use podway_core::{
    CanonicalProcedureJsonV1, ProcedureSnapshotId, ProcedureSourceLabelV1, Revision, SessionId,
    Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, GraphSessionStateV2, IdempotencyKeyV1, ProcedureSnapshotV2,
    ValidatedWorkspaceRootV1,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreClassificationV1,
        DirectStoreErrorV1, DirectStoreV1, DirectTransactionBoundaryV1, DirectWorkspaceLayoutV1,
        ExclusiveMaintenanceLeaseV1, SharedMaintenanceLeaseV1, classify_direct_store_v1,
    },
};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub(super) fn grown_graph(nodes: usize, completed_nodes: usize) -> GraphSessionStateV2 {
    use podway_core::{AttemptId, ItemId};
    use podway_store::{ActiveItemMutationRequestV2, ActiveItemMutationV2};
    let placements: Vec<_> = (0..nodes)
        .map(|index| {
            if index + 1 == nodes {
                json!({"id":format!("n{index:04}"),"use":"work","terminal":true})
            } else {
                json!({"id":format!("n{index:04}"),"use":"work","next":format!("n{:04}",index+1)})
            }
        })
        .collect();
    let items: Vec<_> = (0..16)
        .map(|index| {
            json!({"id":format!("item-{index}"),
        "type":"text","prompt":"Record payload.","required":false,"max_length":65536})
        })
        .collect();
    let canonical = canonicalize_json_v1(&json!({"schema":"podway.procedure/v2","id":"direct-scale",
        "version":"1","name":"Direct scale","purpose":"Measure grown direct Store transactions.",
        "node_definitions":{"work":{"type":"action","title":"Record payloads","intent":"Record payloads.","items":items}},
        "graph":{"entry":"n0000","nodes":placements}})).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000003").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("procedure.yaml").unwrap(),
        UnixMillis::new(20),
    )
    .unwrap();
    let mut graph = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Direct scale",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000000004").unwrap(),
        UnixMillis::new(20),
    )
    .unwrap();
    let attempt = |index| AttemptId::new(format!("00000000-0000-4000-9000-{index:012x}")).unwrap();
    graph = graph
        .begin_v2(Revision::ZERO, attempt(1), None, UnixMillis::new(21))
        .unwrap()
        .into_state();
    for node in 0..completed_nodes {
        // Match the adopted L history: six batches of 16 x 55,000 characters
        // per completed node, with distinct values so every batch changes state.
        for batch in 0..6 {
            let active = graph.trace().active_attempt().unwrap().attempt_id().clone();
            let slots = graph
                .workflow_memory()
                .attempts()
                .last()
                .unwrap()
                .item_slots();
            let mutations: Vec<_> = (0..16)
                .map(|index| {
                    ActiveItemMutationRequestV2::new(
                        ItemId::new(format!("item-{index}")).unwrap(),
                        slots[index].revision(),
                        ActiveItemMutationV2::Set {
                            value: char::from(b'a' + batch).to_string().repeat(55_000),
                        },
                    )
                })
                .collect();
            graph = graph
                .mutate_active_items_v2(
                    &active,
                    &mutations,
                    UnixMillis::new(30 + node as u64 * 10 + u64::from(batch)),
                )
                .unwrap()
                .into_state();
        }
        let active = graph.trace().active_attempt().unwrap().attempt_id().clone();
        graph = graph
            .complete_active_action_v2(
                graph.trace().revision(),
                &active,
                Some(attempt(node + 2)),
                UnixMillis::new(39 + node as u64 * 10),
            )
            .unwrap()
            .into_state();
    }
    graph
}

#[test]
#[ignore = "explicit release-profile diagnostic before the mandatory multiprocess G1/G2 gates"]
fn direct_scale_transaction_probe() {
    use podway_core::ItemId;
    use podway_store::ActiveItemMutationV2;
    use std::sync::{Arc, Mutex};
    let evidence =
        std::env::var_os("PODWAY_DIRECT_TEST_EVIDENCE_ROOT").expect("explicit owned evidence root");
    let evidence = Path::new(&evidence);
    assert!(evidence.is_absolute() && evidence.is_dir());
    for nodes in [300, 1024] {
        let (_root, initial) = fixture();
        let graph = grown_graph(nodes, 7);
        let seed = request("seed-scale");
        initial
            .mutate(
                &seed,
                deadline(),
                || UnixMillis::new(200),
                |transaction| {
                    transaction.now();
                    transaction.create_graph(&graph)?;
                    Ok(success(&seed, json!({"seeded":true}), transaction.now()))
                },
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer = Arc::clone(&events);
        let measured = initial.with_boundary_observer(move |boundary| {
            observer.lock().unwrap().push((boundary, Instant::now()))
        });
        let mut samples = Vec::new();
        for index in 0..30 {
            events.lock().unwrap().clear();
            let request = request(&format!("scale-{index}"));
            let mut phases = Vec::new();
            let prepared = measured.prepare_graph(deadline()).unwrap().unwrap();
            measured
                .mutate(
                    &request,
                    deadline(),
                    || UnixMillis::new(300 + index),
                    |transaction| {
                        phases.push(Instant::now());
                        transaction.use_prepared_graph(&prepared)?;
                        let prior = transaction.current_graph()?.unwrap().clone();
                        phases.push(Instant::now());
                        let active = prior.trace().active_attempt().unwrap().attempt_id();
                        let slot = prior
                            .workflow_memory()
                            .attempts()
                            .last()
                            .unwrap()
                            .item_slots()
                            .first()
                            .unwrap();
                        let now = transaction.now();
                        let next = prior
                            .mutate_active_item_v2(
                                active,
                                &ItemId::new("item-0").unwrap(),
                                slot.revision(),
                                ActiveItemMutationV2::Set {
                                    value: format!("value-{index:064}"),
                                },
                                now,
                            )?
                            .into_state();
                        phases.push(Instant::now());
                        transaction.replace_graph(
                            prior.workspace_revision(),
                            prior.trace().revision(),
                            &next,
                        )?;
                        phases.push(Instant::now());
                        Ok(success(
                            &request,
                            json!({"changed":true}),
                            transaction.now(),
                        ))
                    },
                )
                .unwrap();
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 4);
            samples.push(
                json!({"wait_us":events[1].1.duration_since(events[0].1).as_micros(),
                "hold_us":events[3].1.duration_since(events[1].1).as_micros(),
                "load_us":phases[1].duration_since(phases[0]).as_micros(),
                "transition_us":phases[2].duration_since(phases[1]).as_micros(),
                "persist_us":phases[3].duration_since(phases[2]).as_micros()}),
            );
        }
        fs::write(
            evidence.join(format!("probe-{nodes}.json")),
            serde_json::to_vec_pretty(&json!({
            "nodes":nodes,"completed_nodes":7,"items_per_node":16,"characters_per_item":55000,
            "batches_per_node":6,"samples":samples,"schema":14,"path":"production DirectStoreV1"}))
            .unwrap(),
        )
        .unwrap();
    }
}

fn prepared_graph(now: UnixMillis) -> GraphSessionStateV2 {
    let canonical = canonicalize_json_v1(&json!({
        "schema":"podway.procedure/v2", "id":"direct-store", "version":"1",
        "name":"Direct Store", "purpose":"Verify direct relational persistence.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Complete work."}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]},
        "manual_rework":{"allowed_targets":["work"]}
    }))
    .unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = ProcedureSnapshotV2::new(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000003").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        ProcedureSourceLabelV1::file("procedure.yaml").unwrap(),
        now,
    )
    .unwrap();
    GraphSessionStateV2::prepared(
        Revision::new(1),
        "Direct Store fixture",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000000004").unwrap(),
        now,
    )
    .unwrap()
}

pub(super) fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
}

pub(super) fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

pub(super) fn store(root: &Path) -> DirectStoreV1 {
    DirectStoreV1::new(
        DirectWorkspaceLayoutV1::new(ValidatedWorkspaceRootV1::from_path(root).unwrap()).unwrap(),
        digest('c'),
        digest('a'),
        digest('b'),
    )
}

pub(super) fn request(key: &str) -> DirectMutationRequestV1 {
    DirectMutationRequestV1::new(
        IdempotencyKeyV1::new(key).unwrap(),
        digest('d'),
        "workspace.repair",
        "00000000-0000-4000-8000-000000000002",
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap()
}

pub(super) fn success(
    request: &DirectMutationRequestV1,
    result: serde_json::Value,
    now: UnixMillis,
) -> CanonicalExecutionJsonV1 {
    CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "schema":"podway.output/v4", "request_id":request.response_id(),
            "command":request.command(), "generated_at":fixture_timestamp(now),
            "result":result, "warnings":[]
        }))
        .unwrap(),
    )
    .unwrap()
}

// These deterministic fixture clocks cover the first 32 days of 1970.
fn fixture_timestamp(now: UnixMillis) -> String {
    assert!(now.get() < 32 * 24 * 60 * 60 * 1000);
    let days = now.get() / 86_400_000;
    let (month, day) = if days < 31 {
        (1, days + 1)
    } else {
        (2, days - 30)
    };
    let seconds = (now.get() / 1000) % 86_400;
    format!(
        "1970-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
        now.get() % 1000
    )
}

pub(super) fn fixture() -> (TempDir, DirectStoreV1) {
    let root = TempDir::new().unwrap();
    let store = store(root.path());
    let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), deadline(), true).unwrap();
    let request = request("init");
    store
        .bootstrap(
            &lease,
            WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            "generation-one",
            &request,
            deadline(),
            || UnixMillis::new(10),
            |transaction| {
                Ok(success(
                    &request,
                    json!({"initialized":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    drop(lease);
    (root, store)
}

#[derive(Debug, Eq, PartialEq)]
struct FileImage {
    name: std::ffi::OsString,
    dev: u64,
    ino: u64,
    bytes: Vec<u8>,
}

fn images(directory: &Path) -> Vec<FileImage> {
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            FileImage {
                name: entry.file_name(),
                dev: metadata.dev(),
                ino: metadata.ino(),
                bytes: fs::read(entry.path()).unwrap(),
            }
        })
        .collect();
    files.sort_by(|left, right| left.name.cmp(&right.name));
    files
}

#[test]
fn absent_wal_reads_and_foreign_writer_refusals_are_write_free() {
    for foreign_writer in [false, true] {
        for stale_shm in [false, true] {
            let (_root, store) = fixture();
            let connection = Connection::open(store.layout().database()).unwrap();
            if foreign_writer {
                connection
                    .execute(
                        "UPDATE writer_contract SET contract_manifest_digest=?1 WHERE singleton=1",
                        [format!("sha256:{}", "f".repeat(64))],
                    )
                    .unwrap();
            }
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
            drop(connection);
            assert!(!store.layout().wal().exists());
            assert!(!store.layout().shm().exists());
            if stale_shm {
                fs::write(store.layout().shm(), vec![0; 32768]).unwrap();
            }
            let before = images(&store.layout().runtime_directory());
            let read = store.read(deadline(), |snapshot| {
                assert!(
                    !foreign_writer,
                    "foreign writer must be refused before the callback"
                );
                assert_eq!(
                    snapshot.outcome(request("init").key())?.unwrap().sequence(),
                    1
                );
                Ok(())
            });
            if foreign_writer {
                assert!(matches!(
                    read,
                    Err(DirectStoreErrorV1::WriterMismatch { .. })
                ));
            } else {
                read.unwrap();
            }
            assert!(
                images(&store.layout().runtime_directory()) == before,
                "read changed absent-WAL files: foreign_writer={foreign_writer}, stale_shm={stale_shm}"
            );
            let diagnosis = store.verify_integrity(deadline());
            if foreign_writer {
                assert!(matches!(
                    diagnosis,
                    Err(DirectStoreErrorV1::WriterMismatch { .. })
                ));
            } else {
                diagnosis.unwrap();
            }
            assert!(
                images(&store.layout().runtime_directory()) == before,
                "diagnosis changed absent-WAL files: foreign_writer={foreign_writer}, stale_shm={stale_shm}"
            );
            let migration = store.migrate_declared_writer(deadline(), || {
                panic!("current or unknown writer must not read a migration clock")
            });
            if foreign_writer {
                assert!(matches!(
                    migration,
                    Err(DirectStoreErrorV1::WriterMismatch { .. })
                ));
            } else {
                assert!(!migration.unwrap());
            }
            let lease =
                ExclusiveMaintenanceLeaseV1::acquire(store.layout(), deadline(), false).unwrap();
            let exclusive = store.read_under_exclusive_lease(&lease, deadline(), |snapshot| {
                assert!(!foreign_writer, "foreign writer reached exclusive callback");
                assert_eq!(snapshot.view(UnixMillis::new(20))?.sequence(), 1);
                Ok(())
            });
            if foreign_writer {
                assert!(matches!(
                    exclusive,
                    Err(DirectStoreErrorV1::WriterMismatch { .. })
                ));
            } else {
                exclusive.unwrap();
            }
            drop(lease);
            assert!(
                images(&store.layout().runtime_directory()) == before,
                "exclusive prevalidation changed absent-WAL files: foreign_writer={foreign_writer}, stale_shm={stale_shm}"
            );
            if foreign_writer {
                let refused = store.mutate(
                    &request("refused"),
                    deadline(),
                    || panic!("foreign writer refusal must not read the mutation clock"),
                    |_| panic!("foreign writer refusal must not enter the mutation callback"),
                );
                assert!(matches!(
                    refused,
                    Err(DirectStoreErrorV1::WriterMismatch { .. })
                ));
                assert!(
                    images(&store.layout().runtime_directory()) == before,
                    "mutation prevalidation changed absent-WAL files: stale_shm={stale_shm}"
                );
            }
        }
    }
}

#[test]
fn absent_wal_read_refuses_non_wal_main_format_without_writes() {
    let (_root, store) = fixture();
    let connection = Connection::open(store.layout().database()).unwrap();
    connection
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .unwrap();
    drop(connection);
    assert!(!store.layout().wal().exists());
    let before = images(&store.layout().runtime_directory());
    assert!(matches!(
        store.read::<()>(deadline(), |_| panic!("non-WAL format reached callback")),
        Err(DirectStoreErrorV1::SchemaMismatch)
    ));
    assert!(images(&store.layout().runtime_directory()) == before);
}

#[test]
fn complete_generation_fourteen_bootstrap_and_cold_read_are_write_free() {
    let (_root, store) = fixture();
    assert_eq!(
        classify_direct_store_v1(store.layout()).unwrap(),
        DirectStoreClassificationV1::Current
    );
    let before = images(&store.layout().runtime_directory());
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.binding().schema_version(), 14);
            assert!(snapshot.current_graph()?.is_none());
            assert_eq!(
                snapshot
                    .outcome(&request("init").key())?
                    .unwrap()
                    .sequence(),
                1
            );
            assert_eq!(snapshot.view(UnixMillis::new(11))?.sequence(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(images(&store.layout().runtime_directory()), before);
    store.verify_integrity(deadline()).unwrap();
    assert_eq!(images(&store.layout().runtime_directory()), before);
    let connection = Connection::open(store.layout().database()).unwrap();
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 22);
}

#[test]
fn success_replay_bypasses_callback_clock_and_new_response_uuid() {
    let (_root, store) = fixture();
    let original = request("once");
    let mut clock_reads = 0;
    let first = store
        .mutate(
            &original,
            deadline(),
            || {
                clock_reads += 1;
                UnixMillis::new(20)
            },
            |transaction| {
                assert_eq!(transaction.now(), UnixMillis::new(20));
                assert_eq!(transaction.now(), UnixMillis::new(20));
                Ok(success(
                    &original,
                    json!({"repaired":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    assert!(!first.replayed());
    assert_eq!(clock_reads, 1);
    let retry = DirectMutationRequestV1::new(
        original.key().clone(),
        original.digest().clone(),
        original.command(),
        "00000000-0000-4000-8000-000000000099",
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    let replay = store
        .mutate(
            &retry,
            deadline(),
            || panic!("replay must not read the clock"),
            |_| panic!("replay must not execute a state transition"),
        )
        .unwrap();
    assert!(replay.replayed());
    assert_eq!(replay.outcome(), first.outcome());
    let conflict = DirectMutationRequestV1::new(
        original.key().clone(),
        digest('e'),
        original.command(),
        original.response_id(),
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    assert!(matches!(
        store.mutate(
            &conflict,
            deadline(),
            || panic!("conflict has no clock"),
            |_| panic!("conflict has no transition")
        ),
        Err(DirectStoreErrorV1::IdempotencyKeyReused)
    ));
}

#[test]
fn rejected_response_does_not_record_outcome_or_rebind_root() {
    let (_root, store) = fixture();
    let request = request("reject");
    let result = store.mutate(
        &request,
        deadline(),
        || UnixMillis::new(20),
        |transaction| {
            transaction.now();
            Ok(CanonicalExecutionJsonV1::new("{\"schema\":\"podway.error/v2\"}").unwrap())
        },
    );
    assert!(matches!(
        result,
        Err(DirectStoreErrorV1::InvalidInput { .. })
    ));
    store
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(request.key())?.is_none());
            assert_eq!(snapshot.view(UnixMillis::new(30))?.sequence(), 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn managed_same_process_reads_never_inherit_a_writers_writable_shm_mapping() {
    use std::sync::{Arc, Mutex, mpsc};
    let (_root, direct) = fixture();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release = Arc::new(Mutex::new(release_rx));
    let writer = direct.clone().with_boundary_observer(move |boundary| {
        if boundary == DirectTransactionBoundaryV1::AfterCommitBeforeOutput {
            ready_tx.send(()).unwrap();
            release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    });
    let child = std::thread::spawn(move || {
        let request = request("same-process");
        writer.mutate(
            &request,
            deadline(),
            || UnixMillis::new(20),
            |transaction| {
                transaction.now();
                Ok(success(
                    &request,
                    json!({"repaired":true}),
                    transaction.now(),
                ))
            },
        )
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let before = images(&direct.layout().runtime_directory());
    let blocked = direct.read::<()>(Instant::now() + Duration::from_millis(20), |_| {
        panic!("writer still owns its connection")
    });
    let after = images(&direct.layout().runtime_directory());
    release_tx.send(()).unwrap();
    let result = child.join().unwrap();
    assert!(result.is_ok());
    assert!(matches!(blocked, Err(DirectStoreErrorV1::Busy)));
    assert_eq!(after, before);
    let before = images(&direct.layout().runtime_directory());
    direct
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(request("same-process").key())?.is_some());
            Ok(())
        })
        .unwrap();
    assert_eq!(images(&direct.layout().runtime_directory()), before);
}

#[test]
fn persisted_outcomes_fail_closed_on_invalid_headers_or_future_sequences() {
    for corruption in ["headers", "sequence", "uuid", "command"] {
        let (_root, direct) = fixture();
        let connection = Connection::open(direct.layout().database()).unwrap();
        connection
            .set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                true,
            )
            .unwrap();
        match corruption {
            "headers" => {
                connection
                    .execute("UPDATE mutation_outcomes SET response_json='{\"schema\":\"podway.output/v4\"}'", [])
                    .unwrap();
            }
            "sequence" => {
                connection
                    .execute("UPDATE mutation_outcomes SET workspace_sequence=2", [])
                    .unwrap();
            }
            "uuid" => {
                let mut value: serde_json::Value = serde_json::from_str(
                    &success(
                        &request("init"),
                        json!({"initialized":true}),
                        UnixMillis::new(10),
                    )
                    .as_str(),
                )
                .unwrap();
                value["request_id"] = json!("invalid");
                connection
                    .execute(
                        "UPDATE mutation_outcomes SET response_json=?1",
                        [canonicalize_json_v1(&value).unwrap()],
                    )
                    .unwrap();
            }
            "command" => {
                connection
                    .execute("UPDATE mutation_outcomes SET command_name='INVALID'", [])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(connection);
        let before = images(&direct.layout().runtime_directory());
        assert!(
            matches!(
                direct.read(deadline(), |snapshot| snapshot
                    .outcome(request("init").key())),
                Err(DirectStoreErrorV1::Unreadable)
            ),
            "{corruption}"
        );
        assert!(
            matches!(
                direct.verify_integrity(deadline()),
                Err(DirectStoreErrorV1::Unreadable)
            ),
            "{corruption}"
        );
        assert_eq!(images(&direct.layout().runtime_directory()), before);
    }
}

#[test]
fn foreign_writer_git_and_schema_fail_before_readwrite_open() {
    for kind in ["writer", "git", "schema"] {
        let (_root, current) = fixture();
        if kind == "schema" {
            let connection = Connection::open(current.layout().database()).unwrap();
            connection
                .set_db_config(
                    rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                    true,
                )
                .unwrap();
            connection
                .execute_batch("CREATE TABLE unexpected(id INTEGER);")
                .unwrap();
        }
        let other = DirectStoreV1::new(
            current.layout().clone(),
            if kind == "writer" {
                digest('f')
            } else {
                digest('c')
            },
            if kind == "git" {
                digest('f')
            } else {
                digest('a')
            },
            digest('b'),
        );
        let before = images(&current.layout().runtime_directory());
        let result = other.mutate(
            &request("foreign"),
            deadline(),
            || panic!("no foreign clock"),
            |_| panic!("no foreign mutation"),
        );
        assert!(
            matches!(
                (kind, &result),
                ("writer", Err(DirectStoreErrorV1::WriterMismatch { .. }))
                    | ("git", Err(DirectStoreErrorV1::BindingMismatch))
                    | ("schema", Err(DirectStoreErrorV1::SchemaMismatch))
            ),
            "{kind}: {result:?}"
        );
        assert_eq!(images(&current.layout().runtime_directory()), before);
    }
}

#[test]
fn native_exclusive_lease_is_bounded_and_blocks_new_shared_opens() {
    let (_root, store) = fixture();
    let shared = SharedMaintenanceLeaseV1::acquire(store.layout(), deadline()).unwrap();
    let started = Instant::now();
    assert!(matches!(
        ExclusiveMaintenanceLeaseV1::acquire(
            store.layout(),
            started + Duration::from_millis(20),
            false
        ),
        Err(DirectStoreErrorV1::Maintenance)
    ));
    assert!(started.elapsed() < Duration::from_millis(100));
    drop(shared);
    let exclusive =
        ExclusiveMaintenanceLeaseV1::acquire(store.layout(), deadline(), false).unwrap();
    let started = Instant::now();
    assert!(matches!(
        SharedMaintenanceLeaseV1::acquire(store.layout(), started + Duration::from_millis(20)),
        Err(DirectStoreErrorV1::Maintenance)
    ));
    assert!(started.elapsed() < Duration::from_millis(100));
    drop(exclusive);
    SharedMaintenanceLeaseV1::acquire(store.layout(), deadline()).unwrap();
}

#[test]
fn relational_state_and_outcome_roll_back_together_after_validation_rejection() {
    let (_root, store) = fixture();
    let request = request("reject-state");
    let result = store.mutate(
        &request,
        deadline(),
        || UnixMillis::new(20),
        |transaction| {
            let now = transaction.now();
            transaction.create_graph(&prepared_graph(now))?;
            Ok(CanonicalExecutionJsonV1::new("{\"schema\":\"podway.error/v2\"}").unwrap())
        },
    );
    assert!(matches!(
        result,
        Err(DirectStoreErrorV1::InvalidInput { .. })
    ));
    store
        .read(deadline(), |snapshot| {
            assert!(snapshot.current_graph()?.is_none());
            assert!(snapshot.outcome(request.key())?.is_none());
            assert_eq!(snapshot.view(UnixMillis::new(30))?.sequence(), 1);
            Ok(())
        })
        .unwrap();
    let successful = store
        .mutate(
            &request,
            deadline(),
            || UnixMillis::new(30),
            |transaction| {
                let now = transaction.now();
                transaction.create_graph(&prepared_graph(now))?;
                Ok(success(
                    &request,
                    json!({"repaired":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    assert_eq!(successful.outcome().sequence(), 2);
    store
        .read(deadline(), |snapshot| {
            assert_eq!(
                snapshot.current_graph()?,
                Some(prepared_graph(UnixMillis::new(30)))
            );
            Ok(())
        })
        .unwrap();
}

const CHILD_FIXTURE_ENV: &str = "PODWAY_DIRECT_STORE_TEST_FIXTURE";
const CHILD_TEST: &str = "int_v2dfc005_direct_store::direct_store_process_child";

#[test]
fn direct_store_process_child() {
    let Some(path) = std::env::var_os(CHILD_FIXTURE_ENV) else {
        return;
    };
    let document: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let root = document["root"].as_str().unwrap();
    if document["operation"] == "hold_write_lock" {
        let mut connection = Connection::open(store(Path::new(root)).layout().database()).unwrap();
        connection
            .set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                true,
            )
            .unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        fs::write(document["signal"].as_str().unwrap(), b"ready").unwrap();
        let limit = deadline();
        while !Path::new(document["release"].as_str().unwrap()).exists() {
            assert!(Instant::now() < limit);
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(transaction);
        return;
    }
    let boundary = document["boundary"].as_u64().unwrap();
    let signal = document["signal"].as_str().unwrap().to_owned();
    let boundary = match boundary {
        0 => DirectTransactionBoundaryV1::BeforeBegin,
        1 => DirectTransactionBoundaryV1::AfterBegin,
        2 => DirectTransactionBoundaryV1::AfterStateBeforeOutcome,
        3 => DirectTransactionBoundaryV1::AfterCommitBeforeOutput,
        4 => DirectTransactionBoundaryV1::BeforeMaintenancePublication,
        _ => panic!("invalid fixture transaction boundary"),
    };
    let store = store(Path::new(root)).with_boundary_observer(move |observed| {
        if observed == boundary {
            fs::write(&signal, b"ready").unwrap();
            loop {
                std::thread::park();
            }
        }
    });
    let request = request("killed-state");
    if document["operation"] == "bootstrap" {
        let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), deadline(), true).unwrap();
        store
            .bootstrap(
                &lease,
                WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
                "generation-one",
                &request,
                deadline(),
                || UnixMillis::new(20),
                |transaction| {
                    let now = transaction.now();
                    transaction.create_graph(&prepared_graph(now))?;
                    Ok(success(
                        &request,
                        json!({"initialized":true}),
                        transaction.now(),
                    ))
                },
            )
            .unwrap();
        panic!("parent must kill bootstrap before publication");
    }
    store
        .mutate(
            &request,
            deadline(),
            || UnixMillis::new(20),
            |transaction| {
                let now = transaction.now();
                transaction.create_graph(&prepared_graph(now))?;
                Ok(success(
                    &request,
                    json!({"repaired":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    panic!("the parent must SIGKILL this child at its selected seam");
}

#[test]
fn sigkill_before_private_store_publication_never_exposes_a_partial_store() {
    for iteration in 0..20 {
        let root = TempDir::new().unwrap();
        let direct = store(root.path());
        let signal = root.path().join("bootstrap-ready");
        let input = root.path().join("bootstrap-input.json");
        fs::write(
            &input,
            serde_json::to_vec(
                &json!({"root":root.path(),"operation":"bootstrap","boundary":4,"signal":signal}),
            )
            .unwrap(),
        )
        .unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_FIXTURE_ENV, &input)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let limit = deadline();
        while !signal.exists() && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(2));
        }
        if !signal.exists() {
            let _ = child.kill();
            let status = child.wait().unwrap();
            panic!("bootstrap {iteration} failed before its seam: {status}");
        }
        child.kill().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(child.wait().unwrap().signal(), Some(9));
        assert_eq!(
            classify_direct_store_v1(direct.layout()).unwrap(),
            DirectStoreClassificationV1::Missing
        );
        let before = images(&direct.layout().runtime_directory());
        assert!(matches!(
            direct.read::<()>(deadline(), |_| panic!("unpublished read")),
            Err(DirectStoreErrorV1::NotInitialized)
        ));
        assert!(
            images(&direct.layout().runtime_directory()) == before,
            "ordinary read recovered a private publication"
        );
        let lease =
            ExclusiveMaintenanceLeaseV1::acquire(direct.layout(), deadline(), false).unwrap();
        let initialize = request("killed-state");
        direct
            .bootstrap(
                &lease,
                WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
                "generation-one",
                &initialize,
                deadline(),
                || UnixMillis::new(30),
                |transaction| {
                    let now = transaction.now();
                    transaction.create_graph(&prepared_graph(now))?;
                    Ok(success(
                        &initialize,
                        json!({"initialized":true}),
                        transaction.now(),
                    ))
                },
            )
            .unwrap();
        drop(lease);
        direct.verify_integrity(deadline()).unwrap();
        direct
            .read(deadline(), |snapshot| {
                assert_eq!(snapshot.outcome(initialize.key())?.unwrap().sequence(), 1);
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn private_publication_does_not_clobber_a_raced_destination() {
    let root = TempDir::new().unwrap();
    let direct = store(root.path());
    let path = direct.layout().database();
    let injected = path.clone();
    let direct = direct.with_boundary_observer(move |boundary| {
        if boundary == DirectTransactionBoundaryV1::BeforeMaintenancePublication {
            fs::write(&injected, b"retained foreign inode").unwrap();
        }
    });
    let lease = ExclusiveMaintenanceLeaseV1::acquire(direct.layout(), deadline(), true).unwrap();
    let initialize = request("no-clobber");
    let result = direct.bootstrap(
        &lease,
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        "generation-one",
        &initialize,
        deadline(),
        || UnixMillis::new(20),
        |transaction| {
            Ok(success(
                &initialize,
                json!({"initialized":true}),
                transaction.now(),
            ))
        },
    );
    assert!(matches!(result, Err(DirectStoreErrorV1::Storage(_))));
    assert_eq!(fs::read(path).unwrap(), b"retained foreign inode");
    assert!(!direct.layout().wal().exists());
    assert!(!direct.layout().shm().exists());
}

#[test]
fn a_real_controller_write_lock_returns_busy_within_deadline_without_effects() {
    let (root, direct) = fixture();
    let signal = root.path().join("lock-ready");
    let release = root.path().join("release-lock");
    let input = root.path().join("lock-input.json");
    fs::write(&input,serde_json::to_vec(&json!({"root":root.path(),"operation":"hold_write_lock","signal":signal,"release":release})).unwrap()).unwrap();
    let mut controller = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_FIXTURE_ENV, &input)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let limit = deadline();
    while !signal.exists() && Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(2));
    }
    if !signal.exists() {
        let _ = controller.kill();
        let _ = controller.wait();
        panic!("controller readiness deadline expired");
    }
    let main = fs::read(direct.layout().database()).unwrap();
    let wal = fs::read(direct.layout().wal()).unwrap();
    let started = Instant::now();
    let attempt = request("busy-controller");
    let rejected = direct.mutate(
        &attempt,
        started + Duration::from_millis(500),
        || panic!("busy has no clock"),
        |_| panic!("busy has no state effect"),
    );
    let elapsed = started.elapsed();
    fs::write(&release, b"release").unwrap();
    let status = controller.wait().unwrap();
    assert!(status.success());
    assert!(
        matches!(rejected, Err(DirectStoreErrorV1::Busy)),
        "{rejected:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(480) && elapsed <= Duration::from_millis(600),
        "{elapsed:?}"
    );
    assert_eq!(fs::read(direct.layout().database()).unwrap(), main);
    assert_eq!(fs::read(direct.layout().wal()).unwrap(), wal);
    direct
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.view(UnixMillis::new(50))?.sequence(), 1);
            assert!(snapshot.outcome(attempt.key())?.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn sigkill_at_each_transaction_boundary_preserves_effect_once_recovery() {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };
    use std::os::unix::process::ExitStatusExt;
    // Twenty real SIGKILLs at each of the four Store transaction seams.
    // The fifth reset/removal domain seam belongs to the engine's marker protocol.
    for boundary in 0..4 {
        for iteration in 0..20 {
            let (root, current) = fixture();
            let signal = root.path().join("child-ready");
            let input = root.path().join("child-input.json");
            fs::write(
                &input,
                serde_json::to_vec(&json!({"root":root.path(),
                "boundary":boundary,"signal":signal}))
                .unwrap(),
            )
            .unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD_TEST, "--nocapture"])
                .env(CHILD_FIXTURE_ENV, &input)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let limit = deadline();
            while !signal.exists() && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(2));
            }
            if !signal.exists() {
                let _ = child.kill();
                let status = child.wait().unwrap();
                panic!(
                    "owned child failed before rendezvous: boundary {boundary}, iteration {iteration}, status {status}"
                );
            }
            kill(
                Pid::from_raw(i32::try_from(child.id()).unwrap()),
                Signal::SIGKILL,
            )
            .unwrap();
            assert_eq!(child.wait().unwrap().signal(), Some(9));
            let reopened = store(root.path());
            let request = request("killed-state");
            reopened
                .read(deadline(), |snapshot| {
                    assert_eq!(snapshot.current_graph()?.is_some(), boundary == 3);
                    assert_eq!(snapshot.outcome(request.key())?.is_some(), boundary == 3);
                    Ok(())
                })
                .unwrap();
            let result = reopened
                .mutate(
                    &request,
                    deadline(),
                    || UnixMillis::new(30),
                    |transaction| {
                        let now = transaction.now();
                        transaction.create_graph(&prepared_graph(now))?;
                        Ok(success(
                            &request,
                            json!({"repaired":true}),
                            transaction.now(),
                        ))
                    },
                )
                .unwrap();
            assert_eq!(result.replayed(), boundary == 3);
            assert_eq!(result.outcome().sequence(), 2);
            current.verify_integrity(deadline()).unwrap();
        }
    }
}

#[test]
fn every_legacy_generation_and_marker_is_classified_without_changing_files() {
    use podway_store::test_support::v2dfc001::{LegacyHeaderV1, write_legacy_store_v1};
    for generation in 1..=13 {
        for header in [LegacyHeaderV1::Checkpointed, LegacyHeaderV1::WalOnly] {
            let root = TempDir::new().unwrap();
            let staging = TempDir::new().unwrap();
            let store = store(root.path());
            write_legacy_store_v1(
                staging.path(),
                &store.layout().runtime_directory(),
                generation,
                header,
                None,
            )
            .unwrap();
            let before = images(&store.layout().runtime_directory());
            assert_eq!(
                classify_direct_store_v1(store.layout()).unwrap(),
                DirectStoreClassificationV1::Legacy {
                    generation: Some(generation),
                    marker: None
                }
            );
            assert!(matches!(
                store.read(deadline(), |_| Ok(())),
                Err(DirectStoreErrorV1::Legacy { .. })
            ));
            assert!(matches!(
                store.mutate(
                    &request("legacy"),
                    deadline(),
                    || panic!("no legacy clock"),
                    |_| panic!("no legacy mutation")
                ),
                Err(DirectStoreErrorV1::Legacy { .. })
            ));
            assert_eq!(images(&store.layout().runtime_directory()), before);
        }
    }
    for marker in [
        "reset.marker",
        "workspace-removal.marker",
        "development-v2.marker",
        "mode-switch.json",
    ] {
        let root = TempDir::new().unwrap();
        let store = store(root.path());
        fs::create_dir_all(store.layout().runtime_directory()).unwrap();
        let path = if marker == "mode-switch.json" {
            store.layout().podway_directory().join(marker)
        } else {
            store.layout().runtime_directory().join(marker)
        };
        fs::write(&path, b"legacy marker text").unwrap();
        let before = fs::read(&path).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(
            classify_direct_store_v1(store.layout()).unwrap(),
            DirectStoreClassificationV1::Legacy {
                generation: None,
                marker: Some(marker)
            }
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().ino(), metadata.ino());
    }
}

#[test]
fn legacy_store_rejects_direct_generation_fourteen_as_newer() {
    let (_root, direct) = fixture();
    let identity = direct
        .read(deadline(), |snapshot| {
            Ok(snapshot.binding().identity().clone())
        })
        .unwrap();
    let legacy = podway_store::SqliteStoreV1::open(
        direct.layout().database(),
        direct.layout().root(),
        identity,
        podway_store::SqliteStoreOptionsV1::new(1000).unwrap(),
        UnixMillis::new(30),
    );
    assert!(matches!(
        legacy,
        Err(podway_store::StoreErrorV1::NewerStateV1 { .. })
    ));
}

#[test]
fn cold_read_without_shm_uses_a_write_free_heap_snapshot() {
    let (_root, direct) = fixture();
    fs::remove_file(direct.layout().shm()).unwrap();
    let before = images(&direct.layout().runtime_directory());
    direct
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.view(UnixMillis::new(20))?.sequence(), 1);
            assert_eq!(
                snapshot.outcome(request("init").key())?.unwrap().sequence(),
                1
            );
            Ok(())
        })
        .unwrap();
    direct.verify_integrity(deadline()).unwrap();
    assert_eq!(images(&direct.layout().runtime_directory()), before);
}

#[test]
fn cold_read_with_an_uninitialized_existing_shm_is_write_free() {
    let (root, direct) = fixture();
    let signal = root.path().join("child-ready");
    let input = root.path().join("child-input.json");
    fs::write(
        &input,
        serde_json::to_vec(&json!({"root":root.path(),"boundary":3,"signal":signal})).unwrap(),
    )
    .unwrap();
    let mut writer = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_FIXTURE_ENV, &input)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let limit = deadline();
    while !signal.exists() && Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(2));
    }
    if !signal.exists() {
        let _ = writer.kill();
        let _ = writer.wait();
        panic!("writer rendezvous failed");
    }
    // Preserve a committed WAL that has not reached the checkpoint boundary.
    writer.kill().unwrap();
    writer.wait().unwrap();
    let size = fs::metadata(direct.layout().shm()).unwrap().len();
    fs::write(direct.layout().shm(), vec![0; size as usize]).unwrap();
    let before = images(&direct.layout().runtime_directory());
    direct
        // This checks write-free recovery, not open latency. Other parallel tests
        // can own the process-wide connection gate; use the fixture's normal bound.
        .read(deadline(), |snapshot| {
            assert_eq!(
                snapshot
                    .outcome(request("killed-state").key())?
                    .unwrap()
                    .sequence(),
                2
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(images(&direct.layout().runtime_directory()), before);
}

#[test]
fn hot_wal_read_snapshot_is_write_free_with_an_open_writer_process() {
    let (root, direct) = fixture();
    let signal = root.path().join("child-ready");
    let input = root.path().join("child-input.json");
    fs::write(
        &input,
        serde_json::to_vec(&json!({"root":root.path(),"boundary":3,"signal":signal})).unwrap(),
    )
    .unwrap();
    let mut writer = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_FIXTURE_ENV, &input)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let limit = deadline();
    while !signal.exists() && Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(2));
    }
    if !signal.exists() {
        let _ = writer.kill();
        let _ = writer.wait();
        panic!("writer rendezvous failed");
    }
    let before = images(&direct.layout().runtime_directory());
    let result = direct.read(deadline(), |snapshot| {
        assert_eq!(snapshot.view(UnixMillis::new(50))?.sequence(), 2);
        Ok(())
    });
    // Reap the exact owned writer even when the read returns a failure.
    writer.kill().unwrap();
    writer.wait().unwrap();
    result.unwrap();
    assert!(
        images(&direct.layout().runtime_directory()) == before,
        "read changed an authoritative file"
    );
}

#[test]
fn prepared_graph_is_fenced_before_clock_and_replay_bypasses_stale_preparation() {
    use podway_core::AttemptId;
    let (_root, direct) = fixture();
    let seed = request("seed-prepared");
    direct
        .mutate(
            &seed,
            deadline(),
            || UnixMillis::new(20),
            |transaction| {
                let now = transaction.now();
                transaction.create_graph(&prepared_graph(now))?;
                Ok(success(&seed, json!({"seeded":true}), transaction.now()))
            },
        )
        .unwrap();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    let begin = request("begin-prepared");
    let changed = direct
        .mutate(
            &begin,
            deadline(),
            || UnixMillis::new(30),
            |transaction| {
                transaction.use_prepared_graph(&prepared)?;
                let now = transaction.now();
                let prior = prepared.graph();
                let next = prior
                    .begin_v2(
                        Revision::ZERO,
                        AttemptId::new("00000000-0000-4000-8000-000000000005").unwrap(),
                        None,
                        now,
                    )?
                    .into_state();
                transaction.replace_graph(
                    prior.workspace_revision(),
                    prior.trace().revision(),
                    &next,
                )?;
                Ok(success(&begin, json!({"begun":true}), transaction.now()))
            },
        )
        .unwrap();
    let replay = direct
        .mutate(
            &begin,
            deadline(),
            || panic!("no replay clock"),
            |transaction| {
                transaction.use_prepared_graph(&prepared)?;
                panic!("no replay callback")
            },
        )
        .unwrap();
    assert!(replay.replayed());
    assert_eq!(replay.outcome(), changed.outcome());
    let stale = request("stale-prepared");
    let rejected = direct.mutate(
        &stale,
        deadline(),
        || panic!("stale preparation has no clock"),
        |transaction| {
            transaction.use_prepared_graph(&prepared)?;
            panic!("stale preparation cannot execute")
        },
    );
    assert!(matches!(
        rejected,
        Err(DirectStoreErrorV1::PreparedStateChanged {
            expected_sequence: 2,
            actual_sequence: 3
        })
    ));
    direct
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(stale.key())?.is_none());
            Ok(())
        })
        .unwrap();
    let fresh = direct.prepare_graph(deadline()).unwrap().unwrap();
    assert_eq!(fresh.graph().trace().revision(), Revision::new(1));
}

#[test]
fn prepared_graph_cannot_cross_store_root_or_reuse_after_clock() {
    let (_first_root, first) = fixture();
    let (_second_root, second) = fixture();
    for direct in [&first, &second] {
        let seed = request("same-seed");
        direct
            .mutate(
                &seed,
                deadline(),
                || UnixMillis::new(20),
                |transaction| {
                    let now = transaction.now();
                    transaction.create_graph(&prepared_graph(now))?;
                    Ok(success(&seed, json!({"seeded":true}), transaction.now()))
                },
            )
            .unwrap();
    }
    let prepared = first.prepare_graph(deadline()).unwrap().unwrap();
    let rejected = second.mutate(
        &request("other-root"),
        deadline(),
        || panic!("foreign token has no clock"),
        |transaction| {
            transaction.use_prepared_graph(&prepared)?;
            panic!("foreign token cannot execute")
        },
    );
    assert!(matches!(
        rejected,
        Err(DirectStoreErrorV1::PreparedStateChanged { .. })
    ));
    let rejected = first.mutate(
        &request("late-token"),
        deadline(),
        || UnixMillis::new(30),
        |transaction| {
            transaction.now();
            transaction.use_prepared_graph(&prepared)?;
            panic!("late token cannot execute")
        },
    );
    assert!(matches!(
        rejected,
        Err(DirectStoreErrorV1::InvalidInput { .. })
    ));
}
