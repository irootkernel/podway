//! Authoritative daemon-owned waiting and job reads for Procedure v2 workspaces.

use std::{error::Error, fmt};

use podway_core::SessionId;
use podway_store::{
    DurableWorktreeIdentityV1, GraphWorkspaceViewV2, JobIdV1, JobListQueryV1, JobStateV1,
    JobViewV1, StoreErrorV1, StoreGraphReadContractV2, StoreReadContractV1,
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MonotonicDeadlineV1(u64);

impl MonotonicDeadlineV1 {
    pub const fn new(millis: u64) -> Self {
        Self(millis)
    }

    pub const fn millis(self) -> u64 {
        self.0
    }

    pub fn after(
        clock: &impl MonotonicClockV1,
        timeout_millis: u64,
    ) -> Result<Self, ReadServiceErrorV1> {
        clock
            .now_millis()
            .checked_add(timeout_millis)
            .map(Self)
            .ok_or(ReadServiceErrorV1::DeadlineOverflow)
    }
}

pub trait MonotonicClockV1: Send + Sync {
    fn now_millis(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReadNotificationVersionV1(u64);

impl ReadNotificationVersionV1 {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadWaitOutcomeV1 {
    Notified,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadNotificationErrorV1 {
    Unavailable,
}

pub trait ReadNotificationV1: Send + Sync {
    fn observe(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<ReadNotificationVersionV1, ReadNotificationErrorV1>;

    fn wait_for_change(
        &self,
        identity: &DurableWorktreeIdentityV1,
        observed: ReadNotificationVersionV1,
        deadline: MonotonicDeadlineV1,
    ) -> Result<ReadWaitOutcomeV1, ReadNotificationErrorV1>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadWaitV1 {
    Immediate,
    IdleUntil(MonotonicDeadlineV1),
    AfterJobUntil {
        job_id: JobIdV1,
        deadline: MonotonicDeadlineV1,
    },
}

impl ReadWaitV1 {
    pub const fn immediate() -> Self {
        Self::Immediate
    }

    pub const fn idle_until(deadline: MonotonicDeadlineV1) -> Self {
        Self::IdleUntil(deadline)
    }

    pub fn after_job_until(job_id: JobIdV1, deadline: MonotonicDeadlineV1) -> Self {
        Self::AfterJobUntil { job_id, deadline }
    }

    const fn deadline(&self) -> Option<MonotonicDeadlineV1> {
        match self {
            Self::Immediate => None,
            Self::IdleUntil(deadline) | Self::AfterJobUntil { deadline, .. } => Some(*deadline),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadServiceErrorV1 {
    DeadlineOverflow,
    InconsistentState {
        reason: &'static str,
    },
    JobNotFound {
        job_id: JobIdV1,
    },
    MissingSession,
    SessionIdentityMismatch {
        expected: SessionId,
        actual: Option<SessionId>,
    },
    Notification(ReadNotificationErrorV1),
    ResultShapeConversion,
    Store(StoreErrorV1),
    TimestampOutOfRange,
    WaitTimedOut,
}

impl fmt::Display for ReadServiceErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeadlineOverflow => formatter.write_str("monotonic read deadline overflowed"),
            Self::InconsistentState { reason } => {
                write!(formatter, "inconsistent durable workspace state: {reason}")
            }
            Self::JobNotFound { job_id } => {
                write!(formatter, "job {} was not found", job_id.as_str())
            }
            Self::MissingSession => formatter.write_str("workspace has no current session"),
            Self::SessionIdentityMismatch { .. } => {
                formatter.write_str("current session does not match the requested identity")
            }
            Self::Notification(_) => formatter.write_str("read notification source is unavailable"),
            Self::ResultShapeConversion => {
                formatter.write_str("read projection does not satisfy protocol shape")
            }
            Self::Store(error) => error.fmt(formatter),
            Self::TimestampOutOfRange => {
                formatter.write_str("durable timestamp is outside the protocol timestamp range")
            }
            Self::WaitTimedOut => {
                formatter.write_str("read wait timed out before its predicate became true")
            }
        }
    }
}

impl Error for ReadServiceErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

pub struct AuthoritativeReadServiceV1<Store, Notifications, Clock> {
    store: Store,
    notifications: Notifications,
    clock: Clock,
}

impl<Store, Notifications, Clock> AuthoritativeReadServiceV1<Store, Notifications, Clock>
where
    Store: StoreReadContractV1,
    Notifications: ReadNotificationV1,
    Clock: MonotonicClockV1,
{
    pub fn new(store: Store, notifications: Notifications, clock: Clock) -> Self {
        Self {
            store,
            notifications,
            clock,
        }
    }

    pub fn job(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job_id: &JobIdV1,
        wait: ReadWaitV1,
    ) -> Result<JobViewV1, ReadServiceErrorV1> {
        match wait {
            ReadWaitV1::Immediate => self.read_job(identity, job_id),
            ReadWaitV1::AfterJobUntil {
                job_id: waited,
                deadline,
            } if &waited == job_id => self.wait_for_terminal_job(identity, job_id, deadline),
            ReadWaitV1::AfterJobUntil { .. } => Err(ReadServiceErrorV1::InconsistentState {
                reason: "named job wait target does not match the requested job",
            }),
            ReadWaitV1::IdleUntil(_) => Err(ReadServiceErrorV1::InconsistentState {
                reason: "named job reads cannot wait for workspace idleness",
            }),
        }
    }

    pub fn list_jobs(
        &self,
        identity: &DurableWorktreeIdentityV1,
        query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, ReadServiceErrorV1> {
        self.store
            .list_jobs(identity, query)
            .map_err(ReadServiceErrorV1::Store)
    }

    fn wait_for_terminal_job(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job_id: &JobIdV1,
        deadline: MonotonicDeadlineV1,
    ) -> Result<JobViewV1, ReadServiceErrorV1> {
        loop {
            let observed = self
                .notifications
                .observe(identity)
                .map_err(ReadServiceErrorV1::Notification)?;
            let job = self.read_job(identity, job_id)?;
            if is_terminal_job(job.state()) {
                return Ok(job);
            }
            if self.clock.now_millis() >= deadline.millis() {
                return Err(ReadServiceErrorV1::WaitTimedOut);
            }
            if self
                .notifications
                .wait_for_change(identity, observed, deadline)
                .map_err(ReadServiceErrorV1::Notification)?
                == ReadWaitOutcomeV1::TimedOut
            {
                let job = self.read_job(identity, job_id)?;
                return is_terminal_job(job.state())
                    .then_some(job)
                    .ok_or(ReadServiceErrorV1::WaitTimedOut);
            }
        }
    }

    fn read_job(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job_id: &JobIdV1,
    ) -> Result<JobViewV1, ReadServiceErrorV1> {
        self.store
            .read_job(identity, job_id)
            .map_err(ReadServiceErrorV1::Store)?
            .ok_or_else(|| ReadServiceErrorV1::JobNotFound {
                job_id: job_id.clone(),
            })
    }
}

impl<Store, Notifications, Clock> AuthoritativeReadServiceV1<Store, Notifications, Clock>
where
    Store: StoreReadContractV1 + StoreGraphReadContractV2,
    Notifications: ReadNotificationV1,
    Clock: MonotonicClockV1,
{
    pub fn graph_workspace_view_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
        wait: ReadWaitV1,
        expected_session_id: Option<&SessionId>,
    ) -> Result<GraphWorkspaceViewV2, ReadServiceErrorV1> {
        if matches!(wait, ReadWaitV1::Immediate) {
            return self.read_graph_workspace_view_v2(identity, expected_session_id);
        }
        let deadline = wait
            .deadline()
            .ok_or(ReadServiceErrorV1::InconsistentState {
                reason: "a non-immediate graph wait is missing its deadline",
            })?;
        loop {
            let observed = self
                .notifications
                .observe(identity)
                .map_err(ReadServiceErrorV1::Notification)?;
            if let Some(view) =
                self.graph_predicate_satisfied_v2(identity, &wait, expected_session_id)?
            {
                return Ok(view);
            }
            if self.clock.now_millis() >= deadline.millis() {
                return Err(ReadServiceErrorV1::WaitTimedOut);
            }
            if self
                .notifications
                .wait_for_change(identity, observed, deadline)
                .map_err(ReadServiceErrorV1::Notification)?
                == ReadWaitOutcomeV1::TimedOut
            {
                return self
                    .graph_predicate_satisfied_v2(identity, &wait, expected_session_id)?
                    .ok_or(ReadServiceErrorV1::WaitTimedOut);
            }
        }
    }

    fn graph_predicate_satisfied_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
        wait: &ReadWaitV1,
        expected_session_id: Option<&SessionId>,
    ) -> Result<Option<GraphWorkspaceViewV2>, ReadServiceErrorV1> {
        match wait {
            ReadWaitV1::Immediate => self
                .read_graph_workspace_view_v2(identity, expected_session_id)
                .map(Some),
            ReadWaitV1::IdleUntil(_) => {
                let view = self.read_graph_workspace_view_v2(identity, expected_session_id)?;
                Ok(
                    (view.queued_job_count() == 0 && view.running_job_id().is_none())
                        .then_some(view),
                )
            }
            ReadWaitV1::AfterJobUntil { job_id, .. } => {
                let first = self.read_graph_workspace_view_v2(identity, expected_session_id)?;
                let job = self.read_job(identity, job_id)?;
                if is_terminal_job(job.state()) {
                    self.read_graph_workspace_view_v2(identity, expected_session_id)
                        .map(Some)
                } else {
                    drop(first);
                    Ok(None)
                }
            }
        }
    }

    fn read_graph_workspace_view_v2(
        &self,
        identity: &DurableWorktreeIdentityV1,
        expected_session_id: Option<&SessionId>,
    ) -> Result<GraphWorkspaceViewV2, ReadServiceErrorV1> {
        let view = self
            .store
            .read_graph_workspace_view_v2(identity)
            .map_err(ReadServiceErrorV1::Store)?;
        if view.identity() != identity {
            return Err(ReadServiceErrorV1::InconsistentState {
                reason: "Procedure v2 workspace view identity is inconsistent",
            });
        }
        let actual = view
            .graph_state()
            .map(|state| state.trace().session_id().clone());
        if let Some(expected) = expected_session_id
            && actual.as_ref() != Some(expected)
        {
            return Err(ReadServiceErrorV1::SessionIdentityMismatch {
                expected: expected.clone(),
                actual,
            });
        }
        Ok(view)
    }
}

const fn is_terminal_job(state: JobStateV1) -> bool {
    matches!(
        state,
        JobStateV1::Succeeded | JobStateV1::Failed | JobStateV1::Cancelled
    )
}
