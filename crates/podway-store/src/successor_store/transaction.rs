//! Success-only Store transaction for the isolated successor Engine fixture.

use super::{
    CandidateDefinition, CandidateSources, CandidateStoreError, Result, admit_writer, bounded_text,
    corrupt, create_candidate_fixture, load_candidate_execution, open_candidate,
    persist_candidate_execution, reopen_candidate_fixture,
};
use crate::direct_store::{
    DirectMutationRequestV1, DirectMutationResultV1, DirectOutcomeScopeV1, DirectRecordedOutcomeV1,
    candidate_committed_outcome, commit, next_workspace_sequence, persist_success, read_outcome,
    replay_outcome, validate_success,
};
use crate::{CanonicalExecutionJsonV1, IdempotencyKeyV1, ValidatedWorkspaceRootV1};
use podway_core::execution_groups::ExecutionSessionV3;
use podway_core::{SessionId, UnixMillis, WorkspaceId};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::path::Path;

/// Owns only a local, explicitly constructed v16 fixture. Product Direct Store admission is
/// independent. Test callers provide the canonical owning root and exact fixture workspace UUID.
pub struct CandidateExecutionStore {
    connection: Connection,
    root: ValidatedWorkspaceRootV1,
    workspace: WorkspaceId,
}

impl CandidateExecutionStore {
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
        let millis = i64::try_from(initial.updated_at().get())
            .map_err(|_| corrupt("candidate fixture clock is outside SQLite range"))?;
        // These fixture values are not production Git fingerprints; the exact workspace UUID
        // and owning root are checked before candidate mutation or replay.
        transaction.execute(
            "INSERT INTO workspace_state VALUES (1, ?1, ?2, ?2, ?3, 0, ?4, ?4)",
            params![
                workspace.as_str(),
                initial.base_digest().as_str(),
                root.as_encoded(),
                millis,
            ],
        )?;
        persist_candidate_execution(&transaction, initial, sources)?;
        transaction.commit()?;
        Ok(Self {
            connection,
            root,
            workspace,
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
        })
    }

    pub fn snapshot(
        &mut self,
        session: &SessionId,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources)> {
        let transaction = self.connection.transaction()?;
        admit_writer(&transaction)?;
        verify_binding(&transaction, &self.root, &self.workspace)?;
        load_candidate_execution(&transaction, session, admit)
    }

    /// Outcome readback is a verification handle for this fixture; the public CLI route
    /// continues to use its existing v15 Store.
    pub fn outcome(&self, key: &IdempotencyKeyV1) -> Result<Option<DirectRecordedOutcomeV1>> {
        admit_writer(&self.connection)?;
        verify_binding(&self.connection, &self.root, &self.workspace)?;
        Ok(read_outcome(&self.connection, key)?)
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
        if workspace != &self.workspace {
            return Err(corrupt("candidate workspace identity differs").into());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CandidateStoreError::from)?;
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
        let success = execute(&CandidateExecutionTransaction {
            transaction: &transaction,
            root: &self.root,
            session,
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
        commit(transaction).map_err(CandidateStoreError::from)?;
        Ok(candidate_committed_outcome(outcome))
    }
}

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
        return Err(corrupt("candidate workspace identity differs"));
    }
    Ok(())
}

pub struct CandidateExecutionTransaction<'transaction, 'connection> {
    transaction: &'transaction Transaction<'connection>,
    root: &'transaction ValidatedWorkspaceRootV1,
    session: &'transaction SessionId,
}

impl CandidateExecutionTransaction<'_, '_> {
    pub fn load(
        &self,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources)> {
        load_candidate_execution(self.transaction, self.session, admit)
    }

    pub fn persist(&self, state: &ExecutionSessionV3, sources: &CandidateSources) -> Result<()> {
        if &state.fence().session != self.session {
            return Err(corrupt(
                "candidate state differs from mutation outcome scope",
            ));
        }
        persist_candidate_execution(self.transaction, state, sources)
    }

    pub fn next_sequence(&self) -> Result<u64> {
        Ok(next_workspace_sequence(self.transaction)?)
    }

    pub fn root(&self) -> &ValidatedWorkspaceRootV1 {
        self.root
    }
}

pub struct CandidateSuccess {
    pub response: CanonicalExecutionJsonV1,
    pub now: UnixMillis,
}
