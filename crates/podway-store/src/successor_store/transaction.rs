//! Success-only Store transaction for the isolated successor Engine fixture.

use super::{CandidateDefinition, CandidateSources, CandidateStoreError, Result, corrupt};
#[cfg(feature = "test-support")]
use super::{
    admit_writer, bounded_text, create_candidate_fixture, load_candidate_content, open_candidate,
    persist_candidate_execution, reopen_candidate_fixture,
};
#[cfg(feature = "test-support")]
use crate::IdempotencyKeyV1;
use crate::direct_store::next_workspace_sequence;
#[cfg(feature = "test-support")]
use crate::direct_store::{
    DirectMutationRequestV1, DirectMutationResultV1, DirectOutcomeScopeV1, DirectRecordedOutcomeV1,
    DirectStoreErrorV1, candidate_committed_outcome, commit, persist_success, read_outcome,
    replay_outcome, validate_success,
};
use crate::{CanonicalExecutionJsonV1, ValidatedWorkspaceRootV1};
#[cfg(feature = "test-support")]
use podway_core::Revision;
use podway_core::execution_groups::ExecutionSessionV3;
use podway_core::{
    DomainError, SessionId, SessionLifecycle, Sha256Digest, UnixMillis, WorkspaceId,
};
use rusqlite::{Connection, Transaction};
#[cfg(feature = "test-support")]
use rusqlite::{OpenFlags, TransactionBehavior, params};
use sha2::{Digest, Sha256};
#[cfg(feature = "test-support")]
use std::path::Path;
#[cfg(feature = "test-support")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// The exact publication points available only on a test-support successor fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(feature = "test-support")]
pub enum CandidateCommitBoundary {
    AfterLedgerBeforeCommit,
    AfterCommitBeforeReturn,
}

/// Exact session selection for a candidate history read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateSessionLocation {
    Current,
    Archived,
}

/// Scope of an explicitly destructive test fixture request.
#[cfg(feature = "test-support")]
pub struct CandidateFixtureSession<'a> {
    pub workspace: &'a WorkspaceId,
    pub session: &'a SessionId,
    pub location: CandidateSessionLocation,
    pub revision: Revision,
}

/// Admitted state and its continuation binding, gathered in one read transaction.
pub struct CandidateHistorySnapshot {
    pub state: ExecutionSessionV3,
    pub sources: CandidateSources,
    pub content_digest: Sha256Digest,
}

impl CandidateSessionLocation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Archived => "archived",
        }
    }
}

#[cfg(feature = "test-support")]
type CommitObserver = dyn Fn(CandidateCommitBoundary, &DirectRecordedOutcomeV1) + Send;

/// Owns only a local, explicitly constructed v16 fixture. Product Direct Store admission is
/// independent. Test callers provide the canonical owning root and exact fixture workspace UUID.
#[cfg(feature = "test-support")]
pub struct CandidateExecutionStore {
    connection: Connection,
    root: ValidatedWorkspaceRootV1,
    workspace: WorkspaceId,
    commit_observer: Option<Box<CommitObserver>>,
    beginning_writer: Option<Arc<AtomicBool>>,
}

#[cfg(feature = "test-support")]
impl CandidateExecutionStore {
    pub fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.root
    }

    /// Installs the initial admitted test state without publishing a command outcome.
    pub fn create_fixture(
        path: &Path,
        root: ValidatedWorkspaceRootV1,
        workspace: WorkspaceId,
        initial: &ExecutionSessionV3,
        sources: &CandidateSources,
    ) -> Result<Self> {
        verify_fixture_path(path, &root)?;
        let mut connection = create_candidate_fixture(path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        initialize_fixture(&transaction, &root, &workspace, initial, sources)?;
        transaction.commit()?;
        Ok(Self {
            connection,
            root,
            workspace,
            commit_observer: None,
            beginning_writer: None,
        })
    }

    /// Read-only admission precedes reopening an exact known candidate fixture for writes.
    pub fn reopen_fixture(
        path: &Path,
        root: ValidatedWorkspaceRootV1,
        workspace: WorkspaceId,
    ) -> Result<Self> {
        verify_fixture_path(path, &root)?;
        let reader = reopen_candidate_fixture(path)?;
        verify_binding(&reader, &root, &workspace)?;
        drop(reader);
        let connection = open_candidate(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        Ok(Self {
            connection,
            root,
            workspace,
            commit_observer: None,
            beginning_writer: None,
        })
    }

    /// Install a scoped fixture callback without changing product transaction paths.
    pub fn observe_commit(
        &mut self,
        observer: impl Fn(CandidateCommitBoundary, &DirectRecordedOutcomeV1) + Send + 'static,
    ) {
        self.commit_observer = Some(Box::new(observer));
    }

    /// Observe SQLite lock contention. The flag distinguishes the actual IMMEDIATE begin from
    /// reads or any later SQL statement on this same fixture connection.
    pub fn observe_writer_busy(
        &mut self,
        mut observer: impl FnMut(bool, i32) -> bool + Send + 'static,
    ) -> Result<()> {
        let beginning = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&beginning);
        self.connection.busy_handler(Some(move |attempts| {
            observer(observed.load(Ordering::Relaxed), attempts)
        }))?;
        self.beginning_writer = Some(beginning);
        Ok(())
    }

    pub fn snapshot(
        &mut self,
        session: &SessionId,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources)> {
        let expected_workspace = self.workspace.clone();
        let (state, sources, _) = self.current_snapshot(&expected_workspace, session, admit)?;
        Ok((state, sources))
    }

    /// Obtain the current admitted state and committed sequence from the same read snapshot.
    pub fn current_snapshot(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources, u64)> {
        self.history_snapshot(workspace, session, CandidateSessionLocation::Current, admit)
    }

    /// Load one exact retained session and its revision facts from one read transaction.
    pub fn history_snapshot(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        location: CandidateSessionLocation,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources, u64)> {
        let transaction = self.fixture_read(workspace)?;
        let (state, sources) = load_requested_session(&transaction, session, location, admit)?;
        require_terminal_archive(location, &state)?;
        let sequence: i64 = transaction.query_row(
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let sequence = u64::try_from(sequence)
            .map_err(|_| corrupt("candidate workspace sequence is negative"))?;
        Ok((state, sources, sequence))
    }

    /// Load history and its session-local content identity without consulting the ledger.
    pub fn history_snapshot_with_digest(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        location: CandidateSessionLocation,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<CandidateHistorySnapshot> {
        let transaction = self.fixture_read(workspace)?;
        admitted_history_snapshot(&transaction, session, location, admit)
    }

    fn fixture_read(&mut self, workspace: &WorkspaceId) -> Result<Transaction<'_>> {
        self.require_workspace(workspace)?;
        let transaction = self.connection.transaction()?;
        admit_writer(&transaction)?;
        verify_binding(&transaction, &self.root, workspace)?;
        Ok(transaction)
    }

    /// Archive an admitted terminal session in an isolated fixture. This fixture operation
    /// freezes session rows without creating a public mutation or an outcome.
    pub fn archive_fixture_session(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        expected: Revision,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<()> {
        let transaction = self.fixture_write(workspace)?;
        let (state, _) = load_requested_session(
            &transaction,
            session,
            CandidateSessionLocation::Current,
            admit,
        )?;
        let actual = state.fence().revision;
        if actual != expected {
            return Err(CandidateStoreError::ArchiveRevisionMismatch { expected, actual });
        }
        if !terminal_session(&state) {
            return Err(CandidateStoreError::ArchiveRequiresTerminalSession);
        }
        transaction.execute(
            "UPDATE successor_sessions SET archived = 1 WHERE session_id = ?1",
            [session.as_str()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Fill an empty current slot after archival or removal, without restoring retained state.
    pub fn install_fixture_session(
        &mut self,
        workspace: &WorkspaceId,
        initial: &ExecutionSessionV3,
        sources: &CandidateSources,
    ) -> Result<()> {
        require_initial_fixture_session(initial)?;
        let transaction = self.fixture_write(workspace)?;
        let occupied: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM successor_sessions WHERE archived = 0 OR session_id = ?1)
             OR EXISTS (SELECT 1 FROM mutation_outcomes WHERE scope_kind = 'session' AND scope_session_id = ?1)",
            [initial.fence().session.as_str()],
            |row| row.get(0),
        )?;
        if occupied {
            return Err(DomainError::InvalidState {
                reason: "candidate current slot or retained installation identity is already present",
            }.into());
        }
        persist_candidate_execution(&transaction, initial, sources)?;
        transaction.commit()?;
        Ok(())
    }

    /// Purge an archived fixture session or reset a terminal current fixture session.
    pub fn remove_fixture_session(
        &mut self,
        target: CandidateFixtureSession<'_>,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<()> {
        let transaction = self.fixture_write(target.workspace)?;
        let state = fixture_session(&transaction, &target, admit)?;
        if !terminal_session(&state) {
            return Err(DomainError::InvalidState {
                reason: "candidate session removal requires a terminal session",
            }
            .into());
        }
        transaction.execute(
            "DELETE FROM successor_sessions WHERE session_id = ?1",
            [target.session.as_str()],
        )?;
        if target.location == CandidateSessionLocation::Current {
            transaction.execute(
                "DELETE FROM mutation_outcomes WHERE scope_kind = 'session' AND scope_session_id = ?1",
                [target.session.as_str()],
            )?;
        }
        transaction.execute(
            "DELETE FROM successor_definitions WHERE
             NOT EXISTS (SELECT 1 FROM successor_sessions WHERE base_digest = digest)
             AND NOT EXISTS (SELECT 1 FROM successor_activations WHERE definition_digest = digest)",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Replace the entire isolated fixture runtime with a new workspace and initial session.
    pub fn reset_fixture_workspace(
        &mut self,
        target: CandidateFixtureSession<'_>,
        replacement: WorkspaceId,
        initial: &ExecutionSessionV3,
        sources: &CandidateSources,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<()> {
        require_initial_fixture_session(initial)?;
        if target.location != CandidateSessionLocation::Current
            || &replacement == target.workspace
            || &initial.fence().session == target.session
        {
            return Err(DomainError::InvalidState {
                reason: "candidate workspace reset requires a current target and fresh identities",
            }
            .into());
        }
        let root = self.root.clone();
        let transaction = self.fixture_write(target.workspace)?;
        fixture_session(&transaction, &target, admit)?;
        transaction.execute_batch(
            "DELETE FROM successor_sessions;
             DELETE FROM successor_definitions;
             DELETE FROM mutation_outcomes;
             DELETE FROM operational_journal;
             DELETE FROM sqlite_sequence WHERE name = 'operational_journal';
             DELETE FROM workspace_state;",
        )?;
        initialize_fixture(&transaction, &root, &replacement, initial, sources)?;
        transaction.commit()?;
        self.workspace = replacement;
        Ok(())
    }

    fn fixture_write(&mut self, workspace: &WorkspaceId) -> Result<Transaction<'_>> {
        self.require_workspace(workspace)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        admit_writer(&transaction)?;
        verify_binding(&transaction, &self.root, workspace)?;
        Ok(transaction)
    }

    /// Outcome readback is a verification handle for this fixture; the public CLI route
    /// continues to use its existing v15 Store.
    pub fn outcome(&self, key: &IdempotencyKeyV1) -> Result<Option<DirectRecordedOutcomeV1>> {
        self.lookup_outcome(&self.workspace, key)
    }

    /// Recover the original success by workspace and key independently of any live cursor.
    pub fn lookup_outcome(
        &self,
        workspace: &WorkspaceId,
        key: &IdempotencyKeyV1,
    ) -> Result<Option<DirectRecordedOutcomeV1>> {
        self.require_workspace(workspace)?;
        let transaction = self.connection.unchecked_transaction()?;
        admit_writer(&transaction)?;
        verify_binding(&transaction, &self.root, workspace)?;
        Ok(read_outcome(&transaction, key)?)
    }

    pub fn mutate<E: From<CandidateStoreError>>(
        &mut self,
        workspace: &WorkspaceId,
        request: &DirectMutationRequestV1,
        execute: impl FnOnce(
            &CandidateExecutionTransaction<'_, '_>,
        ) -> std::result::Result<CandidateSuccess, E>,
    ) -> std::result::Result<DirectMutationResultV1, E> {
        let DirectOutcomeScopeV1::Session(session) = request.scope() else {
            return Err(corrupt("candidate mutation requires session outcome scope").into());
        };
        self.require_workspace(workspace)?;
        if let Some(beginning) = &self.beginning_writer {
            beginning.store(true, Ordering::Relaxed);
        }
        let begin = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate);
        if let Some(beginning) = &self.beginning_writer {
            beginning.store(false, Ordering::Relaxed);
        }
        let transaction = begin.map_err(CandidateStoreError::from)?;
        admit_writer(&transaction)?;
        verify_binding(&transaction, &self.root, workspace)?;
        let stored =
            read_outcome(&transaction, request.key()).map_err(CandidateStoreError::from)?;

        if stored.as_ref().is_some_and(|outcome| {
            outcome.digest() == request.digest()
                && (outcome.scope() != request.scope() || outcome.command() != request.command())
        }) {
            return Err(
                corrupt("candidate replay scope differs from stored command identity").into(),
            );
        }
        if let Some(replay) = replay_outcome(request, stored).map_err(CandidateStoreError::from)? {
            return Ok(replay);
        }
        require_session_location(&transaction, session, CandidateSessionLocation::Current)?;
        let success = execute(&CandidateExecutionTransaction {
            transaction: &transaction,
            root: &self.root,
            workspace,
            session,
            verify_progress: None,
            task_title: None,
            binding: None,
        })?;
        validate_success(request, &success.response).map_err(CandidateStoreError::from)?;
        let outcome = persist_success(
            &transaction,
            &self.root,
            request,
            success.response,
            success.now,
        )
        .map_err(CandidateStoreError::from)?;
        if let Some(observer) = &self.commit_observer {
            observer(CandidateCommitBoundary::AfterLedgerBeforeCommit, &outcome);
        }
        commit(transaction).map_err(CandidateStoreError::from)?;
        if let Some(observer) = &self.commit_observer {
            observer(CandidateCommitBoundary::AfterCommitBeforeReturn, &outcome);
        }
        Ok(candidate_committed_outcome(outcome))
    }

    fn require_workspace(&self, workspace: &WorkspaceId) -> Result<()> {
        if workspace != &self.workspace {
            return Err(DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch {
                expected: workspace.clone(),
                actual: self.workspace.clone(),
            }
            .into());
        }
        Ok(())
    }
}

#[cfg(feature = "test-support")]
fn initialize_fixture(
    transaction: &Transaction<'_>,
    root: &ValidatedWorkspaceRootV1,
    workspace: &WorkspaceId,
    initial: &ExecutionSessionV3,
    sources: &CandidateSources,
) -> Result<()> {
    let millis = i64::try_from(initial.updated_at().get())
        .map_err(|_| corrupt("candidate fixture clock is outside SQLite range"))?;
    // Candidate binding uses exact workspace/root identity and fixture-only Git fingerprints.
    transaction.execute(
        "INSERT INTO workspace_state VALUES (1, ?1, ?2, ?2, ?3, 0, ?4, ?4)",
        params![
            workspace.as_str(),
            initial.base_digest().as_str(),
            root.as_encoded(),
            millis
        ],
    )?;
    persist_candidate_execution(transaction, initial, sources)
}

#[cfg(feature = "test-support")]
fn require_initial_fixture_session(initial: &ExecutionSessionV3) -> Result<()> {
    if initial.lifecycle() != SessionLifecycle::Running
        || initial.attempt_history().len() != 1
        || !initial.group_history().is_empty()
    {
        return Err(corrupt(
            "candidate installation requires an initial running session",
        ));
    }
    Ok(())
}

#[cfg(feature = "test-support")]
fn fixture_session(
    transaction: &Transaction<'_>,
    target: &CandidateFixtureSession<'_>,
    admit: impl Fn(&str) -> Result<CandidateDefinition>,
) -> Result<ExecutionSessionV3> {
    let (state, _) = load_requested_session(transaction, target.session, target.location, admit)?;
    let actual = state.fence().revision;
    if actual != target.revision {
        return Err(DomainError::PreconditionFailed {
            expected: target.revision,
            actual,
        }
        .into());
    }
    Ok(state)
}

#[cfg(feature = "test-support")]
fn verify_fixture_path(path: &Path, root: &ValidatedWorkspaceRootV1) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("candidate fixture path has no parent"))?;
    let canonical = parent.canonicalize()?;
    if canonical != parent || !parent.starts_with(root.to_path_buf()) {
        return Err(corrupt(
            "candidate fixture path is outside its canonical root",
        ));
    }
    Ok(())
}

#[cfg(feature = "test-support")]
fn verify_binding(
    connection: &Connection,
    root: &ValidatedWorkspaceRootV1,
    workspace: &WorkspaceId,
) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT workspace_uuid, last_validated_root FROM workspace_state WHERE singleton = 1",
    )?;
    let mut rows = statement.query([])?;
    let row = rows
        .next()?
        .ok_or_else(|| corrupt("candidate workspace binding is absent"))?;
    if bounded_text(row, 0, 36)? != workspace.as_str()
        || bounded_text(row, 1, 8_256)? != root.as_encoded()
    {
        return Err(corrupt("candidate persisted workspace binding differs"));
    }
    Ok(())
}

pub struct CandidateExecutionTransaction<'transaction, 'connection> {
    transaction: &'transaction Transaction<'connection>,
    root: &'transaction ValidatedWorkspaceRootV1,
    workspace: &'transaction WorkspaceId,
    session: &'transaction SessionId,
    verify_progress: Option<(
        &'transaction crate::WorkspaceBindingV1,
        &'transaction super::SuccessorProgressVerifier<'transaction>,
    )>,
    task_title: Option<String>,
    binding: Option<&'transaction crate::WorkspaceBindingV1>,
}

impl<'transaction, 'connection> CandidateExecutionTransaction<'transaction, 'connection> {
    pub(crate) fn from_native(
        transaction: &'transaction Transaction<'connection>,
        root: &'transaction ValidatedWorkspaceRootV1,
        binding: &'transaction crate::WorkspaceBindingV1,
        session: &'transaction SessionId,
        verify_progress: Option<&'transaction super::SuccessorProgressVerifier<'transaction>>,
    ) -> Result<Self> {
        let current = super::current_execution(transaction)?;
        if current.as_ref().map(|(id, _)| id) != Some(session) {
            return Err(DomainError::SessionIdentityMismatch {
                expected: session.clone(),
                actual: current.map(|(id, _)| id),
            }
            .into());
        }
        let metadata = super::session_metadata(transaction, session)?
            .ok_or_else(|| corrupt("current session metadata is absent"))?;
        Ok(Self {
            transaction,
            root,
            workspace: binding.identity().workspace_uuid(),
            session,
            verify_progress: verify_progress.map(|verify| (binding, verify)),
            task_title: Some(metadata.task_title),
            binding: Some(binding),
        })
    }

    pub fn verify_progress(&self, state: &ExecutionSessionV3) -> Result<()> {
        if let Some((binding, verify)) = self.verify_progress {
            verify(binding, state)?;
        }
        Ok(())
    }

    pub fn task_title(&self) -> Option<&str> {
        self.task_title.as_deref()
    }

    pub fn binding(&self) -> Option<&crate::WorkspaceBindingV1> {
        self.binding
    }

    pub fn delegation_receipt(
        &self,
        state: &ExecutionSessionV3,
        attempt: &podway_core::AttemptId,
    ) -> Result<Option<podway_core::execution_groups::ScopedDelegationReceiptV1>> {
        super::read_receipt(self.transaction, state, attempt)
    }

    pub fn load(
        &self,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources)> {
        let (state, sources) = super::load_execution(self.transaction, self.session, admit, None)?;
        if self.binding.is_some() {
            super::require_settled_goal_assessments(&state)?;
            super::verify_receipts(self.transaction, &state)?;
        }
        Ok((state, sources))
    }

    pub fn persist(&self, state: &ExecutionSessionV3, sources: &CandidateSources) -> Result<()> {
        if &state.fence().session != self.session {
            return Err(corrupt(
                "candidate state differs from mutation outcome scope",
            ));
        }
        if self.binding.is_some() {
            super::require_settled_goal_assessments(state)?;
            super::verify_receipts(self.transaction, state)?;
        }
        super::persist_execution(self.transaction, state, sources)
    }

    pub fn next_sequence(&self) -> Result<u64> {
        Ok(next_workspace_sequence(self.transaction)?)
    }

    pub fn root(&self) -> &ValidatedWorkspaceRootV1 {
        self.root
    }

    pub fn workspace(&self) -> &WorkspaceId {
        self.workspace
    }
}

#[cfg(feature = "test-support")]
fn load_requested_session(
    transaction: &Transaction<'_>,
    session: &SessionId,
    location: CandidateSessionLocation,
    admit: impl Fn(&str) -> Result<CandidateDefinition>,
) -> Result<(ExecutionSessionV3, CandidateSources)> {
    load_requested_content(transaction, session, location, admit, None)
}

#[cfg(feature = "test-support")]
fn load_requested_content(
    transaction: &Transaction<'_>,
    session: &SessionId,
    location: CandidateSessionLocation,
    admit: impl Fn(&str) -> Result<CandidateDefinition>,
    checksums: Option<&mut Sha256>,
) -> Result<(ExecutionSessionV3, CandidateSources)> {
    require_session_location(transaction, session, location)?;
    load_candidate_content(transaction, session, admit, checksums)
}

fn require_terminal_archive(
    location: CandidateSessionLocation,
    state: &ExecutionSessionV3,
) -> Result<()> {
    if location == CandidateSessionLocation::Archived && !terminal_session(state) {
        return Err(corrupt("archived candidate session is not terminal"));
    }
    Ok(())
}

fn require_session_location(
    transaction: &Connection,
    session: &SessionId,
    location: CandidateSessionLocation,
) -> Result<()> {
    use rusqlite::OptionalExtension;

    let archived: Option<bool> = transaction
        .query_row(
            "SELECT archived FROM successor_sessions WHERE session_id = ?1",
            [session.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    match (location, archived) {
        (CandidateSessionLocation::Current, Some(false))
        | (CandidateSessionLocation::Archived, Some(true)) => Ok(()),
        (CandidateSessionLocation::Current, Some(true)) => {
            Err(CandidateStoreError::ArchivedSession)
        }
        _ => Err(DomainError::SessionIdentityMismatch {
            expected: session.clone(),
            actual: None,
        }
        .into()),
    }
}

fn terminal_session(state: &ExecutionSessionV3) -> bool {
    matches!(
        state.lifecycle(),
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    )
}

pub struct CandidateSuccess {
    pub response: CanonicalExecutionJsonV1,
    pub now: UnixMillis,
}

pub(crate) fn admitted_history_snapshot(
    connection: &Connection,
    session: &SessionId,
    location: CandidateSessionLocation,
    admit: impl Fn(&str) -> Result<CandidateDefinition>,
) -> Result<CandidateHistorySnapshot> {
    require_session_location(connection, session, location)?;
    let mut checksums = Sha256::new();
    checksums.update(b"podway.candidate-history-snapshot/v1");
    let (state, sources) = super::load_execution(connection, session, admit, Some(&mut checksums))?;
    require_terminal_archive(location, &state)?;
    let content_digest = Sha256Digest::new(format!("sha256:{:x}", checksums.finalize()))
        .expect("hex encoded SHA-256 is a valid digest");
    Ok(CandidateHistorySnapshot {
        state,
        sources,
        content_digest,
    })
}
