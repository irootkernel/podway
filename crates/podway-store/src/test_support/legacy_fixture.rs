//! Frozen rejected daemon-era inputs for direct Store qualification.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

/// How a legacy fixture's generation reaches the database header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyHeaderV1 {
    /// The generation is checkpointed into the main header before later WAL frames.
    Checkpointed,
    /// The generation exists only in the WAL's committed page-one image.
    WalOnly,
}

/// Materializes the frozen generation's main file and hot WAL without SHM.
pub fn write_legacy_store_v1(
    staging_directory: &Path,
    runtime_directory: &Path,
    generation: u32,
    header: LegacyHeaderV1,
) -> Result<(), String> {
    if !(1..=13).contains(&generation) {
        return Err("legacy fixture generation is outside 1 to 13".to_owned());
    }
    let staging = staging_directory.join("state.sqlite3");
    for path in [
        staging.clone(),
        sidecar(&staging, "-wal"),
        sidecar(&staging, "-shm"),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let connection = Connection::open(&staging).map_err(|error| error.to_string())?;
    connection
        .query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .map_err(|error| error.to_string())?;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/storage/daemon-era-v2dfc/generation-1-to-13-ddl.json"
    ))
    .map_err(|error| error.to_string())?;
    for migration in fixture["migrations"]
        .as_array()
        .ok_or("legacy fixture migrations are missing")?
        .iter()
        .take(generation as usize)
    {
        connection
            .execute_batch(
                migration["sql_utf8"]
                    .as_str()
                    .ok_or("legacy fixture migration SQL is missing")?,
            )
            .map_err(|error| error.to_string())?;
    }
    if connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(|error| error.to_string())?
        != generation
    {
        return Err("legacy fixture generation was not applied".to_owned());
    }
    if header == LegacyHeaderV1::Checkpointed {
        let busy: i64 = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if busy != 0 {
            return Err("legacy fixture checkpoint was busy".to_owned());
        }
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS v2dfc001_hot_wal (value INTEGER NOT NULL);
             INSERT INTO v2dfc001_hot_wal (value) VALUES (1), (2), (3);",
        )
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(runtime_directory).map_err(|error| error.to_string())?;
    let target = runtime_directory.join("state.sqlite3");
    let main = fs::read(&staging).map_err(|error| error.to_string())?;
    let wal = fs::read(sidecar(&staging, "-wal")).map_err(|error| error.to_string())?;
    fs::write(&target, &main).map_err(|error| error.to_string())?;
    fs::write(sidecar(&target, "-wal"), &wal).map_err(|error| error.to_string())?;
    if fs::read(&target).map_err(|error| error.to_string())? != main
        || fs::read(sidecar(&target, "-wal")).map_err(|error| error.to_string())? != wal
    {
        return Err("legacy fixture images do not match their source".to_owned());
    }
    drop(connection);
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}
