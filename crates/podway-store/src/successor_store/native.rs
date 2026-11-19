//! Execute the same scoped transitions on fixture and native Direct transactions.

#[cfg(feature = "test-support")]
use super::CandidateExecutionStore;
use super::{
    CandidateDefinition, CandidateExecutionTransaction, CandidateHistorySnapshot,
    CandidateSessionLocation, CandidateSources, CandidateStoreError, CandidateSuccess, Result,
    corrupt,
};
use crate::{
    IdempotencyKeyV1, ValidatedWorkspaceRootV1, WorkspaceBindingV1,
    direct_store::{
        DirectMutationFailureV1, DirectMutationRequestV1, DirectMutationResultV1,
        DirectOutcomeScopeV1, DirectReadFailureV1, DirectReadSnapshotV1, DirectRecordedOutcomeV1,
        DirectStoreErrorV1, DirectStoreV1,
    },
};
use podway_core::{
    DomainError, SessionId, UnixMillis, WorkspaceId, execution_groups::ExecutionSessionV3,
};
use std::time::Instant;

pub trait SuccessorReadStore {
    fn workspace_root(&self) -> &ValidatedWorkspaceRootV1;

    fn response_headroom(&self) -> usize {
        0
    }

    fn current_snapshot(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources, u64)>;

    fn history_snapshot_with_digest(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        location: CandidateSessionLocation,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<CandidateHistorySnapshot>;

    fn lookup_outcome(
        &self,
        workspace: &WorkspaceId,
        key: &IdempotencyKeyV1,
    ) -> Result<Option<DirectRecordedOutcomeV1>>;
}

pub trait SuccessorMutationStore: SuccessorReadStore {
    fn snapshot_for_transition(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources)> {
        let (state, sources, _) = self.current_snapshot(workspace, session, admit)?;
        Ok((state, sources))
    }

    fn scoped_mutation<E: From<CandidateStoreError>>(
        &mut self,
        workspace: &WorkspaceId,
        request: &DirectMutationRequestV1,
        clock: impl FnOnce() -> UnixMillis,
        execute: impl FnOnce(
            &CandidateExecutionTransaction<'_, '_>,
            &mut dyn FnMut() -> UnixMillis,
        ) -> std::result::Result<CandidateSuccess, E>,
    ) -> std::result::Result<DirectMutationResultV1, E>;
}

pub type SuccessorProgressVerifier<'a> =
    dyn Fn(&WorkspaceBindingV1, &ExecutionSessionV3) -> Result<()> + 'a;

#[cfg(feature = "test-support")]
impl SuccessorReadStore for CandidateExecutionStore {
    fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        CandidateExecutionStore::workspace_root(self)
    }

    fn current_snapshot(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources, u64)> {
        CandidateExecutionStore::current_snapshot(self, workspace, session, admit)
    }

    fn history_snapshot_with_digest(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        location: CandidateSessionLocation,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<CandidateHistorySnapshot> {
        CandidateExecutionStore::history_snapshot_with_digest(
            self, workspace, session, location, admit,
        )
    }

    fn lookup_outcome(
        &self,
        workspace: &WorkspaceId,
        key: &IdempotencyKeyV1,
    ) -> Result<Option<DirectRecordedOutcomeV1>> {
        CandidateExecutionStore::lookup_outcome(self, workspace, key)
    }
}

#[cfg(feature = "test-support")]
impl SuccessorMutationStore for CandidateExecutionStore {
    fn scoped_mutation<E: From<CandidateStoreError>>(
        &mut self,
        workspace: &WorkspaceId,
        request: &DirectMutationRequestV1,
        clock: impl FnOnce() -> UnixMillis,
        execute: impl FnOnce(
            &CandidateExecutionTransaction<'_, '_>,
            &mut dyn FnMut() -> UnixMillis,
        ) -> std::result::Result<CandidateSuccess, E>,
    ) -> std::result::Result<DirectMutationResultV1, E> {
        self.mutate(workspace, request, |transaction| {
            let mut clock = Some(clock);
            let mut now = None;
            let mut read =
                || *now.get_or_insert_with(|| clock.take().expect("clock is read once")());
            execute(transaction, &mut read)
        })
    }
}

pub struct NativeSuccessorStore<'store, Admit> {
    store: &'store DirectStoreV1,
    deadline: Instant,
    admit: Admit,
    verify_progress: Option<&'store SuccessorProgressVerifier<'store>>,
}

impl<'store, Admit: Fn(&WorkspaceBindingV1) -> Result<()>> NativeSuccessorStore<'store, Admit> {
    pub fn new(store: &'store DirectStoreV1, deadline: Instant, admit: Admit) -> Self {
        Self {
            store,
            deadline,
            admit,
            verify_progress: None,
        }
    }

    pub fn with_progress_verifier(
        mut self,
        verify_progress: &'store SuccessorProgressVerifier<'store>,
    ) -> Self {
        self.verify_progress = Some(verify_progress);
        self
    }

    fn admit_workspace(&self, binding: &WorkspaceBindingV1, workspace: &WorkspaceId) -> Result<()> {
        if binding.identity().workspace_uuid() != workspace {
            return Err(DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch {
                expected: workspace.clone(),
                actual: binding.identity().workspace_uuid().clone(),
            }
            .into());
        }
        (self.admit)(binding)
    }

    fn read<T>(
        &self,
        workspace: &WorkspaceId,
        read: impl FnOnce(&DirectReadSnapshotV1<'_>) -> Result<T>,
    ) -> Result<T> {
        self.store
            .read_admitting_typed(
                self.deadline,
                |binding| self.admit_workspace(binding, workspace),
                |(), snapshot| read(snapshot),
            )
            .map_err(|error| match error {
                DirectReadFailureV1::Admission(error) | DirectReadFailureV1::Operation(error) => {
                    error.into()
                }
                DirectReadFailureV1::Resolver(error) => error,
            })?
    }
}

impl<Admit: Fn(&WorkspaceBindingV1) -> Result<()>> SuccessorReadStore
    for NativeSuccessorStore<'_, Admit>
{
    fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        self.store.layout().root()
    }

    fn current_snapshot(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<(ExecutionSessionV3, CandidateSources, u64)> {
        self.read(workspace, |snapshot| {
            match snapshot.successor_state(admit)? {
                Some((state, sources)) if &state.fence().session == session => {
                    Ok((state, sources, snapshot.committed_sequence()?))
                }
                other => Err(DomainError::SessionIdentityMismatch {
                    expected: session.clone(),
                    actual: other.map(|(state, _)| state.fence().session),
                }
                .into()),
            }
        })
    }

    fn history_snapshot_with_digest(
        &mut self,
        workspace: &WorkspaceId,
        session: &SessionId,
        location: CandidateSessionLocation,
        admit: impl Fn(&str) -> Result<CandidateDefinition>,
    ) -> Result<CandidateHistorySnapshot> {
        self.read(workspace, |snapshot| {
            snapshot.successor_history(session, location, admit)
        })
    }

    fn lookup_outcome(
        &self,
        workspace: &WorkspaceId,
        key: &IdempotencyKeyV1,
    ) -> Result<Option<DirectRecordedOutcomeV1>> {
        self.read(workspace, |snapshot| Ok(snapshot.outcome(key)?))
    }
}

impl<Admit: Fn(&WorkspaceBindingV1) -> Result<()>> SuccessorMutationStore
    for NativeSuccessorStore<'_, Admit>
{
    fn scoped_mutation<E: From<CandidateStoreError>>(
        &mut self,
        workspace: &WorkspaceId,
        request: &DirectMutationRequestV1,
        clock: impl FnOnce() -> UnixMillis,
        execute: impl FnOnce(
            &CandidateExecutionTransaction<'_, '_>,
            &mut dyn FnMut() -> UnixMillis,
        ) -> std::result::Result<CandidateSuccess, E>,
    ) -> std::result::Result<DirectMutationResultV1, E> {
        let DirectOutcomeScopeV1::Session(session) = request.scope() else {
            return Err(corrupt("scoped successor mutation lacks a session identity").into());
        };
        self.store
            .mutate_typed(request, self.deadline, clock, |transaction| {
                self.admit_workspace(transaction.binding(), workspace)?;
                transaction.apply_successor_transition(session, self.verify_progress, execute)
            })
            .map_err(|error| match error {
                DirectMutationFailureV1::Store(error) => CandidateStoreError::from(error).into(),
                DirectMutationFailureV1::Callback(error) => error,
            })
    }
}
