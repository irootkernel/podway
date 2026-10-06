use std::{
    cell::Cell,
    fs::File,
    os::unix::ffi::OsStrExt,
    sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use podway_core::{SessionId, Sha256Digest, TerminalDispositionV2, UnixMillis, WorkspaceId};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, config::DbConfig};

use crate::{
    DurableWorktreeIdentityV1, GraphSessionStateV2, IdempotencyKeyV1, SessionArchiveListPageV2,
    WorkspaceBindingV1, v2_delegation, v2_state,
};

use super::{
    DIRECT_SCHEMA_GENERATION_V1, DIRECT_STORE_DDL_V1, DirectRecordedOutcomeV1, DirectStoreErrorV1,
    DirectStoreResultV1, DirectTransactionBoundaryV1, DirectWorkspaceLayoutV1,
    maintenance::DirectOpenLease,
};

type BoundaryObserver = Arc<dyn Fn(DirectTransactionBoundaryV1) + Send + Sync>;

// The Unix VFS shares SHM mappings between same-process SQLite connections. A
// read-only connection must never inherit a writable mapping from a direct writer.
// A CLI invocation owns one worktree; this bounded process-local gate creates no
// filesystem/global runtime state and leaves independent processes concurrent.
static CONNECTION_GATE: Mutex<()> = Mutex::new(());

thread_local! {
    static BUSY_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static BUSY_JITTER: Cell<u64> = const { Cell::new(0) };
}

fn short_busy_wait(_attempts: i32) -> bool {
    let Some(deadline) = BUSY_DEADLINE.with(Cell::get) else {
        return false;
    };
    if Instant::now() >= deadline {
        return false;
    }
    let jitter = BUSY_JITTER.with(|state| {
        let mut value = state.get();
        if value == 0 {
            value = (SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(1, |time| time.as_nanos() as u64)
                ^ u64::from(std::process::id()))
                | 1;
        }
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        state.set(value);
        value % 401
    });
    std::thread::sleep(
        Duration::from_micros(100 + jitter).min(deadline.saturating_duration_since(Instant::now())),
    );
    Instant::now() < deadline
}

pub(super) fn install_busy_deadline(
    connection: &Connection,
    deadline: Instant,
) -> DirectStoreResultV1<()> {
    BUSY_DEADLINE.with(|current| current.set(Some(deadline)));
    connection.busy_handler(Some(short_busy_wait))?;
    Ok(())
}

pub(super) fn connection_gate(deadline: Instant) -> DirectStoreResultV1<MutexGuard<'static, ()>> {
    loop {
        ensure_deadline(deadline)?;
        match CONNECTION_GATE.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::WouldBlock) => std::thread::sleep(
                Duration::from_micros(500).min(deadline.saturating_duration_since(Instant::now())),
            ),
            Err(TryLockError::Poisoned(_)) => return Err(DirectStoreErrorV1::Unreadable),
        }
    }
}

/// The production direct Store has no queue, worker, runtime mode or IPC dependency.
/// Each invocation obtains its own lease and connection; this value owns no process.
#[derive(Clone)]
pub struct DirectStoreV1 {
    pub(super) layout: DirectWorkspaceLayoutV1,
    pub(super) manifest: Sha256Digest,
    pub(super) common: Sha256Digest,
    pub(super) admin: Sha256Digest,
    pub(super) observer: Option<BoundaryObserver>,
}

impl std::fmt::Debug for DirectStoreV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DirectStoreV1")
            .field("layout", &self.layout)
            .field("manifest", &self.manifest)
            .field("common", &self.common)
            .field("admin", &self.admin)
            .finish_non_exhaustive()
    }
}

impl DirectStoreV1 {
    /// Git fingerprints are supplied by the canonical read-only workspace resolver.
    pub fn new(
        layout: DirectWorkspaceLayoutV1,
        manifest: Sha256Digest,
        common: Sha256Digest,
        admin: Sha256Digest,
    ) -> Self {
        Self {
            layout,
            manifest,
            common,
            admin,
            observer: None,
        }
    }

    pub fn layout(&self) -> &DirectWorkspaceLayoutV1 {
        &self.layout
    }

    /// Explicit test instrumentation, without environment variables or product flags.
    /// A parent process can rendezvous here and kill only its owned child.
    #[doc(hidden)]
    pub fn with_boundary_observer(
        mut self,
        observer: impl Fn(DirectTransactionBoundaryV1) + Send + Sync + 'static,
    ) -> Self {
        self.observer = Some(Arc::new(observer));
        self
    }

    pub(super) fn boundary(&self, boundary: DirectTransactionBoundaryV1) {
        if let Some(observer) = &self.observer {
            observer(boundary);
        }
    }

    /// One coherent read transaction, including binding, outcomes and domain projections.
    /// Neither this path nor SQLite's read-only WAL index creates or updates sidecars.
    pub fn read<T>(
        &self,
        deadline: Instant,
        read: impl FnOnce(&DirectReadSnapshotV1<'_>) -> DirectStoreResultV1<T>,
    ) -> DirectStoreResultV1<T> {
        let lease = DirectOpenLease::acquire(&self.layout, deadline)?;
        let _connection_gate = connection_gate(deadline)?;
        let database = super::classification::open_read_nofollow(&self.layout.database())?;
        self.qualified_read(deadline, lease.heap_snapshot(), |transaction, binding| {
            self.revalidate_files(&lease, &database)?;
            let snapshot = DirectReadSnapshotV1 {
                connection: transaction,
                binding,
            };
            let result = read(&snapshot)?;
            self.revalidate_files(&lease, &database)?;
            ensure_deadline(deadline)?;
            Ok(result)
        })
    }

    /// A writer may be rebuilding the shared WAL index while a write-free
    /// preflight tries to read it. Retry that admission only; never rerun a caller's
    /// callback or ask a read-only connection to repair authoritative cache files.
    pub(super) fn qualified_read<T>(
        &self,
        deadline: Instant,
        heap_snapshot: bool,
        read: impl FnOnce(&rusqlite::Transaction<'_>, WorkspaceBindingV1) -> DirectStoreResultV1<T>,
    ) -> DirectStoreResultV1<T> {
        loop {
            ensure_deadline(deadline)?;
            let mut connection = match self.open_readonly_connection(deadline, heap_snapshot) {
                Ok(connection) => connection,
                Err(error) if readonly_recovery(&error) => {
                    recovery_wait(deadline)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            match self.validate_connection(&transaction) {
                Ok(binding) => return read(&transaction, binding),
                Err(error) if readonly_recovery(&error) => {
                    drop(transaction);
                    drop(connection);
                    recovery_wait(deadline)?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(super) fn open_connection(
        &self,
        readonly: bool,
        deadline: Instant,
    ) -> DirectStoreResultV1<Connection> {
        self.open_sqlite_connection(readonly, false, deadline)
    }

    /// Heap WAL snapshots require an already held exclusive native lease. The
    /// read-only unix-flock VFS then supplies SQLite's exclusive locking mode
    /// without attempting a write lock on an O_RDONLY POSIX descriptor.
    pub(super) fn open_readonly_connection(
        &self,
        deadline: Instant,
        heap_snapshot: bool,
    ) -> DirectStoreResultV1<Connection> {
        self.open_sqlite_connection(true, heap_snapshot, deadline)
    }

    fn open_sqlite_connection(
        &self,
        readonly: bool,
        heap_snapshot: bool,
        deadline: Instant,
    ) -> DirectStoreResultV1<Connection> {
        ensure_deadline(deadline)?;
        let flags = OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | if readonly {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            };
        let mut uri = file_uri(&self.layout.database(), readonly);
        if heap_snapshot {
            uri.push_str("&vfs=unix-flock");
        }
        let connection = Connection::open_with_flags(uri, flags)?;
        if heap_snapshot {
            connection.execute_batch("PRAGMA locking_mode=EXCLUSIVE;")?;
        }
        configure_connection(&connection, readonly, deadline)?;
        Ok(connection)
    }

    pub(super) fn validate_connection(
        &self,
        connection: &Connection,
    ) -> DirectStoreResultV1<WorkspaceBindingV1> {
        let generation: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if generation != DIRECT_SCHEMA_GENERATION_V1 {
            return Err(if generation > DIRECT_SCHEMA_GENERATION_V1 {
                DirectStoreErrorV1::UnsupportedGeneration { generation }
            } else {
                DirectStoreErrorV1::Legacy {
                    generation: Some(generation),
                    marker: None,
                }
            });
        }
        let mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        if mode != "wal" {
            return Err(DirectStoreErrorV1::SchemaMismatch);
        }
        let writer: (String, String, i64) = connection.query_row(
            "SELECT contract_manifest_digest, generation_id, recorded_at_ms FROM writer_contract WHERE singleton = 1",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        if writer.0 != self.manifest.as_str() {
            return Err(DirectStoreErrorV1::WriterMismatch {
                expected: self.manifest.clone(),
                actual: writer.0,
            });
        }
        if writer.1.is_empty() || writer.1.len() > 64 || writer.2 < 0 {
            return Err(DirectStoreErrorV1::Unreadable);
        }
        verify_schema(connection)?;
        let binding = read_binding(connection)?;
        if binding.identity().common_dir_identity() != &self.common
            || binding.identity().worktree_admin_identity() != &self.admin
        {
            return Err(DirectStoreErrorV1::BindingMismatch);
        }
        Ok(binding)
    }

    pub(super) fn revalidate_files(
        &self,
        lease: &DirectOpenLease,
        database: &File,
    ) -> DirectStoreResultV1<()> {
        lease.revalidate(&self.layout)?;
        if !super::maintenance::same_file(database, &self.layout.database())? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        Ok(())
    }

    /// Expensive integrity checks are explicit maintenance/doctor work, not cheap admission.
    pub fn verify_integrity(&self, deadline: Instant) -> DirectStoreResultV1<()> {
        let lease = super::ExclusiveMaintenanceLeaseV1::acquire(&self.layout, deadline, false)?;
        let _connection_gate = connection_gate(deadline)?;
        super::classify_direct_store_v1(&self.layout)?.require_current()?;
        let database = super::classification::open_read_nofollow(&self.layout.database())?;
        let mut connection = self.open_readonly_connection(
            deadline,
            !super::classification::path_exists(&self.layout.shm())?,
        )?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        self.validate_connection(&transaction)?;
        let check: String = transaction.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(DirectStoreErrorV1::Unreadable);
        }
        if transaction
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some()
        {
            return Err(DirectStoreErrorV1::Unreadable);
        }
        v2_state::verify_v2_graph_state_connection_v2(&transaction)?;
        // Retained inactive graphs are reconstructed one at a time, without an archive Vec.
        let mut statement = transaction.prepare(
            "SELECT session_id FROM v2_task_sessions WHERE activity = 'inactive' ORDER BY session_id")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            ensure_deadline(deadline)?;
            let id: String = row.get(0)?;
            let id = SessionId::new(id).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            let state = v2_state::load_archived_session_connection_v2(&transaction, &id)?
                .ok_or(DirectStoreErrorV1::Unreadable)?;
            v2_state::load_terminal_dispositions_connection_v2(&transaction, &state)?;
        }
        let mut outcomes = transaction
            .prepare("SELECT idempotency_key FROM mutation_outcomes ORDER BY workspace_sequence")?;
        let mut outcomes = outcomes.query([])?;
        while let Some(row) = outcomes.next()? {
            ensure_deadline(deadline)?;
            let key: String = row.get(0)?;
            let key = IdempotencyKeyV1::new(key).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            super::transaction::read_outcome(&transaction, &key)?
                .ok_or(DirectStoreErrorV1::Unreadable)?;
        }
        lease.revalidate(&self.layout)?;
        if !super::maintenance::same_file(&database, &self.layout.database())? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        ensure_deadline(deadline)
    }
}

fn readonly_recovery(error: &DirectStoreErrorV1) -> bool {
    matches!(error, DirectStoreErrorV1::Sqlite(rusqlite::Error::SqliteFailure(error, _))
        if error.extended_code == rusqlite::ffi::SQLITE_READONLY_RECOVERY)
}

fn recovery_wait(deadline: Instant) -> DirectStoreResultV1<()> {
    ensure_deadline(deadline)?;
    std::thread::sleep(
        Duration::from_micros(200).min(deadline.saturating_duration_since(Instant::now())),
    );
    ensure_deadline(deadline)
}

/// A projection from a single WAL snapshot. Ordinary reads never expose a connection.
pub struct DirectReadSnapshotV1<'connection> {
    pub(super) connection: &'connection Connection,
    pub(super) binding: WorkspaceBindingV1,
}

impl DirectReadSnapshotV1<'_> {
    pub fn binding(&self) -> &WorkspaceBindingV1 {
        &self.binding
    }

    pub fn current_graph(&self) -> DirectStoreResultV1<Option<GraphSessionStateV2>> {
        Ok(v2_state::load_graph_session_connection_v2(self.connection)?)
    }

    pub fn outcome(
        &self,
        key: &IdempotencyKeyV1,
    ) -> DirectStoreResultV1<Option<DirectRecordedOutcomeV1>> {
        super::transaction::read_outcome(self.connection, key)
    }

    pub fn latest_outcome(&self) -> DirectStoreResultV1<Option<DirectRecordedOutcomeV1>> {
        use rusqlite::OptionalExtension;
        let key: Option<String> = self.connection.query_row(
            "SELECT idempotency_key FROM mutation_outcomes ORDER BY workspace_sequence DESC LIMIT 1",
            [], |row| row.get(0)).optional()?;
        key.map(|key| {
            let key = IdempotencyKeyV1::new(key).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            self.outcome(&key)?.ok_or(DirectStoreErrorV1::Unreadable)
        })
        .transpose()
    }

    pub fn archived_graph(
        &self,
        session: &SessionId,
    ) -> DirectStoreResultV1<Option<GraphSessionStateV2>> {
        Ok(v2_state::load_archived_session_connection_v2(
            self.connection,
            session,
        )?)
    }

    pub fn archives(&self, window: usize) -> DirectStoreResultV1<SessionArchiveListPageV2> {
        Ok(v2_state::list_archived_sessions_connection_v2(
            self.connection,
            window,
        )?)
    }

    pub fn view(&self, observed_at: UnixMillis) -> DirectStoreResultV1<DirectWorkspaceViewV1> {
        let graph = self.current_graph()?;
        let (receipts, dispositions) = match &graph {
            Some(state) => (
                v2_delegation::receipt_snapshot_connection_v1(
                    self.connection,
                    state,
                    self.binding.identity().workspace_uuid(),
                )?,
                v2_state::load_terminal_dispositions_connection_v2(self.connection, state)?,
            ),
            None => (
                v2_delegation::DelegationReceiptSnapshotV1::empty(),
                Vec::new(),
            ),
        };
        let sequence: i64 = self.connection.query_row(
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let sequence = u64::try_from(sequence).map_err(|_| DirectStoreErrorV1::Unreadable)?;
        Ok(DirectWorkspaceViewV1 {
            binding: self.binding.clone(),
            graph,
            receipts,
            dispositions,
            sequence,
            observed_at,
        })
    }
}

#[derive(Clone, Debug)]
pub struct DirectWorkspaceViewV1 {
    binding: WorkspaceBindingV1,
    graph: Option<GraphSessionStateV2>,
    receipts: v2_delegation::DelegationReceiptSnapshotV1,
    dispositions: Vec<TerminalDispositionV2>,
    sequence: u64,
    observed_at: UnixMillis,
}

impl DirectWorkspaceViewV1 {
    pub fn binding(&self) -> &WorkspaceBindingV1 {
        &self.binding
    }
    pub fn graph(&self) -> Option<&GraphSessionStateV2> {
        self.graph.as_ref()
    }
    pub fn delegation_receipts(&self) -> &v2_delegation::DelegationReceiptSnapshotV1 {
        &self.receipts
    }
    pub fn terminal_dispositions(&self) -> &[TerminalDispositionV2] {
        &self.dispositions
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn observed_at(&self) -> UnixMillis {
        self.observed_at
    }
}

pub(super) fn ensure_deadline(deadline: Instant) -> DirectStoreResultV1<()> {
    if Instant::now() >= deadline {
        return Err(DirectStoreErrorV1::Busy);
    }
    Ok(())
}

pub(super) fn configure_connection(
    connection: &Connection,
    readonly: bool,
    deadline: Instant,
) -> DirectStoreResultV1<()> {
    ensure_deadline(deadline)?;
    install_busy_deadline(connection, deadline)?;
    // Bound SQLite decoding before a corrupt persisted cell can allocate memory.
    // The canonical 1 MiB document budget includes bounded row metadata headroom.
    connection.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        1_048_576 + 16_384,
    )?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY;")?;
    if readonly {
        connection.execute_batch("PRAGMA query_only=ON;")?;
    } else {
        connection
            .execute_batch("PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=4194304;")?;
    }
    Ok(())
}

/// A Store-created, fully validated graph from one read snapshot. Callers cannot
/// construct or edit its identity/revision stamps. A transaction must revalidate
/// them before reusing this state; original-key replay still precedes that check.
#[derive(Clone, Debug)]
pub struct DirectPreparedGraphV1 {
    pub(super) root: crate::ValidatedWorkspaceRootV1,
    pub(super) manifest: Sha256Digest,
    pub(super) generation_id: String,
    pub(super) workspace: WorkspaceId,
    pub(super) sequence: u64,
    pub(super) graph: Arc<GraphSessionStateV2>,
}

impl DirectPreparedGraphV1 {
    pub fn graph(&self) -> &GraphSessionStateV2 {
        &self.graph
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl DirectStoreV1 {
    pub fn prepare_graph(
        &self,
        deadline: Instant,
    ) -> DirectStoreResultV1<Option<DirectPreparedGraphV1>> {
        self.read(deadline, |snapshot| {
            let Some(graph) = snapshot.current_graph()? else {
                return Ok(None);
            };
            let generation_id: String = snapshot.connection.query_row(
                "SELECT generation_id FROM writer_contract WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            let sequence: i64 = snapshot.connection.query_row(
                "SELECT next_workspace_sequence FROM workspace_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            Ok(Some(DirectPreparedGraphV1 {
                root: self.layout.root().clone(),
                manifest: self.manifest.clone(),
                generation_id,
                workspace: snapshot.binding.identity().workspace_uuid().clone(),
                sequence: u64::try_from(sequence).map_err(|_| DirectStoreErrorV1::Unreadable)?,
                graph: Arc::new(graph),
            }))
        })
    }
}

fn file_uri(path: &std::path::Path, readonly: bool) -> String {
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(*byte));
        } else {
            use std::fmt::Write;
            // Writing to an owned String cannot fail.
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    if readonly {
        uri.push_str("?mode=ro&readonly_shm=1");
    }
    uri
}

pub(super) fn read_binding(connection: &Connection) -> DirectStoreResultV1<WorkspaceBindingV1> {
    let (uuid, common, admin, root): (String, String, String, String) = connection.query_row(
        "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, last_validated_root FROM workspace_state WHERE singleton = 1",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
    let identity = DurableWorktreeIdentityV1::new(
        Sha256Digest::new(common).map_err(|_| DirectStoreErrorV1::Unreadable)?,
        WorkspaceId::new(uuid).map_err(|_| DirectStoreErrorV1::Unreadable)?,
        Sha256Digest::new(admin).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    );
    let root = crate::ValidatedWorkspaceRootV1::from_encoded(root)
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    if !root.to_path_buf().is_absolute() {
        return Err(DirectStoreErrorV1::Unreadable);
    }
    Ok(WorkspaceBindingV1::new_with_schema_version(
        identity,
        root,
        DIRECT_SCHEMA_GENERATION_V1,
    ))
}

type SchemaInventory = Vec<(String, String, String, String)>;
static EXPECTED_SCHEMA: OnceLock<Result<SchemaInventory, String>> = OnceLock::new();

pub(super) fn verify_schema(connection: &Connection) -> DirectStoreResultV1<()> {
    let expected = EXPECTED_SCHEMA
        .get_or_init(|| {
            let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
            connection
                .execute_batch(DIRECT_STORE_DDL_V1)
                .map_err(|error| error.to_string())?;
            schema_inventory(&connection).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|_| DirectStoreErrorV1::SchemaMismatch)?;
    if &schema_inventory(connection)? != expected {
        return Err(DirectStoreErrorV1::SchemaMismatch);
    }
    Ok(())
}

/// A predecessor supplies its own reviewed DDL. Qualification must not assume
/// the current schema before the migration has run.
pub(super) fn verify_declared_schema(
    connection: &Connection,
    ddl: &str,
) -> DirectStoreResultV1<()> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(ddl)?;
    if schema_inventory(connection)? != schema_inventory(&reference)? {
        return Err(DirectStoreErrorV1::SchemaMismatch);
    }
    Ok(())
}

fn schema_inventory(connection: &Connection) -> DirectStoreResultV1<SchemaInventory> {
    // Bound both collection count and each returned cell before SQLite allocates it.
    let mut statement = connection.prepare(
        "SELECT substr(type,1,128), substr(name,1,256), substr(tbl_name,1,256), substr(coalesce(sql,''),1,65537) \
         FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name LIMIT 257")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let inventory = rows.collect::<Result<Vec<_>, _>>()?;
    if inventory.len() > 256 || inventory.iter().any(|row| row.3.len() > 65_536) {
        return Err(DirectStoreErrorV1::SchemaMismatch);
    }
    Ok(inventory)
}
