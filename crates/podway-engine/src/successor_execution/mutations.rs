//! Typed current-cursor and item-local fixture mutations.

use super::*;
use podway_core::{
    ActorAttributionV2, BlockerId, BlockerState, BlockerStateV2, OptionId, ReasonV2,
};
#[cfg(feature = "test-support")]
use podway_core::{GoalOutcome, workflow_v2::item_value_json_v2};
#[cfg(feature = "test-support")]
use {podway_core::MAX_ACTIVE_ITEM_MUTATIONS_V2, podway_protocol::serialized_json_len_v1};

#[derive(Clone, Debug)]
pub enum SuccessorCursorMutation {
    Begin(Option<ExecutionGoalV3>),
    Complete,
    #[cfg(feature = "test-support")]
    Decide {
        option: OptionId,
        reason: ReasonV2,
        goal_outcome: Option<GoalOutcome>,
    },
    DecideAssessed {
        option: OptionId,
        reason: ReasonV2,
    },
    AssessCriterion {
        entry: SuccessorCriterionInputV1,
        actor: Option<ActorAttributionV2>,
    },
    AssessCriteria {
        entries: Vec<SuccessorCriterionInputV1>,
        actor: Option<ActorAttributionV2>,
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
    UnblockAll,
}

/// The immutable attempt identity and selected activation are mandatory. A write to one slot does
/// not rely on the session revision or on an unrelated slot's revision.
#[cfg(feature = "test-support")]
pub struct SuccessorItemRequest {
    pub workspace: WorkspaceId,
    pub key: IdempotencyKeyV1,
    pub request_id: RequestIdV1,
    pub target: ScopedAttemptV1,
    pub selected_group: Option<GroupExecutionIdV1>,
    pub response: SuccessorMutationResponseV1,
}

pub fn issue_cursor_mutation(
    store: &mut impl SuccessorMutationStore,
    request: SuccessorMutationRequest,
    operation: SuccessorCursorMutation,
    clock: impl FnOnce() -> UnixMillis,
    ids: &impl DirectIdSourceV1,
) -> Result<DirectMutationResultV1> {
    issue_cursor_mutation_attributed(store, request, operation, None, clock, ids)
}

pub(super) fn issue_cursor_mutation_attributed(
    store: &mut impl SuccessorMutationStore,
    request: SuccessorMutationRequest,
    operation: SuccessorCursorMutation,
    actor: Option<ActorAttributionV2>,
    clock: impl FnOnce() -> UnixMillis,
    ids: &impl DirectIdSourceV1,
) -> Result<DirectMutationResultV1> {
    let command = operation.command_name();
    let mut body = operation.request_body();
    let attribution = actor
        .map(|actor| {
            use SuccessorAttributionTargetV1 as Target;
            let target = match &operation {
                SuccessorCursorMutation::Begin(Some(_))
                | SuccessorCursorMutation::DefineGoal(_) => Target::InitialGoal,
                SuccessorCursorMutation::ReviseGoal(_) => Target::RevisedGoal,
                SuccessorCursorMutation::DecideAssessed { .. }
                | SuccessorCursorMutation::Rework { .. } => Target::ClosedAttempt,
                _ => {
                    return Err(SuccessorExecutionError::Invalid(
                        "this transition cannot retain actor attribution",
                    ));
                }
            };
            body["actor"] = json!(actor.as_str());
            Ok((target, actor))
        })
        .transpose()?;
    let verify_progress = operation.needs_progress_verification();
    scoped_mutation(
        store,
        request,
        command,
        body,
        verify_progress,
        clock,
        |_, state, fence, clock| {
            let now = operation.apply(state, fence, clock, ids)?;
            if let Some((target, actor)) = attribution {
                state.attribute_transition(fence, target, actor)?;
            }
            Ok((None, now))
        },
    )
}

#[cfg(feature = "test-support")]
pub fn record_successor_item(
    store: &mut impl SuccessorMutationStore,
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
        false,
    )?;
    store.scoped_mutation(&request.workspace, &exact, clock, |transaction, now| {
        let (mut state, sources) = transaction.load(admit_candidate_definition)?;
        verify_selection(&state, request.selected_group.as_ref())?;
        state.verify_attempt(&request.target)?;
        let before = state.fence().revision;
        let now = now();
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
            request.response,
        )
    })
}

#[cfg(feature = "test-support")]
pub fn record_successor_many(
    store: &mut impl SuccessorMutationStore,
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
        false,
        clock,
        |_, state, fence, clock| {
            let now = clock();
            // scoped_mutation has already compared the live session revision.
            let changed = state
                .record_many(fence, writes, now)
                .map_err(|error| match error {
                    GraphMutationErrorV2::Domain(error) => item_write_error(error),
                    error => error,
                })?;
            Ok((Some(changed), now))
        },
    )
}

#[cfg(feature = "test-support")]
fn item_write_error(error: DomainError) -> GraphMutationErrorV2 {
    match error {
        DomainError::PreconditionFailed { expected, actual } => {
            GraphMutationErrorV2::ItemRevisionConflict { expected, actual }
        }
        error => GraphMutationErrorV2::Domain(error),
    }
}

#[cfg(feature = "test-support")]
pub fn accept_successor_delegation(
    store: &mut impl SuccessorMutationStore,
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
        false,
        clock,
        |workspace, state, fence, clock| {
            let now = clock();
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
            Ok((None, now))
        },
    )
}

fn scoped_mutation(
    store: &mut impl SuccessorMutationStore,
    request: SuccessorMutationRequest,
    command: &str,
    body: Value,
    verify_progress: bool,
    clock: impl FnOnce() -> UnixMillis,
    apply: impl FnOnce(
        &WorkspaceId,
        &mut ExecutionSessionV3,
        &ExecutionCursorFenceV1,
        &mut dyn FnMut() -> UnixMillis,
    ) -> Result<(Option<bool>, UnixMillis)>,
) -> Result<DirectMutationResultV1> {
    let exact = outcome_request(&request, command, body)?;
    store.scoped_mutation(&request.workspace, &exact, clock, |transaction, now| {
        let (mut state, sources) = transaction.load(admit_candidate_definition)?;
        verify_cursor(&state, &request.cursor)?;
        use podway_core::SessionLifecycle;
        let lifecycle = state.lifecycle();
        if command != "session.begin"
            && lifecycle != SessionLifecycle::Running
            && !(command == "goal.revise" && lifecycle != SessionLifecycle::Prepared)
        {
            return Err(GraphMutationErrorV2::SessionNotRunning.into());
        }
        if command == "session.complete" {
            current::verify_delegated_completion(transaction, &state)?;
        }
        if verify_progress {
            transaction.verify_progress(&state)?;
        }
        let (changed, now) = apply(
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
            request.response,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn finish_current_mutation(
    transaction: &CandidateExecutionTransaction<'_, '_>,
    identity: &DirectMutationRequestV1,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    before: Revision,
    now: UnixMillis,
    changed: Option<bool>,
    response: SuccessorMutationResponseV1,
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
    persist_transition(
        transaction,
        identity,
        state,
        sources,
        before,
        view,
        now,
        response,
    )
}

#[cfg(feature = "test-support")]
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
    pub(crate) const fn command_name(&self) -> &'static str {
        match self {
            Self::Begin(_) => "session.begin",
            Self::Complete => "session.complete",
            #[cfg(feature = "test-support")]
            Self::Decide { .. } => "session.decide",
            Self::DecideAssessed { .. } => "session.decide",
            Self::AssessCriterion { .. } => "goal.assess_criterion",
            Self::AssessCriteria { .. } => "goal.assess_criteria",
            Self::Rework { .. } => "session.rework",
            Self::Retry(_) => "session.retry",
            Self::Skip(_) => "session.skip",
            Self::DefineGoal(_) => "goal.define",
            Self::ReviseGoal(_) => "goal.revise",
            Self::Cancel(_) => "session.cancel",
            Self::Block(_) => "session.block",
            Self::Unblock(_) | Self::UnblockAll => "session.unblock",
        }
    }

    pub(super) fn needs_progress_verification(&self) -> bool {
        match self {
            Self::Complete | Self::DecideAssessed { .. } => true,
            #[cfg(feature = "test-support")]
            Self::Decide { .. } => true,
            _ => false,
        }
    }

    fn request_body(&self) -> Value {
        match self {
            Self::Begin(goal) => json!({"goal": goal.as_ref().map(goal_json)}),
            Self::Complete => json!({}),
            #[cfg(feature = "test-support")]
            Self::Decide {
                option,
                reason,
                goal_outcome,
            } => json!({
                "option": option, "reason": reason.as_str(), "goal_outcome": goal_outcome.map(GoalOutcome::as_str),
            }),
            Self::DecideAssessed { option, reason } => {
                json!({"option": option, "reason": reason.as_str()})
            }
            Self::AssessCriterion { entry, actor } => Self::AssessCriteria {
                entries: vec![entry.clone()],
                actor: actor.clone(),
            }
            .request_body(),
            Self::AssessCriteria { entries, actor } => json!({
                "actor": actor.as_ref().map(ActorAttributionV2::as_str),
                "entries": entries.iter().map(|entry| json!({
                    "criterion": entry.criterion,
                    "status": entry.status.as_str(),
                    "reason": entry.reason.as_str(),
                    "citations": entry.citations.iter().map(criterion_citation_json).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }),
            Self::Rework { target, reason } => json!({"target": target, "reason": reason.as_str()}),
            Self::Retry(reason) => json!({"reason": reason.as_str()}),
            Self::Skip(reason) => json!({"reason": reason.as_ref().map(ReasonV2::as_str)}),
            Self::DefineGoal(goal) => goal_json(goal),
            Self::ReviseGoal(change) => json!({
                "goal": goal_json(&change.goal), "target": change.target, "reactivate": change.reactivate,
                "reason": change.feedback.reason(), "working_copy_basis": change.feedback.working_copy_basis(),
                "feedback": change.feedback.feedback(),
            }),
            Self::Cancel(reason) | Self::Block(reason) => json!({"reason": reason.as_str()}),
            Self::Unblock(blocker) => json!({"blocker": blocker}),
            Self::UnblockAll => json!({"all": true}),
        }
    }

    fn apply(
        self,
        state: &mut ExecutionSessionV3,
        fence: &ExecutionCursorFenceV1,
        clock: &mut dyn FnMut() -> UnixMillis,
        ids: &impl DirectIdSourceV1,
    ) -> Result<UnixMillis> {
        let operation = match self {
            Self::DecideAssessed { option, reason } => {
                return Ok(state.decide_assessed_with_effects(
                    fence,
                    option,
                    reason,
                    clock,
                    || ids.next_attempt_id(),
                )?);
            }
            Self::Rework { target, reason } => {
                return Ok(
                    state.rework_with_effects(fence, &target, reason, clock, || {
                        ids.next_attempt_id()
                    })?,
                );
            }
            operation => operation,
        };
        let now = clock();
        match operation {
            Self::Begin(goal) => state.begin_prepared(fence, goal, ids.next_attempt_id(), now)?,
            Self::Complete => {
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
            #[cfg(feature = "test-support")]
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
            Self::DecideAssessed { .. } => unreachable!("ordinary decision has its own admission"),
            Self::AssessCriterion { entry, actor } => {
                state.assess_criteria(fence, vec![entry], actor, now)?;
            }
            Self::AssessCriteria { entries, actor } => {
                state.assess_criteria(fence, entries, actor, now)?;
            }
            Self::Rework { .. } => unreachable!("manual rework has its own admission"),
            Self::Retry(reason) => state.retry(fence, reason, ids.next_attempt_id(), now)?,
            Self::Skip(reason) => {
                let node = state
                    .current_placement()
                    .ok_or(GraphMutationErrorV2::SessionNotRunning)?
                    .id();
                let policy = state.current_skip_policy().ok_or_else(|| {
                    GraphMutationErrorV2::SkipNotAllowed {
                        graph_node_id: node.clone(),
                    }
                })?;
                if policy.reason_required() && reason.is_none() {
                    return Err(GraphMutationErrorV2::SkipReasonRequired {
                        graph_node_id: node.clone(),
                    }
                    .into());
                }
                state.skip(fence, reason, ids.next_attempt_id(), now)?;
            }
            Self::DefineGoal(goal) => state.define_goal(fence, goal, now)?,
            Self::ReviseGoal(change) => {
                let target = change.target.clone();
                state
                    .revise_goal_with_admission(fence, change, ids.next_attempt_id(), now)
                    .map_err(|error| match error {
                        GroupGoalRevisionErrorV1::TargetNotAllowed(_) => {
                            GraphMutationErrorV2::GoalRevisionTargetNotAllowed {
                                target_graph_node_id: target,
                            }
                        }
                        GroupGoalRevisionErrorV1::TargetNotOnTrace(_) => {
                            GraphMutationErrorV2::ManualReworkTargetNotOnTrace {
                                target_graph_node_id: target,
                            }
                        }
                        GroupGoalRevisionErrorV1::ReactivationFlagRequired(_) => {
                            GraphMutationErrorV2::ReactivationFlagRequired
                        }
                        GroupGoalRevisionErrorV1::Domain(error) => error.into(),
                    })?;
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
                resolve_blocker(blocker, now)
            })?,
            Self::UnblockAll => update_blockers(state, fence, now, |_, blockers| {
                let mut open = blockers
                    .iter_mut()
                    .filter(|blocker| blocker.state() == BlockerState::Open)
                    .peekable();
                if open.peek().is_none() {
                    return Err(GraphMutationErrorV2::NoOpenBlockers.into());
                }
                for blocker in open {
                    resolve_blocker(blocker, now)?;
                }
                Ok(())
            })?,
        }
        Ok(now)
    }
}

pub(crate) fn criterion_citation_json(citation: &SuccessorCriterionCitationV1) -> Value {
    match citation {
        SuccessorCriterionCitationV1::Item(item) => json!({"kind": "item", "item": item}),
        SuccessorCriterionCitationV1::Evidence(source) => evidence_citation_json(source),
    }
}

pub(crate) fn evidence_citation_json(source: &EvidenceSourceV3) -> Value {
    match source {
        EvidenceSourceV3::Local(node) => json!({"kind": "local", "node": node}),
        EvidenceSourceV3::Plan => json!({"kind": "plan"}),
        EvidenceSourceV3::ExecutionResult => json!({"kind": "execution_result"}),
    }
}

fn resolve_blocker(blocker: &mut BlockerStateV2, now: UnixMillis) -> Result<()> {
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
