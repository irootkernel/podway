//! In-memory transitions over admitted successor definitions (ADR-0044).
//! The caller supplies admitted definition identities, fresh IDs, time, and external verification.

use super::*;
use crate::{
    ActionOutcomeV2, AttemptId, AttemptLifecycle, AttemptValidityV2, BlockerState, BlockerStateV2,
    BoundUnitV2, ConditionStateV2, GoalDefinitionV2, GoalOutcome, GoalRevisionNumberV2,
    GoalStatementV2, GraphNodeId, ItemId, ItemSlotStateV2, MAX_ATTEMPT_CONTENT_SCALARS_V2,
    MAX_OPEN_BLOCKERS_V2, OptionId, ReasonV2, RecordedItemSetV2, RecordedItemV2,
    RecordedItemValueV2, Revision, SessionId, SessionLifecycle, Sha256Digest, UnixMillis,
    recorded_content_scalars_v2, recorded_items_digest_v2, recorded_value_digest_v2,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "retained.rs"]
mod retained;
pub use retained::*;

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

/// The exact cursor on which a transition was prepared. Item edits use the narrower slot fence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionCursorFenceV1 {
    pub session: SessionId,
    pub revision: Revision,
    pub attempt: Option<ScopedAttemptV1>,
    pub goal_revision: Option<GoalRevisionNumberV2>,
}

/// One exact current value used to admit a Decision option.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupGuardValueV1 {
    pub source: ScopedAttemptV1,
    pub item: PlanItemBindingV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupDecisionRecordV1 {
    pub option: OptionId,
    pub reason: String,
    pub guard_values: Vec<GroupGuardValueV1>,
    pub goal_assessment: Option<(GoalRevisionNumberV2, GoalOutcome)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DecisionEntry {
    BasePrefix(u64),
    Selected(DecisionEntryV1),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupAttemptStateV1 {
    target: ScopedAttemptV1,
    lifecycle: AttemptLifecycle,
    validity: AttemptValidityV2,
    opened_at: UnixMillis,
    closed_at: Option<UnixMillis>,
    slots: Vec<ItemSlotStateV2>,
    blockers: Vec<BlockerStateV2>,
    delegation: Option<ScopedDelegationBindingV1>,
    decision_entry: Option<DecisionEntry>,
    decision: Option<GroupDecisionRecordV1>,
    reason: Option<ReasonV2>,
}

impl GroupAttemptStateV1 {
    pub fn target(&self) -> &ScopedAttemptV1 {
        &self.target
    }
    pub const fn lifecycle(&self) -> AttemptLifecycle {
        self.lifecycle
    }
    pub const fn validity(&self) -> AttemptValidityV2 {
        self.validity
    }
    pub const fn opened_at(&self) -> UnixMillis {
        self.opened_at
    }
    pub const fn closed_at(&self) -> Option<UnixMillis> {
        self.closed_at
    }
    pub fn slots(&self) -> &[ItemSlotStateV2] {
        &self.slots
    }
    pub fn blockers(&self) -> &[BlockerStateV2] {
        &self.blockers
    }
    pub fn delegation(&self) -> Option<&ScopedDelegationBindingV1> {
        self.delegation.as_ref()
    }
    pub const fn decision_entry(&self) -> Option<u64> {
        match &self.decision_entry {
            Some(DecisionEntry::BasePrefix(ordinal)) => Some(*ordinal),
            Some(DecisionEntry::Selected(entry)) => Some(entry.ordinal()),
            None => None,
        }
    }
    pub fn selected_decision_entry(&self) -> Option<&DecisionEntryV1> {
        match &self.decision_entry {
            Some(DecisionEntry::Selected(entry)) => Some(entry),
            _ => None,
        }
    }
    pub fn decision(&self) -> Option<&GroupDecisionRecordV1> {
        self.decision.as_ref()
    }
    pub fn reason(&self) -> Option<&ReasonV2> {
        self.reason.as_ref()
    }

    fn slot(&self, item: &ItemId) -> Option<&ItemSlotStateV2> {
        self.slots.iter().find(|slot| slot.item_id() == item)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupSelectionClosureV1 {
    pub cause: SelectionClosureCauseV1,
    pub source: ScopedAttemptV1,
    pub at: UnixMillis,
    pub feedback: Option<GroupClosureFeedbackV1>,
}

/// The execution outcome is retained separately from current selection and path validity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupExecutionStateV1 {
    identity: ActivationIdentityV1,
    outcome: GroupExecutionOutcomeV1,
    terminal_attempt: Option<AttemptId>,
    closure: Option<GroupSelectionClosureV1>,
}

impl GroupExecutionStateV1 {
    pub fn identity(&self) -> &ActivationIdentityV1 {
        &self.identity
    }
    pub const fn outcome(&self) -> GroupExecutionOutcomeV1 {
        self.outcome
    }
    pub fn terminal_attempt(&self) -> Option<&AttemptId> {
        self.terminal_attempt.as_ref()
    }
    pub fn closure(&self) -> Option<&GroupSelectionClosureV1> {
        self.closure.as_ref()
    }
}

/// The immutable statement and criteria of one session goal revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionGoalV3 {
    pub statement: GoalStatementV2,
    pub criteria: GoalDefinitionV2,
}

pub struct ExecutionGoalChangeV3 {
    pub goal: ExecutionGoalV3,
    pub target: GraphNodeId,
    pub feedback: GroupClosureFeedbackV1,
    pub reactivate: bool,
}

/// Records goal revision or reactivation even when completion already closed selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupGoalRevisionV1 {
    pub revision: GoalRevisionNumberV2,
    pub goal: ExecutionGoalV3,
    pub source: ScopedAttemptV1,
    pub successor: ScopedAttemptV1,
    pub predecessor: Option<GroupExecutionIdV1>,
    pub closure: GroupSelectionClosureV1,
}

/// A single-cursor Core model. Config admits topology and canonical digests before construction.
/// Workspace fencing, request identity, durable admission and public dispatch are caller concerns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionSessionV3 {
    session: SessionId,
    base: ProcedureDefinitionV3,
    base_digest: Sha256Digest,
    base_suffix: BTreeSet<GraphNodeId>,
    definitions: BTreeMap<Sha256Digest, ExecutionGroupDefinitionV1>,
    executions: Vec<GroupExecutionStateV1>,
    selected: Option<usize>,
    attempts: Vec<GroupAttemptStateV1>,
    attempt_ids: BTreeMap<AttemptId, usize>,
    current_base: BTreeMap<GraphNodeId, usize>,
    current_group: BTreeMap<GraphNodeId, usize>,
    decision_counts: BTreeMap<(Option<GroupExecutionIdV1>, bool, GraphNodeId), u64>,
    initial_goal: Option<ExecutionGoalV3>,
    goal_revisions: Vec<GroupGoalRevisionV1>,
    lifecycle: SessionLifecycle,
    revision: Revision,
    updated_at: UnixMillis,
}

impl ExecutionSessionV3 {
    pub fn begin(
        session: SessionId,
        base: ProcedureDefinitionV3,
        base_digest: Sha256Digest,
        initial_goal: Option<ExecutionGoalV3>,
        entry_attempt: AttemptId,
        now: UnixMillis,
    ) -> Result<Self, DomainError> {
        if initial_goal.is_some() && base.goal_tracking().is_none() {
            return Err(invalid("an untracked base cannot have a goal"));
        }
        let entry = base.graph().entry().clone();
        let mut state = Self {
            session,
            base_suffix: downstream(base.graph(), base.boundary().success_return()),
            base,
            base_digest,
            definitions: BTreeMap::new(),
            executions: Vec::new(),
            selected: None,
            attempts: Vec::new(),
            attempt_ids: BTreeMap::new(),
            current_base: BTreeMap::new(),
            current_group: BTreeMap::new(),
            decision_counts: BTreeMap::new(),
            initial_goal,
            goal_revisions: Vec::new(),
            lifecycle: SessionLifecycle::Running,
            revision: Revision::ZERO,
            updated_at: now,
        };
        let attempt =
            state.prepare_attempt(ExecutionScopeV1::Base, &entry, entry_attempt, now, None)?;
        state.push_attempt(attempt);
        Ok(state)
    }

    pub fn fence(&self) -> ExecutionCursorFenceV1 {
        ExecutionCursorFenceV1 {
            session: self.session.clone(),
            revision: self.revision,
            attempt: self.active_attempt().map(|attempt| attempt.target.clone()),
            goal_revision: self.goal_revision(),
        }
    }

    pub fn active_attempt(&self) -> Option<&GroupAttemptStateV1> {
        self.attempts
            .last()
            .filter(|attempt| attempt.lifecycle == AttemptLifecycle::Active)
    }

    pub fn attempt_history(&self) -> &[GroupAttemptStateV1] {
        &self.attempts
    }

    pub fn group_history(&self) -> &[GroupExecutionStateV1] {
        &self.executions
    }

    pub fn goal_revision_history(&self) -> &[GroupGoalRevisionV1] {
        &self.goal_revisions
    }

    pub fn group_definition(&self, digest: &Sha256Digest) -> Option<&ExecutionGroupDefinitionV1> {
        self.definitions.get(digest)
    }

    pub fn selected(&self) -> Option<&GroupExecutionStateV1> {
        self.selected.map(|index| &self.executions[index])
    }

    pub const fn lifecycle(&self) -> SessionLifecycle {
        self.lifecycle
    }

    pub fn initial_goal(&self) -> Option<&ExecutionGoalV3> {
        self.initial_goal.as_ref()
    }

    pub fn current_goal(&self) -> Option<&ExecutionGoalV3> {
        self.goal_revisions
            .last()
            .map(|revision| &revision.goal)
            .or(self.initial_goal.as_ref())
    }

    fn goal_revision(&self) -> Option<GoalRevisionNumberV2> {
        self.goal_revisions
            .last()
            .map(|revision| revision.revision)
            .or(self
                .initial_goal
                .as_ref()
                .map(|_| GoalRevisionNumberV2::FIRST))
    }

    pub fn current_goal_outcome(&self) -> Option<GoalOutcome> {
        let current = self.goal_revision()?;
        // Terminal readback follows the execution that was selected at session closure.
        let execution = self.selected().or_else(|| {
            self.executions.last().filter(|execution| {
                execution.closure().is_some_and(|closure| {
                    matches!(
                        (self.lifecycle, closure.cause),
                        (
                            SessionLifecycle::Completed,
                            SelectionClosureCauseV1::Completion
                        ) | (SessionLifecycle::Cancelled, SelectionClosureCauseV1::Cancel)
                    )
                })
            })
        })?;
        if execution.outcome() != GroupExecutionOutcomeV1::Completed {
            return None;
        }
        self.current_base
            .values()
            .filter_map(|&index| {
                let attempt = &self.attempts[index];
                let entry = attempt.selected_decision_entry()?;
                let (revision, outcome) = attempt.decision.as_ref()?.goal_assessment?;
                (revision == current && entry.selected_group() == execution.identity().execution())
                    .then_some((index, outcome))
            })
            .max_by_key(|(index, _)| *index)
            .map(|(_, outcome)| outcome)
    }

    pub fn define_goal(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        goal: ExecutionGoalV3,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        if self.active_attempt().is_none()
            || self.base.goal_tracking().is_none()
            || self.initial_goal.is_some()
        {
            return Err(invalid(
                "goal definition requires a running tracked session without a goal",
            ));
        }
        self.initial_goal = Some(goal);
        self.update(revision, now);
        Ok(())
    }

    /// Return the current anchor binding using the same ordered digest and slot values as activation.
    pub fn plan_binding(&self) -> Result<PlanBindingV1, DomainError> {
        let attempt = self
            .active_attempt()
            .ok_or_else(|| invalid("no active design attempt"))?;
        if attempt.target.scope() != &ExecutionScopeV1::Base
            || attempt.target.node() != self.base.boundary().anchor()
        {
            return Err(invalid("the current attempt is not the design anchor"));
        }
        let selected = self.base.boundary().plan_inputs();
        let mut bindings = Vec::with_capacity(selected.len());
        for item in selected {
            let slot = attempt
                .slot(item)
                .ok_or_else(|| invalid("anchor plan item is undeclared"))?;
            if let Some(value) = slot.value() {
                bindings.push(item_binding(slot, value)?);
            }
        }
        let values = attempt
            .slots
            .iter()
            .filter_map(|slot| {
                slot.value()
                    .map(|value| RecordedItemV2::new(slot.item_id().clone(), value.clone()))
            })
            .collect();
        let digest = recorded_items_digest_v2(&RecordedItemSetV2::new(values)?)
            .map_err(|e| invalid(e.reason()))?;
        PlanBindingV1::new(attempt.target.attempt().clone(), digest, bindings)
    }

    pub fn activate(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        identity: ActivationIdentityV1,
        definition: ExecutionGroupDefinitionV1,
        admitted_digest: &Sha256Digest,
        entry_attempt: AttemptId,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let (anchor, placement, items) = self.current_parts()?;
        if anchor.target.scope() != &ExecutionScopeV1::Base
            || placement.id() != self.base.boundary().anchor()
            || self.selected.is_some()
            || (self.base.goal_tracking().is_some() && self.goal_revision().is_none())
        {
            return Err(invalid(
                "activation requires an unselected ready anchor with its current goal",
            ));
        }
        self.require_ready(anchor, placement, items)?;
        if matches!(
            placement.kind(),
            PlacementKindV3::Action {
                delegation: Some(_),
                ..
            }
        ) && anchor.delegation.is_none()
        {
            return Err(invalid("current delegated anchor acceptance is required"));
        }
        if identity.session() != &self.session
            || identity.base_digest() != &self.base_digest
            || identity.anchor() != placement.id()
            || identity.definition_digest() != admitted_digest
            || identity.goal_revision() != self.goal_revision()
            || identity.plan() != &self.plan_binding()?
            || identity.predecessor() != self.latest_group_id()
            || self
                .executions
                .iter()
                .any(|group| group.identity.execution() == identity.execution())
        {
            return Err(invalid(
                "activation identity does not bind the exact current design",
            ));
        }
        let plan = selected_items(items, self.base.boundary().plan_inputs())?;
        let terminal = terminal_items(&definition)?;
        if !same_boundary_contract_v1(&plan, definition.plan_inputs())
            || !same_result_contract(self.base.boundary().result_items(), terminal)
        {
            return Err(invalid("group boundary declarations differ from the base"));
        }
        self.check_prior_work(&identity)?;
        check_bound(
            "group executions",
            self.executions.len() + 1,
            MAX_GROUP_EXECUTIONS_V1,
        )?;
        match self.definitions.get(admitted_digest) {
            Some(retained) if retained != &definition => {
                return Err(invalid(
                    "a retained group digest names a different definition",
                ));
            }
            Some(_) => {}
            None => {
                let placements = self.base.graph().nodes().len()
                    + definition.graph().nodes().len()
                    + self
                        .definitions
                        .values()
                        .map(|definition| definition.graph().nodes().len())
                        .sum::<usize>();
                check_bound("session placements", placements, MAX_SESSION_PLACEMENTS_V1)?;
            }
        }
        let group_scope = ExecutionScopeV1::Group {
            group_execution_id: identity.execution().clone(),
            definition_digest: admitted_digest.clone(),
        };
        let entry = self.prepare_attempt(
            group_scope,
            definition.graph().entry(),
            entry_attempt,
            now,
            Some(&definition),
        )?;
        self.definitions
            .entry(admitted_digest.clone())
            .or_insert(definition);
        self.selected = Some(self.executions.len());
        self.executions.push(GroupExecutionStateV1 {
            identity,
            outcome: GroupExecutionOutcomeV1::Running,
            terminal_attempt: None,
            closure: None,
        });
        self.close_attempt(AttemptLifecycle::Completed, now);
        self.push_attempt(entry);
        self.update(revision, now);
        Ok(())
    }

    /// A current item edit fences the exact scoped attempt and item revision independently.
    pub fn record_item(
        &mut self,
        target: &ScopedAttemptV1,
        item: &ItemId,
        expected_revision: Revision,
        value: Option<RecordedItemValueV2>,
        now: UnixMillis,
    ) -> Result<Revision, DomainError> {
        self.check_time(now)?;
        let (attempt, _, items) = self.current_parts()?;
        if &attempt.target != target {
            return Err(stale_attempt(target, Some(attempt)));
        }
        let index = items
            .iter()
            .position(|declaration| declaration.id() == item)
            .ok_or_else(|| DomainError::ItemNotFound {
                item_id: item.clone(),
            })?;
        let current = &attempt.slots[index];
        if current.revision() != expected_revision {
            return Err(DomainError::PreconditionFailed {
                expected: expected_revision,
                actual: current.revision(),
            });
        }
        if value
            .as_ref()
            .is_some_and(|value| !items[index].admits_recorded_value(value))
        {
            return Err(invalid(
                "recorded value does not match its item declaration",
            ));
        }
        let scalars = recorded_content_scalars_v2(
            attempt
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, slot)| (i != index).then(|| slot.value()).flatten())
                .chain(value.as_ref()),
        );
        if scalars > MAX_ATTEMPT_CONTENT_SCALARS_V2 {
            return Err(crate::attempt_content_bound_error_v2(scalars));
        }
        let next_item_revision = expected_revision.checked_next()?;
        let revision = self.revision.checked_next()?;
        let slot = ItemSlotStateV2::new(
            target.attempt().clone(),
            item.clone(),
            items[index].item_type(),
            next_item_revision,
            value,
            current.created_at(),
            now,
        )
        .map_err(|e| invalid(e.reason()))?;
        let attempt = self
            .attempts
            .last_mut()
            .expect("an admitted current attempt exists");
        attempt.slots[index] = slot;
        // Any change to recorded outputs invalidates the acceptance bound to the prior package.
        attempt.delegation = None;
        self.update(revision, now);
        Ok(next_item_revision)
    }

    /// Admit a monotone blocker successor for the exact cursor.
    pub fn replace_blockers(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        mut blockers: Vec<BlockerStateV2>,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let (attempt, _, _) = self.current_parts()?;
        let mut additions = blockers
            .iter()
            .map(|blocker| (blocker.blocker_id(), blocker))
            .collect::<BTreeMap<_, _>>();
        if additions.len() != blockers.len()
            || blockers
                .iter()
                .any(|blocker| blocker.attempt_id() != attempt.target.attempt())
        {
            return Err(invalid(
                "blocker facts do not identify unique blockers of the current attempt",
            ));
        }
        for old in &attempt.blockers {
            let next = additions
                .remove(old.blocker_id())
                .ok_or_else(|| invalid("retained blocker history cannot be removed"))?;
            if old == next {
                continue;
            }
            if old.state() != BlockerState::Open
                || next
                    != &BlockerStateV2::new(
                        old.blocker_id().clone(),
                        old.attempt_id().clone(),
                        old.reason(),
                        BlockerState::Resolved,
                        old.created_at(),
                        Some(now),
                    )
                    .map_err(|e| invalid(e.reason()))?
            {
                return Err(invalid(
                    "blocker history may change only by explicitly resolving a current open blocker",
                ));
            }
        }
        if additions
            .values()
            .any(|new| new.state() != BlockerState::Open || new.created_at() != now)
            || (!additions.is_empty()
                && self
                    .attempts
                    .iter()
                    .flat_map(|attempt| &attempt.blockers)
                    .any(|old| additions.contains_key(old.blocker_id())))
        {
            return Err(invalid(
                "a new blocker must be a fresh open record at the current time",
            ));
        }
        check_bound(
            "open attempt blockers",
            blockers
                .iter()
                .filter(|blocker| blocker.state() == BlockerState::Open)
                .count(),
            MAX_OPEN_BLOCKERS_V2,
        )?;
        blockers.sort_by(|left, right| blocker_order(left).cmp(&blocker_order(right)));
        self.attempts
            .last_mut()
            .expect("an admitted current attempt exists")
            .blockers = blockers;
        self.update(revision, now);
        Ok(())
    }

    /// Apply acceptance after the caller validates the receipt and worker settlement. The package
    /// digest is the independently prepared current package, never a value trusted from the receipt.
    pub fn accept_delegation(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        binding: ScopedDelegationBindingV1,
        admitted_package: &Sha256Digest,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let (attempt, placement, items) = self.current_parts()?;
        if !matches!(
            placement.kind(),
            PlacementKindV3::Action {
                delegation: Some(_),
                ..
            }
        ) || !binding.applies_to(&attempt.target, admitted_package)
        {
            return Err(invalid(
                "delegation acceptance does not apply to the current action/package",
            ));
        }
        self.require_ready(attempt, placement, items)?;
        self.attempts
            .last_mut()
            .expect("an admitted current attempt exists")
            .delegation = Some(binding);
        self.update(revision, now);
        Ok(())
    }

    /// An ordinary Action advances locally. The selected group's only terminal returns to base.
    pub fn complete(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        successor: Option<AttemptId>,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let (attempt, placement, items) = self.current_parts()?;
        let PlacementKindV3::Action {
            outcome,
            delegation,
            ..
        } = placement.kind()
        else {
            return Err(invalid("only an Action can be completed"));
        };
        if attempt.target.scope() == &ExecutionScopeV1::Base
            && placement.id() == self.base.boundary().anchor()
        {
            return Err(invalid("the design anchor can only advance by activation"));
        }
        self.require_ready(attempt, placement, items)?;
        if delegation.is_some() && attempt.delegation.is_none() {
            return Err(invalid("current delegated action acceptance is required"));
        }
        match (attempt.target.scope(), outcome) {
            (scope, ActionOutcomeV2::Next(node)) => {
                self.check_ordinary_target(scope, node, false)?;
                let next = self.prepare_attempt(
                    scope.clone(),
                    node,
                    required_successor(successor)?,
                    now,
                    None,
                )?;
                self.close_attempt(AttemptLifecycle::Completed, now);
                self.push_attempt(next);
            }
            (ExecutionScopeV1::Group { .. }, ActionOutcomeV2::Terminal) => {
                let selected = self
                    .selected
                    .ok_or_else(|| invalid("group terminal requires current selection"))?;
                let contract = self.base.boundary().result_items();
                // Readiness already enforced the terminal declarations' current requiredness.
                if !same_result_contract(contract, items) {
                    return Err(invalid(
                        "group terminal declarations differ from the base result contract",
                    ));
                }
                let next = self.prepare_attempt(
                    ExecutionScopeV1::Base,
                    self.base.boundary().success_return(),
                    required_successor(successor)?,
                    now,
                    None,
                )?;
                let terminal = attempt.target.attempt().clone();
                self.close_attempt(AttemptLifecycle::Completed, now);
                self.executions[selected].outcome = GroupExecutionOutcomeV1::Completed;
                self.executions[selected].terminal_attempt = Some(terminal);
                self.push_attempt(next);
            }
            (ExecutionScopeV1::Base, ActionOutcomeV2::Terminal) => {
                if successor.is_some()
                    || !self
                        .selected()
                        .is_some_and(|group| group.outcome == GroupExecutionOutcomeV1::Completed)
                    || (self.base.goal_tracking().is_some()
                        && self.current_goal_outcome().is_none())
                {
                    return Err(invalid(
                        "session terminal requires the current completed group and any required goal assessment",
                    ));
                }
                let source = attempt.target.clone();
                self.close_attempt(AttemptLifecycle::Completed, now);
                self.close_selection(SelectionClosureCauseV1::Completion, source, None, now);
                self.lifecycle = SessionLifecycle::Completed;
            }
        }
        self.update(revision, now);
        Ok(())
    }

    pub fn decide(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        option: OptionId,
        reason: ReasonV2,
        determined_goal_outcome: Option<GoalOutcome>,
        successor: AttemptId,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let (attempt, placement, items) = self.current_parts()?;
        let (definition, route) = self.decision(placement, attempt.target.scope(), &option)?;
        let (node, rework) = match route {
            DecisionRouteV3::Advance(node) => (node, false),
            DecisionRouteV3::Rework(node) => (node, true),
            DecisionRouteV3::Replan => {
                return Err(invalid(
                    "the selected option requires explicit group replan",
                ));
            }
        };
        let goal_assessment = match (definition.metadata().assessment(), determined_goal_outcome) {
            (None, None) => None,
            (Some(assessment), Some(outcome))
                if attempt.target.scope() == &ExecutionScopeV1::Base
                    && assessment.outcomes().iter().any(|entry| {
                        entry.option_id() == &option && entry.outcome() == outcome
                    }) =>
            {
                let revision = self
                    .goal_revision()
                    .ok_or_else(|| invalid("goal assessment requires a current goal"))?;
                Some((revision, outcome))
            }
            _ => {
                return Err(invalid(
                    "goal assessment requires the current goal and an option mapping its determined outcome",
                ));
            }
        };
        self.require_ready(attempt, placement, items)?;
        let guard_values = self.guard_values(attempt, placement, definition, &option, false)?;
        self.check_ordinary_target(attempt.target.scope(), node, rework)?;
        let invalidation = rework
            .then(|| self.rework_index(attempt.target.scope(), node))
            .transpose()?;
        let next =
            self.prepare_attempt(attempt.target.scope().clone(), node, successor, now, None)?;
        self.close_attempt(AttemptLifecycle::Completed, now)
            .decision = Some(GroupDecisionRecordV1 {
            option,
            reason: reason.into_inner(),
            guard_values,
            goal_assessment,
        });
        if let Some(index) = invalidation {
            self.invalidate_from(next.target.scope(), index);
        }
        self.push_attempt(next);
        self.update(revision, now);
        Ok(())
    }

    pub fn replan(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        feedback: ReplanFeedbackV1,
        fresh_anchor: AttemptId,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let (attempt, placement, items) = self.current_parts()?;
        let (definition, route) =
            self.decision(placement, attempt.target.scope(), feedback.option())?;
        if feedback.decision() != &attempt.target
            || !matches!(route, DecisionRouteV3::Replan)
            || definition.metadata().assessment().is_some()
            || self.selected.is_none()
            || (attempt.target.scope() == &ExecutionScopeV1::Base
                && !self
                    .base
                    .boundary()
                    .replan_decisions()
                    .contains(placement.id()))
        {
            return Err(invalid(
                "replan requires a declared ordinary Decision at the current selected placement",
            ));
        }
        self.require_ready(attempt, placement, items)?;
        let guard_values =
            self.guard_values(attempt, placement, definition, feedback.option(), true)?;
        let anchor = self.base.boundary().anchor();
        let invalidate_base = self.rework_index(&ExecutionScopeV1::Base, anchor)?;
        let next = self.prepare_attempt(ExecutionScopeV1::Base, anchor, fresh_anchor, now, None)?;
        let source = attempt.target.clone();
        self.close_attempt(AttemptLifecycle::Completed, now)
            .decision = Some(GroupDecisionRecordV1 {
            option: feedback.option().clone(),
            reason: feedback.reason().to_owned(),
            guard_values,
            goal_assessment: None,
        });
        self.invalidate_from(&ExecutionScopeV1::Base, invalidate_base);
        self.close_selection(
            SelectionClosureCauseV1::Replan,
            source,
            Some(feedback.into_closure()),
            now,
        );
        self.push_attempt(next);
        self.update(revision, now);
        Ok(())
    }

    pub fn revise_goal(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        change: ExecutionGoalChangeV3,
        successor: AttemptId,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let ExecutionGoalChangeV3 {
            goal,
            ref target,
            feedback,
            reactivate,
        } = change;
        let next_goal_revision = self
            .goal_revision()
            .ok_or_else(|| invalid("goal revision requires a defined tracked goal"))?
            .checked_next()?;
        let cause = match (self.lifecycle, reactivate) {
            (SessionLifecycle::Running, false) => SelectionClosureCauseV1::GoalRevision,
            (SessionLifecycle::Completed, true) => SelectionClosureCauseV1::Reactivation,
            _ => {
                return Err(invalid(
                    "goal revision requires a running session or explicit completed-session reactivation",
                ));
            }
        };
        let anchor = self.base.boundary().anchor();
        let active_anchor = self.active_attempt().is_some_and(|attempt| {
            attempt.target.scope() == &ExecutionScopeV1::Base && attempt.target.node() == anchor
        });
        if !self.executions.is_empty() || reactivate || active_anchor {
            if target != anchor {
                return Err(invalid(
                    "this goal revision must create a fresh design-anchor attempt",
                ));
            }
        } else if !self.base.graph().manual_rework().contains(target) {
            return Err(invalid(
                "goal revision target is not a declared base target",
            ));
        }
        let index = self.rework_index(&ExecutionScopeV1::Base, target)?;
        let next = self.prepare_attempt(ExecutionScopeV1::Base, target, successor, now, None)?;
        let source = self
            .attempts
            .last()
            .expect("the begun session has a trace")
            .target
            .clone();
        let predecessor = self.latest_group_id().cloned();
        if self.active_attempt().is_some() {
            self.close_attempt(AttemptLifecycle::Abandoned, now);
        }
        self.invalidate_from(&ExecutionScopeV1::Base, index);
        self.close_selection(cause, source.clone(), Some(feedback.clone()), now);
        self.goal_revisions.push(GroupGoalRevisionV1 {
            revision: next_goal_revision,
            goal,
            source: source.clone(),
            successor: next.target.clone(),
            predecessor,
            closure: GroupSelectionClosureV1 {
                cause,
                source,
                at: now,
                feedback: Some(feedback),
            },
        });
        self.lifecycle = SessionLifecycle::Running;
        self.push_attempt(next);
        self.update(revision, now);
        Ok(())
    }

    pub fn cancel(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        reason: ReasonV2,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let source = self
            .active_attempt()
            .ok_or_else(|| invalid("only a running session can cancel"))?
            .target
            .clone();
        self.close_attempt(AttemptLifecycle::Abandoned, now).reason = Some(reason);
        self.close_selection(SelectionClosureCauseV1::Cancel, source, None, now);
        self.lifecycle = SessionLifecycle::Cancelled;
        self.update(revision, now);
        Ok(())
    }

    pub fn retry(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        reason: ReasonV2,
        successor: AttemptId,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let active = self
            .active_attempt()
            .ok_or_else(|| invalid("retry requires a current attempt"))?;
        let next = self.prepare_attempt(
            active.target.scope().clone(),
            active.target.node(),
            successor,
            now,
            None,
        )?;
        let index = self.attempts.len() - 1;
        self.close_attempt(AttemptLifecycle::Abandoned, now).reason = Some(reason);
        self.invalidate_from(next.target.scope(), index);
        self.push_attempt(next);
        self.update(revision, now);
        Ok(())
    }

    pub fn rework(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        target: &GraphNodeId,
        reason: ReasonV2,
        successor: AttemptId,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let active = self
            .active_attempt()
            .ok_or_else(|| invalid("rework requires a current attempt"))?;
        let scope = active.target.scope();
        let (graph, _) = self.definition_for(scope, None)?;
        if !graph.manual_rework().contains(target) {
            return Err(invalid(
                "manual rework target is not declared in the current scope",
            ));
        }
        self.check_ordinary_target(scope, target, true)?;
        let index = self.rework_index(scope, target)?;
        let next = self.prepare_attempt(scope.clone(), target, successor, now, None)?;
        self.close_attempt(AttemptLifecycle::Abandoned, now).reason = Some(reason);
        self.invalidate_from(next.target.scope(), index);
        self.push_attempt(next);
        self.update(revision, now);
        Ok(())
    }

    pub fn skip(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        reason: Option<ReasonV2>,
        successor: AttemptId,
        now: UnixMillis,
    ) -> Result<(), DomainError> {
        let revision = self.check_fence(fence, now)?;
        let (attempt, placement, _) = self.current_parts()?;
        let PlacementKindV3::Action {
            outcome: ActionOutcomeV2::Next(node),
            skip: Some(policy),
            delegation: None,
        } = placement.kind()
        else {
            return Err(invalid("only an eligible nonterminal Action may skip"));
        };
        if !policy.is_allowed()
            || (policy.reason_required() && reason.is_none())
            || (attempt.target.scope() == &ExecutionScopeV1::Base
                && placement.id() == self.base.boundary().anchor())
        {
            return Err(invalid("skip is not permitted at the current placement"));
        }
        if attempt
            .blockers
            .iter()
            .any(|blocker| blocker.state() == BlockerState::Open)
        {
            return Err(DomainError::BlockersPresent);
        }
        self.check_ordinary_target(attempt.target.scope(), node, false)?;
        let next =
            self.prepare_attempt(attempt.target.scope().clone(), node, successor, now, None)?;
        self.close_attempt(AttemptLifecycle::Skipped, now).reason = reason;
        self.push_attempt(next);
        self.update(revision, now);
        Ok(())
    }

    /// Resolve a declared evidence reference on the live path. History is never searched for a
    /// substitute. Frozen guard records use this same resolution.
    pub fn current_evidence(
        &self,
        source: &EvidenceSourceV3,
        item: &ItemId,
    ) -> Option<(&ScopedAttemptV1, &ItemSlotStateV2)> {
        let (attempt, placement, _) = self.current_parts().ok()?;
        if !placement.evidence().iter().any(|reference| {
            reference.source() == source
                && (reference.items().is_empty() || reference.items().contains(item))
        }) {
            return None;
        }
        self.source_exposes_item(source, item).then_some(())?;
        let evidenced = self.evidence_source(attempt.target.scope(), source)?;
        let slot = evidenced.slot(item)?;
        slot.value()?;
        Some((&evidenced.target, slot))
    }

    pub fn next_decision_entry(&self, node: &GraphNodeId) -> Option<u64> {
        let scope = self.active_attempt()?.target.scope();
        let (graph, _) = self.definition_for(scope, None).ok()?;
        matches!(
            placement(graph, node).ok()?.kind(),
            PlacementKindV3::Decision { .. }
        )
        .then(|| self.count_for(self.decision_group(scope, node), scope, node) + 1)
    }

    fn check_time(&self, now: UnixMillis) -> Result<(), DomainError> {
        if now < self.updated_at {
            return Err(invalid(
                "execution transition time predates the latest state",
            ));
        }
        Ok(())
    }

    fn check_fence(
        &self,
        fence: &ExecutionCursorFenceV1,
        now: UnixMillis,
    ) -> Result<Revision, DomainError> {
        self.check_time(now)?;
        if fence.session != self.session {
            return Err(DomainError::SessionIdentityMismatch {
                expected: fence.session.clone(),
                actual: Some(self.session.clone()),
            });
        }
        let actual = self.active_attempt();
        if fence.attempt.as_ref() != actual.map(|attempt| &attempt.target) {
            return Err(match &fence.attempt {
                Some(expected) => stale_attempt(expected, actual),
                None => invalid("cursor fence unexpectedly omits a current attempt"),
            });
        }
        if fence.goal_revision != self.goal_revision() {
            return Err(invalid("current goal revision changed"));
        }
        if fence.revision != self.revision {
            return Err(DomainError::PreconditionFailed {
                expected: fence.revision,
                actual: self.revision,
            });
        }
        self.revision.checked_next()
    }

    fn current_parts(
        &self,
    ) -> Result<(&GroupAttemptStateV1, &PlacementV3, &[ItemSpecV2]), DomainError> {
        let attempt = self
            .active_attempt()
            .ok_or_else(|| invalid("no current execution attempt"))?;
        let (graph, definitions) = self.definition_for(attempt.target.scope(), None)?;
        let placement = placement(graph, attempt.target.node())?;
        Ok((
            attempt,
            placement,
            definition_items(node_definition(definitions, placement)?),
        ))
    }

    fn definition_for<'a>(
        &'a self,
        scope: &ExecutionScopeV1,
        pending: Option<&'a ExecutionGroupDefinitionV1>,
    ) -> Result<(&'a TopologyV3, &'a [NodeDefinitionV3]), DomainError> {
        match scope {
            ExecutionScopeV1::Base => Ok((self.base.graph(), self.base.definitions())),
            ExecutionScopeV1::Group {
                group_execution_id,
                definition_digest,
            } => {
                let group = match pending {
                    Some(group) => group,
                    None => {
                        if !self.selected().is_some_and(|execution| {
                            execution.identity.execution() == group_execution_id
                                && execution.identity.definition_digest() == definition_digest
                                && execution.outcome == GroupExecutionOutcomeV1::Running
                        }) {
                            return Err(invalid(
                                "attempt does not bind the selected running group",
                            ));
                        }
                        self.definitions
                            .get(definition_digest)
                            .ok_or_else(|| invalid("selected group definition is absent"))?
                    }
                };
                Ok((group.graph(), group.definitions()))
            }
        }
    }

    fn decision<'a>(
        &'a self,
        placement: &'a PlacementV3,
        scope: &ExecutionScopeV1,
        option: &OptionId,
    ) -> Result<(&'a DecisionDefinitionV3, &'a DecisionRouteV3), DomainError> {
        let PlacementKindV3::Decision { routes } = placement.kind() else {
            return Err(invalid("the current placement is not a Decision"));
        };
        let (_, definitions) = self.definition_for(scope, None)?;
        let NodeDefinitionV3::Decision(definition) = node_definition(definitions, placement)?
        else {
            return Err(invalid("Decision placement has no Decision definition"));
        };
        let route = routes
            .iter()
            .find(|(id, _)| id == option)
            .map(|(_, route)| route)
            .ok_or_else(|| invalid("the selected option has no declared route"))?;
        Ok((definition, route))
    }

    fn require_ready(
        &self,
        attempt: &GroupAttemptStateV1,
        placement: &PlacementV3,
        items: &[ItemSpecV2],
    ) -> Result<(), DomainError> {
        if attempt
            .blockers
            .iter()
            .any(|blocker| blocker.state() == BlockerState::Open)
        {
            return Err(DomainError::BlockersPresent);
        }
        let values = attempt
            .slots
            .iter()
            .map(ItemSlotStateV2::value)
            .collect::<Vec<_>>();
        if items.iter().zip(&values).any(|(item, value)| {
            item.required_now(items, &values)
                && !value.is_some_and(|value| item.is_satisfied_by(value))
        }) {
            return Err(DomainError::RequiredItemsMissing);
        }
        if placement.evidence().iter().any(|reference| {
            reference.required()
                && self
                    .evidence_source(attempt.target.scope(), reference.source())
                    .is_none()
        }) {
            return Err(invalid("required current evidence source is unavailable"));
        }
        Ok(())
    }

    fn evidence_source(
        &self,
        scope: &ExecutionScopeV1,
        source: &EvidenceSourceV3,
    ) -> Option<&GroupAttemptStateV1> {
        let result = match (scope, source) {
            (_, EvidenceSourceV3::Local(node)) => self.attempts.get(*self.live(scope).get(node)?),
            (ExecutionScopeV1::Group { .. }, EvidenceSourceV3::Plan) => {
                self.history_attempt(self.selected()?.identity.plan().design_attempt())
            }
            (ExecutionScopeV1::Base, EvidenceSourceV3::ExecutionResult) => {
                let group = self.selected()?;
                self.history_attempt(group.terminal_attempt.as_ref()?)
            }
            _ => None,
        }?;
        (result.lifecycle == AttemptLifecycle::Completed && result.validity.is_valid())
            .then_some(result)
    }

    fn source_exposes_item(&self, source: &EvidenceSourceV3, item: &ItemId) -> bool {
        match source {
            EvidenceSourceV3::Local(_) => true,
            EvidenceSourceV3::Plan => self.base.boundary().plan_inputs().contains(item),
            EvidenceSourceV3::ExecutionResult => self
                .base
                .boundary()
                .result_items()
                .iter()
                .any(|result| result.id() == item),
        }
    }

    fn guard_values(
        &self,
        attempt: &GroupAttemptStateV1,
        placement: &PlacementV3,
        definition: &DecisionDefinitionV3,
        option: &OptionId,
        required: bool,
    ) -> Result<Vec<GroupGuardValueV1>, DomainError> {
        let Some(guard) = definition.guards(option) else {
            return if required {
                Err(invalid("replan option lacks eligible guards"))
            } else {
                Ok(Vec::new())
            };
        };
        guard
            .predicates()
            .iter()
            .map(|guard| {
                let item = guard.predicate().item();
                if !self.source_exposes_item(guard.source(), item)
                    || !placement.evidence().iter().any(|reference| {
                        reference.required()
                            && reference.source() == guard.source()
                            && reference.items().contains(item)
                    })
                {
                    return Err(invalid(
                        "guard does not select a required declared source item",
                    ));
                }
                let source = self
                    .evidence_source(attempt.target.scope(), guard.source())
                    .ok_or_else(|| invalid("guard source is unavailable"))?;
                let slot = source
                    .slot(item)
                    .ok_or_else(|| invalid("guard item is undeclared"))?;
                let value = slot
                    .value()
                    .filter(|value| guard.evaluate(Some(value)).state() == ConditionStateV2::Met)
                    .ok_or_else(|| invalid("guard is unmet or unevaluable in current evidence"))?;
                Ok(GroupGuardValueV1 {
                    source: source.target.clone(),
                    item: item_binding(slot, value)?,
                })
            })
            .collect()
    }

    fn prepare_attempt(
        &self,
        scope: ExecutionScopeV1,
        node: &GraphNodeId,
        id: AttemptId,
        now: UnixMillis,
        pending: Option<&ExecutionGroupDefinitionV1>,
    ) -> Result<GroupAttemptStateV1, DomainError> {
        check_bound(
            "session attempts",
            self.attempts.len() + 1,
            MAX_SESSION_ATTEMPTS_V1,
        )?;
        if self.attempt_ids.contains_key(&id) {
            return Err(invalid(
                "fresh attempt ID is already retained in this session",
            ));
        }
        let (graph, definitions) = self.definition_for(&scope, pending)?;
        let placement = placement(graph, node)?;
        let definition = node_definition(definitions, placement)?;
        let slots = definition_items(definition)
            .iter()
            .map(|item| {
                ItemSlotStateV2::new(
                    id.clone(),
                    item.id().clone(),
                    item.item_type(),
                    Revision::ZERO,
                    None,
                    now,
                    now,
                )
                .map_err(|e| invalid(e.reason()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let target = ScopedAttemptV1::new(
            self.session.clone(),
            self.base_digest.clone(),
            scope,
            node.clone(),
            id,
        );
        let decision_entry = if matches!(placement.kind(), PlacementKindV3::Decision { .. }) {
            let group = self.decision_group(target.scope(), node);
            let ordinal = self.count_for(group, target.scope(), node) + 1;
            Some(match group {
                Some(group) => DecisionEntry::Selected(DecisionEntryV1::new(
                    target.clone(),
                    group.clone(),
                    ordinal,
                )?),
                None => DecisionEntry::BasePrefix(ordinal),
            })
        } else {
            None
        };
        Ok(GroupAttemptStateV1 {
            target,
            lifecycle: AttemptLifecycle::Active,
            validity: AttemptValidityV2::Valid,
            opened_at: now,
            closed_at: None,
            slots,
            blockers: Vec::new(),
            delegation: None,
            decision_entry,
            decision: None,
            reason: None,
        })
    }

    fn push_attempt(&mut self, next: GroupAttemptStateV1) {
        let index = self.attempts.len();
        let node = next.target.node();
        let scope = next.target.scope();
        if let Some(ordinal) = next.decision_entry() {
            let group = next
                .selected_decision_entry()
                .map(|entry| entry.selected_group().clone());
            self.decision_counts.insert(
                (group, *scope == ExecutionScopeV1::Base, node.clone()),
                ordinal,
            );
        }
        self.attempt_ids
            .insert(next.target.attempt().clone(), index);
        self.live_mut(scope).insert(node.clone(), index);
        self.attempts.push(next);
    }

    fn decision_group<'a>(
        &'a self,
        scope: &'a ExecutionScopeV1,
        node: &GraphNodeId,
    ) -> Option<&'a GroupExecutionIdV1> {
        match scope {
            ExecutionScopeV1::Group {
                group_execution_id, ..
            } => Some(group_execution_id),
            ExecutionScopeV1::Base if self.base_suffix.contains(node) => self.latest_selected_id(),
            ExecutionScopeV1::Base => None,
        }
    }

    fn count_for(
        &self,
        group: Option<&GroupExecutionIdV1>,
        scope: &ExecutionScopeV1,
        node: &GraphNodeId,
    ) -> u64 {
        self.decision_counts
            .get(&(
                group.cloned(),
                *scope == ExecutionScopeV1::Base,
                node.clone(),
            ))
            .copied()
            .unwrap_or(0)
    }

    fn close_attempt(
        &mut self,
        lifecycle: AttemptLifecycle,
        now: UnixMillis,
    ) -> &mut GroupAttemptStateV1 {
        let attempt = self
            .attempts
            .last_mut()
            .expect("an admitted active attempt exists");
        attempt.lifecycle = lifecycle;
        attempt.closed_at = Some(now);
        attempt
    }

    fn close_selection(
        &mut self,
        cause: SelectionClosureCauseV1,
        source: ScopedAttemptV1,
        feedback: Option<GroupClosureFeedbackV1>,
        now: UnixMillis,
    ) {
        if let Some(index) = self.selected.take() {
            let group = &mut self.executions[index];
            if group.outcome == GroupExecutionOutcomeV1::Running {
                group.outcome = if cause == SelectionClosureCauseV1::Cancel {
                    GroupExecutionOutcomeV1::TerminatedByCancel
                } else {
                    GroupExecutionOutcomeV1::Abandoned
                };
            }
            group.closure = Some(GroupSelectionClosureV1 {
                cause,
                source,
                at: now,
                feedback,
            });
            self.current_group.clear();
        }
    }

    fn latest_group_id(&self) -> Option<&GroupExecutionIdV1> {
        self.executions
            .last()
            .map(|entry| entry.identity.execution())
    }

    fn latest_selected_id(&self) -> Option<&GroupExecutionIdV1> {
        self.selected().map(|entry| entry.identity.execution())
    }

    fn check_prior_work(&self, identity: &ActivationIdentityV1) -> Result<(), DomainError> {
        for work in identity.prior_work() {
            let earlier = self
                .executions
                .iter()
                .find(|execution| execution.identity.execution() == work.group())
                .ok_or_else(|| {
                    invalid("prior work does not identify a retained same-session group")
                })?;
            if earlier.closure.is_none() {
                return Err(invalid("prior work must identify a closed execution"));
            }
            let (attempt, item) = match work.item() {
                None => continue,
                Some(HistoricalReferenceV1::Attempt(attempt)) => (attempt, None),
                Some(HistoricalReferenceV1::Item(item)) => (item.attempt(), Some(item)),
            };
            let retained = self.history_attempt(attempt).filter(|attempt| matches!(attempt.target.scope(),
                ExecutionScopeV1::Group { group_execution_id, .. } if group_execution_id == work.group()
            )).ok_or_else(|| invalid("prior work attempt is outside its retained group"))?;
            if let Some(item) = item {
                let value = retained
                    .slot(item.item())
                    .and_then(ItemSlotStateV2::value)
                    .ok_or_else(|| invalid("the retained work item has no final value"))?;
                if &recorded_value_digest_v2(value).map_err(|e| invalid(e.reason()))?
                    != item.value_digest()
                {
                    return Err(invalid(
                        "prior work value digest differs from retained history",
                    ));
                }
            }
        }
        Ok(())
    }

    fn history_attempt(&self, id: &AttemptId) -> Option<&GroupAttemptStateV1> {
        self.attempt_ids.get(id).map(|&index| &self.attempts[index])
    }

    fn check_ordinary_target(
        &self,
        scope: &ExecutionScopeV1,
        target: &GraphNodeId,
        rework: bool,
    ) -> Result<(), DomainError> {
        if scope != &ExecutionScopeV1::Base {
            return Ok(());
        }
        let suffix = self.base_suffix.contains(target);
        if suffix != self.selected.is_some()
            || (rework
                && (target == self.base.boundary().anchor()
                    || (!suffix && !self.executions.is_empty())))
        {
            return Err(invalid(
                "ordinary base routing cannot cross the execution-group boundary",
            ));
        }
        Ok(())
    }

    fn rework_index(
        &self,
        scope: &ExecutionScopeV1,
        target: &GraphNodeId,
    ) -> Result<usize, DomainError> {
        self.live(scope)
            .get(target)
            .copied()
            .ok_or_else(|| invalid("rework requires a visited current target"))
    }

    fn invalidate_from(&mut self, scope: &ExecutionScopeV1, index: usize) {
        let stale = self
            .live(scope)
            .values()
            .copied()
            .filter(|&attempt| attempt >= index)
            .collect::<Vec<_>>();
        for attempt in stale {
            self.attempts[attempt].validity = AttemptValidityV2::Stale;
        }
        self.live_mut(scope).retain(|_, attempt| *attempt < index);
    }

    fn live(&self, scope: &ExecutionScopeV1) -> &BTreeMap<GraphNodeId, usize> {
        match scope {
            ExecutionScopeV1::Base => &self.current_base,
            ExecutionScopeV1::Group { .. } => &self.current_group,
        }
    }

    fn live_mut(&mut self, scope: &ExecutionScopeV1) -> &mut BTreeMap<GraphNodeId, usize> {
        match scope {
            ExecutionScopeV1::Base => &mut self.current_base,
            ExecutionScopeV1::Group { .. } => &mut self.current_group,
        }
    }

    fn update(&mut self, revision: Revision, now: UnixMillis) {
        self.revision = revision;
        self.updated_at = now;
    }
}

fn blocker_order(blocker: &BlockerStateV2) -> (UnixMillis, &crate::BlockerId) {
    (blocker.created_at(), blocker.blocker_id())
}

fn stale_attempt(expected: &ScopedAttemptV1, actual: Option<&GroupAttemptStateV1>) -> DomainError {
    DomainError::AttemptNotCurrent {
        expected: expected.attempt().clone(),
        actual: actual.map(|attempt| attempt.target.attempt().clone()),
    }
}

fn required_successor(successor: Option<AttemptId>) -> Result<AttemptId, DomainError> {
    successor.ok_or_else(|| invalid("this transition needs a fresh successor attempt ID"))
}

fn check_bound(field: &'static str, actual: usize, maximum: usize) -> Result<(), DomainError> {
    if actual > maximum {
        return Err(DomainError::BoundExceeded {
            field,
            actual: actual as u64,
            maximum: maximum as u64,
            unit: BoundUnitV2::Entries,
        });
    }
    Ok(())
}

fn item_binding(
    slot: &ItemSlotStateV2,
    value: &RecordedItemValueV2,
) -> Result<PlanItemBindingV1, DomainError> {
    PlanItemBindingV1::new(
        slot.item_id().clone(),
        slot.revision(),
        recorded_value_digest_v2(value).map_err(|e| invalid(e.reason()))?,
    )
}

fn selected_items(items: &[ItemSpecV2], ids: &[ItemId]) -> Result<Vec<ItemSpecV2>, DomainError> {
    ids.iter()
        .map(|id| {
            items
                .iter()
                .find(|item| item.id() == id)
                .cloned()
                .ok_or_else(|| invalid("boundary item declaration is absent"))
        })
        .collect()
}

fn placement<'a>(graph: &'a TopologyV3, id: &GraphNodeId) -> Result<&'a PlacementV3, DomainError> {
    graph
        .nodes()
        .iter()
        .find(|placement| placement.id() == id)
        .ok_or_else(|| invalid("placement is absent from its scope"))
}

fn node_definition<'a>(
    definitions: &'a [NodeDefinitionV3],
    placement: &PlacementV3,
) -> Result<&'a NodeDefinitionV3, DomainError> {
    definitions
        .iter()
        .find(|definition| definition.id() == placement.definition())
        .ok_or_else(|| invalid("placement definition is absent from its scope"))
}

fn definition_items(definition: &NodeDefinitionV3) -> &[ItemSpecV2] {
    match definition {
        NodeDefinitionV3::Action(action) => action.items(),
        NodeDefinitionV3::Decision(decision) => decision.metadata().items(),
    }
}

fn same_result_contract(contract: &[ItemSpecV2], terminal: &[ItemSpecV2]) -> bool {
    let exposed = terminal
        .iter()
        .filter(|item| contract.iter().any(|expected| expected.id() == item.id()));
    let mut count = 0;
    for item in exposed {
        if !contract
            .get(count)
            .is_some_and(|expected| expected.same_boundary_contract(item))
        {
            return false;
        }
        count += 1;
    }
    count == contract.len()
}

fn terminal_items(group: &ExecutionGroupDefinitionV1) -> Result<&[ItemSpecV2], DomainError> {
    let mut terminals = group.graph().nodes().iter().filter(|placement| {
        matches!(
            placement.kind(),
            PlacementKindV3::Action {
                outcome: ActionOutcomeV2::Terminal,
                ..
            }
        )
    });
    let terminal = terminals
        .next()
        .filter(|_| terminals.next().is_none())
        .ok_or_else(|| invalid("group must have exactly one terminal Action"))?;
    Ok(definition_items(node_definition(
        group.definitions(),
        terminal,
    )?))
}

fn downstream(graph: &TopologyV3, start: &GraphNodeId) -> BTreeSet<GraphNodeId> {
    let mut seen = BTreeSet::new();
    let mut queue = vec![start];
    while let Some(id) = queue.pop() {
        if let Ok(placement) = placement(graph, id)
            && seen.insert(id.clone())
        {
            match placement.kind() {
                PlacementKindV3::Action {
                    outcome: ActionOutcomeV2::Next(next),
                    ..
                } => queue.push(next),
                PlacementKindV3::Decision { routes } => {
                    queue.extend(routes.iter().filter_map(|(_, route)| {
                        if let DecisionRouteV3::Advance(next) = route {
                            Some(next)
                        } else {
                            None
                        }
                    }))
                }
                PlacementKindV3::Action {
                    outcome: ActionOutcomeV2::Terminal,
                    ..
                } => {}
            }
        }
    }
    seen
}
