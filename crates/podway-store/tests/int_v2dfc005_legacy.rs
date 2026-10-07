//! Frozen rejected legacy inputs survive production daemon-state retirement.

use super::int_v2dfc005_direct_store::{deadline, request, store};
use podway_store::direct_store::{
    DirectStoreClassificationV1, DirectStoreErrorV1, classify_direct_store_v1,
};
use std::fs;
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
