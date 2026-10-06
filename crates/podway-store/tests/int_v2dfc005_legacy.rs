//! Freeze the last legacy producer before the daemon/job implementation is removed.

use super::int_v2dfc005_direct_store::{deadline, request, store};
use podway_core::UnixMillis;
use podway_store::direct_store::{
    DirectStoreClassificationV1, DirectStoreErrorV1, classify_direct_store_v1,
};
use podway_store::{StoreGraphMutationContractV2, StoreGraphStateContractV2};
use std::{fs, path::Path};
use tempfile::TempDir;

const RECIPE: &str =
    include_str!("../../../tests/fixtures/storage/daemon-era-v2dfc/generation-13-populated.sql");
const MARKERS: &str = include_str!("../../../tests/fixtures/storage/daemon-era-v2dfc/markers.json");

#[test]
fn frozen_populated_legacy_recipe_and_every_marker_remain_byte_identical() {
    use std::os::unix::fs::MetadataExt;
    for checkpointed in [false, true] {
        for selected in 0..6 {
            let root = TempDir::new().unwrap();
            let staging = TempDir::new().unwrap();
            let direct = store(root.path());
            let connection =
                rusqlite::Connection::open(staging.path().join("state.sqlite3")).unwrap();
            connection
                .execute_batch("PRAGMA wal_autocheckpoint=0;")
                .unwrap();
            connection.execute_batch(RECIPE).unwrap();
            for table in ["jobs", "v2_delegation_receipts", "v2_task_sessions"] {
                let count: u64 = connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(count, 1);
            }
            if checkpointed {
                connection
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .unwrap();
            }
            connection
                .execute_batch("UPDATE workspace_state SET updated_at_ms=8 WHERE singleton=1;")
                .unwrap();
            fs::create_dir_all(direct.layout().runtime_directory()).unwrap();
            for suffix in ["", "-wal"] {
                fs::copy(
                    staging.path().join(format!("state.sqlite3{suffix}")),
                    direct
                        .layout()
                        .runtime_directory()
                        .join(format!("state.sqlite3{suffix}")),
                )
                .unwrap();
            }
            let markers: Vec<serde_json::Value> = serde_json::from_str(MARKERS).unwrap();
            let mut files = vec![direct.layout().database(), direct.layout().wal()];
            for (index, marker) in markers.iter().enumerate() {
                if selected == index + 1 || selected == 5 {
                    let path = root.path().join(marker["relative_path"].as_str().unwrap());
                    fs::write(&path, marker["bytes_utf8"].as_str().unwrap()).unwrap();
                    files.push(path);
                }
            }
            let before: Vec<_> = files
                .iter()
                .map(|path| {
                    (
                        fs::read(path).unwrap(),
                        fs::metadata(path).unwrap().dev(),
                        fs::metadata(path).unwrap().ino(),
                    )
                })
                .collect();
            assert!(matches!(
                classify_direct_store_v1(direct.layout()).unwrap(),
                DirectStoreClassificationV1::Legacy { .. }
            ));
            assert!(matches!(
                direct.read::<()>(deadline(), |_| panic!("legacy read")),
                Err(DirectStoreErrorV1::Legacy { .. })
            ));
            assert!(matches!(
                direct.mutate(
                    &request("legacy"),
                    deadline(),
                    || panic!("legacy clock"),
                    |_| panic!("legacy write")
                ),
                Err(DirectStoreErrorV1::Legacy { .. })
            ));
            for (path, (bytes, dev, ino)) in files.iter().zip(before) {
                assert_eq!(fs::read(path).unwrap(), bytes);
                assert_eq!(fs::metadata(path).unwrap().dev(), dev);
                assert_eq!(fs::metadata(path).unwrap().ino(), ino);
            }
            assert!(!direct.layout().shm().exists());
        }
    }
}

#[test]
#[ignore = "one-time explicit V2DFC-005 legacy text-recipe producer"]
fn export_generation_thirteen_with_a_completed_job_and_delegation_receipt() {
    let output =
        std::env::var_os("PODWAY_LEGACY_RECIPE_EXPORT").expect("explicit owned output directory");
    let output = Path::new(&output);
    assert!(output.is_absolute() && output.is_dir());
    let root = TempDir::new().unwrap();
    let legacy = super::int_v2sdg004_receipt::open(&root, None);
    let graph = super::int_v2sdg004_receipt::session_state();
    legacy
        .create_graph_session_v2(&super::int_v2sdg004_receipt::identity(), graph.clone())
        .unwrap();
    let job = super::int_v2sdg004_receipt::admit_and_claim(&legacy, &graph);
    let next = graph
        .accept_delegation_successor_v1(graph.trace().active_attempt().unwrap().attempt_id())
        .unwrap();
    legacy
        .commit_graph_delegation_accept_terminal_v1(
            job.claim().clone(),
            graph.workspace_revision(),
            graph.trace().revision(),
            next,
            super::int_v2sdg004_receipt::digest('d'),
            super::int_v2sdg004_receipt::receipt(&graph),
            UnixMillis::new(7),
        )
        .unwrap();
    drop(legacy);
    // The producer's supported close checkpoints its actual Store. The tracked
    // fixture is a SQL/data recipe, not this machine-local database image.
    let source = root.path().join("state.sqlite3");
    let connection =
        rusqlite::Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 13);
    for table in ["jobs", "v2_delegation_receipts", "v2_task_sessions"] {
        let count: u64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }
    drop(connection);
    fs::copy(source, output.join("legacy-generation-13.sqlite3")).unwrap();
}
