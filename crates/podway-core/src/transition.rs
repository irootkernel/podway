use std::collections::BTreeSet;

use crate::aggregate::{
    ArtifactLocationKindV1, AttemptInputV1, AttemptV1, BlockerV1, ItemSlotV1, ItemValueV1,
    SessionAggregateInputV1, SessionAggregateV1, StageProgressV1, required_items_satisfied,
};
use crate::procedure::{ItemSpecV1, ItemTypeV1, StageSpecV1};
use crate::{
    AttemptId, AttemptLifecycle, BlockerId, BlockerState, DomainCommandKind, DomainError, ItemId,
    Revision, SessionId, SessionLifecycle, StageId, StageProgressState, UnixMillis, WorkspaceId,
};

/// Stable execution inputs supplied by the caller rather than read from a clock or store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandContextV1 {
    pub expected_revision: Revision,
    pub now: UnixMillis,
}

/// Preconditions shared by every item mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemMutationPreconditionsV1 {
    pub expected_attempt_id: AttemptId,
    pub expected_item_revision: Revision,
}

/// Inputs required to create a new running session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartSessionV1 {
    pub task_title: String,
    pub snapshot: crate::ProcedureSnapshotV1,
    pub session_id: SessionId,
    pub first_attempt_id: AttemptId,
}

/// Inputs for the destructive start-replace operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartReplaceSessionV1 {
    pub expected_session_id: SessionId,
    pub confirmed: bool,
    pub start: StartSessionV1,
}

/// Inputs for `item.check`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckItemV1 {
    pub item_id: ItemId,
    pub preconditions: ItemMutationPreconditionsV1,
}

/// Inputs for `item.uncheck`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UncheckItemV1 {
    pub item_id: ItemId,
    pub preconditions: ItemMutationPreconditionsV1,
}

/// Inputs for `item.set`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetItemV1 {
    pub item_id: ItemId,
    pub value: ItemValueV1,
    pub preconditions: ItemMutationPreconditionsV1,
}

/// Inputs for `item.add`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddItemV1 {
    pub item_id: ItemId,
    pub value: String,
    pub preconditions: ItemMutationPreconditionsV1,
}

/// Inputs for `item.remove`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveItemV1 {
    pub item_id: ItemId,
    pub value: String,
    pub ignore_missing: bool,
    pub preconditions: ItemMutationPreconditionsV1,
}

/// Inputs for `item.attach`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachItemV1 {
    pub item_id: ItemId,
    pub value: crate::ArtifactValueV1,
    pub preconditions: ItemMutationPreconditionsV1,
}

/// Inputs for `item.clear`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClearItemV1 {
    pub item_id: ItemId,
    pub preconditions: ItemMutationPreconditionsV1,
}

/// Proof produced outside the pure domain layer after re-reading a local artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalArtifactVerificationV1 {
    pub item_id: ItemId,
    pub location: String,
    pub digest: crate::Sha256Digest,
    pub size_bytes: u64,
}

/// Inputs for `session.complete`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteSessionV1 {
    pub expected_attempt_id: AttemptId,
    pub next_attempt_id: Option<AttemptId>,
    pub local_artifact_verifications: Vec<LocalArtifactVerificationV1>,
}

/// Inputs for `session.skip`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkipSessionV1 {
    pub expected_attempt_id: AttemptId,
    pub reason: Option<String>,
    pub next_attempt_id: Option<AttemptId>,
}

/// Inputs for `session.retry`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetrySessionV1 {
    pub expected_attempt_id: AttemptId,
    pub reason: String,
    pub next_attempt_id: AttemptId,
}

/// Inputs for `session.return`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReturnSessionV1 {
    pub expected_attempt_id: AttemptId,
    pub destination_stage_id: StageId,
    pub reason: String,
    pub destination_attempt_id: AttemptId,
}

/// Inputs for `session.block`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockSessionV1 {
    pub expected_attempt_id: AttemptId,
    pub blocker_id: BlockerId,
    pub reason: String,
}

/// Inputs for `session.unblock`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnblockSessionV1 {
    pub expected_attempt_id: AttemptId,
    pub blocker_id: Option<BlockerId>,
    pub unblock_all: bool,
}

/// Inputs for `session.cancel`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelSessionV1 {
    pub expected_attempt_id: AttemptId,
    pub reason: String,
}

/// Inputs for `session.reopen`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReopenSessionV1 {
    pub expected_session_id: SessionId,
    pub destination_stage_id: StageId,
    pub reason: String,
    pub destination_attempt_id: AttemptId,
}

/// Inputs for `session.reset`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetSessionV1 {
    pub expected_session_id: SessionId,
    pub confirmed: bool,
}

/// Inputs for `workspace.reset_all`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetAllWorkspaceV1 {
    pub workspace_id: Option<WorkspaceId>,
    pub confirmed: bool,
}

/// Every canonical pure-domain command and its fully typed payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionCommandV1 {
    Start(StartSessionV1),
    StartReplace(StartReplaceSessionV1),
    Check(CheckItemV1),
    Uncheck(UncheckItemV1),
    Set(SetItemV1),
    Add(AddItemV1),
    Remove(RemoveItemV1),
    Attach(AttachItemV1),
    Clear(ClearItemV1),
    Complete(CompleteSessionV1),
    Skip(SkipSessionV1),
    Retry(RetrySessionV1),
    Return(ReturnSessionV1),
    Block(BlockSessionV1),
    Unblock(UnblockSessionV1),
    Cancel(CancelSessionV1),
    Reopen(ReopenSessionV1),
    Reset(ResetSessionV1),
    ResetAll(ResetAllWorkspaceV1),
}

impl SessionCommandV1 {
    pub const fn kind(&self) -> DomainCommandKind {
        match self {
            Self::Start(_) => DomainCommandKind::SessionStart,
            Self::StartReplace(_) => DomainCommandKind::SessionStartReplace,
            Self::Check(_) => DomainCommandKind::ItemCheck,
            Self::Uncheck(_) => DomainCommandKind::ItemUncheck,
            Self::Set(_) => DomainCommandKind::ItemSet,
            Self::Add(_) => DomainCommandKind::ItemAdd,
            Self::Remove(_) => DomainCommandKind::ItemRemove,
            Self::Attach(_) => DomainCommandKind::ItemAttach,
            Self::Clear(_) => DomainCommandKind::ItemClear,
            Self::Complete(_) => DomainCommandKind::SessionComplete,
            Self::Skip(_) => DomainCommandKind::SessionSkip,
            Self::Retry(_) => DomainCommandKind::SessionRetry,
            Self::Return(_) => DomainCommandKind::SessionReturn,
            Self::Block(_) => DomainCommandKind::SessionBlock,
            Self::Unblock(_) => DomainCommandKind::SessionUnblock,
            Self::Cancel(_) => DomainCommandKind::SessionCancel,
            Self::Reopen(_) => DomainCommandKind::SessionReopen,
            Self::Reset(_) => DomainCommandKind::SessionReset,
            Self::ResetAll(_) => DomainCommandKind::WorkspaceResetAll,
        }
    }
}

/// A non-persistent instruction for the workspace boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransitionEffectV1 {
    /// Recreate disposable workspace runtime state using the recovery-marker protocol.
    WorkspaceResetAll { workspace_id: Option<WorkspaceId> },
}

/// The complete result of applying a pure transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionOutcomeV1 {
    next_aggregate: Option<SessionAggregateV1>,
    changed: bool,
    revision_before: Option<Revision>,
    revision_after: Option<Revision>,
    active_stage_before: Option<StageId>,
    active_stage_after: Option<StageId>,
    affected_stages: Vec<StageId>,
    effect: Option<TransitionEffectV1>,
}

impl TransitionOutcomeV1 {
    pub fn next_aggregate(&self) -> Option<&SessionAggregateV1> {
        self.next_aggregate.as_ref()
    }

    pub const fn changed(&self) -> bool {
        self.changed
    }

    pub const fn revision_before(&self) -> Option<Revision> {
        self.revision_before
    }

    pub const fn revision_after(&self) -> Option<Revision> {
        self.revision_after
    }

    pub fn active_stage_before(&self) -> Option<&StageId> {
        self.active_stage_before.as_ref()
    }

    pub fn active_stage_after(&self) -> Option<&StageId> {
        self.active_stage_after.as_ref()
    }

    pub fn affected_stages(&self) -> &[StageId] {
        &self.affected_stages
    }

    pub fn effect(&self) -> Option<&TransitionEffectV1> {
        self.effect.as_ref()
    }
}

/// Applies one command without mutation, I/O, clock access, UUID generation, or randomness.
///
/// Session-level commands use `context.expected_revision`. Item commands intentionally use their
/// slot revision and active-attempt preconditions instead, so unrelated item writes do not conflict.
pub fn apply_transition_v1(
    prior: Option<&SessionAggregateV1>,
    command: &SessionCommandV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    match command {
        SessionCommandV1::Start(input) => apply_start(prior, input, context),
        SessionCommandV1::StartReplace(input) => apply_start_replace(prior, input, context),
        SessionCommandV1::Check(input) => apply_check(require_session(prior)?, input, context),
        SessionCommandV1::Uncheck(input) => apply_uncheck(require_session(prior)?, input, context),
        SessionCommandV1::Set(input) => apply_set(require_session(prior)?, input, context),
        SessionCommandV1::Add(input) => apply_add(require_session(prior)?, input, context),
        SessionCommandV1::Remove(input) => apply_remove(require_session(prior)?, input, context),
        SessionCommandV1::Attach(input) => apply_attach(require_session(prior)?, input, context),
        SessionCommandV1::Clear(input) => apply_clear(require_session(prior)?, input, context),
        SessionCommandV1::Complete(input) => {
            apply_complete(require_session(prior)?, input, context)
        }
        SessionCommandV1::Skip(input) => apply_skip(require_session(prior)?, input, context),
        SessionCommandV1::Retry(input) => apply_retry(require_session(prior)?, input, context),
        SessionCommandV1::Return(input) => apply_return(require_session(prior)?, input, context),
        SessionCommandV1::Block(input) => apply_block(require_session(prior)?, input, context),
        SessionCommandV1::Unblock(input) => apply_unblock(require_session(prior)?, input, context),
        SessionCommandV1::Cancel(input) => apply_cancel(require_session(prior)?, input, context),
        SessionCommandV1::Reopen(input) => apply_reopen(require_session(prior)?, input, context),
        SessionCommandV1::Reset(input) => apply_reset(require_session(prior)?, input, context),
        SessionCommandV1::ResetAll(input) => apply_reset_all(prior, input, context),
    }
}

/// Computes the exact same outcome as `apply_transition_v1` without a distinct preview path.
pub fn preview_transition_v1(
    prior: Option<&SessionAggregateV1>,
    command: &SessionCommandV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    apply_transition_v1(prior, command, context)
}

fn apply_start(
    prior: Option<&SessionAggregateV1>,
    input: &StartSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    if prior.is_some() {
        return Err(invalid("a session already exists"));
    }
    if context.expected_revision != Revision::ZERO {
        return Err(DomainError::PreconditionFailed {
            expected: context.expected_revision,
            actual: Revision::ZERO,
        });
    }
    let next = SessionAggregateV1::start(
        input.session_id.clone(),
        input.task_title.clone(),
        input.snapshot.clone(),
        input.first_attempt_id.clone(),
        context.now,
    )?;
    Ok(outcome(
        Some(next),
        true,
        None,
        vec![input.snapshot.stages()[0].id().clone()],
        None,
    ))
}

fn apply_start_replace(
    prior: Option<&SessionAggregateV1>,
    input: &StartReplaceSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let prior = require_session(prior)?;
    require_confirmation(input.confirmed)?;
    require_session_identity(prior, &input.expected_session_id)?;
    ensure_now(prior, context.now)?;
    require_revision(prior, context)?;
    if &input.start.session_id == prior.session_id() {
        return Err(invalid("start-replace requires a fresh session identifier"));
    }
    let next = SessionAggregateV1::start(
        input.start.session_id.clone(),
        input.start.task_title.clone(),
        input.start.snapshot.clone(),
        input.start.first_attempt_id.clone(),
        context.now,
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![input.start.snapshot.stages()[0].id().clone()],
        None,
    ))
}

fn apply_check(
    prior: &SessionAggregateV1,
    input: &CheckItemV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let (stage_index, attempt_index, slot_index) = item_location(
        prior,
        &input.item_id,
        &input.preconditions,
        DomainCommandKind::ItemCheck,
    )?;
    let stage = &prior.snapshot().stages()[stage_index];
    let specification = stage
        .item(&input.item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: input.item_id.clone(),
        })?;
    require_item_type(specification, ItemTypeV1::Confirm)?;
    ensure_now(prior, context.now)?;
    let current = &prior.attempts()[attempt_index].item_slots()[slot_index];
    let next_slot = write_slot(current, specification, ItemValueV1::confirm(), context.now)?;
    replace_slot(prior, stage_index, attempt_index, next_slot)
}

fn apply_uncheck(
    prior: &SessionAggregateV1,
    input: &UncheckItemV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let (stage_index, attempt_index, slot_index) = item_location(
        prior,
        &input.item_id,
        &input.preconditions,
        DomainCommandKind::ItemUncheck,
    )?;
    let stage = &prior.snapshot().stages()[stage_index];
    let specification = stage
        .item(&input.item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: input.item_id.clone(),
        })?;
    require_item_type(specification, ItemTypeV1::Confirm)?;
    let current = &prior.attempts()[attempt_index].item_slots()[slot_index];
    ensure_now(prior, context.now)?;
    let next_slot = clear_slot(current, specification, context.now)?;
    replace_slot(prior, stage_index, attempt_index, next_slot)
}

fn apply_set(
    prior: &SessionAggregateV1,
    input: &SetItemV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let (stage_index, attempt_index, slot_index) = item_location(
        prior,
        &input.item_id,
        &input.preconditions,
        DomainCommandKind::ItemSet,
    )?;
    let stage = &prior.snapshot().stages()[stage_index];
    let specification = stage
        .item(&input.item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: input.item_id.clone(),
        })?;
    match specification.item_type() {
        ItemTypeV1::Text | ItemTypeV1::Choice | ItemTypeV1::Integer => {}
        ItemTypeV1::Confirm | ItemTypeV1::List | ItemTypeV1::Artifact => {
            return Err(invalid("set is not valid for this item type"));
        }
    }
    if input.value.value_type() != item_value_type(specification.item_type()) {
        return Err(invalid("set value has the wrong item type"));
    }
    ensure_now(prior, context.now)?;
    let current = &prior.attempts()[attempt_index].item_slots()[slot_index];
    let next_slot = write_slot(current, specification, input.value.clone(), context.now)?;
    replace_slot(prior, stage_index, attempt_index, next_slot)
}

fn apply_add(
    prior: &SessionAggregateV1,
    input: &AddItemV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let (stage_index, attempt_index, slot_index) = item_location(
        prior,
        &input.item_id,
        &input.preconditions,
        DomainCommandKind::ItemAdd,
    )?;
    let stage = &prior.snapshot().stages()[stage_index];
    let specification = stage
        .item(&input.item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: input.item_id.clone(),
        })?;
    require_item_type(specification, ItemTypeV1::List)?;
    ensure_now(prior, context.now)?;
    let current = &prior.attempts()[attempt_index].item_slots()[slot_index];
    let mut values = current
        .value()
        .and_then(ItemValueV1::as_list)
        .map_or_else(Vec::new, ToOwned::to_owned);
    let ItemSpecV1::List(list_specification) = specification else {
        return Err(invalid("item specification type is inconsistent"));
    };
    if list_specification.unique() && values.contains(&input.value) {
        return Err(invalid("list item value is already present"));
    }
    values.push(input.value.clone());
    let value = ItemValueV1::list(values)?;
    let next_slot = write_slot(current, specification, value, context.now)?;
    replace_slot(prior, stage_index, attempt_index, next_slot)
}

fn apply_remove(
    prior: &SessionAggregateV1,
    input: &RemoveItemV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let (stage_index, attempt_index, slot_index) = item_location(
        prior,
        &input.item_id,
        &input.preconditions,
        DomainCommandKind::ItemRemove,
    )?;
    let stage = &prior.snapshot().stages()[stage_index];
    let specification = stage
        .item(&input.item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: input.item_id.clone(),
        })?;
    require_item_type(specification, ItemTypeV1::List)?;
    ensure_now(prior, context.now)?;
    let current = &prior.attempts()[attempt_index].item_slots()[slot_index];
    let mut values = current
        .value()
        .and_then(ItemValueV1::as_list)
        .map_or_else(Vec::new, ToOwned::to_owned);
    let Some(index) = values.iter().position(|value| value == &input.value) else {
        if input.ignore_missing {
            return unchanged_slot_outcome(prior, stage_index);
        }
        return Err(invalid("list item value is not present"));
    };
    values.remove(index);
    let value = ItemValueV1::list(values)?;
    let next_slot = write_slot(current, specification, value, context.now)?;
    replace_slot(prior, stage_index, attempt_index, next_slot)
}

fn apply_attach(
    prior: &SessionAggregateV1,
    input: &AttachItemV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let (stage_index, attempt_index, slot_index) = item_location(
        prior,
        &input.item_id,
        &input.preconditions,
        DomainCommandKind::ItemAttach,
    )?;
    let stage = &prior.snapshot().stages()[stage_index];
    let specification = stage
        .item(&input.item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: input.item_id.clone(),
        })?;
    require_item_type(specification, ItemTypeV1::Artifact)?;
    ensure_now(prior, context.now)?;
    let current = &prior.attempts()[attempt_index].item_slots()[slot_index];
    let next_slot = write_slot(
        current,
        specification,
        ItemValueV1::artifact(input.value.clone()),
        context.now,
    )?;
    replace_slot(prior, stage_index, attempt_index, next_slot)
}

fn apply_clear(
    prior: &SessionAggregateV1,
    input: &ClearItemV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let (stage_index, attempt_index, slot_index) = item_location(
        prior,
        &input.item_id,
        &input.preconditions,
        DomainCommandKind::ItemClear,
    )?;
    let stage = &prior.snapshot().stages()[stage_index];
    let specification = stage
        .item(&input.item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: input.item_id.clone(),
        })?;
    let current = &prior.attempts()[attempt_index].item_slots()[slot_index];
    ensure_now(prior, context.now)?;
    let next_slot = clear_slot(current, specification, context.now)?;
    replace_slot(prior, stage_index, attempt_index, next_slot)
}

fn apply_complete(
    prior: &SessionAggregateV1,
    input: &CompleteSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_running_revision(prior, DomainCommandKind::SessionComplete, context)?;
    let (stage_index, attempt_index) = current_location(
        prior,
        &input.expected_attempt_id,
        DomainCommandKind::SessionComplete,
    )?;
    ensure_attempt_boundary_now(prior, context.now)?;
    let stage = &prior.snapshot().stages()[stage_index];
    let attempt = &prior.attempts()[attempt_index];
    if attempt
        .blockers()
        .iter()
        .any(|blocker| blocker.state() == BlockerState::Open)
    {
        return Err(DomainError::BlockersPresent);
    }
    if !required_items_satisfied(stage, attempt.item_slots()) {
        return Err(DomainError::RequiredItemsMissing);
    }
    if !attempt.is_ready_to_complete(stage) {
        return Err(invalid("the active attempt is not ready to complete"));
    }
    verify_local_artifacts(stage, attempt, &input.local_artifact_verifications)?;
    if stage_index + 1 == prior.stage_progress().len() && input.next_attempt_id.is_some() {
        return Err(invalid(
            "a final completion must not include a next attempt identifier",
        ));
    }
    let revision = prior.revision().checked_next()?;
    let terminal = attempt.with_terminal(stage, AttemptLifecycle::Completed, context.now, None)?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = terminal;
    let mut progress = prior.stage_progress().to_vec();
    let current = progress[stage_index].clone();
    progress[stage_index] = progress_state(&current, StageProgressState::Done)?;

    if stage_index + 1 == progress.len() {
        let next = rebuild(
            prior,
            RebuildInputV1 {
                lifecycle: SessionLifecycle::Completed,
                revision,
                stage_progress: progress,
                attempts,
                active_stage_id: None,
                active_attempt_id: None,
                completed_at: Some(context.now),
                cancelled_at: None,
                cancel_reason: None,
            },
        )?;
        return Ok(outcome(
            Some(next),
            true,
            Some(prior),
            vec![stage.id().clone()],
            None,
        ));
    }

    let next_attempt_id = input
        .next_attempt_id
        .as_ref()
        .ok_or_else(|| invalid("a non-final completion requires a next attempt identifier"))?;
    let next_stage = &prior.snapshot().stages()[stage_index + 1];
    let next_progress = &progress[stage_index + 1];
    ensure_activatable(next_progress)?;
    let next_number = next_attempt_number(next_progress)?;
    let next_attempt = AttemptV1::fresh(
        next_attempt_id.clone(),
        prior.session_id().clone(),
        next_stage,
        next_number,
        context.now,
    )?;
    progress[stage_index + 1] = StageProgressV1::current(
        next_stage.id().clone(),
        stage_index + 1,
        next_attempt_id.clone(),
        next_number,
    )?;
    attempts.push(next_attempt);
    let next = rebuild_running(
        prior,
        revision,
        progress,
        attempts,
        next_stage.id().clone(),
        next_attempt_id.clone(),
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![stage.id().clone(), next_stage.id().clone()],
        None,
    ))
}

fn apply_skip(
    prior: &SessionAggregateV1,
    input: &SkipSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_running_revision(prior, DomainCommandKind::SessionSkip, context)?;
    let (stage_index, attempt_index) = current_location(
        prior,
        &input.expected_attempt_id,
        DomainCommandKind::SessionSkip,
    )?;
    ensure_attempt_boundary_now(prior, context.now)?;
    let stage = &prior.snapshot().stages()[stage_index];
    if !stage.skip_policy().is_allowed() {
        return Err(invalid("the active stage may not be skipped"));
    }
    if stage.skip_policy().reason_required() {
        require_reason(input.reason.as_deref())?;
    }
    let attempt = &prior.attempts()[attempt_index];
    if stage_index + 1 == prior.stage_progress().len() && input.next_attempt_id.is_some() {
        return Err(invalid(
            "a final skip must not include a next attempt identifier",
        ));
    }
    let revision = prior.revision().checked_next()?;
    let terminal = attempt.with_terminal(
        stage,
        AttemptLifecycle::Skipped,
        context.now,
        input.reason.clone(),
    )?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = terminal;
    let mut progress = prior.stage_progress().to_vec();
    let current = progress[stage_index].clone();
    progress[stage_index] = progress_state(&current, StageProgressState::Skipped)?;

    if stage_index + 1 == progress.len() {
        let next = rebuild(
            prior,
            RebuildInputV1 {
                lifecycle: SessionLifecycle::Completed,
                revision,
                stage_progress: progress,
                attempts,
                active_stage_id: None,
                active_attempt_id: None,
                completed_at: Some(context.now),
                cancelled_at: None,
                cancel_reason: None,
            },
        )?;
        return Ok(outcome(
            Some(next),
            true,
            Some(prior),
            vec![stage.id().clone()],
            None,
        ));
    }

    let next_attempt_id = input
        .next_attempt_id
        .as_ref()
        .ok_or_else(|| invalid("a non-final skip requires a next attempt identifier"))?;
    let next_stage = &prior.snapshot().stages()[stage_index + 1];
    let next_progress = &progress[stage_index + 1];
    ensure_activatable(next_progress)?;
    let next_number = next_attempt_number(next_progress)?;
    let next_attempt = AttemptV1::fresh(
        next_attempt_id.clone(),
        prior.session_id().clone(),
        next_stage,
        next_number,
        context.now,
    )?;
    progress[stage_index + 1] = StageProgressV1::current(
        next_stage.id().clone(),
        stage_index + 1,
        next_attempt_id.clone(),
        next_number,
    )?;
    attempts.push(next_attempt);
    let next = rebuild_running(
        prior,
        revision,
        progress,
        attempts,
        next_stage.id().clone(),
        next_attempt_id.clone(),
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![stage.id().clone(), next_stage.id().clone()],
        None,
    ))
}

fn apply_retry(
    prior: &SessionAggregateV1,
    input: &RetrySessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_running_revision(prior, DomainCommandKind::SessionRetry, context)?;
    let (stage_index, attempt_index) = current_location(
        prior,
        &input.expected_attempt_id,
        DomainCommandKind::SessionRetry,
    )?;
    ensure_attempt_boundary_now(prior, context.now)?;
    require_reason(Some(&input.reason))?;
    let stage = &prior.snapshot().stages()[stage_index];
    let revision = prior.revision().checked_next()?;
    let terminal = prior.attempts()[attempt_index].with_terminal(
        stage,
        AttemptLifecycle::Abandoned,
        context.now,
        Some(input.reason.clone()),
    )?;
    let next_number = next_attempt_number(&prior.stage_progress()[stage_index])?;
    let next_attempt = AttemptV1::fresh(
        input.next_attempt_id.clone(),
        prior.session_id().clone(),
        stage,
        next_number,
        context.now,
    )?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = terminal;
    attempts.push(next_attempt);
    let mut progress = prior.stage_progress().to_vec();
    progress[stage_index] = StageProgressV1::current(
        stage.id().clone(),
        stage_index,
        input.next_attempt_id.clone(),
        next_number,
    )?;
    let next = rebuild_running(
        prior,
        revision,
        progress,
        attempts,
        stage.id().clone(),
        input.next_attempt_id.clone(),
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![stage.id().clone()],
        None,
    ))
}

fn apply_return(
    prior: &SessionAggregateV1,
    input: &ReturnSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_running_revision(prior, DomainCommandKind::SessionReturn, context)?;
    let (current_index, attempt_index) = current_location(
        prior,
        &input.expected_attempt_id,
        DomainCommandKind::SessionReturn,
    )?;
    ensure_attempt_boundary_now(prior, context.now)?;
    require_reason(Some(&input.reason))?;
    let destination_index = stage_index(prior, &input.destination_stage_id)?;
    if destination_index >= current_index
        || !prior
            .snapshot()
            .return_policy()
            .allows_destination(&input.destination_stage_id)
    {
        return Err(invalid(
            "return destination is not an allowed earlier stage",
        ));
    }
    let revision = prior.revision().checked_next()?;
    let terminal = prior.attempts()[attempt_index].with_terminal(
        &prior.snapshot().stages()[current_index],
        AttemptLifecycle::Abandoned,
        context.now,
        Some(input.reason.clone()),
    )?;
    let destination = &prior.snapshot().stages()[destination_index];
    let number = next_attempt_number(&prior.stage_progress()[destination_index])?;
    let fresh = AttemptV1::fresh(
        input.destination_attempt_id.clone(),
        prior.session_id().clone(),
        destination,
        number,
        context.now,
    )?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = terminal;
    attempts.push(fresh);
    let mut progress = prior.stage_progress().to_vec();
    progress[destination_index] = StageProgressV1::current(
        destination.id().clone(),
        destination_index,
        input.destination_attempt_id.clone(),
        number,
    )?;
    let highest_reached = progress
        .iter()
        .rposition(|entry| entry.state() != StageProgressState::Pending)
        .ok_or_else(|| invalid("a running session has no reached stage"))?;
    let mut affected = vec![destination.id().clone()];
    for progress_entry in progress
        .iter_mut()
        .take(highest_reached + 1)
        .skip(destination_index + 1)
    {
        let prior_entry = progress_entry.clone();
        *progress_entry = progress_state(&prior_entry, StageProgressState::Redo)?;
        affected.push(progress_entry.stage_id().clone());
    }
    let next = rebuild_running(
        prior,
        revision,
        progress,
        attempts,
        destination.id().clone(),
        input.destination_attempt_id.clone(),
    )?;
    Ok(outcome(Some(next), true, Some(prior), affected, None))
}

fn apply_block(
    prior: &SessionAggregateV1,
    input: &BlockSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_running_revision(prior, DomainCommandKind::SessionBlock, context)?;
    let (stage_index, attempt_index) = current_location(
        prior,
        &input.expected_attempt_id,
        DomainCommandKind::SessionBlock,
    )?;
    ensure_now(prior, context.now)?;
    require_reason(Some(&input.reason))?;
    if prior
        .attempts()
        .iter()
        .flat_map(AttemptV1::blockers)
        .any(|blocker| blocker.blocker_id() == &input.blocker_id)
    {
        return Err(invalid(
            "blocker identifier has already been used in this session",
        ));
    }
    let revision = prior.revision().checked_next()?;
    let blocker = BlockerV1::open(
        input.blocker_id.clone(),
        input.expected_attempt_id.clone(),
        input.reason.clone(),
        context.now,
    )?;
    let replacement = prior.attempts()[attempt_index].with_added_blocker(blocker)?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = replacement;
    let next = rebuild_running(
        prior,
        revision,
        prior.stage_progress().to_vec(),
        attempts,
        prior.snapshot().stages()[stage_index].id().clone(),
        input.expected_attempt_id.clone(),
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![prior.snapshot().stages()[stage_index].id().clone()],
        None,
    ))
}

fn apply_unblock(
    prior: &SessionAggregateV1,
    input: &UnblockSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_running_revision(prior, DomainCommandKind::SessionUnblock, context)?;
    let (stage_index, attempt_index) = current_location(
        prior,
        &input.expected_attempt_id,
        DomainCommandKind::SessionUnblock,
    )?;
    ensure_now(prior, context.now)?;
    match (&input.blocker_id, input.unblock_all) {
        (Some(_), false) | (None, true) => {}
        (None, false) | (Some(_), true) => {
            return Err(invalid(
                "unblock requires exactly one blocker identifier or unblock_all",
            ));
        }
    }
    let attempt = &prior.attempts()[attempt_index];
    let selected: BTreeSet<&BlockerId> = if input.unblock_all {
        attempt
            .blockers()
            .iter()
            .filter(|blocker| blocker.state() == BlockerState::Open)
            .map(BlockerV1::blocker_id)
            .collect()
    } else {
        let blocker_id = input
            .blocker_id
            .as_ref()
            .ok_or_else(|| invalid("blocker identifier is required"))?;
        let Some(blocker) = attempt
            .blockers()
            .iter()
            .find(|blocker| blocker.blocker_id() == blocker_id)
        else {
            if prior
                .attempts()
                .iter()
                .flat_map(AttemptV1::blockers)
                .any(|blocker| blocker.blocker_id() == blocker_id)
            {
                return Err(DomainError::BlockerNotCurrent);
            }
            return Err(invalid("blocker identifier was not found"));
        };
        if blocker.state() != BlockerState::Open {
            return Err(invalid("blocker is already resolved"));
        }
        let mut selected = BTreeSet::new();
        selected.insert(blocker_id);
        selected
    };
    if selected.is_empty() {
        return Err(invalid("the active attempt has no open blockers"));
    }
    let revision = prior.revision().checked_next()?;
    let blockers = attempt
        .blockers()
        .iter()
        .map(|blocker| {
            if selected
                .iter()
                .any(|blocker_id| *blocker_id == blocker.blocker_id())
            {
                blocker.resolve(context.now)
            } else {
                Ok(blocker.clone())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let replacement = AttemptV1::new(AttemptInputV1 {
        attempt_id: attempt.attempt_id().clone(),
        session_id: attempt.session_id().clone(),
        stage: &prior.snapshot().stages()[stage_index],
        number: attempt.number(),
        lifecycle: attempt.lifecycle(),
        started_at: attempt.started_at(),
        ended_at: attempt.ended_at(),
        reason: attempt.reason().map(ToOwned::to_owned),
        item_slots: attempt.item_slots().to_vec(),
        blockers,
    })?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = replacement;
    let next = rebuild_running(
        prior,
        revision,
        prior.stage_progress().to_vec(),
        attempts,
        prior.snapshot().stages()[stage_index].id().clone(),
        input.expected_attempt_id.clone(),
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![prior.snapshot().stages()[stage_index].id().clone()],
        None,
    ))
}

fn apply_cancel(
    prior: &SessionAggregateV1,
    input: &CancelSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_running_revision(prior, DomainCommandKind::SessionCancel, context)?;
    let (stage_index, attempt_index) = current_location(
        prior,
        &input.expected_attempt_id,
        DomainCommandKind::SessionCancel,
    )?;
    ensure_attempt_boundary_now(prior, context.now)?;
    require_reason(Some(&input.reason))?;
    let revision = prior.revision().checked_next()?;
    let terminal = prior.attempts()[attempt_index].with_terminal(
        &prior.snapshot().stages()[stage_index],
        AttemptLifecycle::Abandoned,
        context.now,
        Some(input.reason.clone()),
    )?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = terminal;
    let mut progress = prior.stage_progress().to_vec();
    let current = progress[stage_index].clone();
    progress[stage_index] = progress_state(&current, StageProgressState::Abandoned)?;
    let next = rebuild(
        prior,
        RebuildInputV1 {
            lifecycle: SessionLifecycle::Cancelled,
            revision,
            stage_progress: progress,
            attempts,
            active_stage_id: None,
            active_attempt_id: None,
            completed_at: None,
            cancelled_at: Some(context.now),
            cancel_reason: Some(input.reason.clone()),
        },
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![prior.snapshot().stages()[stage_index].id().clone()],
        None,
    ))
}

fn apply_reopen(
    prior: &SessionAggregateV1,
    input: &ReopenSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_session_identity(prior, &input.expected_session_id)?;
    if prior.lifecycle() != SessionLifecycle::Completed {
        return Err(DomainError::InvalidTransition {
            command: DomainCommandKind::SessionReopen,
            state: prior.lifecycle(),
        });
    }
    require_revision(prior, context)?;
    ensure_attempt_boundary_now(prior, context.now)?;
    require_reason(Some(&input.reason))?;
    let destination_index = stage_index(prior, &input.destination_stage_id)?;
    if !prior
        .snapshot()
        .return_policy()
        .allows_destination(&input.destination_stage_id)
    {
        return Err(invalid(
            "reopen destination is not allowed by the procedure",
        ));
    }
    let destination = &prior.snapshot().stages()[destination_index];
    let number = next_attempt_number(&prior.stage_progress()[destination_index])?;
    let revision = prior.revision().checked_next()?;
    let fresh = AttemptV1::fresh_with_reason(
        input.destination_attempt_id.clone(),
        prior.session_id().clone(),
        destination,
        number,
        context.now,
        Some(input.reason.clone()),
    )?;
    let mut attempts = prior.attempts().to_vec();
    attempts.push(fresh);
    let mut progress = prior.stage_progress().to_vec();
    progress[destination_index] = StageProgressV1::current(
        destination.id().clone(),
        destination_index,
        input.destination_attempt_id.clone(),
        number,
    )?;
    let mut affected = vec![destination.id().clone()];
    for progress_entry in progress.iter_mut().skip(destination_index + 1) {
        let prior_entry = progress_entry.clone();
        if prior_entry.state() != StageProgressState::Pending {
            *progress_entry = progress_state(&prior_entry, StageProgressState::Redo)?;
            affected.push(progress_entry.stage_id().clone());
        }
    }
    let next = rebuild_running(
        prior,
        revision,
        progress,
        attempts,
        destination.id().clone(),
        input.destination_attempt_id.clone(),
    )?;
    Ok(outcome(Some(next), true, Some(prior), affected, None))
}

fn apply_reset(
    prior: &SessionAggregateV1,
    input: &ResetSessionV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_confirmation(input.confirmed)?;
    require_session_identity(prior, &input.expected_session_id)?;
    ensure_now(prior, context.now)?;
    require_revision(prior, context)?;
    let affected_stages = prior
        .stage_progress()
        .iter()
        .map(|progress| progress.stage_id().clone())
        .collect();
    Ok(outcome(None, true, Some(prior), affected_stages, None))
}

fn apply_reset_all(
    prior: Option<&SessionAggregateV1>,
    input: &ResetAllWorkspaceV1,
    context: CommandContextV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    require_confirmation(input.confirmed)?;
    if let Some(prior) = prior {
        ensure_now(prior, context.now)?;
    }
    Ok(outcome(
        None,
        true,
        prior,
        Vec::new(),
        Some(TransitionEffectV1::WorkspaceResetAll {
            workspace_id: input.workspace_id.clone(),
        }),
    ))
}

fn require_session(prior: Option<&SessionAggregateV1>) -> Result<&SessionAggregateV1, DomainError> {
    prior.ok_or_else(|| invalid("the workspace has no session"))
}

fn require_running_revision(
    prior: &SessionAggregateV1,
    command: DomainCommandKind,
    context: CommandContextV1,
) -> Result<(), DomainError> {
    if prior.lifecycle() != SessionLifecycle::Running {
        return Err(DomainError::InvalidTransition {
            command,
            state: prior.lifecycle(),
        });
    }
    require_revision(prior, context)
}

fn require_revision(
    prior: &SessionAggregateV1,
    context: CommandContextV1,
) -> Result<(), DomainError> {
    if context.expected_revision != prior.revision() {
        return Err(DomainError::PreconditionFailed {
            expected: context.expected_revision,
            actual: prior.revision(),
        });
    }
    Ok(())
}

fn require_session_identity(
    prior: &SessionAggregateV1,
    expected_session_id: &SessionId,
) -> Result<(), DomainError> {
    if prior.session_id() != expected_session_id {
        return Err(DomainError::SessionIdentityMismatch {
            expected: expected_session_id.clone(),
            actual: Some(prior.session_id().clone()),
        });
    }
    Ok(())
}

fn require_confirmation(confirmed: bool) -> Result<(), DomainError> {
    if !confirmed {
        return Err(invalid("explicit confirmation is required"));
    }
    Ok(())
}

fn require_reason(reason: Option<&str>) -> Result<(), DomainError> {
    let Some(reason) = reason else {
        return Err(invalid("a non-empty reason is required"));
    };
    if reason.trim().is_empty() || reason.chars().count() > 4_000 {
        return Err(invalid(
            "reason must contain at most 4000 non-blank scalars",
        ));
    }
    Ok(())
}

fn ensure_now(prior: &SessionAggregateV1, now: UnixMillis) -> Result<(), DomainError> {
    if now < prior.latest_recorded_at() {
        return Err(invalid(
            "transition timestamp precedes the latest retained timestamp",
        ));
    }
    Ok(())
}

fn ensure_attempt_boundary_now(
    prior: &SessionAggregateV1,
    now: UnixMillis,
) -> Result<(), DomainError> {
    ensure_now(prior, now)?;
    if now <= prior.latest_attempt_boundary_at() {
        return Err(invalid(
            "attempt lifecycle timestamp must advance beyond the latest attempt boundary",
        ));
    }
    Ok(())
}

fn current_location(
    prior: &SessionAggregateV1,
    expected_attempt_id: &AttemptId,
    command: DomainCommandKind,
) -> Result<(usize, usize), DomainError> {
    if prior.lifecycle() != SessionLifecycle::Running {
        return Err(DomainError::InvalidTransition {
            command,
            state: prior.lifecycle(),
        });
    }
    let active_attempt_id = prior
        .active_attempt_id()
        .ok_or_else(|| invalid("running session has no active attempt"))?;
    if active_attempt_id != expected_attempt_id {
        return Err(DomainError::AttemptNotCurrent {
            expected: expected_attempt_id.clone(),
            actual: Some(active_attempt_id.clone()),
        });
    }
    let active_stage_id = prior
        .active_stage_id()
        .ok_or_else(|| invalid("running session has no active stage"))?;
    let stage_index = stage_index(prior, active_stage_id)?;
    let attempt_index = prior
        .attempts()
        .iter()
        .position(|attempt| attempt.attempt_id() == expected_attempt_id)
        .ok_or_else(|| invalid("active attempt is absent from history"))?;
    if prior.attempts()[attempt_index].lifecycle() != AttemptLifecycle::Active {
        return Err(invalid("the expected attempt is not active"));
    }
    Ok((stage_index, attempt_index))
}

fn item_location(
    prior: &SessionAggregateV1,
    item_id: &ItemId,
    preconditions: &ItemMutationPreconditionsV1,
    command: DomainCommandKind,
) -> Result<(usize, usize, usize), DomainError> {
    let (stage_index, attempt_index) =
        current_location(prior, &preconditions.expected_attempt_id, command)?;
    let slot_index = prior.attempts()[attempt_index]
        .item_slots()
        .iter()
        .position(|slot| slot.item_id() == item_id)
        .ok_or_else(|| DomainError::ItemNotFound {
            item_id: item_id.clone(),
        })?;
    let slot = &prior.attempts()[attempt_index].item_slots()[slot_index];
    if slot.revision() != preconditions.expected_item_revision {
        return Err(DomainError::PreconditionFailed {
            expected: preconditions.expected_item_revision,
            actual: slot.revision(),
        });
    }
    Ok((stage_index, attempt_index, slot_index))
}

fn stage_index(prior: &SessionAggregateV1, stage_id: &StageId) -> Result<usize, DomainError> {
    prior
        .stage_progress()
        .iter()
        .position(|progress| progress.stage_id() == stage_id)
        .ok_or_else(|| invalid("stage identifier is absent from the snapshot"))
}

fn require_item_type(specification: &ItemSpecV1, expected: ItemTypeV1) -> Result<(), DomainError> {
    if specification.item_type() != expected {
        return Err(invalid("item command does not match the item type"));
    }
    Ok(())
}

fn item_value_type(item_type: ItemTypeV1) -> crate::ItemValueTypeV1 {
    match item_type {
        ItemTypeV1::Confirm => crate::ItemValueTypeV1::Confirm,
        ItemTypeV1::Text => crate::ItemValueTypeV1::Text,
        ItemTypeV1::Choice => crate::ItemValueTypeV1::Choice,
        ItemTypeV1::Integer => crate::ItemValueTypeV1::Integer,
        ItemTypeV1::List => crate::ItemValueTypeV1::List,
        ItemTypeV1::Artifact => crate::ItemValueTypeV1::Artifact,
    }
}

fn write_slot(
    current: &ItemSlotV1,
    specification: &ItemSpecV1,
    value: ItemValueV1,
    now: UnixMillis,
) -> Result<ItemSlotV1, DomainError> {
    if current.value() == Some(&value) {
        return Ok(current.clone());
    }
    current.with_value(specification, value, now)
}

fn clear_slot(
    current: &ItemSlotV1,
    specification: &ItemSpecV1,
    now: UnixMillis,
) -> Result<ItemSlotV1, DomainError> {
    if current.value().is_none() {
        return Ok(current.clone());
    }
    current.with_cleared(specification, now)
}

fn replace_slot(
    prior: &SessionAggregateV1,
    stage_index: usize,
    attempt_index: usize,
    slot: ItemSlotV1,
) -> Result<TransitionOutcomeV1, DomainError> {
    let stage = &prior.snapshot().stages()[stage_index];
    let current_slot = prior.attempts()[attempt_index]
        .item_slots()
        .iter()
        .find(|current| current.item_id() == slot.item_id())
        .ok_or_else(|| invalid("replacement item slot is absent"))?;
    if current_slot == &slot {
        return unchanged_slot_outcome(prior, stage_index);
    }
    let replacement = prior.attempts()[attempt_index].with_replaced_slot(stage, slot)?;
    let mut attempts = prior.attempts().to_vec();
    attempts[attempt_index] = replacement;
    let active_attempt_id = prior
        .active_attempt_id()
        .ok_or_else(|| invalid("running session has no active attempt"))?
        .clone();
    let next = rebuild_running(
        prior,
        prior.revision().checked_next()?,
        prior.stage_progress().to_vec(),
        attempts,
        stage.id().clone(),
        active_attempt_id,
    )?;
    Ok(outcome(
        Some(next),
        true,
        Some(prior),
        vec![stage.id().clone()],
        None,
    ))
}

fn unchanged_slot_outcome(
    prior: &SessionAggregateV1,
    stage_index: usize,
) -> Result<TransitionOutcomeV1, DomainError> {
    Ok(outcome(
        Some(prior.clone()),
        false,
        Some(prior),
        vec![prior.snapshot().stages()[stage_index].id().clone()],
        None,
    ))
}

fn verify_local_artifacts(
    stage: &StageSpecV1,
    attempt: &AttemptV1,
    verifications: &[LocalArtifactVerificationV1],
) -> Result<(), DomainError> {
    let mut supplied = BTreeSet::new();
    for verification in verifications {
        if !supplied.insert(&verification.item_id) {
            return Err(invalid(
                "local artifact verification item identifiers must be unique",
            ));
        }
        let specification =
            stage
                .item(&verification.item_id)
                .ok_or_else(|| DomainError::ItemNotFound {
                    item_id: verification.item_id.clone(),
                })?;
        require_item_type(specification, ItemTypeV1::Artifact)?;
        let slot = attempt
            .item_slots()
            .iter()
            .find(|slot| slot.item_id() == &verification.item_id)
            .ok_or_else(|| invalid("artifact item slot is absent"))?;
        let artifact = slot
            .value()
            .and_then(ItemValueV1::as_artifact)
            .ok_or_else(|| invalid("local artifact verification has no attached artifact"))?;
        if artifact.location_kind() != ArtifactLocationKindV1::LocalPath
            || artifact.location() != verification.location
            || artifact.digest() != &verification.digest
            || artifact.size_bytes() != verification.size_bytes
        {
            return Err(invalid(
                "local artifact verification does not match the attached artifact",
            ));
        }
    }
    for specification in stage.items() {
        if !specification.common().required() || specification.item_type() != ItemTypeV1::Artifact {
            continue;
        }
        let slot = attempt
            .item_slots()
            .iter()
            .find(|slot| slot.item_id() == specification.id())
            .ok_or_else(|| invalid("required artifact item slot is absent"))?;
        let Some(artifact) = slot.value().and_then(ItemValueV1::as_artifact) else {
            continue;
        };
        if artifact.location_kind() == ArtifactLocationKindV1::LocalPath
            && !supplied
                .iter()
                .any(|item_id| *item_id == specification.id())
        {
            return Err(invalid("required local artifact was not verified"));
        }
    }
    Ok(())
}

fn next_attempt_number(progress: &StageProgressV1) -> Result<u32, DomainError> {
    progress
        .latest_attempt_number()
        .checked_add(1)
        .ok_or_else(|| invalid("attempt number cannot be incremented"))
}

fn ensure_activatable(progress: &StageProgressV1) -> Result<(), DomainError> {
    match progress.state() {
        StageProgressState::Pending | StageProgressState::Redo => Ok(()),
        StageProgressState::Current
        | StageProgressState::Done
        | StageProgressState::Skipped
        | StageProgressState::Abandoned => Err(invalid("next ordered stage is not activatable")),
    }
}

fn progress_state(
    progress: &StageProgressV1,
    state: StageProgressState,
) -> Result<StageProgressV1, DomainError> {
    StageProgressV1::new(
        progress.stage_id().clone(),
        progress.stage_index(),
        state,
        progress.latest_attempt_number(),
        progress.latest_attempt_id().cloned(),
    )
}

struct RebuildInputV1 {
    lifecycle: SessionLifecycle,
    revision: Revision,
    stage_progress: Vec<StageProgressV1>,
    attempts: Vec<AttemptV1>,
    active_stage_id: Option<StageId>,
    active_attempt_id: Option<AttemptId>,
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<String>,
}

fn rebuild_running(
    prior: &SessionAggregateV1,
    revision: Revision,
    stage_progress: Vec<StageProgressV1>,
    attempts: Vec<AttemptV1>,
    active_stage_id: StageId,
    active_attempt_id: AttemptId,
) -> Result<SessionAggregateV1, DomainError> {
    rebuild(
        prior,
        RebuildInputV1 {
            lifecycle: SessionLifecycle::Running,
            revision,
            stage_progress,
            attempts,
            active_stage_id: Some(active_stage_id),
            active_attempt_id: Some(active_attempt_id),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        },
    )
}

fn rebuild(
    prior: &SessionAggregateV1,
    input: RebuildInputV1,
) -> Result<SessionAggregateV1, DomainError> {
    SessionAggregateV1::new(SessionAggregateInputV1 {
        session_id: prior.session_id().clone(),
        task_title: prior.task_title().to_owned(),
        snapshot: prior.snapshot().clone(),
        lifecycle: input.lifecycle,
        revision: input.revision,
        stage_progress: input.stage_progress,
        attempts: input.attempts,
        active_stage_id: input.active_stage_id,
        active_attempt_id: input.active_attempt_id,
        created_at: prior.created_at(),
        completed_at: input.completed_at,
        cancelled_at: input.cancelled_at,
        cancel_reason: input.cancel_reason,
    })
}

fn outcome(
    next_aggregate: Option<SessionAggregateV1>,
    changed: bool,
    prior: Option<&SessionAggregateV1>,
    affected_stages: Vec<StageId>,
    effect: Option<TransitionEffectV1>,
) -> TransitionOutcomeV1 {
    let revision_before = prior.map(SessionAggregateV1::revision);
    let active_stage_before = prior.and_then(|aggregate| aggregate.active_stage_id().cloned());
    let revision_after = next_aggregate.as_ref().map(SessionAggregateV1::revision);
    let active_stage_after = next_aggregate
        .as_ref()
        .and_then(|aggregate| aggregate.active_stage_id().cloned());
    TransitionOutcomeV1 {
        next_aggregate,
        changed,
        revision_before,
        revision_after,
        active_stage_before,
        active_stage_after,
        affected_stages,
        effect,
    }
}

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}
