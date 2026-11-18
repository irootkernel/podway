//! Admission of retained execution state against the original definitions.

use super::*;

/// Retained fields of one admitted attempt. Live indexes are recovered from the ordered trace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedAttemptV3 {
    pub target: ScopedAttemptV1,
    pub lifecycle: AttemptLifecycle,
    pub validity: AttemptValidityV2,
    pub opened_at: UnixMillis,
    pub closed_at: Option<UnixMillis>,
    pub slots: Vec<ItemSlotStateV2>,
    pub blockers: Vec<BlockerStateV2>,
    pub delegation: Option<ScopedDelegationBindingV1>,
    pub decision_entry: Option<u64>,
    pub decision_group: Option<GroupExecutionIdV1>,
    pub decision: Option<GroupDecisionRecordV1>,
    pub reason: Option<ReasonV2>,
}

impl From<&GroupAttemptStateV1> for RetainedAttemptV3 {
    fn from(value: &GroupAttemptStateV1) -> Self {
        Self {
            target: value.target.clone(),
            lifecycle: value.lifecycle,
            validity: value.validity,
            opened_at: value.opened_at,
            closed_at: value.closed_at,
            slots: value.slots.clone(),
            blockers: value.blockers.clone(),
            delegation: value.delegation.clone(),
            decision_entry: value.decision_entry(),
            decision_group: value
                .selected_decision_entry()
                .map(|entry| entry.selected_group().clone()),
            decision: value.decision.clone(),
            reason: value.reason.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedActivationV3 {
    pub identity: ActivationIdentityV1,
    pub outcome: GroupExecutionOutcomeV1,
    pub terminal_attempt: Option<AttemptId>,
    pub closure: Option<GroupSelectionClosureV1>,
}

impl From<&GroupExecutionStateV1> for RetainedActivationV3 {
    fn from(value: &GroupExecutionStateV1) -> Self {
        Self {
            identity: value.identity.clone(),
            outcome: value.outcome,
            terminal_attempt: value.terminal_attempt.clone(),
            closure: value.closure.clone(),
        }
    }
}

/// Deserialized state plus separately admitted immutable procedure and group source documents.
pub struct RetainedExecutionV3 {
    pub session: SessionId,
    pub base: ProcedureDefinitionV3,
    pub base_digest: Sha256Digest,
    pub definitions: BTreeMap<Sha256Digest, ExecutionGroupDefinitionV1>,
    pub executions: Vec<RetainedActivationV3>,
    pub selected: Option<GroupExecutionIdV1>,
    pub attempts: Vec<RetainedAttemptV3>,
    pub initial_goal: Option<ExecutionGoalV3>,
    pub goal_revisions: Vec<GroupGoalRevisionV1>,
    pub lifecycle: SessionLifecycle,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}

impl ExecutionSessionV3 {
    pub fn base_definition(&self) -> &ProcedureDefinitionV3 {
        &self.base
    }

    pub fn base_digest(&self) -> &Sha256Digest {
        &self.base_digest
    }

    pub const fn updated_at(&self) -> UnixMillis {
        self.updated_at
    }

    /// Re-admit the trace using the same transition rules that created it. Install only final
    /// item and blocker state for each attempt. Intermediate edits are intentionally absent.
    pub fn from_retained(retained: RetainedExecutionV3) -> Result<Self, DomainError> {
        let RetainedExecutionV3 {
            session,
            base,
            base_digest,
            definitions,
            executions,
            selected,
            attempts,
            initial_goal,
            goal_revisions,
            lifecycle,
            revision,
            updated_at,
        } = retained;
        check_bound("session attempts", attempts.len(), MAX_SESSION_ATTEMPTS_V1)?;
        check_bound(
            "group executions",
            executions.len(),
            MAX_GROUP_EXECUTIONS_V1,
        )?;
        let first = attempts
            .first()
            .ok_or_else(|| invalid("retained session has no entry"))?;
        let mut state = Self::begin(
            session,
            base,
            base_digest,
            initial_goal,
            first.target.attempt().clone(),
            first.opened_at,
        )?;
        let activations = indexed_unique(
            executions
                .iter()
                .map(|entry| (entry.identity.plan().design_attempt(), entry)),
        )?;
        let revisions = indexed_unique(
            goal_revisions
                .iter()
                .map(|entry| (entry.source.attempt(), entry)),
        )?;
        let closures = indexed_unique(executions.iter().filter_map(|entry| {
            entry
                .closure
                .as_ref()
                .map(|closure| (closure.source.attempt(), closure))
        }))?;
        let mut minimum_item_writes = 0_u64;
        let mut blocker_ids = BTreeSet::new();
        for (index, attempt) in attempts.iter().enumerate() {
            if attempt
                .blockers
                .iter()
                .any(|blocker| !blocker_ids.insert(blocker.blocker_id()))
            {
                return Err(invalid(
                    "blocker identity is reused across retained attempts",
                ));
            }
            let next = attempts
                .get(index + 1)
                .map(|value| value.target.attempt().clone());
            let id = attempt.target.attempt();
            let goal_change = revisions.get(id).copied();
            let closure = closures.get(id).copied();
            // A single batch can advance several distinct slots in this attempt, but it can
            // advance each one only once and cannot touch another attempt.
            let attempt_writes = attempt
                .slots
                .iter()
                .map(|slot| slot.revision().get())
                .max()
                .unwrap_or(0);
            minimum_item_writes = minimum_item_writes
                .checked_add(attempt_writes)
                .ok_or_else(|| invalid("retained item revisions exceed session revision range"))?;
            state.admit_retained_values(attempt, updated_at)?;
            if let Some(change) = goal_change {
                // Reactivation follows successful termination, even though the new anchor is
                // adjacent in the retained trace.
                if change.closure.cause == SelectionClosureCauseV1::Reactivation {
                    state.complete(
                        &state.fence(),
                        None,
                        attempt
                            .closed_at
                            .ok_or_else(|| invalid("reactivation source is open"))?,
                    )?;
                }
                let next = next.ok_or_else(|| invalid("retained goal change has no successor"))?;
                let feedback = change
                    .closure
                    .feedback
                    .clone()
                    .ok_or_else(|| invalid("retained goal change lacks feedback"))?;
                state.revise_goal(
                    &state.fence(),
                    ExecutionGoalChangeV3 {
                        goal: change.goal.clone(),
                        target: change.successor.node().clone(),
                        feedback,
                        reactivate: change.closure.cause == SelectionClosureCauseV1::Reactivation,
                    },
                    next,
                    change.closure.at,
                )?;
                continue;
            }
            let Some(at) = attempt.closed_at else {
                if next.is_some() || attempt.lifecycle != AttemptLifecycle::Active {
                    return Err(invalid(
                        "retained open attempt is not the final active attempt",
                    ));
                }
                continue;
            };
            let fence = state.fence();
            match attempt.lifecycle {
                AttemptLifecycle::Active => {
                    return Err(invalid("retained active attempt is closed"));
                }
                AttemptLifecycle::Completed => {
                    if let Some(activation) = activations.get(id) {
                        let digest = activation.identity.definition_digest();
                        let definition = definitions
                            .get(digest)
                            .ok_or_else(|| invalid("retained group definition is absent"))?;
                        state.activate(
                            &fence,
                            activation.identity.clone(),
                            definition.clone(),
                            digest,
                            required_successor(next)?,
                            at,
                        )?;
                    } else if let Some(decision) = &attempt.decision {
                        if let Some(closure) =
                            closure.filter(|value| value.cause == SelectionClosureCauseV1::Replan)
                        {
                            let feedback = closure
                                .feedback
                                .as_ref()
                                .ok_or_else(|| invalid("retained replan lacks feedback"))?;
                            state.replan(
                                &fence,
                                ReplanFeedbackV1::new(
                                    attempt.target.clone(),
                                    decision.option.clone(),
                                    feedback.reason().to_owned(),
                                    feedback.working_copy_basis().clone(),
                                    feedback.feedback().to_owned(),
                                )?,
                                required_successor(next)?,
                                at,
                            )?;
                        } else {
                            state.decide(
                                &fence,
                                decision.option.clone(),
                                ReasonV2::new(&decision.reason)?,
                                decision.goal_assessment.map(|(_, outcome)| outcome),
                                required_successor(next)?,
                                at,
                            )?;
                        }
                    } else {
                        state.complete(&fence, next, at)?;
                    }
                }
                AttemptLifecycle::Skipped => {
                    state.skip(
                        &fence,
                        attempt.reason.clone(),
                        required_successor(next)?,
                        at,
                    )?;
                }
                AttemptLifecycle::Abandoned => {
                    let reason = attempt
                        .reason
                        .clone()
                        .ok_or_else(|| invalid("retained abandoned attempt lacks a reason"))?;
                    if let Some(next) = next {
                        let following = &attempts[index + 1].target;
                        if following.node() == attempt.target.node() {
                            state.retry(&fence, reason, next, at)?;
                        } else {
                            state.rework(&fence, following.node(), reason, next, at)?;
                        }
                    } else {
                        state.cancel(&fence, reason, at)?;
                    }
                }
            }
        }
        if state.lifecycle != lifecycle
            || state.updated_at > updated_at
            || state
                .revision
                .get()
                .checked_add(minimum_item_writes)
                .is_none_or(|minimum| minimum > revision.get())
            || state.goal_revisions != goal_revisions
            || state.definitions != definitions
            || state.selected().map(|entry| entry.identity().execution()) != selected.as_ref()
            || !state
                .attempts
                .iter()
                .map(RetainedAttemptV3::from)
                .eq(attempts)
            || !state
                .executions
                .iter()
                .map(RetainedActivationV3::from)
                .eq(executions)
        {
            return Err(invalid("retained trace differs from replayed execution"));
        }
        state.revision = revision;
        state.updated_at = updated_at;
        Ok(state)
    }

    fn admit_retained_values(
        &mut self,
        attempt: &RetainedAttemptV3,
        updated_at: UnixMillis,
    ) -> Result<(), DomainError> {
        let (current, placement, items) = self.current_parts()?;
        let limit = attempt.closed_at.unwrap_or(updated_at);
        if current.target != attempt.target
            || current.opened_at != attempt.opened_at
            || current.decision_entry() != attempt.decision_entry
            || current
                .selected_decision_entry()
                .map(DecisionEntryV1::selected_group)
                != attempt.decision_group.as_ref()
            || limit < attempt.opened_at
            || limit > updated_at
            || items.len() != attempt.slots.len()
            || items.iter().zip(&attempt.slots).any(|(item, slot)| {
                slot.attempt_id() != attempt.target.attempt()
                    || item.id() != slot.item_id()
                    || item.item_type() != slot.item_type()
                    || slot.created_at() != attempt.opened_at
                    || slot.updated_at() > limit
                    || slot
                        .value()
                        .is_some_and(|value| !item.admits_recorded_value(value))
            })
            || recorded_content_scalars_v2(attempt.slots.iter().filter_map(ItemSlotStateV2::value))
                > MAX_ATTEMPT_CONTENT_SCALARS_V2
            || attempt
                .blockers
                .iter()
                .filter(|blocker| blocker.state() == BlockerState::Open)
                .count()
                > MAX_OPEN_BLOCKERS_V2
            || attempt
                .blockers
                .windows(2)
                .any(|pair| blocker_order(&pair[0]) >= blocker_order(&pair[1]))
            || attempt.blockers.iter().any(|blocker| {
                blocker.attempt_id() != attempt.target.attempt()
                    || blocker.created_at() < attempt.opened_at
                    || blocker.created_at() > limit
                    || blocker.resolved_at().is_some_and(|at| at > limit)
            })
            || attempt.delegation.as_ref().is_some_and(|binding| {
                binding.target() != &attempt.target
                    || !matches!(
                        placement.kind(),
                        PlacementKindV3::Action {
                            delegation: Some(_),
                            ..
                        }
                    )
            })
        {
            return Err(invalid(
                "retained attempt fields disagree with the admitted placement",
            ));
        }
        let current = self
            .attempts
            .last_mut()
            .expect("current attempt was admitted");
        current.slots.clone_from(&attempt.slots);
        current.blockers.clone_from(&attempt.blockers);
        current.delegation.clone_from(&attempt.delegation);
        Ok(())
    }
}

fn indexed_unique<K: Ord, V>(
    values: impl Iterator<Item = (K, V)>,
) -> Result<BTreeMap<K, V>, DomainError> {
    let mut index = BTreeMap::new();
    for (key, value) in values {
        if index.insert(key, value).is_some() {
            return Err(invalid("retained state repeats a source attempt"));
        }
    }
    Ok(index)
}
