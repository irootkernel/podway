use std::{cell::OnceCell, fs, sync::Arc, time::Instant};

use podway_core::{
    Revision, SessionId, Sha256Digest, TerminalDispositionV2, UnixMillis, WorkspaceId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    CanonicalExecutionJsonV1, DelegationReceiptV1, GraphSessionStateV2, IdempotencyKeyV1,
    SessionArchiveSummaryV2, WorkspaceBindingV1, v2_delegation, v2_state,
};

use super::{
    DirectStoreErrorV1, DirectStoreResultV1, DirectStoreV1, ExclusiveMaintenanceLeaseV1,
    connection::configure_connection, maintenance::DirectOpenLease,
};

/// Keeps caller failures distinct from transaction and persistence failures.
#[derive(Debug)]
pub enum DirectMutationFailureV1<E> {
    Store(DirectStoreErrorV1),
    Callback(E),
}

impl<E: From<DirectStoreErrorV1>> DirectMutationFailureV1<E> {
    pub fn into_error(self) -> E {
        match self {
            Self::Store(error) => error.into(),
            Self::Callback(error) => error,
        }
    }
}

impl<E> From<DirectStoreErrorV1> for DirectMutationFailureV1<E> {
    fn from(error: DirectStoreErrorV1) -> Self {
        Self::Store(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectOutcomeScopeV1 {
    Workspace,
    Session(SessionId),
}

#[derive(Clone, Debug)]
pub struct DirectMutationRequestV1 {
    key: IdempotencyKeyV1,
    digest: Sha256Digest,
    command: String,
    response_id: String,
    scope: DirectOutcomeScopeV1,
}

impl DirectMutationRequestV1 {
    pub fn new(
        key: IdempotencyKeyV1,
        digest: Sha256Digest,
        command: impl Into<String>,
        response_id: impl Into<String>,
        scope: DirectOutcomeScopeV1,
    ) -> DirectStoreResultV1<Self> {
        let command = command.into();
        let response_id = response_id.into();
        if command.is_empty()
            || command.len() > 128
            || !command
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || matches!(byte, b'.' | b'_'))
        {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "invalid mutation command",
            });
        }
        // Reuse the core canonical UUID validator without adding a protocol dependency.
        WorkspaceId::new(response_id.clone()).map_err(|_| DirectStoreErrorV1::InvalidInput {
            reason: "invalid response UUID",
        })?;
        Ok(Self {
            key,
            digest,
            command,
            response_id,
            scope,
        })
    }
    pub fn key(&self) -> &IdempotencyKeyV1 {
        &self.key
    }
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub fn command(&self) -> &str {
        &self.command
    }
    pub fn response_id(&self) -> &str {
        &self.response_id
    }
    pub fn scope(&self) -> &DirectOutcomeScopeV1 {
        &self.scope
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectRecordedOutcomeV1 {
    digest: Sha256Digest,
    command: String,
    sequence: u64,
    scope: DirectOutcomeScopeV1,
    response: CanonicalExecutionJsonV1,
    recorded_at: UnixMillis,
}

impl DirectRecordedOutcomeV1 {
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub fn command(&self) -> &str {
        &self.command
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn scope(&self) -> &DirectOutcomeScopeV1 {
        &self.scope
    }
    pub fn response(&self) -> &CanonicalExecutionJsonV1 {
        &self.response
    }
    pub fn recorded_at(&self) -> UnixMillis {
        self.recorded_at
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectMutationResultV1 {
    outcome: DirectRecordedOutcomeV1,
    replayed: bool,
}

impl DirectMutationResultV1 {
    pub fn outcome(&self) -> &DirectRecordedOutcomeV1 {
        &self.outcome
    }
    pub fn replayed(&self) -> bool {
        self.replayed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectTransactionBoundaryV1 {
    /// Missing files, final absence of maintenance markers and pinned parent validation.
    UnmarkedMissingReadClassified,
    BeforeBegin,
    AfterBegin,
    AfterStateBeforeOutcome,
    AfterCommitBeforeOutput,
    BeforeMaintenancePublication,
}

/// The connection and transaction are private. Only validated domain operations
/// and a lazily read injected clock are available to the engine callback.
pub struct DirectTransactionV1<'transaction, 'connection, Clock> {
    transaction: &'transaction Transaction<'connection>,
    binding: WorkspaceBindingV1,
    root: crate::ValidatedWorkspaceRootV1,
    previous: OnceCell<Option<Arc<GraphSessionStateV2>>>,
    state_written: bool,
    clock: Option<Clock>,
    now: Option<UnixMillis>,
    created_session_scope: Option<SessionId>,
}

impl<Clock: FnOnce() -> UnixMillis> DirectTransactionV1<'_, '_, Clock> {
    pub fn binding(&self) -> &WorkspaceBindingV1 {
        &self.binding
    }

    pub fn delegation_receipts(
        &self,
    ) -> DirectStoreResultV1<v2_delegation::DelegationReceiptSnapshotV1> {
        let Some(graph) = self.current_graph()? else {
            return Ok(v2_delegation::DelegationReceiptSnapshotV1::empty());
        };
        Ok(v2_delegation::receipt_snapshot_connection_v1(
            self.transaction,
            graph,
            self.binding.identity().workspace_uuid(),
        )?)
    }

    pub fn terminal_dispositions(&self) -> DirectStoreResultV1<Vec<TerminalDispositionV2>> {
        let Some(graph) = self.current_graph()? else {
            return Ok(Vec::new());
        };
        Ok(v2_state::load_terminal_dispositions_connection_v2(
            self.transaction,
            graph,
        )?)
    }

    pub fn archive_summary(
        &self,
        session: &SessionId,
    ) -> DirectStoreResultV1<Option<SessionArchiveSummaryV2>> {
        Ok(v2_state::archived_session_summary_connection_v2(
            self.transaction,
            session,
        )?)
    }

    /// A fresh prepared session has no identity until the Engine creates it after fences.
    pub fn scope_created_session_outcome(
        &mut self,
        session: &SessionId,
    ) -> DirectStoreResultV1<()> {
        self.require_clock()?;
        if !self.state_written
            || self.created_session_scope.is_some()
            || !self.current_graph()?.is_some_and(|graph| {
                graph.trace().session_id() == session
                    && graph.trace().lifecycle() == podway_core::SessionLifecycle::Prepared
                    && graph.trace().revision() == Revision::ZERO
            })
        {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "session outcome scope requires a freshly created prepared session",
            });
        }
        self.created_session_scope = Some(session.clone());
        Ok(())
    }

    /// The sequence this success will commit, reserved by the IMMEDIATE writer.
    /// It is independent of graph revisions and does not read the command clock.
    pub fn next_workspace_sequence(&self) -> DirectStoreResultV1<u64> {
        let sequence: i64 = self.transaction.query_row(
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        sequence
            .checked_add(1)
            .filter(|_| sequence >= 0)
            .and_then(|sequence| u64::try_from(sequence).ok())
            .ok_or(DirectStoreErrorV1::Unreadable)
    }

    /// The engine calls this only after fences and prepared inputs are revalidated.
    /// Further calls return the cached value and never read the injected clock again.
    pub fn now(&mut self) -> UnixMillis {
        if let Some(now) = self.now {
            return now;
        }
        let now = self
            .clock
            .take()
            .expect("direct transaction clock is read once")();
        self.now = Some(now);
        now
    }

    pub fn current_graph(&self) -> DirectStoreResultV1<Option<&GraphSessionStateV2>> {
        if self.previous.get().is_none() {
            let graph = v2_state::load_graph_session_connection_v2(self.transaction)?.map(Arc::new);
            let _ = self.previous.set(graph);
        }
        Ok(self.previous.get().and_then(Option::as_deref))
    }

    /// Select transaction-current state before clock/state effects. Reuse the
    /// prepared Arc only when its sequence and graph stamps are unchanged.
    pub fn use_prepared_graph(
        &mut self,
        prepared: Option<&super::DirectPreparedGraphV1>,
    ) -> DirectStoreResultV1<Option<Arc<GraphSessionStateV2>>> {
        if self.state_written || self.now.is_some() {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "prepared graph must be validated before clock or state effects",
            });
        }
        let Some(prepared) = prepared else {
            self.current_graph()?;
            return Ok(self.previous.get().cloned().flatten());
        };
        let (sequence, generation, manifest, uuid, root): (i64,String,String,String,String) = self.transaction.query_row(
            "SELECT w.next_workspace_sequence,c.generation_id,c.contract_manifest_digest,w.workspace_uuid,w.last_validated_root \
             FROM workspace_state w JOIN writer_contract c ON c.singleton=w.singleton WHERE w.singleton=1",
            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
        let sequence = u64::try_from(sequence).map_err(|_| DirectStoreErrorV1::Unreadable)?;
        if generation != prepared.generation_id
            || manifest != prepared.manifest.as_str()
            || uuid != prepared.workspace.as_str()
            || prepared.root != self.root
            || self.binding.last_validated_root().as_encoded() != root
        {
            return Err(DirectStoreErrorV1::BindingMismatch);
        }
        let stamps: Option<(String,i64,i64,String,String)> = self.transaction.query_row(
            "SELECT s.session_id,s.session_revision,w.workspace_revision,s.procedure_snapshot_id,p.digest \
             FROM v2_workspace_state w JOIN v2_task_sessions s ON s.session_id=w.current_session_id \
             JOIN v2_procedure_snapshots p ON p.snapshot_id=s.procedure_snapshot_id WHERE w.singleton=1",
            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
        let graph = prepared.graph();
        if sequence != prepared.sequence
            || stamps.as_ref().is_none_or(
                |(id, session_revision, workspace_revision, snapshot, digest)| {
                    id != graph.trace().session_id().as_str()
                        || u64::try_from(*session_revision).ok()
                            != Some(graph.trace().revision().get())
                        || u64::try_from(*workspace_revision).ok()
                            != Some(graph.workspace_revision().get())
                        || snapshot != graph.snapshot().snapshot_id().as_str()
                        || digest != graph.snapshot().digest().as_str()
                },
            )
        {
            let current =
                v2_state::load_graph_session_with_cache_v2(self.transaction, Some(graph))?
                    .map(Arc::new);
            self.previous.take();
            let _ = self.previous.set(current);
            return Ok(self.previous.get().cloned().flatten());
        }
        self.previous.take();
        let _ = self.previous.set(Some(Arc::clone(&prepared.graph)));
        Ok(Some(Arc::clone(&prepared.graph)))
    }

    fn require_clock(&self) -> DirectStoreResultV1<UnixMillis> {
        self.now.ok_or(DirectStoreErrorV1::InvalidInput {
            reason: "read the clock after validation before persisting state",
        })
    }

    pub fn create_graph(&mut self, state: &GraphSessionStateV2) -> DirectStoreResultV1<()> {
        self.require_clock()?;
        self.state_written = true;
        v2_state::create_graph_session_transaction_v2(self.transaction, state)?;
        self.previous.take();
        let _ = self.previous.set(Some(Arc::new(state.clone())));
        Ok(())
    }

    pub fn replace_graph(
        &mut self,
        workspace_revision: Revision,
        session_revision: Revision,
        next: &GraphSessionStateV2,
    ) -> DirectStoreResultV1<()> {
        self.replace(workspace_revision, session_revision, next, false)
    }

    fn replace(
        &mut self,
        workspace_revision: Revision,
        session_revision: Revision,
        next: &GraphSessionStateV2,
        delegation: bool,
    ) -> DirectStoreResultV1<()> {
        self.require_clock()?;
        let previous = self
            .current_graph()?
            .ok_or(DirectStoreErrorV1::InvalidInput {
                reason: "no current graph",
            })?;
        v2_state::replace_direct_graph_session_transaction_v2(
            self.transaction,
            previous,
            workspace_revision,
            session_revision,
            next,
            delegation,
        )?;
        self.state_written = true;
        self.previous.take();
        let _ = self.previous.set(Some(Arc::new(next.clone())));
        Ok(())
    }

    pub fn accept_delegation(
        &mut self,
        workspace_revision: Revision,
        session_revision: Revision,
        next: &GraphSessionStateV2,
        expected_package_digest: &Sha256Digest,
        receipt: &DelegationReceiptV1,
    ) -> DirectStoreResultV1<()> {
        let current = self
            .current_graph()?
            .ok_or(DirectStoreErrorV1::InvalidInput {
                reason: "no current graph",
            })?;
        v2_delegation::validate_acceptance_v1(
            current,
            self.binding.identity().workspace_uuid(),
            expected_package_digest,
            receipt,
        )?;
        self.replace(workspace_revision, session_revision, next, true)?;
        v2_delegation::insert_receipt_transaction_v1(
            self.transaction,
            receipt,
            next,
            self.binding.identity().workspace_uuid(),
        )?;
        Ok(())
    }

    pub fn clear_graph(
        &mut self,
        workspace_revision: Revision,
        session_revision: Revision,
    ) -> DirectStoreResultV1<()> {
        self.require_clock()?;
        self.state_written = true;
        let session = self
            .current_graph()?
            .map(|state| state.trace().session_id().clone());
        v2_state::clear_graph_session_transaction_v2(
            self.transaction,
            self.current_graph()?,
            workspace_revision,
            session_revision,
        )?;
        if let Some(session) = session {
            self.transaction.execute(
                "DELETE FROM mutation_outcomes WHERE scope_kind='session' AND scope_session_id=?1",
                [session.as_str()],
            )?;
        }
        self.previous.take();
        Ok(())
    }

    pub fn archive_graph(
        &mut self,
        workspace_revision: Revision,
        session_revision: Revision,
    ) -> DirectStoreResultV1<SessionArchiveSummaryV2> {
        let now = self.require_clock()?;
        self.state_written = true;
        let summary = v2_state::archive_current_session_transaction_v2(
            self.transaction,
            self.current_graph()?,
            workspace_revision,
            session_revision,
            now,
        )?;
        self.previous.take();
        Ok(summary)
    }

    pub fn purge_archive(
        &mut self,
        session: &SessionId,
        session_revision: Revision,
    ) -> DirectStoreResultV1<SessionArchiveSummaryV2> {
        self.require_clock()?;
        self.state_written = true;
        // Outcomes have no session FK and intentionally survive archive purging.
        Ok(v2_state::purge_archived_session_transaction_v2(
            self.transaction,
            session,
            session_revision,
        )?)
    }

    pub fn record_disposition(
        &mut self,
        disposition: &TerminalDispositionV2,
    ) -> DirectStoreResultV1<()> {
        self.require_clock()?;
        self.state_written = true;
        Ok(v2_state::record_terminal_disposition_transaction_v2(
            self.transaction,
            disposition,
        )?)
    }
}

impl DirectStoreV1 {
    /// Original-key replay precedes the callback, its fences, and its clock. Every
    /// new success includes state, outcome, journal, pruning and rebinding in one commit.
    pub fn mutate<Clock, Execute>(
        &self,
        request: &DirectMutationRequestV1,
        deadline: Instant,
        clock: Clock,
        execute: Execute,
    ) -> DirectStoreResultV1<DirectMutationResultV1>
    where
        Clock: FnOnce() -> UnixMillis,
        Execute: FnOnce(
            &mut DirectTransactionV1<'_, '_, Clock>,
        ) -> DirectStoreResultV1<CanonicalExecutionJsonV1>,
    {
        self.mutate_resolving_identity(
            &request.key,
            deadline,
            clock,
            |_| Ok(request.clone()),
            execute,
        )
    }

    /// Source-backed identities may recover their original pinned digest from the
    /// authoritative outcome lookup. Identity resolution is pure and precedes replay.
    pub fn mutate_resolving_identity<Clock, Identity, Execute>(
        &self,
        key: &IdempotencyKeyV1,
        deadline: Instant,
        clock: Clock,
        identity: Identity,
        execute: Execute,
    ) -> DirectStoreResultV1<DirectMutationResultV1>
    where
        Clock: FnOnce() -> UnixMillis,
        Identity: FnOnce(
            Option<&DirectRecordedOutcomeV1>,
        ) -> DirectStoreResultV1<DirectMutationRequestV1>,
        Execute: FnOnce(
            &mut DirectTransactionV1<'_, '_, Clock>,
        ) -> DirectStoreResultV1<CanonicalExecutionJsonV1>,
    {
        self.mutate_resolving_identity_typed(key, deadline, clock, identity, execute)
            .map_err(DirectMutationFailureV1::into_error)
    }

    pub fn mutate_typed<Clock, Execute, E>(
        &self,
        request: &DirectMutationRequestV1,
        deadline: Instant,
        clock: Clock,
        execute: Execute,
    ) -> Result<DirectMutationResultV1, DirectMutationFailureV1<E>>
    where
        Clock: FnOnce() -> UnixMillis,
        Execute:
            FnOnce(&mut DirectTransactionV1<'_, '_, Clock>) -> Result<CanonicalExecutionJsonV1, E>,
    {
        self.mutate_resolving_identity_typed(
            &request.key,
            deadline,
            clock,
            |_| Ok(request.clone()),
            execute,
        )
    }

    /// Identity still runs before replay selection; only new requests execute work.
    pub fn mutate_resolving_identity_typed<Clock, Identity, Execute, E>(
        &self,
        key: &IdempotencyKeyV1,
        deadline: Instant,
        clock: Clock,
        identity: Identity,
        execute: Execute,
    ) -> Result<DirectMutationResultV1, DirectMutationFailureV1<E>>
    where
        Clock: FnOnce() -> UnixMillis,
        Identity: FnOnce(Option<&DirectRecordedOutcomeV1>) -> Result<DirectMutationRequestV1, E>,
        Execute:
            FnOnce(&mut DirectTransactionV1<'_, '_, Clock>) -> Result<CanonicalExecutionJsonV1, E>,
    {
        let lease = DirectOpenLease::acquire(&self.layout, deadline, &mut || {
            self.boundary(DirectTransactionBoundaryV1::UnmarkedMissingReadClassified)
        })?;
        let _connection_gate = super::connection::connection_gate(deadline)?;
        let database = super::classification::open_read_nofollow(&self.layout.database())
            .map_err(DirectStoreErrorV1::from)?;
        // A foreign or corrupt Store is rejected before a read-write SQLite open
        // could create SHM. This preflight does not replace transaction revalidation.
        self.qualified_read(deadline, lease.heap_snapshot(), |_, _| {
            self.revalidate_files(&lease, &database)?;
            Ok(())
        })?;
        let mut connection = self.open_connection(false, deadline)?;
        self.boundary(DirectTransactionBoundaryV1::BeforeBegin);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DirectStoreErrorV1::from)?;
        self.boundary(DirectTransactionBoundaryV1::AfterBegin);
        let binding = self.validate_connection(&transaction)?;
        self.revalidate_files(&lease, &database)?;
        let outcome = read_outcome(&transaction, key)?;
        let mut request = identity(outcome.as_ref()).map_err(DirectMutationFailureV1::Callback)?;
        if request.key() != key {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "resolved request changed the outcome key",
            }
            .into());
        }
        if let Some(outcome) = outcome {
            if outcome.digest != request.digest {
                return Err(DirectStoreErrorV1::IdempotencyKeyReused.into());
            }
            return Ok(DirectMutationResultV1 {
                outcome,
                replayed: true,
            });
        }
        let mut context = DirectTransactionV1 {
            transaction: &transaction,
            binding,
            root: self.layout.root().clone(),
            previous: OnceCell::new(),
            state_written: false,
            clock: Some(clock),
            now: None,
            created_session_scope: None,
        };
        let response = execute(&mut context).map_err(DirectMutationFailureV1::Callback)?;
        let now = context.require_clock()?;
        if let Some(session) = context.created_session_scope.take() {
            request.scope = DirectOutcomeScopeV1::Session(session);
        }
        validate_success(&request, &response)?;
        self.boundary(DirectTransactionBoundaryV1::AfterStateBeforeOutcome);
        let outcome = persist_success(&transaction, &self.layout, &request, response, now)?;
        drop(context);
        self.revalidate_files(&lease, &database)?;
        commit(transaction)?;
        self.boundary(DirectTransactionBoundaryV1::AfterCommitBeforeOutput);
        checkpoint_after_success(&connection, &self.layout);
        Ok(DirectMutationResultV1 {
            outcome,
            replayed: false,
        })
    }

    /// Bootstrap a private complete Store under an already held exclusive lease.
    /// All initialization state is checkpointed and synced before no-clobber publication.
    #[allow(clippy::too_many_arguments)]
    pub fn bootstrap<Clock, Execute>(
        &self,
        lease: &ExclusiveMaintenanceLeaseV1,
        workspace: WorkspaceId,
        generation_id: &str,
        request: &DirectMutationRequestV1,
        deadline: Instant,
        clock: Clock,
        execute: Execute,
    ) -> DirectStoreResultV1<DirectMutationResultV1>
    where
        Clock: FnOnce() -> UnixMillis,
        Execute: FnOnce(
            &mut DirectTransactionV1<'_, '_, Clock>,
        ) -> DirectStoreResultV1<CanonicalExecutionJsonV1>,
    {
        let _connection_gate = super::connection::connection_gate(deadline)?;
        lease.revalidate(&self.layout)?;
        if generation_id.is_empty()
            || generation_id.len() > 64
            || request.scope != DirectOutcomeScopeV1::Workspace
        {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "invalid Store bootstrap identity or outcome scope",
            });
        }
        if super::classification::classify_unmarked_store_v1(&self.layout)?
            != super::DirectStoreClassificationV1::Missing
        {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "Store bootstrap requires a missing database",
            });
        }
        let temporary = tempfile::Builder::new()
            .prefix(".direct-store-")
            .tempfile_in(self.layout.runtime_directory())?;
        let path = temporary.path().to_path_buf();
        let cleanup = PrivateSidecars(path.clone());
        let mut connection = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.execute_batch(super::DIRECT_STORE_DDL_V1)?;
        configure_connection(&connection, false, deadline)?;
        self.boundary(DirectTransactionBoundaryV1::BeforeBegin);
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.boundary(DirectTransactionBoundaryV1::AfterBegin);
        // No persisted command fences exist during first creation. The engine's
        // pure preparation is complete before bootstrap is entered.
        let now = clock();
        let millis = sqlite_time(now)?;
        transaction.execute("INSERT INTO workspace_state (singleton,workspace_uuid,git_common_fingerprint,git_worktree_fingerprint,last_validated_root,next_workspace_sequence,created_at_ms,updated_at_ms) VALUES(1,?1,?2,?3,?4,0,?5,?5)",
            params![workspace.as_str(), self.common.as_str(), self.admin.as_str(), self.layout.root().as_encoded(), millis])?;
        transaction.execute(
            "INSERT INTO writer_contract VALUES(1,?1,?2,?3)",
            params![self.manifest.as_str(), generation_id, millis],
        )?;
        let binding = self.validate_connection(&transaction)?;
        let mut context = DirectTransactionV1 {
            transaction: &transaction,
            binding,
            root: self.layout.root().clone(),
            previous: OnceCell::new(),
            state_written: false,
            clock: None::<Clock>,
            now: Some(now),
            created_session_scope: None,
        };
        let response = execute(&mut context)?;
        validate_success(request, &response)?;
        self.boundary(DirectTransactionBoundaryV1::AfterStateBeforeOutcome);
        let outcome = persist_success(&transaction, &self.layout, request, response, now)?;
        drop(context);
        lease.revalidate(&self.layout)?;
        // This commit affects only an unpublished private inode. Its failure
        // cannot make the public mutation outcome uncertain.
        transaction.commit()?;
        let checkpoint: (i64, i64, i64) =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        if checkpoint.0 != 0 || checkpoint.1 != 0 || checkpoint.2 != 0 {
            return Err(DirectStoreErrorV1::Busy);
        }
        drop(connection);
        temporary.as_file().sync_all()?;
        lease.revalidate(&self.layout)?;
        self.boundary(DirectTransactionBoundaryV1::BeforeMaintenancePublication);
        // A write-free WAL reader needs existing sidecars. Publish the fully
        // checkpointed WAL/index before the main file, under the exclusive lease;
        // the main file is the publication point. Never create missing sidecars in
        // an ordinary read. Engine maintenance can repair an interrupted publication.
        let mut sidecars = PublishedSidecars::default();
        for (source, target) in [
            (super::sidecar(&path, "-wal"), self.layout.wal()),
            (super::sidecar(&path, "-shm"), self.layout.shm()),
        ] {
            fs::hard_link(&source, &target)?;
            let file = super::classification::open_read_nofollow(&target)?;
            file.sync_all()?;
            sidecars.files.push((target, file));
        }
        // persist_noclobber links the private inode without replacing another Store.
        let published = temporary
            .persist_noclobber(self.layout.database())
            .map_err(|error| DirectStoreErrorV1::Storage(error.error))?;
        published.sync_all()?;
        super::maintenance::open_directory(&self.layout.runtime_directory())?.sync_all()?;
        sidecars.published = true;
        drop(cleanup);
        self.boundary(DirectTransactionBoundaryV1::AfterCommitBeforeOutput);
        Ok(DirectMutationResultV1 {
            outcome,
            replayed: false,
        })
    }
}

struct PrivateSidecars(std::path::PathBuf);
impl Drop for PrivateSidecars {
    fn drop(&mut self) {
        for suffix in ["-wal", "-shm"] {
            let _ = fs::remove_file(super::sidecar(&self.0, suffix));
        }
    }
}

#[derive(Default)]
struct PublishedSidecars {
    files: Vec<(std::path::PathBuf, std::fs::File)>,
    published: bool,
}

impl Drop for PublishedSidecars {
    fn drop(&mut self) {
        if !self.published {
            for (path, file) in &self.files {
                if super::maintenance::same_file(file, path).unwrap_or(false) {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

fn validate_success(
    request: &DirectMutationRequestV1,
    response: &CanonicalExecutionJsonV1,
) -> DirectStoreResultV1<()> {
    let document: serde_json::Value =
        serde_json::from_str(response.as_str()).map_err(|_| DirectStoreErrorV1::Unreadable)?;
    validate_success_document(request, &document)
}

fn validate_success_document(
    request: &DirectMutationRequestV1,
    document: &serde_json::Value,
) -> DirectStoreResultV1<()> {
    // Engine owns complete command-result schema validation. Store independently
    // requires the complete canonical success, its command and original response UUID.
    let object = document
        .as_object()
        .ok_or(DirectStoreErrorV1::InvalidInput {
            reason: "success must be an object",
        })?;
    let allowed = [
        "schema",
        "request_id",
        "command",
        "generated_at",
        "workspace",
        "session",
        "result",
        "warnings",
    ];
    if object.keys().any(|key| !allowed.contains(&key.as_str()))
        || document.get("schema").and_then(serde_json::Value::as_str) != Some("podway.output/v4")
        || document
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            != Some(request.response_id())
        || document.get("command").and_then(serde_json::Value::as_str) != Some(request.command())
        || !document
            .get("generated_at")
            .is_some_and(serde_json::Value::is_string)
        || !document
            .get("result")
            .is_some_and(serde_json::Value::is_object)
        || !document
            .get("warnings")
            .is_some_and(serde_json::Value::is_array)
    {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "incomplete or mismatched canonical success envelope",
        });
    }
    Ok(())
}

fn sqlite_time(now: UnixMillis) -> DirectStoreResultV1<i64> {
    i64::try_from(now.get()).map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "clock is outside SQLite timestamp range",
    })
}

fn persist_success(
    transaction: &Transaction<'_>,
    layout: &super::DirectWorkspaceLayoutV1,
    request: &DirectMutationRequestV1,
    response: CanonicalExecutionJsonV1,
    now: UnixMillis,
) -> DirectStoreResultV1<DirectRecordedOutcomeV1> {
    let millis = sqlite_time(now)?;
    let sequence: i64 = transaction.query_row(
        "UPDATE workspace_state SET next_workspace_sequence=next_workspace_sequence+1, last_validated_root=?1, updated_at_ms=?2 \
         WHERE singleton=1 AND next_workspace_sequence<9223372036854775807 RETURNING next_workspace_sequence",
        params![layout.root().as_encoded(), millis], |row| row.get(0))?;
    let (kind, session) = match &request.scope {
        DirectOutcomeScopeV1::Workspace => ("workspace", None),
        DirectOutcomeScopeV1::Session(session) => ("session", Some(session.as_str())),
    };
    transaction.execute("INSERT INTO mutation_outcomes (idempotency_key,request_digest,command_name,workspace_sequence,scope_kind,scope_session_id,response_json,recorded_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![request.key.as_str(), request.digest.as_str(), request.command, sequence, kind, session, response.as_str(), millis])?;
    transaction.execute("INSERT INTO operational_journal (recorded_at_ms,level,event_name,workspace_sequence,summary) VALUES(?1,'info','mutation_committed',?2,?3)", params![millis,sequence,request.command])?;
    // Workspace bounds only: session outcomes intentionally survive archive purges.
    transaction.execute("DELETE FROM mutation_outcomes WHERE scope_kind='workspace' AND (recorded_at_ms<?1 OR workspace_sequence NOT IN (SELECT workspace_sequence FROM mutation_outcomes WHERE scope_kind='workspace' ORDER BY workspace_sequence DESC LIMIT 100))",
        [millis.saturating_sub(30*24*60*60*1000)])?;
    transaction.execute("DELETE FROM operational_journal WHERE journal_id NOT IN (SELECT journal_id FROM operational_journal ORDER BY journal_id DESC LIMIT 200) AND (recorded_at_ms<?1 OR journal_id NOT IN (SELECT journal_id FROM operational_journal ORDER BY journal_id DESC LIMIT 10000))",
        [millis.saturating_sub(7*24*60*60*1000)])?;
    Ok(DirectRecordedOutcomeV1 {
        digest: request.digest.clone(),
        command: request.command.clone(),
        sequence: u64::try_from(sequence).map_err(|_| DirectStoreErrorV1::Unreadable)?,
        scope: request.scope.clone(),
        response,
        recorded_at: now,
    })
}

pub(super) fn read_outcome(
    connection: &Connection,
    key: &IdempotencyKeyV1,
) -> DirectStoreResultV1<Option<DirectRecordedOutcomeV1>> {
    type PersistedOutcomeRow = (
        String,
        String,
        i64,
        String,
        Option<String>,
        String,
        i64,
        i64,
    );
    let row: Option<PersistedOutcomeRow> = connection.query_row(
        "SELECT request_digest,command_name,workspace_sequence,scope_kind,scope_session_id, \
         CASE WHEN length(CAST(response_json AS BLOB))<=1048576 THEN response_json ELSE NULL END,recorded_at_ms, \
         (SELECT next_workspace_sequence FROM workspace_state WHERE singleton=1) \
         FROM mutation_outcomes WHERE idempotency_key=?1", [key.as_str()],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).optional()?;
    row.map(
        |(digest, command, sequence, kind, session, response, time, committed_sequence)| {
            if sequence < 1 || sequence > committed_sequence {
                return Err(DirectStoreErrorV1::Unreadable);
            }
            let sequence = u64::try_from(sequence).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            let time = u64::try_from(time).map_err(|_| DirectStoreErrorV1::Unreadable)?;
            let scope = match (kind.as_str(), session) {
                ("workspace", None) => DirectOutcomeScopeV1::Workspace,
                ("session", Some(id)) => DirectOutcomeScopeV1::Session(
                    SessionId::new(id).map_err(|_| DirectStoreErrorV1::Unreadable)?,
                ),
                _ => return Err(DirectStoreErrorV1::Unreadable),
            };
            let (response, document) = CanonicalExecutionJsonV1::admit(response)
                .map_err(|_| DirectStoreErrorV1::Unreadable)?;
            let response_id = document
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .ok_or(DirectStoreErrorV1::Unreadable)?;
            let validation = DirectMutationRequestV1::new(
                key.clone(),
                Sha256Digest::new(digest.clone()).map_err(|_| DirectStoreErrorV1::Unreadable)?,
                command.clone(),
                response_id,
                scope.clone(),
            )
            .map_err(|_| DirectStoreErrorV1::Unreadable)?;
            validate_success_document(&validation, &document)
                .map_err(|_| DirectStoreErrorV1::Unreadable)?;
            if sequence == 0 {
                return Err(DirectStoreErrorV1::Unreadable);
            }
            Ok(DirectRecordedOutcomeV1 {
                digest: Sha256Digest::new(digest).map_err(|_| DirectStoreErrorV1::Unreadable)?,
                command,
                sequence,
                scope,
                response,
                recorded_at: UnixMillis::new(time),
            })
        },
    )
    .transpose()
}

fn commit(transaction: Transaction<'_>) -> DirectStoreResultV1<()> {
    transaction.commit().map_err(commit_error)
}

fn commit_error(error: rusqlite::Error) -> DirectStoreErrorV1 {
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::SystemIoFailure) {
        DirectStoreErrorV1::MutationOutcomeUnknown
    } else {
        error.into()
    }
}

fn checkpoint_after_success(connection: &Connection, layout: &super::DirectWorkspaceLayoutV1) {
    // A committed success must not become an error because best-effort bounded
    // checkpointing meets another active snapshot or writer. Passive work never
    // waits; an oversized WAL gets at most 100 ms to truncate on close.
    let _ = connection.busy_timeout(std::time::Duration::ZERO);
    let checkpoint = connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    // A pinned older snapshot already proves truncation cannot finish now.
    // Repeated waits would hold SQLite's writer lock behind the same reader.
    if checkpoint.is_ok_and(|(busy, frames, copied)| busy == 0 && frames >= 0 && frames == copied)
        && fs::metadata(layout.wal()).is_ok_and(|metadata| metadata.len() > 4 * 1024 * 1024)
    {
        let _ = super::connection::install_busy_deadline(
            connection,
            Instant::now() + std::time::Duration::from_millis(100),
        );
        let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncertain_outcome_is_only_the_commit_io_error_mapping() {
        let io = || {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
                None,
            )
        };
        assert!(matches!(
            DirectStoreErrorV1::from(io()),
            DirectStoreErrorV1::Sqlite(_)
        ));
        assert!(matches!(
            commit_error(io()),
            DirectStoreErrorV1::MutationOutcomeUnknown
        ));
        assert!(matches!(
            commit_error(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                None
            )),
            DirectStoreErrorV1::Busy
        ));
        assert!(matches!(
            commit_error(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                None
            )),
            DirectStoreErrorV1::Sqlite(_)
        ));
    }
    #[test]
    fn sqlite_full_rolls_back_rich_direct_state_and_retry_commits_one_outcome() {
        use super::super::DirectWorkspaceLayoutV1;
        use crate::{ActiveItemMutationV2, ValidatedWorkspaceRootV1};
        use podway_core::{
            AttemptId, CanonicalProcedureJsonV1, ItemId, ProcedureSnapshotId,
            ProcedureSourceLabelV1, canonicalize_json_v1,
        };
        use serde_json::json;
        use sha2::{Digest, Sha256};
        let temporary = tempfile::TempDir::new().unwrap();
        let path = std::fs::canonicalize(temporary.path()).unwrap();
        let digest = |nibble: char| {
            Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
        };
        let deadline = || Instant::now() + std::time::Duration::from_secs(10);
        let direct = DirectStoreV1::new(
            DirectWorkspaceLayoutV1::new(ValidatedWorkspaceRootV1::from_path(&path).unwrap())
                .unwrap(),
            digest('c'),
            digest('a'),
            digest('b'),
        );
        let request = |key: &str| {
            DirectMutationRequestV1::new(
                IdempotencyKeyV1::new(key).unwrap(),
                digest('d'),
                "item.set",
                "00000000-0000-4000-8000-000000000002",
                DirectOutcomeScopeV1::Workspace,
            )
            .unwrap()
        };
        let response = |request: &DirectMutationRequestV1, now: UnixMillis| {
            CanonicalExecutionJsonV1::new(
            canonicalize_json_v1(&json!({"schema":"podway.output/v4", "request_id":request.response_id(),
                "command":request.command(), "generated_at":format!("1970-01-01T00:00:00.{:03}Z",now.get()),
                "result":{"changed":true},"warnings":[]})).unwrap(),
        ).unwrap()
        };
        let bootstrap = request("init");
        let lease =
            ExclusiveMaintenanceLeaseV1::acquire(direct.layout(), deadline(), true).unwrap();
        direct
            .bootstrap(
                &lease,
                WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
                "current",
                &bootstrap,
                deadline(),
                || UnixMillis::new(1),
                |context| Ok(response(&bootstrap, context.now())),
            )
            .unwrap();
        drop(lease);
        let canonical=canonicalize_json_v1(&json!({"schema":"podway.procedure/v2","id":"pager-full","version":"1",
            "name":"Pager full","purpose":"Preserve atomic state after real SQLite exhaustion.",
            "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record output.",
                "items":[{"id":"note","type":"text","prompt":"Note","required":false,"max_length":65536}]}},
            "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}})).unwrap();
        let snapshot = crate::admit_procedure_snapshot_v2(
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000003").unwrap(),
            CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
            Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
                .unwrap(),
            ProcedureSourceLabelV1::file("pager-full.yaml").unwrap(),
            UnixMillis::new(10),
        )
        .unwrap();
        let current = GraphSessionStateV2::prepared(
            Revision::new(1),
            "Pager full",
            snapshot,
            SessionId::new("00000000-0000-4000-8000-000000000004").unwrap(),
            UnixMillis::new(10),
        )
        .unwrap()
        .begin_v2(
            Revision::ZERO,
            AttemptId::new("00000000-0000-4000-8000-000000000005").unwrap(),
            None,
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
        let seed = request("seed");
        direct
            .mutate(
                &seed,
                deadline(),
                || UnixMillis::new(21),
                |context| {
                    context.now();
                    context.create_graph(&current)?;
                    Ok(response(&seed, context.now()))
                },
            )
            .unwrap();
        let next = current
            .mutate_active_item_v2(
                current.trace().active_attempt().unwrap().attempt_id(),
                &ItemId::new("note").unwrap(),
                Revision::ZERO,
                ActiveItemMutationV2::Set {
                    value: "x".repeat(16384),
                },
                UnixMillis::new(30),
            )
            .unwrap()
            .into_state();
        let mutation = request("set-pager-full");
        let sequence = direct
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap();
        {
            // Constrain only this owned unit fixture connection. No public fault knob or legacy
            // Store options survive retirement; these are the real direct persistence primitives.
            let lease = DirectOpenLease::acquire(direct.layout(), deadline(), &mut || {}).unwrap();
            let gate = super::super::connection::connection_gate(deadline()).unwrap();
            let mut connection = direct.open_connection(false, deadline()).unwrap();
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")
                .unwrap();
            let count: i64 = connection
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .unwrap();
            let free: i64 = connection
                .query_row("PRAGMA freelist_count", [], |row| row.get(0))
                .unwrap();
            assert_eq!(free, 0);
            let cap: i64 = connection
                .query_row(&format!("PRAGMA max_page_count={count}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(cap, count);
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            let binding = direct.validate_connection(&transaction).unwrap();
            let mut context = DirectTransactionV1 {
                transaction: &transaction,
                binding,
                root: direct.layout.root().clone(),
                previous: OnceCell::new(),
                state_written: false,
                clock: Some(|| UnixMillis::new(30)),
                now: None,
                created_session_scope: None,
            };
            context.now();
            let failure = context
                .replace_graph(
                    current.workspace_revision(),
                    current.trace().revision(),
                    &next,
                )
                .unwrap_err();
            assert!(matches!(
                failure,
                DirectStoreErrorV1::State(crate::StoreErrorV1::StorageUnavailableV1 {
                    reason: crate::StoreUnavailableReasonV1::StorageIo,
                })
            ));
            // SQLITE_FULL aborts the actual SQLite transaction rather than merely returning a
            // synthetic Store failure. No outcome is written after the failed state transition.
            assert!(transaction.is_autocommit());
            drop(context);
            drop(transaction);
            drop(connection);
            drop(gate);
            drop(lease);
        }
        direct
            .read(deadline(), |snapshot| {
                assert_eq!(snapshot.current_graph()?, Some(current.clone()));
                assert_eq!(snapshot.committed_sequence()?, sequence);
                assert!(snapshot.outcome(mutation.key())?.is_none());
                Ok(())
            })
            .unwrap();
        let committed = direct
            .mutate(
                &mutation,
                deadline(),
                || UnixMillis::new(31),
                |context| {
                    context.now();
                    context.replace_graph(
                        current.workspace_revision(),
                        current.trace().revision(),
                        &next,
                    )?;
                    Ok(response(&mutation, context.now()))
                },
            )
            .unwrap();
        assert!(!committed.replayed());
        let replayed = direct
            .mutate(
                &mutation,
                deadline(),
                || panic!("replay clock"),
                |_| panic!("replay transition"),
            )
            .unwrap();
        assert!(replayed.replayed());
        assert_eq!(replayed.outcome(), committed.outcome());
        direct
            .read(deadline(), |snapshot| {
                assert_eq!(snapshot.current_graph()?, Some(next));
                assert_eq!(snapshot.committed_sequence()?, sequence + 1);
                assert_eq!(
                    snapshot.outcome(mutation.key())?.as_ref(),
                    Some(committed.outcome())
                );
                Ok(())
            })
            .unwrap();
        direct.verify_integrity(deadline()).unwrap();
    }
}
