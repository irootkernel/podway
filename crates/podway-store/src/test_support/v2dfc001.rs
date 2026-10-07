//! `V2DFC-001` feasibility prototype for the daemon-free direct-write model.
//!
//! These test-only primitives commit one Procedure v2 mutation together with its success
//! outcome in a single IMMEDIATE transaction over the crate's relational helpers. They also
//! provide the write-free classification, the cheap open, and the process-independent
//! maintenance protocol that the feasibility gate measures. A prototype Store keeps the
//! generation-13 relational DDL, adds two prototype tables, and is marked generation 14 so that
//! classification behaves as designed. `V2DFC-007` deletes this module.

use std::cell::Cell;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use podway_core::{AttemptId, GraphNodeId, ItemId, Revision, SessionId, Sha256Digest, UnixMillis};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::schema::{apply_connection_pragmas_v1, verify_workspace_identity_v1};
use crate::v2_delegation::receipt_snapshot_connection_v1;
use crate::v2_state::{
    load_graph_session_connection_v2, load_terminal_dispositions_connection_v2,
    replace_graph_session_transaction_v2, terminal_disposition_is_current_v2,
};
use crate::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, DurableWorktreeIdentityV1,
    GraphMutationErrorV2, GraphSessionStateV2, GraphWorkspaceViewV2, ValidatedWorkspaceRootV1,
};

/// Header generation that marks a prototype Store as current.
const PROTOTYPE_GENERATION_V1: u32 = 14;
const STATE_DATABASE_FILE_NAME_V1: &str = "state.sqlite3";
const RESET_MARKER_FILE_NAME_V1: &str = "v2dfc001-reset.marker";
const REMOVAL_MARKER_FILE_NAME_V1: &str = "v2dfc001-removal.marker";
/// Daemon-era markers under `.podway/runtime/` that classification rejects as legacy state.
pub const LEGACY_RUNTIME_MARKERS_V1: [&str; 3] = [
    "reset.marker",
    "workspace-removal.marker",
    "development-v2.marker",
];
/// Daemon-era marker under `.podway/` that classification rejects as legacy state.
pub const LEGACY_PODWAY_MARKER_V1: &str = "mode-switch.json";
/// WAL size above which a closing writer truncates the WAL.
const CLOSE_CHECKPOINT_THRESHOLD_BYTES_V1: u64 = 4 * 1024 * 1024;

const LOCK_FILE_NAME: &str = "v2dfc001.lock";
const INTENT_FILE_NAME: &str = "v2dfc001.intent";
const SEED_TEMPORARY_FILE_NAME: &str = ".v2dfc001-seed.tmp";
const MARKER_TEMPORARY_SUFFIX: &str = ".tmp";
const RESET_MARKER_SCHEMA: &str = "podway.v2dfc001-reset-marker/v1";
const REMOVAL_MARKER_SCHEMA: &str = "podway.v2dfc001-removal-marker/v1";
const SQLITE_HEADER_BYTES: usize = 100;
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const WAL_HEADER_BYTES: usize = 32;
const WAL_FRAME_HEADER_BYTES: usize = 24;
const MAX_CLASSIFIED_WAL_BYTES: u64 = 512 * 1024 * 1024;
const JOURNAL_SIZE_LIMIT_BYTES: i64 = 4 * 1024 * 1024;
const JOURNAL_PROTECTED_COUNT: i64 = 200;
const JOURNAL_ABSOLUTE_COUNT: i64 = 10_000;
const JOURNAL_MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const LOCK_RETRY_SLEEP: Duration = Duration::from_micros(500);
const CLOSE_CHECKPOINT_WAIT: Duration = Duration::from_millis(100);
const BUSY_RETRY_BASE_MICROS: u64 = 100;
const BUSY_RETRY_JITTER_MICROS: u64 = 400;
const MAX_PRAGMA_BUSY_TIMEOUT_MS: u128 = 5_000;
const NEXT_WORKSPACE_SEQUENCE_SQL: &str = "UPDATE workspace_state \
     SET next_workspace_sequence = next_workspace_sequence + 1, \
     updated_at_ms = MAX(updated_at_ms, ?1) WHERE singleton = 1 RETURNING next_workspace_sequence";
const PROTOTYPE_DDL: &str = "
CREATE TABLE v2dfc001_writer (
    singleton        INTEGER PRIMARY KEY CHECK (singleton = 1),
    writer_identity  TEXT NOT NULL,
    epoch            TEXT NOT NULL,
    schema_digest    TEXT NOT NULL
) STRICT;
CREATE TABLE v2dfc001_outcomes (
    idempotency_key         TEXT PRIMARY KEY,
    request_digest          TEXT NOT NULL,
    canonical_request       TEXT NOT NULL,
    workspace_sequence      INTEGER NOT NULL UNIQUE,
    session_revision_after  INTEGER NOT NULL,
    response_json           TEXT NOT NULL,
    recorded_at_ms          INTEGER NOT NULL
) STRICT;
";

static EPOCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static BUSY_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static BUSY_JITTER_STATE: Cell<u64> = const { Cell::new(0) };
}

/// One failure of the prototype, named after the daemon-free error catalog where one exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrototypeErrorV1 {
    WorkspaceBusy,
    WorkspaceMaintenance,
    WorkspaceNotInitialized,
    LegacyRuntimeState {
        generation: Option<u32>,
        marker: Option<&'static str>,
    },
    NewerRuntimeState {
        generation: u32,
    },
    UnrecognizedRuntimeState,
    WriterMismatch,
    BindingMismatch,
    SchemaMismatch,
    IdempotencyKeyReused,
    Rejected {
        reason: String,
    },
    ArtifactChanged,
    Storage {
        reason: String,
    },
}

impl PrototypeErrorV1 {
    pub fn code(&self) -> &'static str {
        match self {
            Self::WorkspaceBusy => "WORKSPACE_BUSY",
            Self::WorkspaceMaintenance => "WORKSPACE_MAINTENANCE",
            Self::WorkspaceNotInitialized => "WORKSPACE_NOT_INITIALIZED",
            Self::LegacyRuntimeState { .. } => "LEGACY_RUNTIME_STATE_UNSUPPORTED",
            Self::NewerRuntimeState { .. } | Self::SchemaMismatch => "WORKSPACE_SCHEMA_UNSUPPORTED",
            Self::UnrecognizedRuntimeState => "WORKSPACE_STATE_UNREADABLE",
            Self::WriterMismatch => "WORKSPACE_CONTRACT_MISMATCH",
            Self::BindingMismatch => "WORKSPACE_UUID_MISMATCH",
            Self::IdempotencyKeyReused => "IDEMPOTENCY_KEY_REUSED",
            Self::Rejected { .. } => "PRECONDITION_FAILED",
            Self::ArtifactChanged => "ARTIFACT_CHANGED",
            Self::Storage { .. } => "INTERNAL_ERROR",
        }
    }

    /// The read-only recovery recipe a caller would print with the error.
    pub fn recipe(&self) -> Option<&'static str> {
        match self {
            Self::LegacyRuntimeState { .. } => Some("podway reset --all --yes"),
            Self::WorkspaceBusy => Some("retry the same command with the same idempotency key"),
            Self::WorkspaceMaintenance => {
                Some("retry after maintenance completes, or resume it with its exact command")
            }
            _ => None,
        }
    }
}

fn storage(error: impl std::fmt::Display) -> PrototypeErrorV1 {
    PrototypeErrorV1::Storage {
        reason: error.to_string(),
    }
}

fn sqlite(error: rusqlite::Error) -> PrototypeErrorV1 {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            PrototypeErrorV1::WorkspaceBusy
        }
        _ => storage(error),
    }
}

fn rejected(error: GraphMutationErrorV2) -> PrototypeErrorV1 {
    PrototypeErrorV1::Rejected {
        reason: format!("{error:?}"),
    }
}

/// Worktree-local paths used by the prototype.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceLayoutV1 {
    root: PathBuf,
}

impl WorkspaceLayoutV1 {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn podway_directory(&self) -> PathBuf {
        self.root.join(".podway")
    }

    pub fn runtime_directory(&self) -> PathBuf {
        self.root.join(".podway/runtime")
    }

    pub fn database(&self) -> PathBuf {
        self.runtime_directory().join(STATE_DATABASE_FILE_NAME_V1)
    }

    pub fn wal(&self) -> PathBuf {
        sidecar(&self.database(), "-wal")
    }

    pub fn shm(&self) -> PathBuf {
        sidecar(&self.database(), "-shm")
    }

    fn reset_marker(&self) -> PathBuf {
        self.runtime_directory().join(RESET_MARKER_FILE_NAME_V1)
    }

    fn removal_marker(&self) -> PathBuf {
        self.runtime_directory().join(REMOVAL_MARKER_FILE_NAME_V1)
    }

    fn lock_file(&self) -> PathBuf {
        self.runtime_directory().join(LOCK_FILE_NAME)
    }

    fn intent_file(&self) -> PathBuf {
        self.runtime_directory().join(INTENT_FILE_NAME)
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}

/// Write-free classification of one worktree's runtime state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreClassificationV1 {
    Current,
    Missing,
    Legacy {
        generation: Option<u32>,
        marker: Option<&'static str>,
    },
    Newer {
        generation: u32,
    },
    Unrecognized,
}

/// Classifies runtime state from file headers and marker names without opening SQLite.
///
/// The main database header is authoritative once a generation is checkpointed into it. A
/// header generation of zero, or a main file too short to hold a header, falls back to the
/// newest committed page-one image in the WAL, which a never-checkpointed Store holds alone.
pub fn classify_store_v1(
    layout: &WorkspaceLayoutV1,
) -> Result<StoreClassificationV1, PrototypeErrorV1> {
    let runtime = layout.runtime_directory();
    for marker in LEGACY_RUNTIME_MARKERS_V1 {
        if path_exists(&runtime.join(marker))? {
            return Ok(StoreClassificationV1::Legacy {
                generation: None,
                marker: Some(marker),
            });
        }
    }
    if path_exists(&layout.podway_directory().join(LEGACY_PODWAY_MARKER_V1))? {
        return Ok(StoreClassificationV1::Legacy {
            generation: None,
            marker: Some(LEGACY_PODWAY_MARKER_V1),
        });
    }
    let header = match read_prefix(&layout.database(), SQLITE_HEADER_BYTES) {
        Ok(header) => header,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(StoreClassificationV1::Missing);
        }
        Err(error) => return Err(storage(error)),
    };
    let mut generation = 0;
    if header.len() == SQLITE_HEADER_BYTES {
        if &header[..16] != SQLITE_MAGIC {
            return Ok(StoreClassificationV1::Unrecognized);
        }
        generation = be_u32(&header[60..64]);
    }
    if generation == 0 {
        generation = wal_committed_page_one_generation(&layout.wal())?.unwrap_or(0);
    }
    Ok(match generation {
        PROTOTYPE_GENERATION_V1 => StoreClassificationV1::Current,
        0 => StoreClassificationV1::Unrecognized,
        generation if generation < PROTOTYPE_GENERATION_V1 => StoreClassificationV1::Legacy {
            generation: Some(generation),
            marker: None,
        },
        generation => StoreClassificationV1::Newer { generation },
    })
}

fn classification_error(classification: StoreClassificationV1) -> Option<PrototypeErrorV1> {
    match classification {
        StoreClassificationV1::Current => None,
        StoreClassificationV1::Missing => Some(PrototypeErrorV1::WorkspaceNotInitialized),
        StoreClassificationV1::Legacy { generation, marker } => {
            Some(PrototypeErrorV1::LegacyRuntimeState { generation, marker })
        }
        StoreClassificationV1::Newer { generation } => {
            Some(PrototypeErrorV1::NewerRuntimeState { generation })
        }
        StoreClassificationV1::Unrecognized => Some(PrototypeErrorV1::UnrecognizedRuntimeState),
    }
}

fn path_exists(path: &Path) -> Result<bool, PrototypeErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage(error)),
    }
}

fn read_prefix(path: &Path, length: usize) -> io::Result<Vec<u8>> {
    let mut prefix = Vec::with_capacity(length);
    File::open(path)?
        .take(length as u64)
        .read_to_end(&mut prefix)?;
    Ok(prefix)
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Returns the generation in the newest page-one image of the WAL's committed prefix.
fn wal_committed_page_one_generation(wal: &Path) -> Result<Option<u32>, PrototypeErrorV1> {
    let mut file = match File::open(wal) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage(error)),
    };
    let length = file.metadata().map_err(storage)?.len();
    if length > MAX_CLASSIFIED_WAL_BYTES {
        return Err(PrototypeErrorV1::UnrecognizedRuntimeState);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).map_err(storage)?);
    file.read_to_end(&mut bytes).map_err(storage)?;
    Ok(committed_page_one(&bytes).map(|page| be_u32(&page[60..64])))
}

fn committed_page_one(wal: &[u8]) -> Option<&[u8]> {
    if wal.len() < WAL_HEADER_BYTES {
        return None;
    }
    let magic = be_u32(&wal[0..4]);
    if magic != 0x377f_0682 && magic != 0x377f_0683 {
        return None;
    }
    let big_endian = magic & 1 == 1;
    let page_size = usize::try_from(be_u32(&wal[8..12])).ok()?;
    if !page_size.is_power_of_two() || !(512..=65_536).contains(&page_size) {
        return None;
    }
    let header_checksum = wal_checksum(big_endian, (0, 0), &wal[0..24]);
    if header_checksum != (be_u32(&wal[24..28]), be_u32(&wal[28..32])) {
        return None;
    }
    let salts = &wal[16..24];
    let frame_size = WAL_FRAME_HEADER_BYTES + page_size;
    let mut checksum = header_checksum;
    let mut latest_page_one = None;
    let mut committed_page_one = None;
    let mut offset = WAL_HEADER_BYTES;
    while offset + frame_size <= wal.len() {
        let frame = &wal[offset..offset + frame_size];
        if &frame[8..16] != salts {
            break;
        }
        checksum = wal_checksum(big_endian, checksum, &frame[0..8]);
        checksum = wal_checksum(big_endian, checksum, &frame[WAL_FRAME_HEADER_BYTES..]);
        if checksum != (be_u32(&frame[16..20]), be_u32(&frame[20..24])) {
            break;
        }
        if be_u32(&frame[0..4]) == 1 {
            latest_page_one = Some(&frame[WAL_FRAME_HEADER_BYTES..]);
        }
        if be_u32(&frame[4..8]) != 0 {
            committed_page_one = latest_page_one.or(committed_page_one);
        }
        offset += frame_size;
    }
    committed_page_one
}

fn wal_checksum(big_endian: bool, seed: (u32, u32), data: &[u8]) -> (u32, u32) {
    let (mut first, mut second) = seed;
    for chunk in data.chunks_exact(8) {
        let word = |bytes: &[u8]| {
            let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if big_endian {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        };
        first = first.wrapping_add(word(&chunk[0..4])).wrapping_add(second);
        second = second.wrapping_add(word(&chunk[4..8])).wrapping_add(first);
    }
    (first, second)
}

/// A shared hold on the maintenance lock that an ordinary open keeps until it closes.
#[derive(Debug)]
struct SharedLockV1 {
    _lock: File,
}

/// An exclusive hold on the intent and maintenance locks.
#[derive(Debug)]
struct ExclusiveLockV1 {
    _intent: File,
    _lock: File,
}

/// Takes the maintenance lock shared within the deadline.
///
/// A pending exclusive request holds the intent lock, so new shared holders wait behind it and
/// macOS `flock`, which has no writer preference, cannot starve maintenance.
fn acquire_shared_lock_v1(
    layout: &WorkspaceLayoutV1,
    deadline: Instant,
) -> Result<SharedLockV1, PrototypeErrorV1> {
    loop {
        let intent = open_lock_file(&layout.intent_file(), false)?;
        if !try_lock(&intent, false)? {
            wait_or_maintenance(deadline)?;
            continue;
        }
        if !same_file(&intent, &layout.intent_file())? {
            continue;
        }
        let lock = open_lock_file(&layout.lock_file(), false)?;
        if !try_lock(&lock, false)? {
            wait_or_maintenance(deadline)?;
            continue;
        }
        if !same_file(&lock, &layout.lock_file())? {
            continue;
        }
        drop(intent);
        return Ok(SharedLockV1 { _lock: lock });
    }
}

/// Takes the intent lock and then the maintenance lock exclusively within the deadline.
fn acquire_exclusive_lock_v1(
    layout: &WorkspaceLayoutV1,
    deadline: Instant,
    create: bool,
) -> Result<ExclusiveLockV1, PrototypeErrorV1> {
    loop {
        let intent = open_lock_file(&layout.intent_file(), create)?;
        while !try_lock(&intent, true)? {
            wait_or_maintenance(deadline)?;
        }
        if !same_file(&intent, &layout.intent_file())? {
            continue;
        }
        let lock = open_lock_file(&layout.lock_file(), create)?;
        while !try_lock(&lock, true)? {
            wait_or_maintenance(deadline)?;
        }
        if !same_file(&lock, &layout.lock_file())? {
            continue;
        }
        return Ok(ExclusiveLockV1 {
            _intent: intent,
            _lock: lock,
        });
    }
}

fn open_lock_file(path: &Path, create: bool) -> Result<File, PrototypeErrorV1> {
    let opened = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .mode(0o600)
        .open(path);
    match opened {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(PrototypeErrorV1::WorkspaceNotInitialized)
        }
        Err(error) => Err(storage(error)),
    }
}

fn try_lock(file: &File, exclusive: bool) -> Result<bool, PrototypeErrorV1> {
    let attempt = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    match attempt {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(storage(error)),
    }
}

fn wait_or_maintenance(deadline: Instant) -> Result<(), PrototypeErrorV1> {
    if Instant::now() >= deadline {
        return Err(PrototypeErrorV1::WorkspaceMaintenance);
    }
    thread::sleep(LOCK_RETRY_SLEEP);
    Ok(())
}

fn same_file(file: &File, path: &Path) -> Result<bool, PrototypeErrorV1> {
    let held = file.metadata().map_err(storage)?;
    match fs::metadata(path) {
        Ok(current) => Ok(held.dev() == current.dev() && held.ino() == current.ino()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage(error)),
    }
}

/// The identities an open must match before it reads or writes the Store.
///
/// Git resolution supplies the worktree fingerprints and root. The workspace UUID is the
/// Store's own worktree-local authority, so the open reads it from the bound row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreBindingV1 {
    pub common_dir_identity: Sha256Digest,
    pub worktree_admin_identity: Sha256Digest,
    pub root: ValidatedWorkspaceRootV1,
    pub writer_identity: String,
}

/// Matches the bound workspace row against the resolved worktree and returns its identity.
fn verify_binding(
    connection: &Connection,
    binding: &StoreBindingV1,
) -> Result<DurableWorktreeIdentityV1, PrototypeErrorV1> {
    let (workspace_uuid, common, worktree, root) = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
             last_validated_root FROM workspace_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(sqlite)?;
    if common != binding.common_dir_identity.as_str()
        || worktree != binding.worktree_admin_identity.as_str()
        || root != binding.root.as_encoded()
    {
        return Err(PrototypeErrorV1::BindingMismatch);
    }
    let identity = DurableWorktreeIdentityV1::new(
        binding.common_dir_identity.clone(),
        crate::WorkspaceUuidV1::new(workspace_uuid).map_err(storage)?,
        binding.worktree_admin_identity.clone(),
    );
    verify_workspace_identity_v1(connection, &identity)
        .map_err(|_| PrototypeErrorV1::BindingMismatch)?;
    Ok(identity)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenModeV1 {
    ReadOnly,
    Mutation,
}

/// One cheaply opened prototype Store connection and the shared lock it holds.
pub struct OpenStoreV1 {
    connection: Connection,
    mode: OpenModeV1,
    epoch: String,
    identity: DurableWorktreeIdentityV1,
    wal: PathBuf,
    _lock: SharedLockV1,
}

impl OpenStoreV1 {
    /// The Store generation epoch observed under the shared lock.
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// The durable identity verified by the open.
    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    /// Disables automatic checkpoints so fixture writes accumulate in a hot WAL.
    pub fn disable_auto_checkpoint(&self) -> Result<(), PrototypeErrorV1> {
        self.connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .map_err(sqlite)
    }
}

/// A closing writer that finds the WAL over its bound truncates it, waiting briefly for readers
/// to leave. Read-only opens never checkpoint, because reads must not write.
impl Drop for OpenStoreV1 {
    fn drop(&mut self) {
        if self.mode != OpenModeV1::Mutation {
            return;
        }
        let large = fs::metadata(&self.wal)
            .is_ok_and(|metadata| metadata.len() > CLOSE_CHECKPOINT_THRESHOLD_BYTES_V1);
        if large {
            set_busy_deadline(Some(Instant::now() + CLOSE_CHECKPOINT_WAIT));
            let _ = self
                .connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        }
    }
}

/// Opens the Store after classification, the shared lock, the marker check, and the identity
/// checks: writer identity, binding identity, root, and schema-object digest.
pub fn open_store_v1(
    layout: &WorkspaceLayoutV1,
    binding: &StoreBindingV1,
    mode: OpenModeV1,
    deadline: Instant,
) -> Result<OpenStoreV1, PrototypeErrorV1> {
    if maintenance_marked(layout)? {
        return Err(PrototypeErrorV1::WorkspaceMaintenance);
    }
    if let Some(error) = classification_error(classify_store_v1(layout)?) {
        return Err(maintenance_unless_removed(layout, error));
    }
    let lock = acquire_shared_lock_v1(layout, deadline)
        .map_err(|error| maintenance_unless_removed(layout, error))?;
    if maintenance_marked(layout)? {
        return Err(PrototypeErrorV1::WorkspaceMaintenance);
    }
    let flags = match mode {
        OpenModeV1::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY,
        OpenModeV1::Mutation => OpenFlags::SQLITE_OPEN_READ_WRITE,
    } | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut connection = Connection::open_with_flags(layout.database(), flags).map_err(sqlite)?;
    apply_connection_pragmas_v1(&connection, pragma_busy_timeout_ms(deadline)).map_err(storage)?;
    set_busy_deadline(Some(deadline));
    connection
        .busy_handler(Some(bounded_busy_handler))
        .map_err(sqlite)?;
    if mode == OpenModeV1::Mutation {
        connection
            .pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)
            .map_err(sqlite)?;
    }
    let transaction = connection.transaction().map_err(sqlite)?;
    let (writer_identity, epoch, recorded_schema_digest) = transaction
        .query_row(
            "SELECT writer_identity, epoch, schema_digest FROM v2dfc001_writer \
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite)?
        .ok_or(PrototypeErrorV1::WriterMismatch)?;
    if writer_identity != binding.writer_identity {
        return Err(PrototypeErrorV1::WriterMismatch);
    }
    if schema_digest(&transaction)? != recorded_schema_digest {
        return Err(PrototypeErrorV1::SchemaMismatch);
    }
    let identity = verify_binding(&transaction, binding)?;
    transaction.commit().map_err(sqlite)?;
    Ok(OpenStoreV1 {
        connection,
        mode,
        epoch,
        identity,
        wal: layout.wal(),
        _lock: lock,
    })
}

fn maintenance_marked(layout: &WorkspaceLayoutV1) -> Result<bool, PrototypeErrorV1> {
    Ok(path_exists(&layout.reset_marker())? || path_exists(&layout.removal_marker())?)
}

/// An open that passed the first marker check can race a reset or removal that deletes the
/// Store or its lock files, so a missing file reads as maintenance while a marker remains.
fn maintenance_unless_removed(
    layout: &WorkspaceLayoutV1,
    error: PrototypeErrorV1,
) -> PrototypeErrorV1 {
    if !matches!(error, PrototypeErrorV1::WorkspaceNotInitialized) {
        return error;
    }
    match maintenance_marked(layout) {
        Ok(true) => PrototypeErrorV1::WorkspaceMaintenance,
        Ok(false) => error,
        Err(marker_error) => marker_error,
    }
}

/// Holds the SQLite write lock in an IMMEDIATE transaction until `release` returns.
///
/// The transaction writes nothing and rolls back, so contenders see only the lock.
pub fn hold_write_lock_v1(
    store: &mut OpenStoreV1,
    deadline: Instant,
    release: &mut dyn FnMut(),
) -> Result<(), PrototypeErrorV1> {
    set_busy_deadline(Some(deadline));
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite)?;
    release();
    drop(transaction);
    Ok(())
}

fn pragma_busy_timeout_ms(deadline: Instant) -> u32 {
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_millis();
    u32::try_from(remaining.clamp(1, MAX_PRAGMA_BUSY_TIMEOUT_MS)).unwrap_or(1)
}

fn set_busy_deadline(deadline: Option<Instant>) {
    BUSY_DEADLINE.with(|cell| cell.set(deadline));
}

/// Retries a busy lock with short jittered sleeps until the caller's deadline.
///
/// SQLite's default handler backs off up to 100 ms per retry, which lets a waiter lose many
/// consecutive releases. Short uniform sleeps keep each waiter's chance per release similar.
fn bounded_busy_handler(_attempts: i32) -> bool {
    let Some(deadline) = BUSY_DEADLINE.with(Cell::get) else {
        return false;
    };
    if Instant::now() >= deadline {
        return false;
    }
    let jitter = BUSY_JITTER_STATE.with(|state| {
        let mut value = state.get();
        if value == 0 {
            value = epoch_seed() | 1;
        }
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        state.set(value);
        value % BUSY_RETRY_JITTER_MICROS
    });
    thread::sleep(Duration::from_micros(BUSY_RETRY_BASE_MICROS + jitter));
    Instant::now() < deadline
}

fn schema_digest(connection: &Connection) -> Result<String, PrototypeErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_schema \
             ORDER BY type, name",
        )
        .map_err(sqlite)?;
    let mut rows = statement.query([]).map_err(sqlite)?;
    let mut hasher = Sha256::new();
    while let Some(row) = rows.next().map_err(sqlite)? {
        for column in 0..4 {
            let value: String = row.get(column).map_err(sqlite)?;
            hasher.update(value.len().to_be_bytes());
            hasher.update(value.as_bytes());
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn epoch_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    (nanos as u64) ^ u64::from(std::process::id()).rotate_left(32)
}

fn new_epoch() -> String {
    let mut hasher = Sha256::new();
    hasher.update(epoch_seed().to_be_bytes());
    hasher.update(EPOCH_SEQUENCE.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    format!("{:x}", hasher.finalize())[..32].to_owned()
}

fn now_millis() -> UnixMillis {
    UnixMillis::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            }),
    )
}

fn sqlite_i64(value: u64) -> Result<i64, PrototypeErrorV1> {
    i64::try_from(value).map_err(storage)
}

/// Reads one committed snapshot in a WAL read transaction.
pub fn read_workspace_view_v1(
    store: &mut OpenStoreV1,
) -> Result<GraphWorkspaceViewV2, PrototypeErrorV1> {
    let identity = store.identity.clone();
    let transaction = store.connection.transaction().map_err(sqlite)?;
    let graph_state = load_graph_session_connection_v2(&transaction).map_err(storage)?;
    let current_terminal_disposition = match graph_state.as_ref() {
        Some(state) => {
            let dispositions =
                load_terminal_dispositions_connection_v2(&transaction, state).map_err(storage)?;
            terminal_disposition_is_current_v2(&dispositions, state.trace().revision())
        }
        None => false,
    };
    let delegation_receipts = match graph_state.as_ref() {
        Some(state) => {
            receipt_snapshot_connection_v1(&transaction, state, identity.workspace_uuid())
                .map_err(storage)?
        }
        None => crate::v2_delegation::DelegationReceiptSnapshotV1::empty(),
    };
    let (sequence, observed_at): (i64, i64) = transaction
        .query_row(
            "SELECT next_workspace_sequence, updated_at_ms FROM workspace_state \
             WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite)?;
    transaction.commit().map_err(sqlite)?;
    Ok(GraphWorkspaceViewV2::new(
        identity,
        graph_state,
        0,
        None,
        u64::try_from(sequence).map_err(storage)?,
        UnixMillis::new(u64::try_from(observed_at).map_err(storage)?),
    )
    .with_current_terminal_disposition(current_terminal_disposition)
    .with_delegation_receipts(delegation_receipts))
}

/// One recorded success outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutcomeRecordV1 {
    pub idempotency_key: String,
    pub request_digest: String,
    pub canonical_request: String,
    pub workspace_sequence: u64,
    pub session_revision_after: u64,
    pub response: String,
    pub recorded_at: UnixMillis,
}

const OUTCOME_COLUMNS: &str = "idempotency_key, request_digest, canonical_request, \
     workspace_sequence, session_revision_after, response_json, recorded_at_ms";

fn outcome_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutcomeRecordV1> {
    Ok(OutcomeRecordV1 {
        idempotency_key: row.get(0)?,
        request_digest: row.get(1)?,
        canonical_request: row.get(2)?,
        workspace_sequence: row.get::<_, i64>(3)?.try_into().unwrap_or(u64::MAX),
        session_revision_after: row.get::<_, i64>(4)?.try_into().unwrap_or(u64::MAX),
        response: row.get(5)?,
        recorded_at: UnixMillis::new(row.get::<_, i64>(6)?.try_into().unwrap_or(u64::MAX)),
    })
}

/// Read-only outcome lookup by idempotency key.
pub fn lookup_outcome_v1(
    store: &mut OpenStoreV1,
    idempotency_key: &str,
) -> Result<Option<OutcomeRecordV1>, PrototypeErrorV1> {
    store
        .connection
        .query_row(
            &format!("SELECT {OUTCOME_COLUMNS} FROM v2dfc001_outcomes WHERE idempotency_key = ?1"),
            [idempotency_key],
            outcome_row,
        )
        .optional()
        .map_err(sqlite)
}

/// Every recorded outcome in commit order.
pub fn list_outcomes_v1(store: &mut OpenStoreV1) -> Result<Vec<OutcomeRecordV1>, PrototypeErrorV1> {
    list_outcomes_connection(&store.connection)
}

fn list_outcomes_connection(
    connection: &Connection,
) -> Result<Vec<OutcomeRecordV1>, PrototypeErrorV1> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {OUTCOME_COLUMNS} FROM v2dfc001_outcomes ORDER BY workspace_sequence"
        ))
        .map_err(sqlite)?;
    let rows = statement.query_map([], outcome_row).map_err(sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sqlite)
}

/// Runs `PRAGMA integrity_check` and returns its first result row.
pub fn integrity_check_v1(store: &mut OpenStoreV1) -> Result<String, PrototypeErrorV1> {
    store
        .connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(sqlite)
}

/// Runs `PRAGMA quick_check` and returns its first result row.
pub fn quick_check_v1(store: &mut OpenStoreV1) -> Result<String, PrototypeErrorV1> {
    store
        .connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(sqlite)
}

/// Appends rows to the operational journal until the WAL reaches the requested size.
///
/// Callers disable automatic checkpoints first so the frames remain a hot WAL for fixtures.
pub fn grow_wal_v1(
    store: &mut OpenStoreV1,
    minimum_wal_bytes: u64,
) -> Result<u64, PrototypeErrorV1> {
    let summary = "w".repeat(4_000);
    loop {
        let size = fs::metadata(&store.wal).map_or(0, |metadata| metadata.len());
        if size >= minimum_wal_bytes {
            return Ok(size);
        }
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        transaction
            .execute(
                "INSERT INTO operational_journal \
                 (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, \
                 details_json) VALUES (?1, 'debug', 'v2dfc001.wal_fixture', NULL, NULL, ?2, NULL)",
                params![sqlite_i64(now_millis().get())?, summary.as_str()],
            )
            .map_err(sqlite)?;
        transaction.commit().map_err(sqlite)?;
    }
}

/// Returns the current WAL size, or zero when no WAL exists.
pub fn wal_size_bytes_v1(layout: &WorkspaceLayoutV1) -> u64 {
    fs::metadata(layout.wal()).map_or(0, |metadata| metadata.len())
}

/// The prepared content of one direct mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectMutationV1 {
    Set {
        item_id: ItemId,
        expected_item_revision: Revision,
        value: String,
    },
    Record {
        expected_session_revision: Revision,
        operations: Vec<ActiveItemMutationRequestV2>,
    },
    Complete {
        expected_session_revision: Revision,
        fresh_attempt_id: AttemptId,
        prepared_artifacts: Vec<PreparedArtifactV1>,
    },
}

/// One fenced, keyed direct mutation whose preparation finished before any lock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectRequestV1 {
    pub idempotency_key: String,
    pub request_digest: String,
    pub canonical_request: String,
    pub expected_session_id: SessionId,
    pub expected_attempt_id: AttemptId,
    pub mutation: DirectMutationV1,
}

/// Process-local stamp of a prepared artifact file; any write changes its change time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStampV1 {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileStampV1 {
    fn observe(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

/// One attached local artifact hashed during preparation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedArtifactV1 {
    pub item_id: ItemId,
    pub path: PathBuf,
    pub digest: Sha256Digest,
    pub stamp: FileStampV1,
}

/// Hashes every local artifact attached to the active attempt, outside any lock.
pub fn prepare_local_artifacts_v1(
    layout: &WorkspaceLayoutV1,
    state: &GraphSessionStateV2,
) -> Result<Vec<PreparedArtifactV1>, PrototypeErrorV1> {
    let mut prepared = Vec::new();
    for (item_id, location) in attached_local_artifacts(state) {
        let path = layout.root().join(&location);
        let stamp = FileStampV1::observe(&path).map_err(|_| PrototypeErrorV1::ArtifactChanged)?;
        let bytes = fs::read(&path).map_err(|_| PrototypeErrorV1::ArtifactChanged)?;
        let digest =
            Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(&bytes))).map_err(storage)?;
        if FileStampV1::observe(&path).map_err(|_| PrototypeErrorV1::ArtifactChanged)? != stamp {
            return Err(PrototypeErrorV1::ArtifactChanged);
        }
        prepared.push(PreparedArtifactV1 {
            item_id,
            path,
            digest,
            stamp,
        });
    }
    Ok(prepared)
}

fn attached_local_artifacts(state: &GraphSessionStateV2) -> Vec<(ItemId, String)> {
    let Some(active) = state.trace().active_attempt() else {
        return Vec::new();
    };
    state
        .workflow_memory()
        .attempts()
        .iter()
        .filter(|attempt| attempt.attempt_id() == active.attempt_id())
        .flat_map(|attempt| attempt.item_slots())
        .filter_map(|slot| {
            let artifact = slot.value()?.as_artifact()?;
            (artifact.location_kind() == podway_core::ArtifactLocationKindV1::LocalPath)
                .then(|| (slot.item_id().clone(), artifact.location().to_owned()))
        })
        .collect()
}

fn recorded_artifact_digest(state: &GraphSessionStateV2, item_id: &ItemId) -> Option<Sha256Digest> {
    let active = state.trace().active_attempt()?;
    state
        .workflow_memory()
        .attempts()
        .iter()
        .filter(|attempt| attempt.attempt_id() == active.attempt_id())
        .flat_map(|attempt| attempt.item_slots())
        .find(|slot| slot.item_id() == item_id)
        .and_then(|slot| slot.value()?.as_artifact())
        .map(|artifact| artifact.digest().clone())
}

/// The commit boundaries at which the gate harness may pause or kill a writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectBoundaryV1 {
    BeforeBegin,
    AfterBegin,
    AfterStateWrite,
    AfterCommit,
}

/// One item outcome inside a committed batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectItemOutcomeV1 {
    pub item_id: ItemId,
    pub expected_item_revision: Revision,
    pub changed: bool,
    pub item_revision: Revision,
    pub value_digest: Option<Sha256Digest>,
}

/// The transition facts a caller needs to render the success response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectDetailV1 {
    Item(DirectItemOutcomeV1),
    Items {
        changed: bool,
        items: Vec<DirectItemOutcomeV1>,
    },
    Complete {
        from_graph_node_id: GraphNodeId,
        from_attempt_id: AttemptId,
        to_graph_node_id: Option<GraphNodeId>,
        to_attempt_id: Option<AttemptId>,
    },
}

/// Inputs to the response renderer, called inside the transaction before the outcome write.
pub struct DirectCommitContextV1<'a> {
    pub previous: &'a GraphSessionStateV2,
    pub next: &'a GraphSessionStateV2,
    pub detail: &'a DirectDetailV1,
    pub workspace_sequence: u64,
    pub committed_at: UnixMillis,
}

/// IMMEDIATE wait and lock-hold durations of one commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransactionTimingV1 {
    pub wait: Duration,
    pub hold: Duration,
}

/// The acknowledged result of one direct mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectOutcomeV1 {
    Committed {
        response: String,
        workspace_sequence: u64,
        session_revision_after: Revision,
        timing: TransactionTimingV1,
    },
    Replayed {
        response: String,
        timing: TransactionTimingV1,
    },
}

/// Applies one mutation and records its success outcome in a single IMMEDIATE transaction.
///
/// The transaction looks the key up first, then validates the fences and the prepared inputs,
/// reads the clock once, applies the pure transition, and writes the state, outcome, journal
/// entry, and journal pruning before COMMIT. A rejection rolls back and records nothing.
pub fn commit_direct_mutation_v1(
    store: &mut OpenStoreV1,
    request: &DirectRequestV1,
    deadline: Instant,
    hook: &mut dyn FnMut(DirectBoundaryV1),
    respond: &mut dyn FnMut(&DirectCommitContextV1<'_>) -> Result<String, String>,
) -> Result<DirectOutcomeV1, PrototypeErrorV1> {
    if store.mode != OpenModeV1::Mutation {
        return Err(storage("direct mutation requires a mutation open"));
    }
    hook(DirectBoundaryV1::BeforeBegin);
    set_busy_deadline(Some(deadline));
    let waiting = Instant::now();
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite)?;
    let acquired = Instant::now();
    let wait = acquired - waiting;
    hook(DirectBoundaryV1::AfterBegin);

    if let Some((digest, response)) = transaction
        .query_row(
            "SELECT request_digest, response_json FROM v2dfc001_outcomes \
             WHERE idempotency_key = ?1",
            [request.idempotency_key.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sqlite)?
    {
        drop(transaction);
        if digest != request.request_digest {
            return Err(PrototypeErrorV1::IdempotencyKeyReused);
        }
        return Ok(DirectOutcomeV1::Replayed {
            response,
            timing: TransactionTimingV1 {
                wait,
                hold: acquired.elapsed(),
            },
        });
    }

    let previous = load_graph_session_connection_v2(&transaction)
        .map_err(storage)?
        .ok_or_else(|| PrototypeErrorV1::Rejected {
            reason: "no current Procedure v2 session".to_owned(),
        })?;
    let now = now_millis();
    let (next, changed, detail) = apply_mutation(&previous, request, now)?;
    if changed {
        replace_graph_session_transaction_v2(
            &transaction,
            previous.workspace_revision(),
            previous.trace().revision(),
            &next,
        )
        .map_err(|error| PrototypeErrorV1::Rejected {
            reason: format!("{error:?}"),
        })?;
    }
    hook(DirectBoundaryV1::AfterStateWrite);

    let recorded_at = sqlite_i64(now.get())?;
    let workspace_sequence: i64 = transaction
        .query_row(NEXT_WORKSPACE_SEQUENCE_SQL, [recorded_at], |row| row.get(0))
        .map_err(sqlite)?;
    let workspace_sequence = u64::try_from(workspace_sequence).map_err(storage)?;
    let response = respond(&DirectCommitContextV1 {
        previous: &previous,
        next: &next,
        detail: &detail,
        workspace_sequence,
        committed_at: now,
    })
    .map_err(storage)?;
    let session_revision_after = next.trace().revision();
    transaction
        .execute(
            "INSERT INTO v2dfc001_outcomes (idempotency_key, request_digest, \
             canonical_request, workspace_sequence, session_revision_after, response_json, \
             recorded_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                request.idempotency_key,
                request.request_digest,
                request.canonical_request,
                sqlite_i64(workspace_sequence)?,
                sqlite_i64(session_revision_after.get())?,
                response,
                recorded_at,
            ],
        )
        .map_err(sqlite)?;
    transaction
        .execute(
            "INSERT INTO operational_journal \
             (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, \
             details_json) VALUES (?1, 'info', 'mutation.committed', ?2, NULL, \
             'direct mutation committed', NULL)",
            params![recorded_at, sqlite_i64(workspace_sequence)?],
        )
        .map_err(sqlite)?;
    prune_journal(&transaction, now)?;
    transaction.commit().map_err(sqlite)?;
    let timing = TransactionTimingV1 {
        wait,
        hold: acquired.elapsed(),
    };
    hook(DirectBoundaryV1::AfterCommit);
    Ok(DirectOutcomeV1::Committed {
        response,
        workspace_sequence,
        session_revision_after,
        timing,
    })
}

/// Applies one recorded mutation with the commit path's fences and pure transition.
///
/// The gate harness replays every recorded outcome through this function in commit order and
/// compares the result with the committed Store state.
pub fn apply_direct_mutation_v1(
    previous: &GraphSessionStateV2,
    request: &DirectRequestV1,
    now: UnixMillis,
) -> Result<GraphSessionStateV2, PrototypeErrorV1> {
    apply_mutation(previous, request, now).map(|(state, _, _)| state)
}

fn apply_mutation(
    previous: &GraphSessionStateV2,
    request: &DirectRequestV1,
    now: UnixMillis,
) -> Result<(GraphSessionStateV2, bool, DirectDetailV1), PrototypeErrorV1> {
    if previous.trace().session_id() != &request.expected_session_id {
        return Err(PrototypeErrorV1::Rejected {
            reason: "session identity fence failed".to_owned(),
        });
    }
    match &request.mutation {
        DirectMutationV1::Set {
            item_id,
            expected_item_revision,
            value,
        } => {
            let outcome = previous
                .mutate_active_item_v2(
                    &request.expected_attempt_id,
                    item_id,
                    *expected_item_revision,
                    ActiveItemMutationV2::Set {
                        value: value.clone(),
                    },
                    now,
                )
                .map_err(rejected)?;
            let detail = DirectDetailV1::Item(DirectItemOutcomeV1 {
                item_id: item_id.clone(),
                expected_item_revision: *expected_item_revision,
                changed: outcome.changed(),
                item_revision: outcome.item_revision(),
                value_digest: outcome.value_digest().cloned(),
            });
            let changed = outcome.changed();
            Ok((outcome.into_state(), changed, detail))
        }
        DirectMutationV1::Record {
            expected_session_revision,
            operations,
        } => {
            if previous.trace().revision() != *expected_session_revision {
                return Err(PrototypeErrorV1::Rejected {
                    reason: "session revision fence failed".to_owned(),
                });
            }
            let outcome = previous
                .mutate_active_items_v2(&request.expected_attempt_id, operations, now)
                .map_err(rejected)?;
            let items = outcome
                .items()
                .iter()
                .map(|item| {
                    let expected_item_revision = operations
                        .iter()
                        .find(|operation| operation.item_id() == item.item_id())
                        .map_or(
                            Revision::ZERO,
                            ActiveItemMutationRequestV2::expected_item_revision,
                        );
                    DirectItemOutcomeV1 {
                        item_id: item.item_id().clone(),
                        expected_item_revision,
                        changed: item.changed(),
                        item_revision: item.item_revision(),
                        value_digest: item.value_digest().cloned(),
                    }
                })
                .collect();
            let changed = outcome.changed();
            Ok((
                outcome.into_state(),
                changed,
                DirectDetailV1::Items { changed, items },
            ))
        }
        DirectMutationV1::Complete {
            expected_session_revision,
            fresh_attempt_id,
            prepared_artifacts,
        } => {
            for artifact in prepared_artifacts {
                let current = FileStampV1::observe(&artifact.path)
                    .map_err(|_| PrototypeErrorV1::ArtifactChanged)?;
                if current != artifact.stamp
                    || recorded_artifact_digest(previous, &artifact.item_id).as_ref()
                        != Some(&artifact.digest)
                {
                    return Err(PrototypeErrorV1::ArtifactChanged);
                }
            }
            if attached_local_artifacts(previous).len() != prepared_artifacts.len() {
                return Err(PrototypeErrorV1::ArtifactChanged);
            }
            let fresh =
                (!active_placement_is_terminal(previous)?).then(|| fresh_attempt_id.clone());
            let outcome = previous
                .complete_active_action_v2(
                    *expected_session_revision,
                    &request.expected_attempt_id,
                    fresh,
                    now,
                )
                .map_err(rejected)?;
            let detail = DirectDetailV1::Complete {
                from_graph_node_id: outcome.from_graph_node_id().clone(),
                from_attempt_id: outcome.from_attempt_id().clone(),
                to_graph_node_id: outcome.to_graph_node_id().cloned(),
                to_attempt_id: outcome.to_attempt_id().cloned(),
            };
            Ok((outcome.into_state(), true, detail))
        }
    }
}

fn active_placement_is_terminal(state: &GraphSessionStateV2) -> Result<bool, PrototypeErrorV1> {
    let active = state
        .trace()
        .active_attempt()
        .ok_or_else(|| PrototypeErrorV1::Rejected {
            reason: "no active attempt".to_owned(),
        })?;
    let node = state
        .snapshot()
        .graph_node(active.graph_node_id())
        .ok_or_else(|| storage("active graph node is absent"))?;
    let placement: Value =
        serde_json::from_str(node.canonical_placement_json()).map_err(storage)?;
    Ok(placement.get("terminal").and_then(Value::as_bool) == Some(true))
}

fn prune_journal(connection: &Connection, now: UnixMillis) -> Result<(), PrototypeErrorV1> {
    let cutoff = sqlite_i64(now.get().saturating_sub(JOURNAL_MAX_AGE_MS))?;
    connection
        .execute(
            "DELETE FROM operational_journal WHERE journal_id IN (
                 WITH ranked AS (
                     SELECT journal_id, recorded_at_ms,
                            ROW_NUMBER() OVER (
                                ORDER BY recorded_at_ms DESC, journal_id DESC
                            ) AS journal_rank
                     FROM operational_journal
                 )
                 SELECT journal_id FROM ranked
                 WHERE journal_rank > ?1 AND (recorded_at_ms < ?2 OR journal_rank > ?3)
             )",
            params![JOURNAL_PROTECTED_COUNT, cutoff, JOURNAL_ABSOLUTE_COUNT],
        )
        .map_err(sqlite)?;
    Ok(())
}

/// Converts a closed generation-13 Store into a prototype Store and creates its lock files.
///
/// The conversion adds the writer and outcome tables, records the writer identity, epoch, and
/// schema-object digest, marks the header generation, and checkpoints so classification reads
/// the generation from the main database header.
pub fn convert_to_prototype_v1(
    layout: &WorkspaceLayoutV1,
    binding: &StoreBindingV1,
) -> Result<String, PrototypeErrorV1> {
    let mut connection = Connection::open_with_flags(
        layout.database(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite)?;
    apply_connection_pragmas_v1(&connection, 5_000).map_err(storage)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite)?;
    verify_binding(&transaction, binding)?;
    transaction.execute_batch(PROTOTYPE_DDL).map_err(sqlite)?;
    let epoch = new_epoch();
    let digest = schema_digest(&transaction)?;
    transaction
        .execute(
            "INSERT INTO v2dfc001_writer (singleton, writer_identity, epoch, schema_digest) \
             VALUES (1, ?1, ?2, ?3)",
            params![binding.writer_identity, epoch, digest],
        )
        .map_err(sqlite)?;
    transaction
        .pragma_update(None, "user_version", PROTOTYPE_GENERATION_V1)
        .map_err(sqlite)?;
    transaction.commit().map_err(sqlite)?;
    checkpoint_truncate(&connection)?;
    drop(connection);
    create_lock_files(layout)?;
    Ok(epoch)
}

fn checkpoint_truncate(connection: &Connection) -> Result<(), PrototypeErrorV1> {
    let busy: i64 = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(sqlite)?;
    if busy != 0 {
        return Err(PrototypeErrorV1::WorkspaceBusy);
    }
    Ok(())
}

fn create_lock_files(layout: &WorkspaceLayoutV1) -> Result<(), PrototypeErrorV1> {
    for path in [layout.lock_file(), layout.intent_file()] {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(storage)?;
    }
    sync_directory(&layout.runtime_directory())
}

fn sync_directory(path: &Path) -> Result<(), PrototypeErrorV1> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(storage)
}

/// The maintenance steps at which the gate harness may pause or kill the holder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceBoundaryV1 {
    ExclusiveAcquired,
    MarkerPublished,
    StateUnlinked,
    StoreSeeded,
    MarkerRemoved,
    ContentsRemoved,
    MarkerUnlinked,
    DirectoriesRemoved,
}

/// The exact key and digest that identify, resume, and replay one maintenance operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceRequestV1 {
    pub idempotency_key: String,
    pub request_digest: String,
}

/// The outcome of one maintenance call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceOutcomeV1 {
    /// Time from the exclusive request to holding both locks.
    pub acquisition: Duration,
    /// The operation had already completed, so this call changed nothing.
    pub replayed: bool,
    /// This call resumed an interrupted operation from its marker.
    pub resumed: bool,
    /// The Store epoch the operation retired, when this call retired one.
    pub retired_epoch: Option<String>,
    /// The epoch of the Store the operation left in place.
    pub epoch: Option<String>,
}

/// Outcome keys of one Store generation, taken under the exclusive lock before retirement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetiredGenerationV1 {
    pub epoch: String,
    pub outcome_keys: Vec<String>,
}

/// The seed copied into place when maintenance creates a Store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreSeedV1 {
    /// A closed prototype database to copy.
    pub database: PathBuf,
    /// A `.podway` tree without runtime state, copied when the workspace itself is absent.
    pub podway_tree: PathBuf,
}

/// Resets the worktree to a freshly seeded Store under the marker protocol.
///
/// An exact retry with the same key and digest resumes an interrupted reset from its marker and
/// answers a completed one from the outcome recorded in the new Store.
pub fn reset_all_v1(
    layout: &WorkspaceLayoutV1,
    seed: &StoreSeedV1,
    request: &MaintenanceRequestV1,
    deadline: Instant,
    hook: &mut dyn FnMut(MaintenanceBoundaryV1),
    retire: &mut dyn FnMut(&RetiredGenerationV1),
) -> Result<MaintenanceOutcomeV1, PrototypeErrorV1> {
    let started = Instant::now();
    let lock = acquire_exclusive_lock_v1(layout, deadline, false)?;
    let acquisition = started.elapsed();
    hook(MaintenanceBoundaryV1::ExclusiveAcquired);
    if path_exists(&layout.removal_marker())? {
        return Err(PrototypeErrorV1::WorkspaceMaintenance);
    }
    let (target_epoch, retired_epoch, resumed) = match read_marker(&layout.reset_marker())? {
        Some(marker) => {
            if marker.idempotency_key != request.idempotency_key
                || marker.request_digest != request.request_digest
            {
                return Err(PrototypeErrorV1::WorkspaceMaintenance);
            }
            (marker.target_epoch, None, true)
        }
        None => {
            let retired = read_generation(layout)?;
            if retired
                .outcomes
                .iter()
                .any(|outcome| outcome.idempotency_key == request.idempotency_key)
            {
                return replay(&retired, request, acquisition);
            }
            retire(&RetiredGenerationV1 {
                epoch: retired.epoch.clone(),
                outcome_keys: retired
                    .outcomes
                    .iter()
                    .map(|outcome| outcome.idempotency_key.clone())
                    .collect(),
            });
            let target_epoch = new_epoch();
            write_marker(
                &layout.reset_marker(),
                RESET_MARKER_SCHEMA,
                &MarkerV1 {
                    idempotency_key: request.idempotency_key.clone(),
                    request_digest: request.request_digest.clone(),
                    target_epoch: target_epoch.clone(),
                },
            )?;
            hook(MaintenanceBoundaryV1::MarkerPublished);
            (target_epoch, Some(retired.epoch), false)
        }
    };
    if current_epoch(layout)?.as_deref() != Some(target_epoch.as_str()) {
        for path in [layout.database(), layout.wal(), layout.shm()] {
            remove_if_present(&path)?;
        }
        sync_directory(&layout.runtime_directory())?;
        hook(MaintenanceBoundaryV1::StateUnlinked);
        seed_store(layout, seed, request, &target_epoch, "workspace.reset_all")?;
        hook(MaintenanceBoundaryV1::StoreSeeded);
    }
    remove_if_present(&layout.reset_marker())?;
    sync_directory(&layout.runtime_directory())?;
    hook(MaintenanceBoundaryV1::MarkerRemoved);
    drop(lock);
    Ok(MaintenanceOutcomeV1 {
        acquisition,
        replayed: false,
        resumed,
        retired_epoch,
        epoch: Some(target_epoch),
    })
}

/// Removes the worktree's `.podway` directory with the removal marker last.
pub fn remove_workspace_v1(
    layout: &WorkspaceLayoutV1,
    request: &MaintenanceRequestV1,
    deadline: Instant,
    hook: &mut dyn FnMut(MaintenanceBoundaryV1),
    retire: &mut dyn FnMut(&RetiredGenerationV1),
) -> Result<MaintenanceOutcomeV1, PrototypeErrorV1> {
    let started = Instant::now();
    if !path_exists(&layout.runtime_directory())? {
        if path_exists(&layout.podway_directory())? {
            remove_tree(&layout.podway_directory())?;
        }
        return Ok(MaintenanceOutcomeV1 {
            acquisition: started.elapsed(),
            replayed: true,
            resumed: false,
            retired_epoch: None,
            epoch: None,
        });
    }
    let lock = acquire_exclusive_lock_v1(layout, deadline, true)?;
    let acquisition = started.elapsed();
    hook(MaintenanceBoundaryV1::ExclusiveAcquired);
    if path_exists(&layout.reset_marker())? {
        return Err(PrototypeErrorV1::WorkspaceMaintenance);
    }
    let marker = read_marker(&layout.removal_marker())?;
    // Without a marker or a database, an earlier removal unlinked its marker and was interrupted
    // before removing the directories. Removal never runs concurrently with Store creation.
    let unlinked = marker.is_none() && !path_exists(&layout.database())?;
    let (retired_epoch, resumed) = match marker {
        Some(marker) => {
            if marker.idempotency_key != request.idempotency_key
                || marker.request_digest != request.request_digest
            {
                return Err(PrototypeErrorV1::WorkspaceMaintenance);
            }
            (None, true)
        }
        None if unlinked => (None, true),
        None => {
            let retired = read_generation(layout)?;
            retire(&RetiredGenerationV1 {
                epoch: retired.epoch.clone(),
                outcome_keys: retired
                    .outcomes
                    .iter()
                    .map(|outcome| outcome.idempotency_key.clone())
                    .collect(),
            });
            write_marker(
                &layout.removal_marker(),
                REMOVAL_MARKER_SCHEMA,
                &MarkerV1 {
                    idempotency_key: request.idempotency_key.clone(),
                    request_digest: request.request_digest.clone(),
                    target_epoch: String::new(),
                },
            )?;
            hook(MaintenanceBoundaryV1::MarkerPublished);
            (Some(retired.epoch), false)
        }
    };
    let runtime = layout.runtime_directory();
    for entry in fs::read_dir(layout.podway_directory()).map_err(storage)? {
        let path = entry.map_err(storage)?.path();
        if path != runtime {
            remove_tree(&path)?;
        }
    }
    for entry in fs::read_dir(&runtime).map_err(storage)? {
        let path = entry.map_err(storage)?.path();
        if path != layout.removal_marker() {
            remove_tree(&path)?;
        }
    }
    sync_directory(&runtime)?;
    hook(MaintenanceBoundaryV1::ContentsRemoved);
    remove_if_present(&layout.removal_marker())?;
    sync_directory(&runtime)?;
    hook(MaintenanceBoundaryV1::MarkerUnlinked);
    fs::remove_dir(&runtime).map_err(storage)?;
    fs::remove_dir(layout.podway_directory()).map_err(storage)?;
    sync_directory(layout.root())?;
    hook(MaintenanceBoundaryV1::DirectoriesRemoved);
    drop(lock);
    Ok(MaintenanceOutcomeV1 {
        acquisition,
        replayed: false,
        resumed,
        retired_epoch,
        epoch: None,
    })
}

/// Creates the workspace and seeds a fresh Store under the exclusive lock.
pub fn create_workspace_v1(
    layout: &WorkspaceLayoutV1,
    seed: &StoreSeedV1,
    request: &MaintenanceRequestV1,
    deadline: Instant,
) -> Result<MaintenanceOutcomeV1, PrototypeErrorV1> {
    let started = Instant::now();
    if !path_exists(&layout.podway_directory())? {
        copy_tree(&seed.podway_tree, &layout.podway_directory())?;
    }
    fs::create_dir_all(layout.runtime_directory()).map_err(storage)?;
    let lock = acquire_exclusive_lock_v1(layout, deadline, true)?;
    let acquisition = started.elapsed();
    if path_exists(&layout.reset_marker())? || path_exists(&layout.removal_marker())? {
        return Err(PrototypeErrorV1::WorkspaceMaintenance);
    }
    if path_exists(&layout.database())? {
        let current = read_generation(layout)?;
        return replay(&current, request, acquisition);
    }
    let epoch = new_epoch();
    seed_store(layout, seed, request, &epoch, "workspace.init")?;
    drop(lock);
    Ok(MaintenanceOutcomeV1 {
        acquisition,
        replayed: false,
        resumed: false,
        retired_epoch: None,
        epoch: Some(epoch),
    })
}

struct GenerationV1 {
    epoch: String,
    outcomes: Vec<OutcomeRecordV1>,
}

fn read_generation(layout: &WorkspaceLayoutV1) -> Result<GenerationV1, PrototypeErrorV1> {
    let connection = Connection::open_with_flags(
        layout.database(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite)?;
    let epoch = connection
        .query_row(
            "SELECT epoch FROM v2dfc001_writer WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite)?;
    let outcomes = list_outcomes_connection(&connection)?;
    Ok(GenerationV1 { epoch, outcomes })
}

fn current_epoch(layout: &WorkspaceLayoutV1) -> Result<Option<String>, PrototypeErrorV1> {
    if !path_exists(&layout.database())? {
        return Ok(None);
    }
    read_generation(layout).map(|generation| Some(generation.epoch))
}

fn replay(
    generation: &GenerationV1,
    request: &MaintenanceRequestV1,
    acquisition: Duration,
) -> Result<MaintenanceOutcomeV1, PrototypeErrorV1> {
    let recorded = generation
        .outcomes
        .iter()
        .find(|outcome| outcome.idempotency_key == request.idempotency_key)
        .ok_or(PrototypeErrorV1::WorkspaceMaintenance)?;
    if recorded.request_digest != request.request_digest {
        return Err(PrototypeErrorV1::IdempotencyKeyReused);
    }
    Ok(MaintenanceOutcomeV1 {
        acquisition,
        replayed: true,
        resumed: false,
        retired_epoch: None,
        epoch: Some(generation.epoch.clone()),
    })
}

/// Copies the seed into a temporary file, records the operation outcome and the new epoch in
/// it, and renames it into place so the Store appears complete or not at all.
fn seed_store(
    layout: &WorkspaceLayoutV1,
    seed: &StoreSeedV1,
    request: &MaintenanceRequestV1,
    epoch: &str,
    command: &str,
) -> Result<(), PrototypeErrorV1> {
    let temporary = layout.runtime_directory().join(SEED_TEMPORARY_FILE_NAME);
    for path in [
        temporary.clone(),
        sidecar(&temporary, "-wal"),
        sidecar(&temporary, "-shm"),
    ] {
        remove_if_present(&path)?;
    }
    fs::copy(&seed.database, &temporary).map_err(storage)?;
    {
        let connection = Connection::open_with_flags(
            &temporary,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sqlite)?;
        apply_connection_pragmas_v1(&connection, 5_000).map_err(storage)?;
        let now = now_millis();
        let recorded_at = sqlite_i64(now.get())?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(sqlite)?;
        connection
            .execute(
                "UPDATE v2dfc001_writer SET epoch = ?1 WHERE singleton = 1",
                [epoch],
            )
            .map_err(sqlite)?;
        let sequence: i64 = connection
            .query_row(NEXT_WORKSPACE_SEQUENCE_SQL, [recorded_at], |row| row.get(0))
            .map_err(sqlite)?;
        let session_revision: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(session_revision), 0) FROM v2_task_sessions \
                 WHERE activity = 'current'",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite)?;
        let response = json!({"command": command, "epoch": epoch}).to_string();
        connection
            .execute(
                "INSERT INTO v2dfc001_outcomes (idempotency_key, request_digest, \
                 canonical_request, workspace_sequence, session_revision_after, response_json, \
                 recorded_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    request.idempotency_key,
                    request.request_digest,
                    command,
                    sequence,
                    session_revision,
                    response,
                    recorded_at,
                ],
            )
            .map_err(sqlite)?;
        connection.execute_batch("COMMIT").map_err(sqlite)?;
        checkpoint_truncate(&connection)?;
    }
    remove_if_present(&sidecar(&temporary, "-wal"))?;
    remove_if_present(&sidecar(&temporary, "-shm"))?;
    File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(storage)?;
    fs::rename(&temporary, layout.database()).map_err(storage)?;
    sync_directory(&layout.runtime_directory())
}

struct MarkerV1 {
    idempotency_key: String,
    request_digest: String,
    target_epoch: String,
}

fn write_marker(path: &Path, schema: &str, marker: &MarkerV1) -> Result<(), PrototypeErrorV1> {
    let temporary = sidecar(path, MARKER_TEMPORARY_SUFFIX);
    let document = json!({
        "schema": schema,
        "idempotency_key": marker.idempotency_key,
        "request_digest": marker.request_digest,
        "target_epoch": marker.target_epoch,
    });
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(storage)?;
    file.write_all(document.to_string().as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(storage)?;
    fs::rename(&temporary, path).map_err(storage)?;
    sync_directory(
        path.parent()
            .ok_or_else(|| storage("marker has no parent"))?,
    )
}

fn read_marker(path: &Path) -> Result<Option<MarkerV1>, PrototypeErrorV1> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage(error)),
    };
    let document: Value = serde_json::from_slice(&bytes).map_err(storage)?;
    let field = |name: &str| {
        document
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| storage("maintenance marker is malformed"))
    };
    Ok(Some(MarkerV1 {
        idempotency_key: field("idempotency_key")?,
        request_digest: field("request_digest")?,
        target_epoch: field("target_epoch")?,
    }))
}

fn remove_if_present(path: &Path) -> Result<(), PrototypeErrorV1> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage(error)),
    }
}

fn remove_tree(path: &Path) -> Result<(), PrototypeErrorV1> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage(error)),
    };
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(storage)
    } else {
        remove_if_present(path)
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), PrototypeErrorV1> {
    fs::create_dir_all(destination).map_err(storage)?;
    for entry in fs::read_dir(source).map_err(storage)? {
        let entry = entry.map_err(storage)?;
        let target = destination.join(entry.file_name());
        if entry.file_type().map_err(storage)?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(storage)?;
        }
    }
    Ok(())
}

/// How a legacy fixture's generation reaches the database header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyHeaderV1 {
    /// The generation is checkpointed into the main header and later frames stay in the WAL.
    Checkpointed,
    /// The generation exists only in the WAL's committed page-one image.
    WalOnly,
}

/// Writes a legacy Store with a hot WAL and no shared-memory file into `runtime_directory`.
///
/// The Store is the cumulative public DDL through `generation`, or a copy of `base` when given.
/// Main and WAL images are materialized and verified while the building connection is idle
/// and still open, so closing the producer cannot checkpoint away the hot WAL.
pub fn write_legacy_store_v1(
    staging_directory: &Path,
    runtime_directory: &Path,
    generation: u32,
    header: LegacyHeaderV1,
    base: Option<&Path>,
) -> Result<(), PrototypeErrorV1> {
    let staging = staging_directory.join(STATE_DATABASE_FILE_NAME_V1);
    for path in [
        staging.clone(),
        sidecar(&staging, "-wal"),
        sidecar(&staging, "-shm"),
    ] {
        remove_if_present(&path)?;
    }
    if let Some(base) = base {
        fs::copy(base, &staging).map_err(storage)?;
    }
    let connection = Connection::open(&staging).map_err(sqlite)?;
    connection
        .query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
        .map_err(sqlite)?;
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .map_err(sqlite)?;
    if base.is_none() {
        for version in 1..=generation {
            connection
                .execute_batch(cumulative_ddl(version)?)
                .map_err(sqlite)?;
        }
        if connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .map_err(sqlite)?
            != generation
        {
            return Err(storage("legacy fixture generation was not applied"));
        }
    }
    if header == LegacyHeaderV1::Checkpointed {
        checkpoint_truncate(&connection)?;
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS v2dfc001_hot_wal (value INTEGER NOT NULL);
             INSERT INTO v2dfc001_hot_wal (value) VALUES (1), (2), (3);",
        )
        .map_err(sqlite)?;
    fs::create_dir_all(runtime_directory).map_err(storage)?;
    let target = runtime_directory.join(STATE_DATABASE_FILE_NAME_V1);
    let source_main = fs::read(&staging).map_err(storage)?;
    let source_wal = fs::read(sidecar(&staging, "-wal")).map_err(storage)?;
    fs::write(&target, &source_main).map_err(storage)?;
    fs::write(sidecar(&target, "-wal"), &source_wal).map_err(storage)?;
    if fs::read(&target).map_err(storage)? != source_main
        || fs::read(sidecar(&target, "-wal")).map_err(storage)? != source_wal
    {
        return Err(storage("legacy fixture images do not match their source"));
    }
    drop(connection);
    Ok(())
}

fn cumulative_ddl(version: u32) -> Result<&'static str, PrototypeErrorV1> {
    use crate::schema::*;
    Ok(match version {
        1 => sqlite_v1_ddl(),
        2 => sqlite_v2_ddl(),
        3 => sqlite_v3_ddl(),
        4 => sqlite_v4_ddl(),
        5 => sqlite_v5_ddl(),
        6 => sqlite_v6_ddl(),
        7 => sqlite_v7_ddl(),
        8 => sqlite_v8_ddl(),
        9 => sqlite_v9_ddl(),
        10 => sqlite_v10_ddl(),
        11 => sqlite_v11_ddl(),
        12 => sqlite_v12_ddl(),
        13 => sqlite_v13_ddl(),
        _ => return Err(storage("legacy fixture generation is outside 1 to 13")),
    })
}
