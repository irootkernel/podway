use std::{
    cell::{Cell, RefCell},
    fs::File,
    io::Read,
    os::unix::ffi::OsStrExt,
    sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use podway_core::{
    AttemptId, SessionId, Sha256Digest, TerminalDispositionV2, UnixMillis, WorkspaceId,
};
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
        match CONNECTION_GATE.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(DirectStoreErrorV1::ConnectionBusy);
                }
                std::thread::sleep(
                    Duration::from_micros(500)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
                if Instant::now() >= deadline {
                    return Err(DirectStoreErrorV1::ConnectionBusy);
                }
            }
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
        let lease = DirectOpenLease::acquire(&self.layout, deadline, &mut || {
            self.boundary(DirectTransactionBoundaryV1::UnmarkedMissingReadClassified)
        })?;
        let _connection_gate = connection_gate(deadline)?;
        let database = super::classification::open_read_nofollow(&self.layout.database())?;
        self.qualified_read(deadline, lease.heap_snapshot(), |transaction, binding| {
            self.revalidate_files(&lease, &database)?;
            let snapshot = DirectReadSnapshotV1 {
                connection: transaction,
                binding,
                staged_current: RefCell::new(StagedCurrentAdmissionV1::Unadmitted),
            };
            let result = read(&snapshot)?;
            self.revalidate_files(&lease, &database)?;
            Ok(result)
        })
    }

    /// Qualified coherent read while the engine owns an exact maintenance intent.
    /// This never acquires a nested lock or resumes/removes a marker.
    pub fn read_under_exclusive_lease<T>(
        &self,
        lease: &super::ExclusiveMaintenanceLeaseV1,
        deadline: Instant,
        read: impl FnOnce(&DirectReadSnapshotV1<'_>) -> DirectStoreResultV1<T>,
    ) -> DirectStoreResultV1<T> {
        (|| {
            lease.revalidate(&self.layout)?;
            super::classification::classify_unmarked_store_v1(&self.layout)?.require_current()?;
            let _gate = connection_gate(deadline)?;
            let database = super::classification::open_read_nofollow(&self.layout.database())?;
            let result = (|| {
                let (mut connection, immutable) = self
                    .open_readonly_connection(
                        deadline,
                        !super::classification::path_exists(&self.layout.wal())?
                            || !super::classification::path_exists(&self.layout.shm())?,
                    )
                    .map_err(invalid_admission_error)?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Deferred)
                    .map_err(DirectStoreErrorV1::from)
                    .map_err(invalid_admission_error)?;
                let binding = self.validate_connection_with_mode(&transaction, immutable)?;
                read(&DirectReadSnapshotV1 {
                    connection: &transaction,
                    binding,
                    staged_current: RefCell::new(StagedCurrentAdmissionV1::Unadmitted),
                })
                .map_err(invalid_admission_error)
            })();
            // Invalid admission can authorize confirmed recovery only for this exact file.
            lease.revalidate(&self.layout)?;
            if !super::maintenance::same_file(&database, &self.layout.database())? {
                return Err(DirectStoreErrorV1::Maintenance);
            }
            result
        })()
        .map_err(read_admission_error)
    }

    /// Resume only a committed private bootstrap generation owned by the exact
    /// engine intent. Fixed sidecars must be absent or hard links to that private
    /// generation; an unrelated publication is never replaced or removed.
    pub fn recover_bootstrap_publication(
        &self,
        lease: &super::ExclusiveMaintenanceLeaseV1,
        workspace: &WorkspaceId,
        generation: &str,
        request: &super::DirectMutationRequestV1,
        deadline: Instant,
    ) -> DirectStoreResultV1<Option<DirectRecordedOutcomeV1>> {
        use std::fs;
        lease.revalidate(&self.layout)?;
        if super::classification::path_exists(&self.layout.database())? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        let _gate = connection_gate(deadline)?;
        let mut owned = None;
        for (index, entry) in fs::read_dir(self.layout.runtime_directory())?.enumerate() {
            if index >= 1024 {
                return Err(DirectStoreErrorV1::Unreadable);
            }
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(".direct-store-")
                || name.ends_with("-wal")
                || name.ends_with("-shm")
            {
                continue;
            }
            let path = entry.path();
            let file = super::classification::open_read_nofollow(&path)?;
            let connection = match Connection::open_with_flags(
                file_uri(&path, true),
                OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            ) {
                Ok(connection) => connection,
                Err(_) => continue,
            };
            configure_connection(&connection, true, deadline)?;
            let Ok(binding) = self.validate_connection(&connection) else {
                continue;
            };
            let found_generation: String = connection.query_row(
                "SELECT generation_id FROM writer_contract WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            if found_generation != generation
                || binding.identity().workspace_uuid() != workspace
                || binding.last_validated_root() != self.layout.root()
            {
                continue;
            }
            let Some(outcome) = super::transaction::read_outcome(&connection, request.key())?
            else {
                continue;
            };
            if outcome.digest() != request.digest() || outcome.command() != request.command() {
                return Err(DirectStoreErrorV1::IdempotencyKeyReused);
            }
            if owned.is_some() {
                return Err(DirectStoreErrorV1::Maintenance);
            }
            drop(connection);
            // A committed WAL can be published with its exact main/index files;
            // no recovery writes or conversion are necessary.
            let mut sidecars = Vec::new();
            for suffix in ["-wal", "-shm"] {
                let source = super::sidecar(&path, suffix);
                let source_file = super::classification::open_read_nofollow(&source)?;
                let target = super::sidecar(&self.layout.database(), suffix);
                if super::classification::path_exists(&target)?
                    && !super::maintenance::same_file(&source_file, &target)?
                {
                    return Err(DirectStoreErrorV1::Maintenance);
                }
                sidecars.push((source, source_file, target));
            }
            owned = Some((path, file, sidecars, outcome));
        }
        let Some((path, file, sidecars, outcome)) = owned else {
            return Ok(None);
        };
        lease.revalidate(&self.layout)?;
        if !super::maintenance::same_file(&file, &path)? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        for (source, source_file, target) in &sidecars {
            if !super::maintenance::same_file(source_file, source)? {
                return Err(DirectStoreErrorV1::Maintenance);
            }
            if !super::classification::path_exists(target)? {
                fs::hard_link(source, target)?;
            }
            if !super::maintenance::same_file(source_file, target)? {
                return Err(DirectStoreErrorV1::Maintenance);
            }
            source_file.sync_all()?;
        }
        file.sync_all()?;
        fs::hard_link(&path, self.layout.database())?;
        super::maintenance::open_directory(&self.layout.runtime_directory())?.sync_all()?;
        // Remove only the exact private inodes after durable main publication.
        if super::maintenance::same_file(&file, &path)? {
            fs::remove_file(path)?;
        }
        for (source, source_file, _) in sidecars {
            if super::maintenance::same_file(&source_file, &source)? {
                fs::remove_file(source)?;
            }
        }
        super::maintenance::open_directory(&self.layout.runtime_directory())?.sync_all()?;
        Ok(Some(outcome))
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
            let (mut connection, immutable) =
                match self.open_readonly_connection(deadline, heap_snapshot) {
                    Ok(connection) => connection,
                    Err(error) if readonly_recovery(&error) => {
                        recovery_wait(deadline)?;
                        continue;
                    }
                    Err(error) => return Err(read_admission_error(error)),
                };
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(DirectStoreErrorV1::from)
                .map_err(read_admission_error)?;
            match self.validate_connection_with_mode(&transaction, immutable) {
                Ok(binding) => return read(&transaction, binding).map_err(read_admission_error),
                Err(error) if readonly_recovery(&error) => {
                    drop(transaction);
                    drop(connection);
                    recovery_wait(deadline)?;
                }
                Err(error) => return Err(read_admission_error(error)),
            }
        }
    }

    pub(super) fn open_connection(
        &self,
        readonly: bool,
        deadline: Instant,
    ) -> DirectStoreResultV1<Connection> {
        self.open_sqlite_connection(readonly, false, false, deadline)
    }

    /// Cold snapshots require an already held exclusive native lease. Without
    /// WAL, immutable mode avoids creating it. With WAL, unix-flock supplies
    /// exclusive locking without a write lock on an O_RDONLY POSIX descriptor.
    pub(super) fn open_readonly_connection(
        &self,
        deadline: Instant,
        heap_snapshot: bool,
    ) -> DirectStoreResultV1<(Connection, bool)> {
        // The exclusive native lease makes an absent-WAL main file immutable
        // for this snapshot. Never use immutable mode while a WAL is present:
        // SQLite would omit its committed frames.
        let immutable = heap_snapshot && !super::classification::path_exists(&self.layout.wal())?;
        if immutable {
            let mut header = [0; 20];
            super::classification::open_read_nofollow(&self.layout.database())?
                .read_exact(&mut header)?;
            // Immutable SQLite reports journal_mode=delete. Qualify the actual
            // persistent WAL format before accepting that connection-local mode.
            if &header[..16] != b"SQLite format 3\0" || header[18..20] != [2, 2] {
                return Err(DirectStoreErrorV1::SchemaMismatch);
            }
        }
        Ok((
            self.open_sqlite_connection(true, heap_snapshot, immutable, deadline)?,
            immutable,
        ))
    }

    fn open_sqlite_connection(
        &self,
        readonly: bool,
        heap_snapshot: bool,
        immutable: bool,
        deadline: Instant,
    ) -> DirectStoreResultV1<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | if readonly {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            };
        let mut uri = file_uri(&self.layout.database(), readonly);
        if immutable {
            uri.push_str("&immutable=1");
        } else if heap_snapshot {
            uri.push_str("&vfs=unix-flock");
        }
        let connection = Connection::open_with_flags(uri, flags)?;
        if heap_snapshot && !immutable {
            connection.execute_batch("PRAGMA locking_mode=EXCLUSIVE;")?;
        }
        configure_connection(&connection, readonly, deadline)?;
        Ok(connection)
    }

    pub(super) fn validate_connection(
        &self,
        connection: &Connection,
    ) -> DirectStoreResultV1<WorkspaceBindingV1> {
        self.validate_connection_with_mode(connection, false)
    }

    pub(super) fn validate_connection_with_mode(
        &self,
        connection: &Connection,
        immutable: bool,
    ) -> DirectStoreResultV1<WorkspaceBindingV1> {
        self.validate_current_connection(connection, immutable)
            .map_err(invalid_admission_error)
    }

    fn validate_current_connection(
        &self,
        connection: &Connection,
        immutable: bool,
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
        if mode != if immutable { "delete" } else { "wal" } {
            return Err(DirectStoreErrorV1::SchemaMismatch);
        }
        verify_schema(connection)?;
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
        self.verify_integrity_under_exclusive_lease(&lease, deadline, |_| Ok(()))
    }

    /// Diagnose and project the exact same committed snapshot under exclusion.
    pub fn verify_integrity_under_exclusive_lease<T>(
        &self,
        lease: &super::ExclusiveMaintenanceLeaseV1,
        deadline: Instant,
        read: impl FnOnce(&DirectReadSnapshotV1<'_>) -> DirectStoreResultV1<T>,
    ) -> DirectStoreResultV1<T> {
        (|| {
            lease.revalidate(&self.layout)?;
            let _connection_gate = connection_gate(deadline)?;
            super::classify_direct_store_v1(&self.layout)?.require_current()?;
            let database = super::classification::open_read_nofollow(&self.layout.database())?;
            let (mut connection, immutable) = self.open_readonly_connection(
                deadline,
                !super::classification::path_exists(&self.layout.wal())?
                    || !super::classification::path_exists(&self.layout.shm())?,
            )?;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let binding = self.validate_connection_with_mode(&transaction, immutable)?;
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
            let admission = v2_state::verify_v2_graph_state_connection_v2(&transaction)?;
            // Retained inactive graphs are reconstructed one at a time, without an archive Vec.
            let mut statement = transaction.prepare(
                "SELECT session_id FROM v2_task_sessions WHERE activity = 'inactive' ORDER BY session_id")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let id = SessionId::new(id).map_err(|_| DirectStoreErrorV1::Unreadable)?;
                v2_state::load_archived_session_connection_v2(&transaction, &id)?
                    .ok_or(DirectStoreErrorV1::Unreadable)?;
            }
            let mut outcomes = transaction
                .prepare("SELECT idempotency_key FROM mutation_outcomes ORDER BY workspace_sequence")?;
            let mut outcomes = outcomes.query([])?;
            while let Some(row) = outcomes.next()? {
                let key: String = row.get(0)?;
                let key = IdempotencyKeyV1::new(key).map_err(|_| DirectStoreErrorV1::Unreadable)?;
                super::transaction::read_outcome(&transaction, &key)?
                    .ok_or(DirectStoreErrorV1::Unreadable)?;
            }
            let snapshot = DirectReadSnapshotV1 {
                connection: &transaction,
                binding,
                // Seeded only here, after every verifier check: the callback
                // reuses the admission this verifier already validated.
                staged_current: RefCell::new(StagedCurrentAdmissionV1::Admitted(Box::new(
                    admission,
                ))),
            };
            let result = read(&snapshot)?;
            lease.revalidate(&self.layout)?;
            if !super::maintenance::same_file(&database, &self.layout.database())? {
                return Err(DirectStoreErrorV1::Maintenance);
            }
            Ok(result)
        })().map_err(read_admission_error)
    }
}

fn readonly_recovery(error: &DirectStoreErrorV1) -> bool {
    matches!(error, DirectStoreErrorV1::Sqlite(rusqlite::Error::SqliteFailure(error, _))
        if error.extended_code == rusqlite::ffi::SQLITE_READONLY_RECOVERY)
}

fn recovery_wait(deadline: Instant) -> DirectStoreResultV1<()> {
    if Instant::now() >= deadline {
        return Err(DirectStoreErrorV1::ReadAdmissionBusy);
    }
    std::thread::sleep(
        Duration::from_micros(200).min(deadline.saturating_duration_since(Instant::now())),
    );
    if Instant::now() >= deadline {
        return Err(DirectStoreErrorV1::ReadAdmissionBusy);
    }
    Ok(())
}

pub(super) fn read_admission_error(error: DirectStoreErrorV1) -> DirectStoreErrorV1 {
    match error {
        DirectStoreErrorV1::Busy => DirectStoreErrorV1::ReadAdmissionBusy,
        error => error,
    }
}

fn invalid_admission_error(error: DirectStoreErrorV1) -> DirectStoreErrorV1 {
    match error {
        DirectStoreErrorV1::State(crate::StoreErrorV1::CorruptStateV1 { .. }) => {
            DirectStoreErrorV1::Unreadable
        }
        DirectStoreErrorV1::Sqlite(
            rusqlite::Error::QueryReturnedNoRows
            | rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
            | rusqlite::Error::FromSqlConversionFailure(..),
        ) => DirectStoreErrorV1::Unreadable,
        DirectStoreErrorV1::Sqlite(rusqlite::Error::SqliteFailure(code, _))
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            DirectStoreErrorV1::Unreadable
        }
        error => error,
    }
}

/// A projection from a single WAL snapshot. Ordinary reads never expose a connection.
pub struct DirectReadSnapshotV1<'connection> {
    pub(super) connection: &'connection Connection,
    pub(super) binding: WorkspaceBindingV1,
    /// One admitted current graph staged for later reuse in this snapshot.
    staged_current: RefCell<StagedCurrentAdmissionV1>,
}

/// Staged reuse state for the current graph. `Admitted(None)` records an
/// admitted empty store so an empty snapshot is not re-admitted.
enum StagedCurrentAdmissionV1 {
    Unadmitted,
    Admitted(Box<Option<v2_state::AdmittedGraphStateV2>>),
}

impl DirectReadSnapshotV1<'_> {
    pub fn binding(&self) -> &WorkspaceBindingV1 {
        &self.binding
    }

    pub fn committed_sequence(&self) -> DirectStoreResultV1<u64> {
        let sequence: i64 = crate::query_accounting::accounted_query_row(
            self.connection,
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        u64::try_from(sequence).map_err(|_| DirectStoreErrorV1::Unreadable)
    }

    pub fn current_graph(&self) -> DirectStoreResultV1<Option<GraphSessionStateV2>> {
        Ok(v2_state::load_graph_session_connection_v2(self.connection)?)
    }

    /// Lends this snapshot's admitted current graph, admitting it first when
    /// this snapshot has not admitted one yet. The admitted graph and its
    /// terminal dispositions stay staged for a later [`Self::view`] call in
    /// the same snapshot.
    ///
    /// Re-entrancy contract: `body` must not call [`Self::view`] on the same
    /// snapshot. The staged admission is borrowed shared for `body`'s
    /// duration, so such a call is a programming error and panics on that
    /// borrow. A nested `with_admitted_current_graph` only re-lends the same
    /// admitted graph.
    pub fn with_admitted_current_graph<T, E>(
        &self,
        body: impl FnOnce(Option<&GraphSessionStateV2>) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<DirectStoreErrorV1>,
    {
        if matches!(
            &*self.staged_current.borrow(),
            StagedCurrentAdmissionV1::Unadmitted
        ) {
            let admitted = v2_state::load_graph_session_admission_connection_v2(self.connection)
                .map_err(DirectStoreErrorV1::from)?;
            *self.staged_current.borrow_mut() =
                StagedCurrentAdmissionV1::Admitted(Box::new(admitted));
        }
        let staged = self.staged_current.borrow();
        let graph = match &*staged {
            StagedCurrentAdmissionV1::Admitted(admitted) => {
                admitted.as_ref().as_ref().map(|admitted| &admitted.graph)
            }
            StagedCurrentAdmissionV1::Unadmitted => None,
        };
        body(graph)
    }

    pub fn outcome(
        &self,
        key: &IdempotencyKeyV1,
    ) -> DirectStoreResultV1<Option<DirectRecordedOutcomeV1>> {
        super::transaction::read_outcome(self.connection, key)
    }

    pub fn latest_outcome(&self) -> DirectStoreResultV1<Option<DirectRecordedOutcomeV1>> {
        use rusqlite::OptionalExtension;
        let key: Option<String> = crate::query_accounting::accounted_query_row(
            self.connection,
            "SELECT idempotency_key FROM mutation_outcomes ORDER BY workspace_sequence DESC LIMIT 1",
            [], |row| row.get(0),
        )
        .optional()?;
        key.map(|key| {
            let key = IdempotencyKeyV1::new(key).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            self.outcome(&key)?.ok_or(DirectStoreErrorV1::Unreadable)
        })
        .transpose()
    }

    /// Bounded, read-only qualification inventory from this exact WAL snapshot.
    #[doc(hidden)]
    pub fn retained_outcome_keys_for_qualification_v1(
        &self,
        maximum: usize,
    ) -> DirectStoreResultV1<Vec<IdempotencyKeyV1>> {
        if maximum == 0 || maximum > 10_000 {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "qualification outcome inventory bound is invalid",
            });
        }
        let outcome_keys_sql =
            "SELECT idempotency_key FROM mutation_outcomes ORDER BY workspace_sequence LIMIT ?1";
        let mut statement =
            crate::query_accounting::accounted_prepare(self.connection, outcome_keys_sql)?;
        let rows = crate::query_accounting::accounted_query_map_prepared(
            &mut statement,
            outcome_keys_sql,
            [i64::try_from(maximum + 1).expect("bounded qualification inventory")],
            |row| row.get::<_, String>(0),
        )?;
        let mut keys = Vec::new();
        for row in rows {
            if keys.len() == maximum {
                return Err(DirectStoreErrorV1::InvalidInput {
                    reason: "qualification outcome inventory exceeds its bound",
                });
            }
            keys.push(IdempotencyKeyV1::new(row?).map_err(|_| DirectStoreErrorV1::Unreadable)?);
        }
        Ok(keys)
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

    pub fn archive_summary(
        &self,
        session: &SessionId,
    ) -> DirectStoreResultV1<Option<crate::SessionArchiveSummaryV2>> {
        Ok(v2_state::archived_session_summary_connection_v2(
            self.connection,
            session,
        )?)
    }

    pub fn delegation_receipt(
        &self,
        session: &SessionId,
        attempt: &AttemptId,
    ) -> DirectStoreResultV1<Option<crate::DelegationReceiptV1>> {
        Ok(v2_delegation::read_receipt_connection_v1(
            self.connection,
            self.binding.identity().workspace_uuid(),
            session,
            attempt,
        )?)
    }

    pub fn archived_view(
        &self,
        session: &SessionId,
        observed_at: UnixMillis,
    ) -> DirectStoreResultV1<Option<DirectWorkspaceViewV1>> {
        let Some(admitted) =
            v2_state::load_archived_session_admission_connection_v2(self.connection, session)?
        else {
            return Ok(None);
        };
        let receipts = v2_delegation::receipt_snapshot_connection_v1(
            self.connection,
            &admitted.graph,
            self.binding.identity().workspace_uuid(),
            v2_delegation::DelegationReadinessAcquisitionV1::AdmittedWindow,
        )?;
        Ok(Some(DirectWorkspaceViewV1 {
            binding: self.binding.clone(),
            graph: Some(admitted.graph),
            receipts,
            dispositions: admitted.dispositions,
            sequence: self.committed_sequence()?,
            observed_at,
        }))
    }

    pub fn archives(&self, window: usize) -> DirectStoreResultV1<SessionArchiveListPageV2> {
        Ok(v2_state::list_archived_sessions_connection_v2(
            self.connection,
            window,
        )?)
    }

    pub fn view(&self, observed_at: UnixMillis) -> DirectStoreResultV1<DirectWorkspaceViewV1> {
        // Take any staged admission; otherwise admit once here. A shared
        // borrow from `with_admitted_current_graph` makes this take panic
        // instead of silently re-admitting.
        let staged = std::mem::replace(
            &mut *self.staged_current.borrow_mut(),
            StagedCurrentAdmissionV1::Unadmitted,
        );
        let admitted = match staged {
            StagedCurrentAdmissionV1::Admitted(admitted) => *admitted,
            StagedCurrentAdmissionV1::Unadmitted => {
                v2_state::load_graph_session_admission_connection_v2(self.connection)?
            }
        };
        let (receipts, dispositions, graph) = match admitted {
            Some(admitted) => {
                let receipts = v2_delegation::receipt_snapshot_connection_v1(
                    self.connection,
                    &admitted.graph,
                    self.binding.identity().workspace_uuid(),
                    v2_delegation::DelegationReadinessAcquisitionV1::AdmittedWindow,
                )?;
                (receipts, admitted.dispositions, Some(admitted.graph))
            }
            None => (
                v2_delegation::DelegationReceiptSnapshotV1::empty(),
                Vec::new(),
                None,
            ),
        };
        let sequence = self.committed_sequence()?;
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

pub(super) fn configure_connection(
    connection: &Connection,
    readonly: bool,
    deadline: Instant,
) -> DirectStoreResultV1<()> {
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
            let generation_id: String = crate::query_accounting::accounted_query_row(
                snapshot.connection,
                "SELECT generation_id FROM writer_contract WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            let sequence: i64 = crate::query_accounting::accounted_query_row(
                snapshot.connection,
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
    let (uuid, common, admin, root): (String, String, String, String) =
        crate::query_accounting::accounted_query_row(
            connection,
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, last_validated_root FROM workspace_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
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
