//! Typed current-cursor and item-local fixture mutations.

use super::*;
use podway_core::workflow_v2::item_value_json_v2;
use podway_core::{
    BlockerId, BlockerState, BlockerStateV2, GoalOutcome, MAX_ACTIVE_ITEM_MUTATIONS_V2, OptionId,
    ReasonV2,
};
use podway_protocol::serialized_json_len_v1;

pub enum SuccessorCursorMutation {
    Complete,
    Decide {
        option: OptionId,
        reason: ReasonV2,
        goal_outcome: Option<GoalOutcome>,
    },
    Rework {
        target: podway_core::GraphNodeId,
        reason: ReasonV2,
    },
    Retry(ReasonV2),
    Skip(Option<ReasonV2>),
    DefineGoal(ExecutionGoalV3),
    ReviseGoal(ExecutionGoalChangeV3),
    Cancel(ReasonV2),
    Block(ReasonV2),
    Unblock(BlockerId),
}

/// The immutable attempt identity and selected activation are mandatory. A write to one slot does
/// not rely on the session revision or on an unrelated slot's revision.
pub struct SuccessorItemRequest {
    pub workspace: WorkspaceId,
    pub key: IdempotencyKeyV1,
    pub request_id: RequestIdV1,
    pub target: ScopedAttemptV1,
    pub selected_group: Option<GroupExecutionIdV1>,
}

pub fn issue_cursor_mutation(
    store: &mut CandidateExecutionStore,
    request: SuccessorMutationRequest,
    operation: SuccessorCursorMutation,
    clock: impl FnOnce() -> UnixMillis,
    ids: &impl DirectIdSourceV1,
) -> Result<DirectMutationResultV1> {
    let (command, body) = operation.request_parts();
    scoped_mutation(
        store,
        request,
        command,
        body,
        clock,
        |workspace, state, fence, now| {
            operation.apply(workspace, state, fence, now, ids)?;
            Ok(None)
        },
    )
}

pub fn record_successor_item(
    store: &mut CandidateExecutionStore,
    request: SuccessorItemRequest,
    write: GroupItemWriteV1,
    clock: impl FnOnce() -> UnixMillis,
) -> Result<DirectMutationResultV1> {
    let body = item_write_json(&write)?;
    let exact = MutationIdentity {
        workspace: &request.workspace,
        key: &request.key,
        request_id: &request.request_id,
        session: request.target.session(),
    }
    .prepare(
        "item.record",
        json!({
            "attempt": request.target,
            "selected_group": request.selected_group,
        }),
        body,
    )?;
    store.mutate(&request.workspace, &exact, |transaction| {
        let (mut state, sources) = transaction.load(admit_candidate_definition)?;
        verify_selection(&state, request.selected_group.as_ref())?;
        state.verify_attempt(&request.target)?;
        let before = state.fence().revision;
        let now = clock();
        let revision = state
            .record_item(
                &request.target,
                &write.item,
                write.expected_revision,
                write.value,
                now,
            )
            .map_err(item_write_error)?;
        finish_current_mutation(
            transaction,
            &exact,
            &state,
            &sources,
            before,
            now,
            Some(revision != write.expected_revision),
        )
    })
}

pub fn record_successor_many(
    store: &mut CandidateExecutionStore,
    request: SuccessorMutationRequest,
    mut writes: Vec<GroupItemWriteV1>,
    clock: impl FnOnce() -> UnixMillis,
) -> Result<DirectMutationResultV1> {
    if writes.len() > MAX_ACTIVE_ITEM_MUTATIONS_V2 {
        return Err(GraphMutationErrorV2::TooManyItemMutations {
            maximum: MAX_ACTIVE_ITEM_MUTATIONS_V2 as u32,
        }
        .into());
    }
    writes.sort_by(|left, right| left.item.cmp(&right.item));
    let mut body = Vec::with_capacity(writes.len());
    let mut remaining = MAX_DIRECT_INPUT_BYTES_V1;
    // Charge each input before building the entire body. The canonical identity also charges the
    // surrounding punctuation, command and exact fences.
    for write in &writes {
        let write = item_write_json(write)?;
        let encoded = serialized_json_len_v1(&write).map_err(|_| {
            SuccessorExecutionError::Invalid("candidate record-many body cannot be encoded")
        })?;
        remaining = remaining
            .checked_sub(encoded)
            .ok_or(SuccessorExecutionError::Invalid(
                "candidate request exceeds the direct input bound",
            ))?;
        body.push(write);
    }
    scoped_mutation(
        store,
        request,
        "item.record_many",
        Value::Array(body),
        clock,
        |_, state, fence, now| {
            // scoped_mutation has already compared the live session revision.
            let changed = state
                .record_many(fence, writes, now)
                .map_err(|error| match error {
                    GraphMutationErrorV2::Domain(error) => item_write_error(error),
                    error => error,
                })?;
            Ok(Some(changed))
        },
    )
}

fn item_write_error(error: DomainError) -> GraphMutationErrorV2 {
    match error {
        DomainError::PreconditionFailed { expected, actual } => {
            GraphMutationErrorV2::ItemRevisionConflict { expected, actual }
        }
        error => GraphMutationErrorV2::Domain(error),
    }
}

pub fn accept_successor_delegation(
    store: &mut CandidateExecutionStore,
    request: SuccessorMutationRequest,
    binding: ScopedDelegationBindingV1,
    clock: impl FnOnce() -> UnixMillis,
) -> Result<DirectMutationResultV1> {
    let body = json!({"binding": binding});
    scoped_mutation(
        store,
        request,
        "delegation.accept",
        body,
        clock,
        |workspace, state, fence, now| {
            let package = current::delegation_package(workspace, state)?;
            if state
                .active_attempt()
                .and_then(GroupAttemptStateV1::delegation)
                .is_some()
            {
                return Err(SuccessorExecutionError::Invalid(
                    "candidate action already has an acceptance",
                ));
            }
            state.accept_delegation(fence, binding, package.package_digest(), now)?;
            Ok(None)
        },
    )
}

fn scoped_mutation(
    store: &mut CandidateExecutionStore,
    request: SuccessorMutationRequest,
    command: &str,
    body: Value,
    clock: impl FnOnce() -> UnixMillis,
    apply: impl FnOnce(
        &WorkspaceId,
        &mut ExecutionSessionV3,
        &ExecutionCursorFenceV1,
        UnixMillis,
    ) -> Result<Option<bool>>,
) -> Result<DirectMutationResultV1> {
    let exact = outcome_request(&request, command, body)?;
    store.mutate(&request.workspace, &exact, |transaction| {
        let (mut state, sources) = transaction.load(admit_candidate_definition)?;
        verify_cursor(&state, &request.cursor)?;
        let now = clock();
        let changed = apply(
            transaction.workspace(),
            &mut state,
            &request.cursor.execution,
            now,
        )?;
        finish_current_mutation(
            transaction,
            &exact,
            &state,
            &sources,
            request.cursor.execution.revision,
            now,
            changed,
        )
    })
}

fn finish_current_mutation(
    transaction: &CandidateExecutionTransaction<'_, '_>,
    identity: &DirectMutationRequestV1,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    before: Revision,
    now: UnixMillis,
    changed: Option<bool>,
) -> Result<CandidateSuccess> {
    let mut view = current::observation(
        transaction.workspace(),
        state,
        sources,
        transaction.next_sequence()?,
    )?;
    if let Some(changed) = changed {
        view["changed"] = json!(changed);
    }
    persist_transition(transaction, identity, state, sources, before, view, now)
}

fn item_write_json(write: &GroupItemWriteV1) -> Result<Value> {
    let value = write
        .value
        .as_ref()
        .map(item_value_json_v2)
        .transpose()
        .map_err(|error| DomainError::InvalidState {
            reason: error.reason(),
        })?;
    Ok(json!({"item": write.item, "expected_revision": write.expected_revision, "value": value}))
}

impl SuccessorCursorMutation {
    fn request_parts(&self) -> (&'static str, Value) {
        match self {
            Self::Complete => ("complete", json!({})),
            Self::Decide {
                option,
                reason,
                goal_outcome,
            } => (
                "decide",
                json!({
                    "option": option, "reason": reason.as_str(), "goal_outcome": goal_outcome.map(GoalOutcome::as_str),
                }),
            ),
            Self::Rework { target, reason } => (
                "rework",
                json!({"target": target, "reason": reason.as_str()}),
            ),
            Self::Retry(reason) => ("retry", json!({"reason": reason.as_str()})),
            Self::Skip(reason) => (
                "skip",
                json!({"reason": reason.as_ref().map(ReasonV2::as_str)}),
            ),
            Self::DefineGoal(goal) => ("goal.define", goal_json(goal)),
            Self::ReviseGoal(change) => (
                "goal.revise",
                json!({
                    "goal": goal_json(&change.goal), "target": change.target, "reactivate": change.reactivate,
                    "reason": change.feedback.reason(), "working_copy_basis": change.feedback.working_copy_basis(),
                    "feedback": change.feedback.feedback(),
                }),
            ),
            Self::Cancel(reason) => ("cancel", json!({"reason": reason.as_str()})),
            Self::Block(reason) => ("block", json!({"reason": reason.as_str()})),
            Self::Unblock(blocker) => ("unblock", json!({"blocker": blocker})),
        }
    }

    fn apply(
        self,
        workspace: &WorkspaceId,
        state: &mut ExecutionSessionV3,
        fence: &ExecutionCursorFenceV1,
        now: UnixMillis,
        ids: &impl DirectIdSourceV1,
    ) -> Result<()> {
        match self {
            Self::Complete => {
                current::verify_delegated_completion(workspace, state)?;
                let base_terminal = fence.attempt.as_ref().is_some_and(|attempt| {
                    attempt.scope() == &ExecutionScopeV1::Base
                        && state.current_placement().is_some_and(|placement| {
                            matches!(
                                placement.kind(),
                                PlacementKindV3::Action {
                                    outcome: ActionOutcomeV2::Terminal,
                                    ..
                                }
                            )
                        })
                });
                state.complete(fence, (!base_terminal).then(|| ids.next_attempt_id()), now)?;
            }
            Self::Decide {
                option,
                reason,
                goal_outcome,
            } => state.decide(
                fence,
                option,
                reason,
                goal_outcome,
                ids.next_attempt_id(),
                now,
            )?,
            Self::Rework { target, reason } => {
                state.rework(fence, &target, reason, ids.next_attempt_id(), now)?
            }
            Self::Retry(reason) => state.retry(fence, reason, ids.next_attempt_id(), now)?,
            Self::Skip(reason) => state.skip(fence, reason, ids.next_attempt_id(), now)?,
            Self::DefineGoal(goal) => state.define_goal(fence, goal, now)?,
            Self::ReviseGoal(change) => {
                state.revise_goal(fence, change, ids.next_attempt_id(), now)?
            }
            Self::Cancel(reason) => state.cancel(fence, reason, now)?,
            Self::Block(reason) => update_blockers(state, fence, now, |attempt, blockers| {
                blockers.push(
                    BlockerStateV2::new(
                        ids.next_blocker_id(),
                        attempt.target().attempt().clone(),
                        reason.into_inner(),
                        BlockerState::Open,
                        now,
                        None,
                    )
                    .map_err(|error| DomainError::InvalidState {
                        reason: error.reason(),
                    })?,
                );
                Ok(())
            })?,
            Self::Unblock(id) => update_blockers(state, fence, now, |_, blockers| {
                let blocker = blockers
                    .iter_mut()
                    .find(|blocker| {
                        blocker.blocker_id() == &id && blocker.state() == BlockerState::Open
                    })
                    .ok_or(DomainError::BlockerNotCurrent)?;
                *blocker = BlockerStateV2::new(
                    blocker.blocker_id().clone(),
                    blocker.attempt_id().clone(),
                    blocker.reason(),
                    BlockerState::Resolved,
                    blocker.created_at(),
                    Some(now),
                )
                .map_err(|error| DomainError::InvalidState {
                    reason: error.reason(),
                })?;
                Ok(())
            })?,
        }
        Ok(())
    }
}

fn update_blockers(
    state: &mut ExecutionSessionV3,
    fence: &ExecutionCursorFenceV1,
    now: UnixMillis,
    change: impl FnOnce(&GroupAttemptStateV1, &mut Vec<BlockerStateV2>) -> Result<()>,
) -> Result<()> {
    let attempt = state
        .active_attempt()
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate blocker requires a current attempt",
        ))?;
    let mut next = attempt.blockers().to_vec();
    change(attempt, &mut next)?;
    Ok(state.replace_blockers(fence, next, now)?)
}

fn goal_json(goal: &ExecutionGoalV3) -> Value {
    json!({
        "statement": goal.statement.as_str(),
        "criteria": goal.criteria.criteria().iter().map(|criterion| json!({
            "id": criterion.id(), "statement": criterion.statement(),
        })).collect::<Vec<_>>(),
    })
}
