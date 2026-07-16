//! Authoritative daemon-owned projections for workspace session and job reads.
//!
//! Store remains the only durable authority. Notification versions only close the race between a
//! predicate read and a wait; every wake-up is followed by another Store read before a result can
//! be returned.

use std::{error::Error, fmt};

use podway_core::{
    ArtifactLocationKindV1, AttemptLifecycle, AttemptV1, DerivedStageStatusV1, ItemTypeV1,
    ItemValueTypeV1, SessionAggregateV1, SessionLifecycle, UnixMillis, derive_next_work_v1,
    derive_session_status_v1,
};
use podway_protocol::{
    AllowedActionsResultV1, AttemptLifecycleResultV1, BlockerResultV1, CommandSuggestionResultV1,
    CurrentAttemptResultV1, ItemTypeResultV1, NextItemResultV1, NextResultV1,
    NextStageAfterCompletionResultV1, NextStageResultV1, PreviousAttemptResultV1, QueueResultV1,
    Rfc3339MillisV1, SessionLifecycleV1, StageStatusResultV1, StatusItemResultV1,
    StatusProcedureV1, StatusResultV1, StatusSessionV1, StatusStageResultV1, StatusTaskV1,
};
use podway_store::{
    DurableWorktreeIdentityV1, JobIdV1, JobListQueryV1, JobStateV1, JobViewV1, StoreErrorV1,
    StoreReadContractV1, WorkspaceViewV1,
};
use serde_json::{Value, json};

/// A monotonic millisecond deadline supplied by the daemon runtime.
///
/// This deliberately has no relationship to persisted UTC timestamps. It is only for bounding a
/// local wait and is therefore injectable in tests.
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

/// The daemon's injectable monotonic clock seam.
pub trait MonotonicClockV1: Send + Sync {
    fn now_millis(&self) -> u64;
}

/// One notification version captured before an authoritative predicate read.
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

/// Whether a notification wait ended because of a hint or because its deadline elapsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadWaitOutcomeV1 {
    Notified,
    TimedOut,
}

/// An unavailable notification source fails a wait closed rather than treating a missing hint as
/// proof that the predicate has changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadNotificationErrorV1 {
    Unavailable,
}

/// The notification boundary for one durable workspace identity.
///
/// Implementations must make `wait_for_change` return promptly when the supplied `observed`
/// version is already stale. They never decide the idle or terminal-job predicate.
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

/// A read either returns immediately or waits for a durable predicate until a monotonic deadline.
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

/// Closed errors from daemon-owned read projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadServiceErrorV1 {
    DeadlineOverflow,
    InconsistentState { reason: &'static str },
    JobNotFound { job_id: JobIdV1 },
    MissingSession,
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

/// Daemon-owned service that projects coherent Store facts into protocol result DTOs.
///
/// `identity` is received only from the daemon's validated resolver boundary. The service never
/// accepts a path or uses any path-derived key.
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

    pub fn status(
        &self,
        identity: &DurableWorktreeIdentityV1,
        wait: ReadWaitV1,
    ) -> Result<StatusResultV1, ReadServiceErrorV1> {
        self.status_with_verbose(identity, wait, false)
    }

    pub fn status_with_verbose(
        &self,
        identity: &DurableWorktreeIdentityV1,
        wait: ReadWaitV1,
        verbose: bool,
    ) -> Result<StatusResultV1, ReadServiceErrorV1> {
        self.read_with_wait(identity, &wait, |view| Self::project_status(view, verbose))
    }

    pub fn next(
        &self,
        identity: &DurableWorktreeIdentityV1,
        wait: ReadWaitV1,
    ) -> Result<NextResultV1, ReadServiceErrorV1> {
        self.read_with_wait(identity, &wait, Self::project_next)
    }
    /// Reads a named job from committed Store state.
    ///
    /// A terminal wait first establishes the terminal predicate through
    /// [`Self::read_with_wait`], then re-reads the job record. The terminal observation and
    /// returned job therefore both come from Store, never from a scheduler notification.
    pub fn job(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job_id: &JobIdV1,
        wait: ReadWaitV1,
    ) -> Result<JobViewV1, ReadServiceErrorV1> {
        match wait {
            ReadWaitV1::Immediate => self.read_job(identity, job_id),
            ReadWaitV1::AfterJobUntil { .. } => {
                self.read_with_wait(identity, &wait, |_| Ok(()))?;
                self.read_job(identity, job_id)
            }
            ReadWaitV1::IdleUntil(_) => Err(ReadServiceErrorV1::InconsistentState {
                reason: "named job reads cannot wait for workspace idleness",
            }),
        }
    }

    /// Lists jobs from one authoritative committed Store snapshot.
    pub fn list_jobs(
        &self,
        identity: &DurableWorktreeIdentityV1,
        query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, ReadServiceErrorV1> {
        self.store
            .list_jobs(identity, query)
            .map_err(ReadServiceErrorV1::Store)
    }

    fn read_with_wait<Output, Project>(
        &self,
        identity: &DurableWorktreeIdentityV1,
        wait: &ReadWaitV1,
        project: Project,
    ) -> Result<Output, ReadServiceErrorV1>
    where
        Project: Fn(&WorkspaceViewV1) -> Result<Output, ReadServiceErrorV1>,
    {
        if matches!(wait, ReadWaitV1::Immediate) {
            return project(&self.read_workspace_view(identity)?);
        }

        let deadline = wait
            .deadline()
            .ok_or(ReadServiceErrorV1::InconsistentState {
                reason: "a non-immediate wait is missing its deadline",
            })?;
        loop {
            // Observe first so a progress event between the subsequent Store read and the wait
            // cannot be lost. A wake-up still supplies no proof and merely restarts this loop.
            let observed = self
                .notifications
                .observe(identity)
                .map_err(ReadServiceErrorV1::Notification)?;

            if let Some(view) = self.predicate_satisfied_view(identity, wait)? {
                return project(&view);
            }
            if self.clock.now_millis() >= deadline.millis() {
                return Err(ReadServiceErrorV1::WaitTimedOut);
            }

            match self
                .notifications
                .wait_for_change(identity, observed, deadline)
                .map_err(ReadServiceErrorV1::Notification)?
            {
                ReadWaitOutcomeV1::Notified => {}
                ReadWaitOutcomeV1::TimedOut => {
                    // Recheck once after a timeout. A terminal commit or queue drain can race the
                    // waiter's deadline notification, but the notification itself is never proof.
                    if let Some(view) = self.predicate_satisfied_view(identity, wait)? {
                        return project(&view);
                    }
                    return Err(ReadServiceErrorV1::WaitTimedOut);
                }
            }
        }
    }

    fn predicate_satisfied_view(
        &self,
        identity: &DurableWorktreeIdentityV1,
        wait: &ReadWaitV1,
    ) -> Result<Option<WorkspaceViewV1>, ReadServiceErrorV1> {
        match wait {
            ReadWaitV1::Immediate => Ok(Some(self.read_workspace_view(identity)?)),
            ReadWaitV1::IdleUntil(_) => {
                let view = self.read_workspace_view(identity)?;
                let pending_mutations =
                    view.queued_job_count() != 0 || view.running_job_id().is_some();
                Ok((!pending_mutations).then_some(view))
            }
            ReadWaitV1::AfterJobUntil { job_id, .. } => {
                let job = self
                    .store
                    .read_job(identity, job_id)
                    .map_err(ReadServiceErrorV1::Store)?
                    .ok_or_else(|| ReadServiceErrorV1::JobNotFound {
                        job_id: job_id.clone(),
                    })?;
                if is_terminal_job(job.state()) {
                    // The job check occurs before this read, so the returned projection cannot
                    // predate the terminal job observation.
                    Ok(Some(self.read_workspace_view(identity)?))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn read_workspace_view(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, ReadServiceErrorV1> {
        let view = self
            .store
            .read_workspace_view(identity)
            .map_err(ReadServiceErrorV1::Store)?;
        validate_workspace_view(&view, identity)?;
        Ok(view)
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

    fn project_status(
        view: &WorkspaceViewV1,
        verbose: bool,
    ) -> Result<StatusResultV1, ReadServiceErrorV1> {
        let aggregate = current_session(view)?;
        let status = derive_session_status_v1(aggregate);
        let current_attempt = active_attempt(aggregate)?;
        let current = match (aggregate.lifecycle(), status.current()) {
            (SessionLifecycle::Running, Some(current)) => {
                let attempt = current_attempt.ok_or(ReadServiceErrorV1::InconsistentState {
                    reason: "running session has no active attempt",
                })?;
                if attempt.attempt_id() != current.attempt_id()
                    || attempt.stage_id() != current.stage_id()
                    || attempt.number() != current.attempt_number()
                {
                    return Err(ReadServiceErrorV1::InconsistentState {
                        reason: "derived current attempt disagrees with the durable cursor",
                    });
                }
                Some(CurrentAttemptResultV1 {
                    stage_id: current.stage_id().clone(),
                    stage_index: usize_to_u64(current.stage_index())?,
                    title: current.title().to_owned(),
                    attempt_id: current.attempt_id().clone(),
                    attempt_number: current.attempt_number(),
                    blocked: current.blocked(),
                    ready_to_complete: current.ready_to_complete(),
                })
            }
            (SessionLifecycle::Running, None) => {
                return Err(ReadServiceErrorV1::InconsistentState {
                    reason: "running session has no derived current attempt",
                });
            }
            (SessionLifecycle::Completed | SessionLifecycle::Cancelled, None) => None,
            (SessionLifecycle::Completed | SessionLifecycle::Cancelled, Some(_)) => {
                return Err(ReadServiceErrorV1::InconsistentState {
                    reason: "terminal session has a derived current attempt",
                });
            }
        };

        let items = match (status.current(), current_attempt) {
            (Some(current), Some(attempt)) => current
                .required_items()
                .iter()
                .chain(current.optional_items())
                .map(|item| status_item(item, attempt))
                .collect::<Result<Vec<_>, _>>()?,
            (None, None) => Vec::new(),
            _ => {
                return Err(ReadServiceErrorV1::InconsistentState {
                    reason: "current item projection disagrees with the active attempt",
                });
            }
        };
        let blockers = status
            .current()
            .map(|current| {
                current
                    .open_blockers()
                    .iter()
                    .map(|blocker| BlockerResultV1 {
                        id: blocker.blocker_id().clone(),
                        attempt_id: blocker.attempt_id().clone(),
                        reason: blocker.reason().to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let previous_attempts = verbose
            .then(|| {
                aggregate
                    .attempts()
                    .iter()
                    .filter(|attempt| aggregate.active_attempt_id() != Some(attempt.attempt_id()))
                    .map(|attempt| {
                        Ok(PreviousAttemptResultV1 {
                            stage_id: attempt.stage_id().clone(),
                            attempt_id: attempt.attempt_id().clone(),
                            attempt_number: attempt.number(),
                            lifecycle: attempt_lifecycle(attempt.lifecycle()),
                            started_at: rfc3339_millis(attempt.started_at())?,
                            ended_at: attempt.ended_at().map(rfc3339_millis).transpose()?,
                            reason: attempt.reason().map(ToOwned::to_owned),
                        })
                    })
                    .collect::<Result<Vec<_>, ReadServiceErrorV1>>()
            })
            .transpose()?;
        let result = StatusResultV1 {
            task: StatusTaskV1 {
                title: aggregate.task_title().to_owned(),
                procedure: StatusProcedureV1 {
                    id: aggregate.snapshot().procedure_id().to_owned(),
                    version: aggregate.snapshot().procedure_version().to_owned(),
                    name: aggregate.snapshot().name().to_owned(),
                    digest: aggregate.snapshot().digest().clone(),
                },
            },
            session: StatusSessionV1 {
                id: aggregate.session_id().clone(),
                lifecycle: session_lifecycle(aggregate.lifecycle()),
                revision: aggregate.revision(),
                created_at: rfc3339_millis(aggregate.created_at())?,
                completed_at: aggregate.completed_at().map(rfc3339_millis).transpose()?,
                cancelled_at: aggregate.cancelled_at().map(rfc3339_millis).transpose()?,
            },
            current,
            previous_attempts,
            stages: status
                .stages()
                .iter()
                .map(|stage| {
                    Ok(StatusStageResultV1 {
                        id: stage.stage_id().clone(),
                        index: usize_to_u64(stage.stage_index())?,
                        title: stage.title().to_owned(),
                        status: stage_status(stage.status()),
                        latest_attempt_number: stage.latest_attempt_number(),
                    })
                })
                .collect::<Result<Vec<_>, ReadServiceErrorV1>>()?,
            items,
            blockers,
            queue: queue_result(view),
        };
        validate_status_result(result)
    }

    fn project_next(view: &WorkspaceViewV1) -> Result<NextResultV1, ReadServiceErrorV1> {
        let aggregate = current_session(view)?;
        let status = derive_session_status_v1(aggregate);
        let next = derive_next_work_v1(aggregate);
        let allowed_actions = allowed_actions(aggregate, &status)?;
        let suggestions = suggestions(&next);

        let result = NextResultV1 {
            stage: next.stage().map(|stage| NextStageResultV1 {
                id: stage.stage_id().clone(),
                title: stage.title().to_owned(),
                attempt_id: stage.attempt_id().clone(),
                attempt_number: stage.attempt_number(),
                instructions: stage.instructions().to_vec(),
            }),
            missing_required_items: next
                .missing_required_items()
                .iter()
                .map(|item| NextItemResultV1 {
                    id: item.item_id().clone(),
                    item_type: item_type(item.item_type()),
                    prompt: item.prompt().to_owned(),
                })
                .collect(),
            blockers: next
                .open_blockers()
                .iter()
                .map(|blocker| BlockerResultV1 {
                    id: blocker.blocker_id().clone(),
                    attempt_id: blocker.attempt_id().clone(),
                    reason: blocker.reason().to_owned(),
                })
                .collect(),
            allowed_actions,
            next_stage_after_completion: next.next_stage_after_completion().map(|stage| {
                NextStageAfterCompletionResultV1 {
                    id: stage.stage_id().clone(),
                    title: stage.title().to_owned(),
                }
            }),
            suggestions,
        };
        validate_next_result(result)
    }
}

fn validate_workspace_view(
    view: &WorkspaceViewV1,
    identity: &DurableWorktreeIdentityV1,
) -> Result<(), ReadServiceErrorV1> {
    if view.identity() != identity {
        return Err(ReadServiceErrorV1::InconsistentState {
            reason: "Store read returned a different durable identity",
        });
    }
    if view.state().workspace_id() != identity.workspace_uuid() {
        return Err(ReadServiceErrorV1::InconsistentState {
            reason: "workspace state does not match the durable identity",
        });
    }
    let aggregate = view
        .current_session()
        .ok_or(ReadServiceErrorV1::MissingSession)?;
    let session = view
        .state()
        .session()
        .ok_or(ReadServiceErrorV1::InconsistentState {
            reason: "workspace state is missing the current session cursor",
        })?;
    if session.session_id() != aggregate.session_id()
        || session.lifecycle() != aggregate.lifecycle()
        || session.revision() != aggregate.revision()
        || session.active_stage_id() != aggregate.active_stage_id()
        || session.active_attempt_id() != aggregate.active_attempt_id()
    {
        return Err(ReadServiceErrorV1::InconsistentState {
            reason: "workspace session cursor disagrees with the aggregate",
        });
    }
    validate_active_attempt(aggregate)
}

fn current_session(view: &WorkspaceViewV1) -> Result<&SessionAggregateV1, ReadServiceErrorV1> {
    view.current_session()
        .ok_or(ReadServiceErrorV1::MissingSession)
}

fn validate_active_attempt(aggregate: &SessionAggregateV1) -> Result<(), ReadServiceErrorV1> {
    let active_count = aggregate
        .attempts()
        .iter()
        .filter(|attempt| attempt.lifecycle() == AttemptLifecycle::Active)
        .count();
    match aggregate.lifecycle() {
        SessionLifecycle::Running => {
            let stage_id =
                aggregate
                    .active_stage_id()
                    .ok_or(ReadServiceErrorV1::InconsistentState {
                        reason: "running session has no active stage",
                    })?;
            let attempt_id =
                aggregate
                    .active_attempt_id()
                    .ok_or(ReadServiceErrorV1::InconsistentState {
                        reason: "running session has no active attempt",
                    })?;
            if aggregate
                .snapshot()
                .stages()
                .iter()
                .filter(|stage| stage.id() == stage_id)
                .count()
                != 1
                || aggregate
                    .attempts()
                    .iter()
                    .filter(|attempt| {
                        attempt.attempt_id() == attempt_id
                            && attempt.stage_id() == stage_id
                            && attempt.lifecycle() == AttemptLifecycle::Active
                    })
                    .count()
                    != 1
                || active_count != 1
            {
                return Err(ReadServiceErrorV1::InconsistentState {
                    reason: "running session does not have exactly one current active attempt",
                });
            }
        }
        SessionLifecycle::Completed | SessionLifecycle::Cancelled => {
            if aggregate.active_stage_id().is_some()
                || aggregate.active_attempt_id().is_some()
                || active_count != 0
            {
                return Err(ReadServiceErrorV1::InconsistentState {
                    reason: "terminal session retains an active attempt",
                });
            }
        }
    }
    Ok(())
}

fn active_attempt(
    aggregate: &SessionAggregateV1,
) -> Result<Option<&AttemptV1>, ReadServiceErrorV1> {
    validate_active_attempt(aggregate)?;
    match aggregate.lifecycle() {
        SessionLifecycle::Running => {
            let stage_id =
                aggregate
                    .active_stage_id()
                    .ok_or(ReadServiceErrorV1::InconsistentState {
                        reason: "running session has no active stage",
                    })?;
            let attempt_id =
                aggregate
                    .active_attempt_id()
                    .ok_or(ReadServiceErrorV1::InconsistentState {
                        reason: "running session has no active attempt",
                    })?;
            aggregate
                .attempts()
                .iter()
                .find(|attempt| {
                    attempt.attempt_id() == attempt_id && attempt.stage_id() == stage_id
                })
                .map(Some)
                .ok_or(ReadServiceErrorV1::InconsistentState {
                    reason: "active attempt is absent from the aggregate history",
                })
        }
        SessionLifecycle::Completed | SessionLifecycle::Cancelled => Ok(None),
    }
}

fn status_item(
    item: &podway_core::ItemProgressViewV1,
    attempt: &AttemptV1,
) -> Result<StatusItemResultV1, ReadServiceErrorV1> {
    let mut slots = attempt
        .item_slots()
        .iter()
        .filter(|slot| slot.item_id() == item.item_id());
    let slot = slots.next().ok_or(ReadServiceErrorV1::InconsistentState {
        reason: "derived item is absent from the active attempt",
    })?;
    if slots.next().is_some()
        || slot.attempt_id() != attempt.attempt_id()
        || slot.item_type() != item.item_type()
        || slot.revision() != item.revision()
    {
        return Err(ReadServiceErrorV1::InconsistentState {
            reason: "active item slot disagrees with the derived item projection",
        });
    }
    Ok(StatusItemResultV1 {
        id: item.item_id().clone(),
        item_type: item_type(item.item_type()),
        prompt: item.prompt().to_owned(),
        required: item.required(),
        satisfied: item.satisfied(),
        revision: item.revision(),
        value: item_value(slot.value())?,
    })
}

fn item_value(value: Option<&podway_core::ItemValueV1>) -> Result<Value, ReadServiceErrorV1> {
    let Some(value) = value else {
        return Ok(Value::Null);
    };
    match value.value_type() {
        ItemValueTypeV1::Confirm => Ok(Value::Bool(true)),
        ItemValueTypeV1::Text => value
            .as_text()
            .map(|text| Value::String(text.to_owned()))
            .ok_or(ReadServiceErrorV1::InconsistentState {
                reason: "text item has a non-text value",
            }),
        ItemValueTypeV1::Choice => value
            .as_choice()
            .map(|choice| Value::String(choice.to_owned()))
            .ok_or(ReadServiceErrorV1::InconsistentState {
                reason: "choice item has a non-choice value",
            }),
        ItemValueTypeV1::Integer => value
            .as_integer()
            .map(|integer| Value::Number(integer.into()))
            .ok_or(ReadServiceErrorV1::InconsistentState {
                reason: "integer item has a non-integer value",
            }),
        ItemValueTypeV1::List => value
            .as_list()
            .map(|items| Value::Array(items.iter().cloned().map(Value::String).collect::<Vec<_>>()))
            .ok_or(ReadServiceErrorV1::InconsistentState {
                reason: "list item has a non-list value",
            }),
        ItemValueTypeV1::Artifact => {
            let artifact = value
                .as_artifact()
                .ok_or(ReadServiceErrorV1::InconsistentState {
                    reason: "artifact item has a non-artifact value",
                })?;
            let location_type = match artifact.location_kind() {
                ArtifactLocationKindV1::LocalPath => "path",
                ArtifactLocationKindV1::ExternalReference => "reference",
            };
            Ok(json!({
                "location_type": location_type,
                "location": artifact.location(),
                "sha256_digest": artifact.digest().as_str(),
                "size_bytes": artifact.size_bytes(),
                "media_type": artifact.media_type(),
            }))
        }
    }
}

fn queue_result(view: &WorkspaceViewV1) -> QueueResultV1 {
    let queued_count = u64::from(view.queued_job_count());
    let running_job_id = view.running_job_id().cloned();
    QueueResultV1 {
        pending_mutations: queued_count != 0 || running_job_id.is_some(),
        queued_count,
        running_job_id,
        latest_workspace_sequence: view.latest_workspace_sequence(),
    }
}

fn allowed_actions(
    aggregate: &SessionAggregateV1,
    status: &podway_core::SessionStatusViewV1,
) -> Result<AllowedActionsResultV1, ReadServiceErrorV1> {
    let Some(current) = status.current() else {
        return Ok(AllowedActionsResultV1 {
            complete: false,
            skip: false,
            retry: false,
            return_to: Vec::new(),
            cancel: false,
        });
    };
    let stage = aggregate
        .snapshot()
        .stages()
        .get(current.stage_index())
        .filter(|stage| stage.id() == current.stage_id())
        .ok_or(ReadServiceErrorV1::InconsistentState {
            reason: "derived current stage is absent from the procedure snapshot",
        })?;
    let return_to = aggregate
        .snapshot()
        .stages()
        .iter()
        .take(current.stage_index())
        .filter(|candidate| {
            aggregate
                .snapshot()
                .return_policy()
                .allows_destination(candidate.id())
        })
        .map(|candidate| candidate.id().clone())
        .collect();

    Ok(AllowedActionsResultV1 {
        complete: current.ready_to_complete(),
        skip: stage.skip_policy().is_allowed(),
        retry: true,
        return_to,
        cancel: true,
    })
}

fn suggestions(next: &podway_core::NextWorkViewV1) -> Vec<CommandSuggestionResultV1> {
    if !next.open_blockers().is_empty() {
        return next
            .open_blockers()
            .iter()
            .map(|blocker| CommandSuggestionResultV1 {
                command: "session.unblock".to_owned(),
                argv: vec![
                    "podway".to_owned(),
                    "unblock".to_owned(),
                    blocker.blocker_id().as_str().to_owned(),
                ],
                item_id: None,
            })
            .collect();
    }

    next.missing_required_items()
        .iter()
        .map(|item| {
            let (command, verb, placeholder) = match item.item_type() {
                ItemTypeV1::Confirm => ("item.check", "check", None),
                ItemTypeV1::Text => ("item.set", "set", Some("<text>")),
                ItemTypeV1::Choice => ("item.set", "set", Some("<choice>")),
                ItemTypeV1::Integer => ("item.set", "set", Some("<integer>")),
                ItemTypeV1::List => ("item.add", "add", Some("<value>")),
                ItemTypeV1::Artifact => ("item.attach", "attach", Some("<path>")),
            };
            let mut argv = vec![
                "podway".to_owned(),
                verb.to_owned(),
                item.item_id().as_str().to_owned(),
            ];
            if let Some(placeholder) = placeholder {
                argv.push(placeholder.to_owned());
            }
            CommandSuggestionResultV1 {
                command: command.to_owned(),
                argv,
                item_id: Some(item.item_id().clone()),
            }
        })
        .collect()
}

fn validate_status_result(result: StatusResultV1) -> Result<StatusResultV1, ReadServiceErrorV1> {
    StatusResultV1::from_result_map(&result.to_result_map())
        .map_err(|_| ReadServiceErrorV1::ResultShapeConversion)
}

fn validate_next_result(result: NextResultV1) -> Result<NextResultV1, ReadServiceErrorV1> {
    NextResultV1::from_result_map(&result.to_result_map())
        .map_err(|_| ReadServiceErrorV1::ResultShapeConversion)
}

const fn is_terminal_job(state: JobStateV1) -> bool {
    matches!(
        state,
        JobStateV1::Succeeded | JobStateV1::Failed | JobStateV1::Cancelled
    )
}

const fn item_type(item_type: ItemTypeV1) -> ItemTypeResultV1 {
    match item_type {
        ItemTypeV1::Confirm => ItemTypeResultV1::Confirm,
        ItemTypeV1::Text => ItemTypeResultV1::Text,
        ItemTypeV1::Choice => ItemTypeResultV1::Choice,
        ItemTypeV1::Integer => ItemTypeResultV1::Integer,
        ItemTypeV1::List => ItemTypeResultV1::List,
        ItemTypeV1::Artifact => ItemTypeResultV1::Artifact,
    }
}

const fn stage_status(status: DerivedStageStatusV1) -> StageStatusResultV1 {
    match status {
        DerivedStageStatusV1::Pending => StageStatusResultV1::Pending,
        DerivedStageStatusV1::Current => StageStatusResultV1::Current,
        DerivedStageStatusV1::Blocked => StageStatusResultV1::Blocked,
        DerivedStageStatusV1::Done => StageStatusResultV1::Done,
        DerivedStageStatusV1::Skipped => StageStatusResultV1::Skipped,
        DerivedStageStatusV1::Redo => StageStatusResultV1::Redo,
        DerivedStageStatusV1::Abandoned => StageStatusResultV1::Abandoned,
    }
}

const fn attempt_lifecycle(lifecycle: AttemptLifecycle) -> AttemptLifecycleResultV1 {
    match lifecycle {
        AttemptLifecycle::Active => AttemptLifecycleResultV1::Active,
        AttemptLifecycle::Completed => AttemptLifecycleResultV1::Completed,
        AttemptLifecycle::Skipped => AttemptLifecycleResultV1::Skipped,
        AttemptLifecycle::Abandoned => AttemptLifecycleResultV1::Abandoned,
    }
}
const fn session_lifecycle(lifecycle: SessionLifecycle) -> SessionLifecycleV1 {
    match lifecycle {
        SessionLifecycle::Running => SessionLifecycleV1::Running,
        SessionLifecycle::Completed => SessionLifecycleV1::Completed,
        SessionLifecycle::Cancelled => SessionLifecycleV1::Cancelled,
    }
}

fn usize_to_u64(value: usize) -> Result<u64, ReadServiceErrorV1> {
    u64::try_from(value).map_err(|_| ReadServiceErrorV1::InconsistentState {
        reason: "stage index exceeds the protocol range",
    })
}

fn rfc3339_millis(value: UnixMillis) -> Result<Rfc3339MillisV1, ReadServiceErrorV1> {
    let seconds = value.get() / 1_000;
    let millis = value.get() % 1_000;
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_date_from_unix_days(days);
    if !(0..=9_999).contains(&year) {
        return Err(ReadServiceErrorV1::TimestampOutOfRange);
    }
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    Rfc3339MillisV1::new(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    ))
    .map_err(|_| ReadServiceErrorV1::TimestampOutOfRange)
}

/// Howard Hinnant's civil-from-days conversion, evaluated in `i128` to keep every `u64`
/// millisecond value representable before protocol-range validation.
fn civil_date_from_unix_days(days: u64) -> (i128, i128, i128) {
    let z = i128::from(days) + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}
