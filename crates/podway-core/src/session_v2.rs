//! Pure Procedure v2 session cursor and execution-trace invariants.
//!
//! Enforces the single-cursor model of ADR-0017 and promoted invariants `INV-S02`, `INV-S03`,
//! `INV-V2S01`, and `INV-V2S08`: a running session has exactly one active attempt (the last
//! appended member), a terminal session has none, every graph node has at most one valid attempt,
//! and validity moves one direction only. The transitions are pure invariant-preserving primitives
//! over lifecycle, validity, sequence, and identity; item, evidence, reason, option, goal, and
//! admission policy is owned by V2RUN/V2DRW/V2GOL, and recorded item values, blockers, timestamps,
//! and evidence references by V2MOD-003/V2RUN.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    AttemptId, AttemptLifecycle, DomainError, GraphNodeId, Revision, SessionId, SessionLifecycle,
};

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}

/// A session-scoped trace sequence assigned at activation (section 9.7). `FIRST` is the first
/// assigned value; sequence numbers are never reassigned. `ZERO` is a non-authoritative sentinel.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TraceSequenceV2(u64);

impl TraceSequenceV2 {
    pub const ZERO: Self = Self(0);
    pub const FIRST: Self = Self(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, DomainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DomainError::RevisionOverflow {
                revision: Revision::new(self.0),
            })
    }
}

/// A graph-node-scoped attempt number (section 4.4). The first attempt of a node is `FIRST`; each
/// later attempt of the same node is one greater.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttemptNumberV2(u64);

impl AttemptNumberV2 {
    pub const ZERO: Self = Self(0);
    pub const FIRST: Self = Self(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, DomainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DomainError::RevisionOverflow {
                revision: Revision::new(self.0),
            })
    }
}

/// A session-goal revision identity bound at activation (section 4.5). The immutable goal revision
/// record is owned by V2MOD-003; this carries only the monotonic identity, or absence.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GoalRevisionNumberV2(u64);

impl GoalRevisionNumberV2 {
    pub const ZERO: Self = Self(0);
    pub const FIRST: Self = Self(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, DomainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DomainError::RevisionOverflow {
                revision: Revision::new(self.0),
            })
    }
}

/// Whether an attempt may satisfy the current path (section 9.7). Monotone: valid may become
/// stale; stale never becomes valid again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptValidityV2 {
    Valid,
    Stale,
}

impl AttemptValidityV2 {
    pub const fn is_valid(self) -> bool {
        matches!(self, Self::Valid)
    }
}

/// One runtime attempt of one graph node (sections 4.4 and 9.7). Carries only cursor-invariant
/// identity and state; item values, blockers, timestamps, and evidence references are not
/// represented here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAttemptV2 {
    attempt_id: AttemptId,
    graph_node_id: GraphNodeId,
    number: AttemptNumberV2,
    trace: TraceSequenceV2,
    lifecycle: AttemptLifecycle,
    validity: AttemptValidityV2,
    goal_revision: Option<GoalRevisionNumberV2>,
}

impl SessionAttemptV2 {
    /// Reconstructs an attempt, enforcing the authoritative seams: nonzero trace/number (and
    /// nonzero bound goal revision), active implies valid, and abandoned implies stale.
    pub fn new(
        attempt_id: AttemptId,
        graph_node_id: GraphNodeId,
        number: AttemptNumberV2,
        trace: TraceSequenceV2,
        lifecycle: AttemptLifecycle,
        validity: AttemptValidityV2,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<Self, DomainError> {
        if number < AttemptNumberV2::FIRST {
            return Err(invalid("attempt number must be nonzero"));
        }
        if trace < TraceSequenceV2::FIRST {
            return Err(invalid("trace sequence must be nonzero"));
        }
        if matches!(goal_revision, Some(goal) if goal < GoalRevisionNumberV2::FIRST) {
            return Err(invalid("goal revision must be nonzero"));
        }
        if lifecycle == AttemptLifecycle::Active && !validity.is_valid() {
            return Err(invalid("an active attempt must be valid"));
        }
        if lifecycle == AttemptLifecycle::Abandoned && validity.is_valid() {
            return Err(invalid("an abandoned attempt must be stale"));
        }
        Ok(Self {
            attempt_id,
            graph_node_id,
            number,
            trace,
            lifecycle,
            validity,
            goal_revision,
        })
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub fn graph_node_id(&self) -> &GraphNodeId {
        &self.graph_node_id
    }

    pub const fn number(&self) -> AttemptNumberV2 {
        self.number
    }

    pub const fn trace(&self) -> TraceSequenceV2 {
        self.trace
    }

    pub const fn lifecycle(&self) -> AttemptLifecycle {
        self.lifecycle
    }

    pub const fn validity(&self) -> AttemptValidityV2 {
        self.validity
    }

    pub fn goal_revision(&self) -> Option<GoalRevisionNumberV2> {
        self.goal_revision
    }

    pub const fn is_active(&self) -> bool {
        matches!(self.lifecycle, AttemptLifecycle::Active)
    }
}

/// The authoritative active-position triple of a running session (INV-V2S01). Derived only from the
/// active attempt, so its fields cannot drift apart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCursorV2 {
    graph_node_id: GraphNodeId,
    attempt_id: AttemptId,
    trace: TraceSequenceV2,
}

impl ActiveCursorV2 {
    fn from_attempt(attempt: &SessionAttemptV2) -> Self {
        Self {
            graph_node_id: attempt.graph_node_id.clone(),
            attempt_id: attempt.attempt_id.clone(),
            trace: attempt.trace,
        }
    }

    pub fn graph_node_id(&self) -> &GraphNodeId {
        &self.graph_node_id
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub const fn trace(&self) -> TraceSequenceV2 {
        self.trace
    }
}

/// How `advance` and `finish` label the attempt being terminalized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvanceTerminalV2 {
    Completed,
    Skipped,
}

/// The append-only execution trace of one v2 session and its authoritative cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTraceV2 {
    session_id: SessionId,
    lifecycle: SessionLifecycle,
    revision: Revision,
    attempts: Vec<SessionAttemptV2>,
}

impl SessionTraceV2 {
    /// Trust-boundary reconstruction. Rejects collections that no legitimate runtime transition can
    /// produce: non-consecutive trace sequence, per-node attempt numbers that do not start at
    /// `FIRST` and increase by one in trace order, duplicate attempt ids, more than one valid
    /// attempt per node, and an active-count inconsistent with the lifecycle.
    pub fn from_parts(
        session_id: SessionId,
        lifecycle: SessionLifecycle,
        revision: Revision,
        attempts: Vec<SessionAttemptV2>,
    ) -> Result<Self, DomainError> {
        validate_reconstruction(&attempts)?;
        let active_count = attempts
            .iter()
            .filter(|attempt| attempt.is_active())
            .count();
        match lifecycle {
            SessionLifecycle::Running => {
                if active_count != 1 {
                    return Err(invalid(
                        "a running session requires exactly one active attempt",
                    ));
                }
                if !attempts.last().is_some_and(|attempt| attempt.is_active()) {
                    return Err(invalid("the active attempt must be the last trace member"));
                }
            }
            SessionLifecycle::Completed | SessionLifecycle::Cancelled => {
                if active_count != 0 {
                    return Err(invalid("a terminal session must have no active attempt"));
                }
                let Some(last) = attempts.last() else {
                    return Err(invalid("a terminal session requires an attempt"));
                };
                let terminal_shape_valid = match lifecycle {
                    SessionLifecycle::Completed => {
                        matches!(
                            last.lifecycle,
                            AttemptLifecycle::Completed | AttemptLifecycle::Skipped
                        ) && last.validity.is_valid()
                    }
                    SessionLifecycle::Cancelled => {
                        last.lifecycle == AttemptLifecycle::Abandoned && !last.validity.is_valid()
                    }
                    SessionLifecycle::Running => unreachable!(),
                };
                if !terminal_shape_valid {
                    return Err(invalid("the terminal session attempt is inconsistent"));
                }
            }
        }
        Ok(Self {
            session_id,
            lifecycle,
            revision,
            attempts,
        })
    }

    /// Activates the first attempt of the entry graph node in a fresh running session.
    pub fn start(
        session_id: SessionId,
        entry_node: GraphNodeId,
        attempt_id: AttemptId,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<Self, DomainError> {
        let attempt = SessionAttemptV2::new(
            attempt_id,
            entry_node,
            AttemptNumberV2::FIRST,
            TraceSequenceV2::FIRST,
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
            goal_revision,
        )?;
        Ok(Self {
            session_id,
            lifecycle: SessionLifecycle::Running,
            revision: Revision::ZERO,
            attempts: vec![attempt],
        })
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub const fn lifecycle(&self) -> SessionLifecycle {
        self.lifecycle
    }

    pub const fn revision(&self) -> Revision {
        self.revision
    }

    pub fn attempts(&self) -> &[SessionAttemptV2] {
        &self.attempts
    }

    pub fn active_attempt(&self) -> Option<&SessionAttemptV2> {
        self.attempts.iter().find(|attempt| attempt.is_active())
    }

    pub fn active_cursor(&self) -> Option<ActiveCursorV2> {
        self.active_attempt().map(ActiveCursorV2::from_attempt)
    }

    /// Binds immutable goal revision 1 to the unchanged active attempt.
    pub fn bind_initial_goal_revision(
        &mut self,
        expected_active: &AttemptId,
    ) -> Result<(), DomainError> {
        let active_index = self.require_running_active(expected_active)?;
        if self.attempts[active_index].goal_revision.is_some() {
            return Err(invalid("the active attempt already has a goal revision"));
        }
        self.attempts[active_index].goal_revision = Some(GoalRevisionNumberV2::FIRST);
        self.revision = self.revision.checked_next()?;
        Ok(())
    }

    /// Binds revision 1 while constructing a brand-new admitted session without creating a
    /// second session revision.
    pub fn bind_initial_goal_revision_at_start(
        &mut self,
        expected_active: &AttemptId,
    ) -> Result<(), DomainError> {
        if self.revision != Revision::new(1) || self.attempts.len() != 1 {
            return Err(invalid(
                "initial goal admission requires a fresh session trace",
            ));
        }
        let active_index = self.require_running_active(expected_active)?;
        if self.attempts[active_index].goal_revision.is_some() {
            return Err(invalid("the active attempt already has a goal revision"));
        }
        self.attempts[active_index].goal_revision = Some(GoalRevisionNumberV2::FIRST);
        Ok(())
    }

    /// Advances the cursor: terminalizes the active attempt and activates a fresh attempt of the
    /// next graph node. Covers completing or skipping a non-terminal action and a decision option
    /// whose route declares `effect: advance`.
    pub fn advance(
        &mut self,
        expected_active: &AttemptId,
        prior_terminal: AdvanceTerminalV2,
        next_node: GraphNodeId,
        fresh_attempt_id: AttemptId,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<(), DomainError> {
        let active_index = self.require_running_active(expected_active)?;
        if self.valid_attempt_trace(&next_node).is_some() {
            return Err(invalid("the target graph node already has a valid attempt"));
        }
        let (fresh, next_revision) =
            self.prepare_fresh(&next_node, &fresh_attempt_id, goal_revision)?;
        self.attempts[active_index].lifecycle = terminal_lifecycle(prior_terminal);
        self.attempts.push(fresh);
        self.revision = next_revision;
        Ok(())
    }

    /// Completes the session: terminalizes the active attempt and moves the session to `Completed`.
    pub fn finish(
        &mut self,
        expected_active: &AttemptId,
        prior_terminal: AdvanceTerminalV2,
    ) -> Result<(), DomainError> {
        let active_index = self.require_running_active(expected_active)?;
        let next_revision = self.revision.checked_next()?;
        self.attempts[active_index].lifecycle = terminal_lifecycle(prior_terminal);
        self.lifecycle = SessionLifecycle::Completed;
        self.revision = next_revision;
        Ok(())
    }

    /// Abandons the active attempt and activates a clean attempt of the same graph node. Retry is
    /// the degenerate one-attempt form of suffix invalidation: no earlier attempt changes validity.
    pub fn retry(
        &mut self,
        expected_active: &AttemptId,
        fresh_attempt_id: AttemptId,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<(), DomainError> {
        let active_index = self.require_running_active(expected_active)?;
        let node = self.attempts[active_index].graph_node_id.clone();
        let (fresh, next_revision) = self.prepare_fresh(&node, &fresh_attempt_id, goal_revision)?;
        self.attempts[active_index].lifecycle = AttemptLifecycle::Abandoned;
        self.attempts[active_index].validity = AttemptValidityV2::Stale;
        self.attempts.push(fresh);
        self.revision = next_revision;
        Ok(())
    }

    /// Cancels the running session by abandoning and staling its current attempt.
    pub fn cancel(&mut self, expected_active: &AttemptId) -> Result<(), DomainError> {
        let active_index = self.require_running_active(expected_active)?;
        let next_revision = self.revision.checked_next()?;
        self.attempts[active_index].lifecycle = AttemptLifecycle::Abandoned;
        self.attempts[active_index].validity = AttemptValidityV2::Stale;
        self.lifecycle = SessionLifecycle::Cancelled;
        self.revision = next_revision;
        Ok(())
    }

    /// Reworks the cursor to a target graph node that currently holds a valid attempt: completes
    /// the active attempt, conservatively stales the trace suffix from the target (including the
    /// active), and activates a fresh target attempt (sections 9.3 and 9.6). Detailed declared versus
    /// manual rework policy is owned by V2DRW.
    pub fn rework_to(
        &mut self,
        expected_active: &AttemptId,
        target_node: GraphNodeId,
        fresh_attempt_id: AttemptId,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<(), DomainError> {
        let active_index = self.require_running_active(expected_active)?;
        let target_trace = self
            .valid_attempt_trace(&target_node)
            .ok_or_else(|| invalid("rework target has no valid attempt on the trace"))?;
        let (fresh, next_revision) =
            self.prepare_fresh(&target_node, &fresh_attempt_id, goal_revision)?;
        self.attempts[active_index].lifecycle = AttemptLifecycle::Completed;
        self.stale_suffix_from_trace(target_trace);
        self.attempts.push(fresh);
        self.revision = next_revision;
        Ok(())
    }

    /// Manually re-enters one valid trace target from either a running or completed session.
    /// Running callers must fence the exact active attempt; completed callers must supply no
    /// attempt fence. Cancelled sessions cannot be reactivated.
    pub fn manual_rework(
        &mut self,
        expected_active: Option<&AttemptId>,
        target_node: GraphNodeId,
        fresh_attempt_id: AttemptId,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<(), DomainError> {
        match self.lifecycle {
            SessionLifecycle::Running => {
                let expected_active = expected_active
                    .ok_or_else(|| invalid("running manual rework requires the active attempt"))?;
                let active_index = self.require_running_active(expected_active)?;
                let target_trace = self
                    .valid_attempt_trace(&target_node)
                    .ok_or_else(|| invalid("rework target has no valid attempt on the trace"))?;
                let (fresh, next_revision) =
                    self.prepare_fresh(&target_node, &fresh_attempt_id, goal_revision)?;
                self.attempts[active_index].lifecycle = AttemptLifecycle::Abandoned;
                self.stale_suffix_from_trace(target_trace);
                self.attempts.push(fresh);
                self.revision = next_revision;
                Ok(())
            }
            SessionLifecycle::Completed => {
                if expected_active.is_some() {
                    return Err(invalid(
                        "completed manual rework accepts no active attempt fence",
                    ));
                }
                let target_trace = self
                    .valid_attempt_trace(&target_node)
                    .ok_or_else(|| invalid("rework target has no valid attempt on the trace"))?;
                let (fresh, next_revision) =
                    self.prepare_fresh(&target_node, &fresh_attempt_id, goal_revision)?;
                self.stale_suffix_from_trace(target_trace);
                self.attempts.push(fresh);
                self.lifecycle = SessionLifecycle::Running;
                self.revision = next_revision;
                Ok(())
            }
            SessionLifecycle::Cancelled => {
                Err(invalid("a cancelled session cannot be manually reworked"))
            }
        }
    }

    fn require_running_active(&self, expected: &AttemptId) -> Result<usize, DomainError> {
        if self.lifecycle != SessionLifecycle::Running {
            return Err(invalid("the session is not running"));
        }
        let index = self
            .attempts
            .iter()
            .position(|attempt| attempt.is_active())
            .ok_or(DomainError::AttemptNotCurrent {
                expected: expected.clone(),
                actual: None,
            })?;
        if &self.attempts[index].attempt_id != expected {
            return Err(DomainError::AttemptNotCurrent {
                expected: expected.clone(),
                actual: Some(self.attempts[index].attempt_id.clone()),
            });
        }
        Ok(index)
    }

    fn valid_attempt_trace(&self, node: &GraphNodeId) -> Option<TraceSequenceV2> {
        self.attempts
            .iter()
            .find(|attempt| attempt.validity.is_valid() && &attempt.graph_node_id == node)
            .map(|attempt| attempt.trace)
    }

    fn stale_suffix_from_trace(&mut self, from_trace: TraceSequenceV2) {
        for attempt in self.attempts.iter_mut() {
            if attempt.validity.is_valid() && attempt.trace >= from_trace {
                attempt.validity = AttemptValidityV2::Stale;
            }
        }
    }

    /// Read-only preparation of a fresh active attempt and the next revision. Every fallible step
    /// (duplicate id, attempt-number, trace, and revision overflow) precedes any mutation, so a
    /// returned `Err` leaves the trace unchanged.
    fn prepare_fresh(
        &self,
        node: &GraphNodeId,
        fresh_attempt_id: &AttemptId,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<(SessionAttemptV2, Revision), DomainError> {
        if self
            .attempts
            .iter()
            .any(|attempt| &attempt.attempt_id == fresh_attempt_id)
        {
            return Err(invalid("attempt identifiers must be globally unique"));
        }
        let number = self.next_attempt_number_for(node)?;
        let trace = self.next_trace()?;
        let next_revision = self.revision.checked_next()?;
        let attempt = SessionAttemptV2::new(
            fresh_attempt_id.clone(),
            node.clone(),
            number,
            trace,
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
            goal_revision,
        )?;
        Ok((attempt, next_revision))
    }

    fn next_trace(&self) -> Result<TraceSequenceV2, DomainError> {
        match self.attempts.last() {
            Some(attempt) => attempt.trace.checked_next(),
            None => Ok(TraceSequenceV2::FIRST),
        }
    }

    fn next_attempt_number_for(&self, node: &GraphNodeId) -> Result<AttemptNumberV2, DomainError> {
        match self
            .attempts
            .iter()
            .rev()
            .find(|attempt| &attempt.graph_node_id == node)
        {
            Some(attempt) => attempt.number.checked_next(),
            None => Ok(AttemptNumberV2::FIRST),
        }
    }
}

fn terminal_lifecycle(prior_terminal: AdvanceTerminalV2) -> AttemptLifecycle {
    match prior_terminal {
        AdvanceTerminalV2::Completed => AttemptLifecycle::Completed,
        AdvanceTerminalV2::Skipped => AttemptLifecycle::Skipped,
    }
}

fn validate_reconstruction(attempts: &[SessionAttemptV2]) -> Result<(), DomainError> {
    let mut ids = BTreeSet::new();
    let mut per_node_number: BTreeMap<GraphNodeId, AttemptNumberV2> = BTreeMap::new();
    let mut per_node_valid: BTreeSet<GraphNodeId> = BTreeSet::new();
    let mut next_trace = TraceSequenceV2::FIRST;
    for attempt in attempts {
        if attempt.trace != next_trace {
            return Err(invalid(
                "trace sequence numbers must start at FIRST and be consecutive",
            ));
        }
        next_trace = next_trace.checked_next()?;
        if matches!(attempt.goal_revision, Some(goal) if goal < GoalRevisionNumberV2::FIRST) {
            return Err(invalid("goal revision must be nonzero"));
        }
        if !ids.insert(attempt.attempt_id.clone()) {
            return Err(invalid("attempt identifiers must be globally unique"));
        }
        match per_node_number.get(&attempt.graph_node_id).copied() {
            None => {
                if attempt.number != AttemptNumberV2::FIRST {
                    return Err(invalid("a node's first attempt number must be FIRST"));
                }
            }
            Some(previous) => {
                if attempt.number != previous.checked_next()? {
                    return Err(invalid(
                        "attempt numbers must increase by one per graph node",
                    ));
                }
            }
        }
        per_node_number.insert(attempt.graph_node_id.clone(), attempt.number);
        if attempt.validity.is_valid() && !per_node_valid.insert(attempt.graph_node_id.clone()) {
            return Err(invalid("a graph node has more than one valid attempt"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionId {
        SessionId::new("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn node(value: &str) -> GraphNodeId {
        GraphNodeId::new(value).unwrap()
    }

    fn attempt_id(n: u64) -> AttemptId {
        AttemptId::new(format!("00000000-0000-0000-0000-{n:012x}")).unwrap()
    }

    fn active(
        id: u64,
        graph_node: &str,
        number: u64,
        trace: u64,
        goal: Option<u64>,
    ) -> SessionAttemptV2 {
        SessionAttemptV2::new(
            attempt_id(id),
            node(graph_node),
            AttemptNumberV2::new(number),
            TraceSequenceV2::new(trace),
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
            goal.map(GoalRevisionNumberV2::new),
        )
        .unwrap()
    }

    fn terminal(
        id: u64,
        graph_node: &str,
        number: u64,
        trace: u64,
        lifecycle: AttemptLifecycle,
        validity: AttemptValidityV2,
        goal: Option<u64>,
    ) -> SessionAttemptV2 {
        SessionAttemptV2::new(
            attempt_id(id),
            node(graph_node),
            AttemptNumberV2::new(number),
            TraceSequenceV2::new(trace),
            lifecycle,
            validity,
            goal.map(GoalRevisionNumberV2::new),
        )
        .unwrap()
    }

    fn identity(
        attempt: &SessionAttemptV2,
    ) -> (
        AttemptId,
        GraphNodeId,
        AttemptNumberV2,
        TraceSequenceV2,
        Option<GoalRevisionNumberV2>,
    ) {
        (
            attempt.attempt_id().clone(),
            attempt.graph_node_id().clone(),
            attempt.number(),
            attempt.trace(),
            attempt.goal_revision(),
        )
    }

    fn assert_unchanged(
        before: &SessionTraceV2,
        result: Result<(), DomainError>,
        after: &SessionTraceV2,
    ) {
        assert!(result.is_err(), "expected Err, got Ok");
        assert_eq!(after, before, "trace changed after a failed transition");
    }

    #[test]
    fn number_types_are_monotonic_with_first_one_and_checked_overflow() {
        assert_eq!(TraceSequenceV2::FIRST, TraceSequenceV2::new(1));
        assert_eq!(AttemptNumberV2::FIRST, AttemptNumberV2::new(1));
        assert_eq!(GoalRevisionNumberV2::FIRST, GoalRevisionNumberV2::new(1));
        assert_eq!(
            AttemptNumberV2::new(2).checked_next().unwrap(),
            AttemptNumberV2::new(3)
        );
        assert_eq!(
            TraceSequenceV2::new(u64::MAX).checked_next(),
            Err(DomainError::RevisionOverflow {
                revision: Revision::new(u64::MAX)
            })
        );
    }

    #[test]
    fn attempt_new_rejects_zero_identity_and_invalid_lifecycle_links() {
        let build = |number: u64, trace: u64, lifecycle, validity, goal: Option<u64>| {
            SessionAttemptV2::new(
                attempt_id(1),
                node("n"),
                AttemptNumberV2::new(number),
                TraceSequenceV2::new(trace),
                lifecycle,
                validity,
                goal.map(GoalRevisionNumberV2::new),
            )
        };
        assert!(
            build(
                1,
                1,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                None
            )
            .is_ok()
        );
        assert_eq!(
            build(
                0,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                None
            )
            .unwrap_err(),
            invalid("attempt number must be nonzero")
        );
        assert_eq!(
            build(
                1,
                0,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                None
            )
            .unwrap_err(),
            invalid("trace sequence must be nonzero")
        );
        assert_eq!(
            build(
                1,
                1,
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                Some(0)
            )
            .unwrap_err(),
            invalid("goal revision must be nonzero")
        );
        assert_eq!(
            build(
                1,
                1,
                AttemptLifecycle::Active,
                AttemptValidityV2::Stale,
                None
            )
            .unwrap_err(),
            invalid("an active attempt must be valid")
        );
        assert_eq!(
            build(
                1,
                1,
                AttemptLifecycle::Abandoned,
                AttemptValidityV2::Valid,
                None
            )
            .unwrap_err(),
            invalid("an abandoned attempt must be stale")
        );
    }

    #[test]
    fn from_parts_accepts_running_and_terminal_shapes() {
        let running = SessionTraceV2::from_parts(
            session(),
            SessionLifecycle::Running,
            Revision::new(3),
            vec![active(1, "entry", 1, 1, None)],
        )
        .unwrap();
        assert_eq!(
            running.active_attempt().unwrap().attempt_id(),
            &attempt_id(1)
        );
        assert_eq!(
            running.active_cursor().unwrap().graph_node_id(),
            &node("entry")
        );
        assert_eq!(running.revision(), Revision::new(3));

        let completed = SessionTraceV2::from_parts(
            session(),
            SessionLifecycle::Completed,
            Revision::ZERO,
            vec![terminal(
                1,
                "terminal",
                1,
                1,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                None,
            )],
        )
        .unwrap();
        assert!(completed.active_attempt().is_none());

        let cancelled = SessionTraceV2::from_parts(
            session(),
            SessionLifecycle::Cancelled,
            Revision::ZERO,
            vec![terminal(
                1,
                "entry",
                1,
                1,
                AttemptLifecycle::Abandoned,
                AttemptValidityV2::Stale,
                None,
            )],
        )
        .unwrap();
        assert!(cancelled.active_cursor().is_none());
    }

    #[test]
    fn from_parts_rejects_invalid_reachable_sequences() {
        let running = SessionLifecycle::Running;
        let completed = |id, graph_node, number, trace, validity| {
            terminal(
                id,
                graph_node,
                number,
                trace,
                AttemptLifecycle::Completed,
                validity,
                None,
            )
        };

        // trace gap (1 then 3). Zero trace/number is rejected earlier at `SessionAttemptV2::new`
        // and therefore cannot reach this constructor through the public API.
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![
                    completed(1, "a", 1, 1, AttemptValidityV2::Valid),
                    active(2, "b", 1, 3, None),
                ],
            ),
            Err(invalid(
                "trace sequence numbers must start at FIRST and be consecutive"
            ))
        );
        // per-node first attempt number not FIRST.
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![
                    completed(1, "a", 2, 1, AttemptValidityV2::Stale),
                    active(2, "a", 1, 2, None),
                ],
            ),
            Err(invalid("a node's first attempt number must be FIRST"))
        );
        // per-node attempt-number gap (#1 then #3).
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![
                    completed(1, "a", 1, 1, AttemptValidityV2::Stale),
                    active(2, "a", 3, 2, None),
                ],
            ),
            Err(invalid(
                "attempt numbers must increase by one per graph node"
            ))
        );
        // duplicate attempt id.
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![
                    completed(1, "a", 1, 1, AttemptValidityV2::Stale),
                    active(1, "b", 1, 2, None),
                ],
            ),
            Err(invalid("attempt identifiers must be globally unique"))
        );
        // one node with two valid attempts.
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![
                    completed(1, "a", 1, 1, AttemptValidityV2::Valid),
                    active(2, "a", 2, 2, None),
                ],
            ),
            Err(invalid("a graph node has more than one valid attempt"))
        );
    }

    #[test]
    fn from_parts_rejects_active_count_inconsistent_with_lifecycle() {
        let running = SessionLifecycle::Running;
        let completed = |id, graph_node, trace| {
            terminal(
                id,
                graph_node,
                1,
                trace,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                None,
            )
        };
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![completed(1, "a", 1)]
            ),
            Err(invalid(
                "a running session requires exactly one active attempt"
            ))
        );
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![active(1, "a", 1, 1, None), active(2, "b", 1, 2, None)],
            ),
            Err(invalid(
                "a running session requires exactly one active attempt"
            ))
        );
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                running,
                Revision::ZERO,
                vec![active(1, "a", 1, 1, None), completed(2, "b", 2)],
            ),
            Err(invalid("the active attempt must be the last trace member"))
        );
        assert_eq!(
            SessionTraceV2::from_parts(
                session(),
                SessionLifecycle::Completed,
                Revision::ZERO,
                vec![active(1, "a", 1, 1, None)],
            ),
            Err(invalid("a terminal session must have no active attempt"))
        );
    }

    #[test]
    fn start_advances_and_preserves_immutable_prior_attempts() {
        let mut trace =
            SessionTraceV2::start(session(), node("entry"), attempt_id(1), None).unwrap();
        let entry_identity = identity(trace.active_attempt().unwrap());

        trace
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("middle"),
                attempt_id(2),
                None,
            )
            .unwrap();
        assert_eq!(trace.attempts().len(), 2);
        assert_eq!(trace.active_attempt().unwrap().attempt_id(), &attempt_id(2));
        assert_eq!(
            trace.active_cursor().unwrap().trace(),
            TraceSequenceV2::new(2)
        );
        assert_eq!(trace.attempts()[0].lifecycle(), AttemptLifecycle::Completed);
        assert_eq!(identity(&trace.attempts()[0]), entry_identity);
        assert_eq!(trace.revision(), Revision::new(1));
    }

    #[test]
    fn finish_completes_session_with_no_active() {
        let mut trace =
            SessionTraceV2::start(session(), node("entry"), attempt_id(1), None).unwrap();
        trace
            .finish(&attempt_id(1), AdvanceTerminalV2::Skipped)
            .unwrap();
        assert_eq!(trace.lifecycle(), SessionLifecycle::Completed);
        assert!(trace.active_attempt().is_none());
        assert!(trace.active_cursor().is_none());
        assert_eq!(
            trace.attempts().last().unwrap().lifecycle(),
            AttemptLifecycle::Skipped
        );
    }

    #[test]
    fn retry_stales_only_the_active_attempt_and_repeats_the_same_node() {
        let mut trace =
            SessionTraceV2::start(session(), node("entry"), attempt_id(1), None).unwrap();
        trace
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("review"),
                attempt_id(2),
                Some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap();
        trace
            .retry(
                &attempt_id(2),
                attempt_id(3),
                Some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap();

        assert_eq!(trace.revision(), Revision::new(2));
        assert_eq!(trace.attempts()[0].validity(), AttemptValidityV2::Valid);
        assert_eq!(trace.attempts()[1].lifecycle(), AttemptLifecycle::Abandoned);
        assert_eq!(trace.attempts()[1].validity(), AttemptValidityV2::Stale);
        let fresh = trace.active_attempt().unwrap();
        assert_eq!(fresh.graph_node_id(), &node("review"));
        assert_eq!(fresh.attempt_id(), &attempt_id(3));
        assert_eq!(fresh.number(), AttemptNumberV2::new(2));
        assert_eq!(fresh.trace(), TraceSequenceV2::new(3));
        assert_eq!(fresh.goal_revision(), Some(GoalRevisionNumberV2::FIRST));
    }

    #[test]
    fn rework_to_stales_suffix_and_activates_fresh_target() {
        let mut trace =
            SessionTraceV2::start(session(), node("implement"), attempt_id(1), None).unwrap();
        trace
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("test"),
                attempt_id(2),
                None,
            )
            .unwrap();
        trace
            .advance(
                &attempt_id(2),
                AdvanceTerminalV2::Completed,
                node("decide"),
                attempt_id(3),
                None,
            )
            .unwrap();
        let pre: Vec<_> = trace.attempts().iter().map(identity).collect();

        trace
            .rework_to(&attempt_id(3), node("implement"), attempt_id(4), None)
            .unwrap();
        assert_eq!(trace.active_attempt().unwrap().attempt_id(), &attempt_id(4));
        assert_eq!(
            trace.active_attempt().unwrap().number(),
            AttemptNumberV2::new(2)
        );
        assert_eq!(trace.attempts()[0].validity(), AttemptValidityV2::Stale);
        assert_eq!(trace.attempts()[1].validity(), AttemptValidityV2::Stale);
        assert_eq!(trace.attempts()[2].validity(), AttemptValidityV2::Stale);
        assert_eq!(trace.attempts()[2].lifecycle(), AttemptLifecycle::Completed);
        assert_eq!(
            trace
                .attempts()
                .iter()
                .filter(|a| a.validity().is_valid())
                .count(),
            1
        );
        assert_eq!(identity(&trace.attempts()[0]), pre[0]);
        assert_eq!(identity(&trace.attempts()[1]), pre[1]);
        assert_eq!(identity(&trace.attempts()[2]), pre[2]);
    }

    #[test]
    fn manual_rework_unifies_running_reentry_and_completed_reactivation() {
        let mut running =
            SessionTraceV2::start(session(), node("implement"), attempt_id(1), None).unwrap();
        running
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("finish"),
                attempt_id(2),
                None,
            )
            .unwrap();
        running
            .manual_rework(Some(&attempt_id(2)), node("finish"), attempt_id(3), None)
            .unwrap();
        assert_eq!(running.lifecycle(), SessionLifecycle::Running);
        assert_eq!(running.attempts()[1].validity(), AttemptValidityV2::Stale);
        assert_eq!(
            running.active_attempt().unwrap().attempt_id(),
            &attempt_id(3)
        );

        let mut completed =
            SessionTraceV2::start(session(), node("implement"), attempt_id(1), None).unwrap();
        completed
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("finish"),
                attempt_id(2),
                None,
            )
            .unwrap();
        completed
            .finish(&attempt_id(2), AdvanceTerminalV2::Completed)
            .unwrap();
        completed
            .manual_rework(None, node("implement"), attempt_id(3), None)
            .unwrap();
        assert_eq!(completed.lifecycle(), SessionLifecycle::Running);
        assert!(
            completed.attempts()[..2]
                .iter()
                .all(|attempt| attempt.validity() == AttemptValidityV2::Stale)
        );
        assert_eq!(
            completed.active_attempt().unwrap().attempt_id(),
            &attempt_id(3)
        );
        assert_eq!(completed.revision(), Revision::new(3));
    }

    #[test]
    fn repeated_valid_rework_cycle_has_no_semantic_traversal_limit_and_one_cursor() {
        let mut trace =
            SessionTraceV2::start(session(), node("implement"), attempt_id(1), None).unwrap();
        trace
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("review"),
                attempt_id(2),
                None,
            )
            .unwrap();

        for traversal in 0..128_u64 {
            let review_attempt = trace.active_attempt().unwrap().attempt_id().clone();
            let implement_attempt = attempt_id(3 + traversal * 2);
            trace
                .rework_to(
                    &review_attempt,
                    node("implement"),
                    implement_attempt.clone(),
                    None,
                )
                .unwrap();
            let review_successor = attempt_id(4 + traversal * 2);
            trace
                .advance(
                    &implement_attempt,
                    AdvanceTerminalV2::Completed,
                    node("review"),
                    review_successor,
                    None,
                )
                .unwrap();
            assert_eq!(
                trace
                    .attempts()
                    .iter()
                    .filter(|attempt| attempt.lifecycle() == AttemptLifecycle::Active)
                    .count(),
                1
            );
        }

        assert_eq!(trace.attempts().len(), 258);
        let cursor = trace.active_cursor().unwrap();
        assert_eq!(cursor.graph_node_id().as_str(), "review");
        let active = trace.active_attempt().unwrap();
        assert_eq!(active.number(), AttemptNumberV2::new(129));
        assert_eq!(active.trace(), TraceSequenceV2::new(258));
        assert_eq!(
            trace
                .attempts()
                .iter()
                .filter(|attempt| attempt.validity().is_valid())
                .count(),
            2
        );
    }

    #[test]
    fn failures_leave_trace_bit_for_bit_unchanged() {
        let mut trace =
            SessionTraceV2::start(session(), node("entry"), attempt_id(1), None).unwrap();
        trace
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("middle"),
                attempt_id(2),
                None,
            )
            .unwrap();
        let active = trace.active_attempt().unwrap().attempt_id().clone();
        let wrong = attempt_id(0xdead);
        let before = trace.clone();

        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.advance(
                &wrong,
                AdvanceTerminalV2::Completed,
                node("late"),
                attempt_id(3),
                None,
            ),
            &copy,
        );
        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("late"),
                attempt_id(3),
                None,
            ),
            &copy,
        );
        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.advance(
                &active,
                AdvanceTerminalV2::Completed,
                node("middle"),
                attempt_id(3),
                None,
            ),
            &copy,
        );
        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.advance(
                &active,
                AdvanceTerminalV2::Completed,
                node("late"),
                attempt_id(1),
                None,
            ),
            &copy,
        );
        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.rework_to(&wrong, node("entry"), attempt_id(3), None),
            &copy,
        );
        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.rework_to(&active, node("never"), attempt_id(3), None),
            &copy,
        );
        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.rework_to(&active, node("entry"), attempt_id(2), None),
            &copy,
        );
        let mut copy = trace.clone();
        assert_unchanged(
            &before,
            copy.finish(&wrong, AdvanceTerminalV2::Completed),
            &copy,
        );

        let mut completed = trace.clone();
        completed
            .finish(&active, AdvanceTerminalV2::Completed)
            .unwrap();
        let completed_before = completed.clone();
        let mut copy = completed.clone();
        assert_unchanged(
            &completed_before,
            copy.advance(
                &active,
                AdvanceTerminalV2::Completed,
                node("late"),
                attempt_id(9),
                None,
            ),
            &copy,
        );
        let mut copy = completed.clone();
        assert_unchanged(
            &completed_before,
            copy.rework_to(&active, node("entry"), attempt_id(9), None),
            &copy,
        );
        let mut copy = completed.clone();
        assert_unchanged(
            &completed_before,
            copy.finish(&active, AdvanceTerminalV2::Completed),
            &copy,
        );
    }

    #[test]
    fn revision_overflow_fails_before_mutation() {
        let maxed = SessionTraceV2::from_parts(
            session(),
            SessionLifecycle::Running,
            Revision::new(u64::MAX),
            vec![active(1, "entry", 1, 1, None)],
        )
        .unwrap();
        let before = maxed.clone();
        let overflow = DomainError::RevisionOverflow {
            revision: Revision::new(u64::MAX),
        };
        let mut copy = maxed.clone();
        assert_eq!(
            copy.finish(&attempt_id(1), AdvanceTerminalV2::Completed)
                .unwrap_err(),
            overflow
        );
        let mut copy = maxed.clone();
        assert_eq!(
            copy.advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("late"),
                attempt_id(2),
                None
            )
            .unwrap_err(),
            overflow
        );
        let mut copy = maxed.clone();
        assert_eq!(
            copy.rework_to(&attempt_id(1), node("entry"), attempt_id(2), None)
                .unwrap_err(),
            overflow
        );
        assert_eq!(maxed, before);
    }

    #[test]
    fn arbitrary_transition_sequences_preserve_the_single_cursor_invariant() {
        let entry = node("entry");
        let middle = node("middle");
        let terminal = node("terminal");
        let start = SessionTraceV2::start(session(), entry.clone(), attempt_id(1), None).unwrap();
        let mut fresh = 2u64;
        let mut next_id = || {
            let id = attempt_id(fresh);
            fresh += 1;
            id
        };
        explore(&start, &mut next_id, &entry, &middle, &terminal, 0, 6);
    }

    #[test]
    fn manual_rework_preserves_the_exact_suffix_for_every_valid_target() {
        let mut running =
            SessionTraceV2::start(session(), node("entry"), attempt_id(1), None).unwrap();
        running
            .advance(
                &attempt_id(1),
                AdvanceTerminalV2::Completed,
                node("middle"),
                attempt_id(2),
                None,
            )
            .unwrap();
        running
            .advance(
                &attempt_id(2),
                AdvanceTerminalV2::Skipped,
                node("terminal"),
                attempt_id(3),
                None,
            )
            .unwrap();
        running
            .rework_to(&attempt_id(3), node("entry"), attempt_id(4), None)
            .unwrap();
        running
            .advance(
                &attempt_id(4),
                AdvanceTerminalV2::Completed,
                node("middle"),
                attempt_id(5),
                None,
            )
            .unwrap();
        running
            .advance(
                &attempt_id(5),
                AdvanceTerminalV2::Completed,
                node("terminal"),
                attempt_id(6),
                None,
            )
            .unwrap();

        let running_targets = valid_targets(&running);
        for (offset, target) in running_targets.into_iter().enumerate() {
            let before = running.clone();
            let source = before.active_attempt().unwrap().attempt_id().clone();
            let fresh_id = attempt_id(100 + offset as u64);
            let mut after = before.clone();
            after
                .manual_rework(Some(&source), target.clone(), fresh_id.clone(), None)
                .unwrap();
            assert_exact_suffix_reentry(
                &before,
                &after,
                &target,
                &fresh_id,
                Some(AttemptLifecycle::Abandoned),
            );
        }

        let mut completed = running;
        completed
            .finish(&attempt_id(6), AdvanceTerminalV2::Completed)
            .unwrap();
        let completed_targets = valid_targets(&completed);
        for (offset, target) in completed_targets.into_iter().enumerate() {
            let before = completed.clone();
            let fresh_id = attempt_id(200 + offset as u64);
            let mut after = before.clone();
            after
                .manual_rework(None, target.clone(), fresh_id.clone(), None)
                .unwrap();
            assert_exact_suffix_reentry(&before, &after, &target, &fresh_id, None);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn explore(
        trace: &SessionTraceV2,
        next_id: &mut dyn FnMut() -> AttemptId,
        entry: &GraphNodeId,
        middle: &GraphNodeId,
        terminal: &GraphNodeId,
        depth: usize,
        max_depth: usize,
    ) {
        assert_invariants(trace);
        if depth == max_depth || trace.lifecycle() != SessionLifecycle::Running {
            return;
        }
        let active_id = trace.active_attempt().unwrap().attempt_id().clone();
        let active_node = trace.active_attempt().unwrap().graph_node_id().clone();

        let advance_target = match &active_node {
            n if n == entry => Some(middle.clone()),
            n if n == middle => Some(terminal.clone()),
            _ => None,
        };
        if let Some(target) = advance_target {
            let mut next = trace.clone();
            next.advance(
                &active_id,
                AdvanceTerminalV2::Completed,
                target,
                next_id(),
                None,
            )
            .expect("advance along the chain is rollback-safe and cannot fail here");
            explore(
                &next,
                next_id,
                entry,
                middle,
                terminal,
                depth + 1,
                max_depth,
            );
        }

        let mut finished = trace.clone();
        finished
            .finish(&active_id, AdvanceTerminalV2::Completed)
            .expect("finish is rollback-safe and cannot fail here");
        assert_invariants(&finished);

        // rework to the active's own node: degenerate same-node fresh-attempt case.
        let mut same = trace.clone();
        let same_fresh_id = next_id();
        if same
            .rework_to(&active_id, active_node.clone(), same_fresh_id.clone(), None)
            .is_ok()
        {
            assert_exact_suffix_reentry(
                trace,
                &same,
                &active_node,
                &same_fresh_id,
                Some(AttemptLifecycle::Completed),
            );
            assert_invariants(&same);
            explore(
                &same,
                next_id,
                entry,
                middle,
                terminal,
                depth + 1,
                max_depth,
            );
        } else {
            assert_eq!(same, *trace, "rejected rework changed the trace");
        }

        for target in [entry.clone(), middle.clone()] {
            let on_trace = trace.attempts().iter().any(|a| {
                a.validity().is_valid() && *a.graph_node_id() == target && target != active_node
            });
            if !on_trace {
                continue;
            }
            let mut reworked = trace.clone();
            let fresh_id = next_id();
            if reworked
                .rework_to(&active_id, target.clone(), fresh_id.clone(), None)
                .is_ok()
            {
                assert_exact_suffix_reentry(
                    trace,
                    &reworked,
                    &target,
                    &fresh_id,
                    Some(AttemptLifecycle::Completed),
                );
                assert_invariants(&reworked);
                explore(
                    &reworked,
                    next_id,
                    entry,
                    middle,
                    terminal,
                    depth + 1,
                    max_depth,
                );
            } else {
                assert_eq!(reworked, *trace, "rejected rework changed the trace");
            }
        }
    }

    fn valid_targets(trace: &SessionTraceV2) -> Vec<GraphNodeId> {
        trace
            .attempts()
            .iter()
            .filter(|attempt| attempt.validity().is_valid())
            .map(|attempt| attempt.graph_node_id().clone())
            .collect()
    }

    fn assert_exact_suffix_reentry(
        before: &SessionTraceV2,
        after: &SessionTraceV2,
        target: &GraphNodeId,
        fresh_id: &AttemptId,
        source_lifecycle: Option<AttemptLifecycle>,
    ) {
        let target_trace = before
            .attempts()
            .iter()
            .find(|attempt| attempt.validity().is_valid() && attempt.graph_node_id() == target)
            .expect("a successful rework target must have been valid")
            .trace();
        let source_index = before
            .attempts()
            .iter()
            .position(SessionAttemptV2::is_active);
        assert_eq!(
            source_index.is_some(),
            source_lifecycle.is_some(),
            "only running rework terminalizes a causal source attempt"
        );

        assert_eq!(after.session_id(), before.session_id());
        assert_eq!(after.lifecycle(), SessionLifecycle::Running);
        assert_eq!(
            after.revision(),
            before.revision().checked_next().unwrap(),
            "re-entry must advance the session revision exactly once"
        );
        assert_eq!(after.attempts().len(), before.attempts().len() + 1);

        for (index, (prior, current)) in before.attempts().iter().zip(after.attempts()).enumerate()
        {
            assert_eq!(
                identity(current),
                identity(prior),
                "re-entry changed pre-existing attempt identity at index {index}"
            );
            let expected_lifecycle = if Some(index) == source_index {
                source_lifecycle.expect("running source lifecycle is specified")
            } else {
                prior.lifecycle()
            };
            assert_eq!(
                current.lifecycle(),
                expected_lifecycle,
                "re-entry changed a non-causal lifecycle at index {index}"
            );
            let expected_validity = if prior.validity().is_valid() && prior.trace() >= target_trace
            {
                AttemptValidityV2::Stale
            } else {
                prior.validity()
            };
            assert_eq!(
                current.validity(),
                expected_validity,
                "re-entry did not invalidate exactly the valid target suffix at index {index}"
            );
            if prior.trace() < target_trace {
                assert_eq!(
                    current, prior,
                    "the trace prefix before the target must be preserved bit for bit"
                );
            }
        }

        let prior_target_number = before
            .attempts()
            .iter()
            .filter(|attempt| attempt.graph_node_id() == target)
            .map(SessionAttemptV2::number)
            .max()
            .expect("the target has a prior attempt");
        let prior_last_trace = before
            .attempts()
            .last()
            .expect("a session has at least one attempt")
            .trace();
        let fresh = after.attempts().last().unwrap();
        assert_eq!(fresh.attempt_id(), fresh_id);
        assert_eq!(fresh.graph_node_id(), target);
        assert_eq!(
            fresh.number(),
            prior_target_number.checked_next().unwrap(),
            "the fresh target attempt number must follow its complete history"
        );
        assert_eq!(
            fresh.trace(),
            prior_last_trace.checked_next().unwrap(),
            "the fresh attempt must append exactly one trace position"
        );
        assert_eq!(fresh.lifecycle(), AttemptLifecycle::Active);
        assert_eq!(fresh.validity(), AttemptValidityV2::Valid);
        assert_eq!(fresh.goal_revision(), None);
        assert_eq!(after.active_attempt(), Some(fresh));
        assert_eq!(
            after
                .attempts()
                .iter()
                .filter(|attempt| attempt.is_active())
                .count(),
            1,
            "the fresh target attempt must be the sole active cursor"
        );
        assert_invariants(after);
    }

    fn assert_invariants(trace: &SessionTraceV2) {
        let active_count = trace.attempts().iter().filter(|a| a.is_active()).count();
        match trace.lifecycle() {
            SessionLifecycle::Running => {
                assert_eq!(
                    active_count, 1,
                    "running session must have exactly one active"
                );
                assert!(
                    trace.attempts().last().unwrap().is_active(),
                    "the active attempt must be the last trace member"
                );
                let active = trace.active_attempt().unwrap();
                assert_eq!(active.validity(), AttemptValidityV2::Valid);
                let cursor = trace.active_cursor().unwrap();
                assert_eq!(cursor.attempt_id(), active.attempt_id());
                assert_eq!(cursor.graph_node_id(), active.graph_node_id());
                assert_eq!(cursor.trace(), active.trace());
            }
            SessionLifecycle::Completed | SessionLifecycle::Cancelled => {
                assert_eq!(
                    active_count, 0,
                    "terminal session must have no active attempt"
                );
                assert!(trace.active_cursor().is_none());
            }
        }
        let mut valid_nodes = BTreeSet::new();
        let mut prev_trace = None;
        for attempt in trace.attempts() {
            if attempt.lifecycle() == AttemptLifecycle::Active {
                assert_eq!(attempt.validity(), AttemptValidityV2::Valid);
            }
            if attempt.lifecycle() == AttemptLifecycle::Abandoned {
                assert_eq!(attempt.validity(), AttemptValidityV2::Stale);
            }
            if attempt.validity().is_valid() && !valid_nodes.insert(attempt.graph_node_id().clone())
            {
                panic!("a graph node has more than one valid attempt");
            }
            if let Some(previous) = prev_trace {
                assert!(
                    previous < attempt.trace(),
                    "trace must be strictly increasing"
                );
            }
            prev_trace = Some(attempt.trace());
        }
    }
}
