//! Daemon-owned durable command worker orchestration.
//!
//! The Store remains the queue and result authority. A worker receives the scheduler that was
//! selected by the workspace runtime manager; it never owns a second scheduler registry or derives
//! identity from a path. Scheduler and context notifications are wake-up hints only, so every
//! decision after a wake is made from a fresh Store read.

use std::{
    error::Error,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    thread,
    time::Duration,
};

use podway_protocol::SliceRequestV1;
use podway_store::{
    AdmitOutcomeV1, IdempotencyKeyV1, JobIdV1, JobReceiptOrTerminalV1, JobViewV1,
    PersistedTerminalReceiptV1, StoreErrorV1, TerminalReceiptV1, WorkerIdV1, WorkspaceBindingV1,
};

use crate::{
    execution::{
        ArtifactVerifierV1, DaemonExecutionEngineV1, ExecutionClockV1, ExecutionErrorV1,
        ExecutionIdSourceV1, ProcedureProviderV1, WorkspaceRevalidatorV1,
    },
    read_service::{MonotonicClockV1, MonotonicDeadlineV1},
    runtime_workspace::WorkspaceSchedulerContextV1,
    scheduler::{
        WorkspaceSchedulerProgressErrorV1, WorkspaceSchedulerRegistryV1,
        WorkspaceSchedulerRetirementErrorV1, WorkspaceSchedulerRetirementRetryV1,
        WorkspaceSchedulerRetirementStartErrorV1, WorkspaceSchedulerV1,
    },
};

pub use crate::scheduler::WorkspaceSchedulerKeyV1;

/// The active, scheduler-owned context operations required by the worker.
///
/// The production implementation is [`WorkspaceSchedulerContextV1`]. Test contexts can implement
/// this narrow contract, but production orchestration never creates another workspace context.
pub trait WorkerWorkspaceContextV1: Send + Sync {
    fn binding(&self) -> &WorkspaceBindingV1;

    fn read_job(&self, job: &JobIdV1) -> Result<Option<JobViewV1>, StoreErrorV1>;

    /// Acquires the close/admission/claim gate, snapshots this context's immutable binding, and
    /// refuses work after retirement has closed the gate. The binding snapshot must be taken before
    /// the callback runs so rebinding cannot change an in-flight operation or deadlock behind it.
    fn with_claim_permission<R>(
        &self,
        operation: impl FnOnce(&WorkspaceBindingV1) -> R,
    ) -> Option<R>;

    /// Closes this generation's shared admission/claim permission gate permanently.
    fn stop_claims(&self);
    fn mark_recovery_required(&self);
    fn recovery_required(&self) -> bool;

    /// Returns and waits on a non-authoritative wake-up version. Waiters must always re-read Store
    /// after this changes or times out.
    fn notification_version(&self) -> u64;
    fn wait_for_notification_after(&self, observed: u64, timeout: Duration);
    fn notify_after_authoritative_change(&self);
}

impl WorkerWorkspaceContextV1 for WorkspaceSchedulerContextV1 {
    fn binding(&self) -> &WorkspaceBindingV1 {
        WorkspaceSchedulerContextV1::binding(self)
    }

    fn read_job(&self, job: &JobIdV1) -> Result<Option<JobViewV1>, StoreErrorV1> {
        self.worker_read_job(job)
    }

    fn with_claim_permission<R>(
        &self,
        operation: impl FnOnce(&WorkspaceBindingV1) -> R,
    ) -> Option<R> {
        WorkspaceSchedulerContextV1::with_claim_permission(self, operation)
    }

    fn stop_claims(&self) {
        WorkspaceSchedulerContextV1::stop_claims(self);
    }

    fn mark_recovery_required(&self) {
        WorkspaceSchedulerContextV1::mark_recovery_required(self);
    }

    fn recovery_required(&self) -> bool {
        WorkspaceSchedulerContextV1::recovery_required(self)
    }

    fn notification_version(&self) -> u64 {
        WorkspaceSchedulerContextV1::notification_version(self)
    }

    fn wait_for_notification_after(&self, observed: u64, timeout: Duration) {
        WorkspaceSchedulerContextV1::wait_for_notification_after(self, observed, timeout);
    }

    fn notify_after_authoritative_change(&self) {
        WorkspaceSchedulerContextV1::notify_after_authoritative_change(self);
    }
}

/// The execution boundary used after a runtime-manager scheduler has been selected.
pub trait WorkerExecutionV1<Context>: Send + Sync
where
    Context: WorkerWorkspaceContextV1,
{
    fn admit(
        &self,
        workspace: &Context,
        binding: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1>;

    fn execute_next(
        &self,
        workspace: &Context,
        binding: &WorkspaceBindingV1,
        worker: WorkerIdV1,
    ) -> Result<Option<TerminalReceiptV1>, ExecutionErrorV1>;
}

/// A directly injected engine remains useful for deterministic worker tests. Production uses a
/// context-aware adapter so each engine is constructed from the context's exact SQLite Store.
impl<Context, Store, Ids, Clock, Procedures, Artifacts, Workspaces> WorkerExecutionV1<Context>
    for DaemonExecutionEngineV1<Store, Ids, Clock, Procedures, Artifacts, Workspaces>
where
    Context: WorkerWorkspaceContextV1,
    Store: podway_store::StoreContractV1 + podway_store::StoreIdempotencyReadContractV1,
    Ids: ExecutionIdSourceV1,
    Clock: ExecutionClockV1,
    Procedures: ProcedureProviderV1,
    Artifacts: ArtifactVerifierV1,
    Workspaces: WorkspaceRevalidatorV1,
{
    fn admit(
        &self,
        workspace: &Context,
        binding: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1> {
        if workspace.binding() != binding {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "scheduler context binding changed during admission",
            });
        }
        DaemonExecutionEngineV1::admit_for_workspace(self, binding, request, idempotency_key)
    }

    fn execute_next(
        &self,
        workspace: &Context,
        binding: &WorkspaceBindingV1,
        worker: WorkerIdV1,
    ) -> Result<Option<TerminalReceiptV1>, ExecutionErrorV1> {
        if workspace.binding() != binding {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "scheduler context binding changed during claim",
            });
        }
        DaemonExecutionEngineV1::execute_next(self, binding, worker)
    }
}

/// Clock used solely to make a synchronous wait deadline explicit and testable. Durable execution
/// timestamps remain on the separate [`ExecutionClockV1`] wall-clock path.
pub trait WorkerClockV1: MonotonicClockV1 {
    /// Returns the real blocking interval until `deadline` based on this clock's monotonic reading.
    /// A deterministic test clock can return zero and advance before the next Store recheck.
    fn wait_duration_until(&self, deadline: MonotonicDeadlineV1) -> Duration {
        Duration::from_millis(deadline.millis().saturating_sub(self.now_millis()))
    }
}

/// A durable admission's requested client-completion behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerCompletionModeV1 {
    /// Return after the Store has durably admitted/replayed the request and a drain has been woken.
    Detached,
    /// Return a terminal receipt when it becomes durable, or a non-cancelling deadline result.
    WaitUntil(MonotonicDeadlineV1),
}

/// A Store-derived result for a synchronous wait.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerWaitResultV1 {
    Terminal(Box<PersistedTerminalReceiptV1>),
    TimedOut(Box<JobViewV1>),
}

/// The result returned after admission has committed and the requested response behavior finished.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerSubmissionV1 {
    admission: AdmitOutcomeV1,
    completion: Option<WorkerWaitResultV1>,
}

impl WorkerSubmissionV1 {
    pub fn admission(&self) -> &AdmitOutcomeV1 {
        &self.admission
    }

    /// `None` is the detached acknowledgement. A terminal replay is still available in
    /// `admission`; a wait obtains its terminal receipt through the Store.
    pub fn completion(&self) -> Option<&WorkerWaitResultV1> {
        self.completion.as_ref()
    }
}

/// Failure from daemon worker orchestration. Domain command failures are terminal Store receipts,
/// not worker errors.
#[derive(Debug)]
pub enum WorkerErrorV1 {
    Store(StoreErrorV1),
    Execution(ExecutionErrorV1),
    Progress(WorkspaceSchedulerProgressErrorV1),
    JobNotFound(JobIdV1),
    BackgroundPanicked,
    RecoveryRequired,
    RetirementRejected,
}

impl fmt::Display for WorkerErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(source) => source.fmt(formatter),
            Self::Execution(source) => source.fmt(formatter),
            Self::Progress(source) => source.fmt(formatter),
            Self::JobNotFound(job) => write!(formatter, "durable job is not present: {job}"),
            Self::BackgroundPanicked => formatter.write_str("detached worker drain panicked"),
            Self::RecoveryRequired => formatter.write_str(
                "a claimed job requires Store startup recovery before this workspace can retire",
            ),
            Self::RetirementRejected => {
                formatter.write_str("workspace retirement hook rejected close")
            }
        }
    }
}

impl Error for WorkerErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(source) => Some(source),
            Self::Execution(source) => Some(source),
            Self::Progress(source) => Some(source),
            Self::JobNotFound(_)
            | Self::BackgroundPanicked
            | Self::RecoveryRequired
            | Self::RetirementRejected => None,
        }
    }
}

impl From<StoreErrorV1> for WorkerErrorV1 {
    fn from(source: StoreErrorV1) -> Self {
        Self::Store(source)
    }
}

impl From<ExecutionErrorV1> for WorkerErrorV1 {
    fn from(source: ExecutionErrorV1) -> Self {
        Self::Execution(source)
    }
}

impl From<WorkspaceSchedulerProgressErrorV1> for WorkerErrorV1 {
    fn from(source: WorkspaceSchedulerProgressErrorV1) -> Self {
        Self::Progress(source)
    }
}

/// The count of terminal Store transactions observed during a completed drain.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkerDrainReportV1 {
    terminal_job_count: u64,
}

impl WorkerDrainReportV1 {
    pub const fn terminal_job_count(self) -> u64 {
        self.terminal_job_count
    }
}

/// A join handle for one intentionally detached worker drain. Dropping it never cancels work.
pub struct WorkerDrainHandleV1 {
    join: thread::JoinHandle<Result<WorkerDrainReportV1, WorkerErrorV1>>,
}

impl WorkerDrainHandleV1 {
    pub fn join(self) -> Result<WorkerDrainReportV1, WorkerErrorV1> {
        self.join
            .join()
            .map_err(|_| WorkerErrorV1::BackgroundPanicked)?
    }
}

/// Daemon worker orchestration over runtime-manager scheduler handles and durable Store claims.
pub struct DaemonWorkerV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    inner: Arc<DaemonWorkerInnerV1<Context, Execution, Clock>>,
}

struct DaemonWorkerInnerV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    execution: Arc<Execution>,
    clock: Arc<Clock>,
    worker_id: WorkerIdV1,
    context: std::marker::PhantomData<fn() -> Context>,
}

impl<Context, Execution, Clock> Clone for DaemonWorkerV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<Context, Execution, Clock> DaemonWorkerV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1 + 'static,
    Execution: WorkerExecutionV1<Context> + 'static,
    Clock: WorkerClockV1 + 'static,
{
    pub fn new(execution: Arc<Execution>, clock: Arc<Clock>, worker_id: WorkerIdV1) -> Self {
        Self {
            inner: Arc::new(DaemonWorkerInnerV1 {
                execution,
                clock,
                worker_id,
                context: std::marker::PhantomData,
            }),
        }
    }

    /// Durably admits and starts a drain on this exact runtime-manager generation. No
    /// acknowledgement is returned before admission succeeds; notification hints cannot suppress it.
    pub fn submit(
        &self,
        scheduler: &Arc<WorkspaceSchedulerV1<Context>>,
        request: &SliceRequestV1,
        idempotency_key: IdempotencyKeyV1,
        completion_mode: WorkerCompletionModeV1,
    ) -> Result<WorkerSubmissionV1, WorkerErrorV1> {
        if scheduler.context_snapshot().recovery_required() {
            return Err(WorkerErrorV1::RetirementRejected);
        }
        let outcome = scheduler.with_serialized(|context| {
            if context.recovery_required() {
                return Err(WorkerErrorV1::RetirementRejected);
            }
            context
                .with_claim_permission(|binding| {
                    self.inner
                        .execution
                        .admit(context.as_ref(), binding, request, idempotency_key)
                })
                .ok_or(WorkerErrorV1::RetirementRejected)?
                .map_err(Into::into)
        })?;
        let Some(job) = job_to_drive(&outcome) else {
            let completion = match completion_mode {
                WorkerCompletionModeV1::Detached => None,
                WorkerCompletionModeV1::WaitUntil(_) => terminal_replay(&outcome)
                    .map(|receipt| WorkerWaitResultV1::Terminal(Box::new(receipt))),
            };
            return Ok(WorkerSubmissionV1 {
                admission: outcome,
                completion,
            });
        };

        let notification = self.notify_authoritative_change(scheduler);
        let _drain = self.drain_workspace_detached(Arc::clone(scheduler));
        notification?;
        let completion = match completion_mode {
            WorkerCompletionModeV1::Detached => None,
            WorkerCompletionModeV1::WaitUntil(deadline) => {
                Some(self.wait_for_terminal(scheduler, job, deadline)?)
            }
        };
        Ok(WorkerSubmissionV1 {
            admission: outcome,
            completion,
        })
    }

    /// Drains this exact identity generation synchronously until the Store has no claim. Claims
    /// and execution run beneath this generation's serialization mutex.
    pub fn drain_workspace(
        &self,
        scheduler: Arc<WorkspaceSchedulerV1<Context>>,
    ) -> Result<WorkerDrainReportV1, WorkerErrorV1> {
        self.drain_scheduler(scheduler)
    }

    /// Starts a supervised detached drain for this exact identity generation. Dropping the returned
    /// handle never cancels its Store-backed work; a panic becomes a typed error after this
    /// generation is marked recovery-required.
    pub fn drain_workspace_detached(
        &self,
        scheduler: Arc<WorkspaceSchedulerV1<Context>>,
    ) -> WorkerDrainHandleV1 {
        let worker = self.clone();
        let recovery_context = scheduler.context_snapshot();
        WorkerDrainHandleV1 {
            join: thread::spawn(move || {
                match catch_unwind(AssertUnwindSafe(|| worker.drain_scheduler(scheduler))) {
                    Ok(result) => result,
                    Err(_) => {
                        // This snapshot belongs to the scheduler generation passed to this drain.
                        // Do not consult the registry: it could already contain a replacement.
                        let _ = Self::recover_after_panic(recovery_context.as_ref());
                        Err(WorkerErrorV1::BackgroundPanicked)
                    }
                }
            }),
        }
    }

    /// Drains schedulers activated during startup recovery. Recovery itself is performed by Store;
    /// no in-memory queue is reconstructed here.
    pub fn drain_recovered_queues(
        &self,
        schedulers: impl IntoIterator<Item = Arc<WorkspaceSchedulerV1<Context>>>,
    ) -> Vec<Result<WorkerDrainReportV1, WorkerErrorV1>> {
        schedulers
            .into_iter()
            .map(|scheduler| self.drain_scheduler(scheduler))
            .collect()
    }

    /// Reads the context Store on every hint and returns a deterministic timeout without cancelling
    /// a queued or running job.
    pub fn wait_for_terminal(
        &self,
        scheduler: &Arc<WorkspaceSchedulerV1<Context>>,
        job: &JobIdV1,
        deadline: MonotonicDeadlineV1,
    ) -> Result<WorkerWaitResultV1, WorkerErrorV1> {
        loop {
            let context = scheduler.context_snapshot();
            let view = self.read_required_job(context.as_ref(), job)?;
            if let Some(receipt) = view.terminal_receipt().cloned() {
                return Ok(WorkerWaitResultV1::Terminal(Box::new(receipt)));
            }
            if self.inner.clock.now_millis() >= deadline.millis() {
                return Ok(WorkerWaitResultV1::TimedOut(Box::new(view)));
            }

            // Snapshot both hint versions before the second Store read. A changed scheduler version
            // means a completion raced this waiter; the next loop performs a fresh Store read.
            let scheduler_version = scheduler.progress_version();
            let context_notification = context.notification_version();
            let view = self.read_required_job(context.as_ref(), job)?;
            if let Some(receipt) = view.terminal_receipt().cloned() {
                return Ok(WorkerWaitResultV1::Terminal(Box::new(receipt)));
            }
            if self.inner.clock.now_millis() >= deadline.millis() {
                return Ok(WorkerWaitResultV1::TimedOut(Box::new(view)));
            }
            if scheduler.progress_version() != scheduler_version {
                continue;
            }

            context.wait_for_notification_after(
                context_notification,
                self.inner.clock.wait_duration_until(deadline),
            );
        }
    }

    /// Retires this exact runtime-manager scheduler generation. The registry comparison prevents a
    /// closing generation from removing a replacement and leaves close failures fail-closed.
    pub fn retire_workspace(
        &self,
        registry: &WorkspaceSchedulerRegistryV1<Context>,
        scheduler: &Arc<WorkspaceSchedulerV1<Context>>,
    ) -> Result<(), WorkerRetirementErrorV1<Context, Execution, Clock>> {
        self.retire_workspace_with(registry, scheduler, |_| Ok(()))
    }

    pub fn retire_workspace_with<F>(
        &self,
        registry: &WorkspaceSchedulerRegistryV1<Context>,
        scheduler: &Arc<WorkspaceSchedulerV1<Context>>,
        close: F,
    ) -> Result<(), WorkerRetirementErrorV1<Context, Execution, Clock>>
    where
        F: FnOnce(&WorkspaceBindingV1) -> Result<(), WorkerErrorV1>,
    {
        registry
            .retire(scheduler, |retiring| self.close_scheduler(retiring, close))
            .map_err(|error| self.map_retirement_error(error))
    }

    fn drain_scheduler(
        &self,
        scheduler: Arc<WorkspaceSchedulerV1<Context>>,
    ) -> Result<WorkerDrainReportV1, WorkerErrorV1> {
        if scheduler.context_snapshot().recovery_required() {
            return Ok(WorkerDrainReportV1::default());
        }
        scheduler.with_serialized(|context| {
            if context.recovery_required() {
                return Ok(WorkerDrainReportV1::default());
            }
            match catch_unwind(AssertUnwindSafe(|| {
                self.drain_context(&scheduler, context.as_ref())
            })) {
                Ok(result) => result,
                Err(_) => {
                    self.fail_closed_drain(context.as_ref(), WorkerErrorV1::BackgroundPanicked)
                }
            }
        })
    }

    fn drain_context(
        &self,
        scheduler: &WorkspaceSchedulerV1<Context>,
        context: &Context,
    ) -> Result<WorkerDrainReportV1, WorkerErrorV1> {
        let mut report = WorkerDrainReportV1::default();
        if context.recovery_required() {
            return Ok(report);
        }
        loop {
            let attempt = context.with_claim_permission(|binding| {
                catch_unwind(AssertUnwindSafe(|| {
                    self.inner.execution.execute_next(
                        context,
                        binding,
                        self.inner.worker_id.clone(),
                    )
                }))
            });
            match attempt {
                None | Some(Ok(Ok(None))) => return Ok(report),
                Some(Ok(Ok(Some(_terminal)))) => {
                    report.terminal_job_count = report
                        .terminal_job_count
                        .checked_add(1)
                        .expect("terminal job count cannot overflow before scheduler progress");
                    if let Err(error) = self.notify_authoritative_change(scheduler) {
                        return self.fail_closed_drain(context, error);
                    }
                }
                Some(Ok(Err(error))) => {
                    return self.fail_closed_drain(context, error.into());
                }
                Some(Err(_)) => {
                    return self.fail_closed_drain(context, WorkerErrorV1::BackgroundPanicked);
                }
            }
        }
    }

    fn fail_closed_drain(
        &self,
        context: &Context,
        error: WorkerErrorV1,
    ) -> Result<WorkerDrainReportV1, WorkerErrorV1> {
        if Self::recover_after_panic(context) {
            Err(WorkerErrorV1::BackgroundPanicked)
        } else {
            Err(error)
        }
    }

    /// Attempts both fail-closed hooks even when either hook panics. A hook panic cannot make this
    /// function panic again, so detached supervision retains its typed error boundary.
    fn recover_after_panic(context: &Context) -> bool {
        let stop_claims_panicked =
            catch_unwind(AssertUnwindSafe(|| context.stop_claims())).is_err();
        let mark_recovery_required_panicked =
            catch_unwind(AssertUnwindSafe(|| context.mark_recovery_required())).is_err();
        stop_claims_panicked || mark_recovery_required_panicked
    }

    fn close_scheduler<F>(
        &self,
        scheduler: &WorkspaceSchedulerV1<Context>,
        close: F,
    ) -> Result<(), WorkerErrorV1>
    where
        F: FnOnce(&WorkspaceBindingV1) -> Result<(), WorkerErrorV1>,
    {
        if scheduler.context_snapshot().recovery_required() {
            return Err(WorkerErrorV1::RecoveryRequired);
        }
        scheduler.with_serialized(|context| {
            if context.recovery_required() {
                return Err(WorkerErrorV1::RecoveryRequired);
            }
            if let Err(error) = self.drain_context(scheduler, context.as_ref()) {
                context.stop_claims();
                return Err(error);
            }
            context.stop_claims();
            close(context.binding())
        })
    }

    fn read_required_job(
        &self,
        context: &Context,
        job: &JobIdV1,
    ) -> Result<JobViewV1, WorkerErrorV1> {
        context
            .read_job(job)?
            .ok_or_else(|| WorkerErrorV1::JobNotFound(job.clone()))
    }

    fn notify_authoritative_change(
        &self,
        scheduler: &WorkspaceSchedulerV1<Context>,
    ) -> Result<(), WorkerErrorV1> {
        let progress = scheduler.notify_progress();
        scheduler
            .context_snapshot()
            .notify_after_authoritative_change();
        progress.map(|_| ()).map_err(Into::into)
    }

    fn map_retirement_error(
        &self,
        error: WorkspaceSchedulerRetirementErrorV1<Context, WorkerErrorV1>,
    ) -> WorkerRetirementErrorV1<Context, Execution, Clock> {
        match error {
            WorkspaceSchedulerRetirementErrorV1::Start(source) => {
                WorkerRetirementErrorV1::Start(Box::new(source))
            }
            WorkspaceSchedulerRetirementErrorV1::CloseFailed { source, retry } => {
                WorkerRetirementErrorV1::CloseFailed {
                    source: Box::new(source),
                    retry: Box::new(WorkerRetirementRetryV1 {
                        worker: self.clone(),
                        retry,
                    }),
                }
            }
            WorkspaceSchedulerRetirementErrorV1::StaleCompletion { key, generation } => {
                WorkerRetirementErrorV1::StaleCompletion {
                    key: Box::new(key),
                    generation,
                }
            }
            WorkspaceSchedulerRetirementErrorV1::CloseFailedStale {
                source,
                key,
                generation,
            } => WorkerRetirementErrorV1::CloseFailedStale {
                source: Box::new(source),
                key: Box::new(key),
                generation,
            },
        }
    }
}

/// A failed retirement's only retry capability. It never reopens the retired scheduler slot.
pub struct WorkerRetirementRetryV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    worker: DaemonWorkerV1<Context, Execution, Clock>,
    retry: WorkspaceSchedulerRetirementRetryV1<Context>,
}

impl<Context, Execution, Clock> fmt::Debug for WorkerRetirementRetryV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerRetirementRetryV1")
            .field("key", self.retry.key())
            .field("generation", &self.retry.generation())
            .finish()
    }
}

impl<Context, Execution, Clock> WorkerRetirementRetryV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1 + 'static,
    Execution: WorkerExecutionV1<Context> + 'static,
    Clock: WorkerClockV1 + 'static,
{
    pub fn key(&self) -> &WorkspaceSchedulerKeyV1 {
        self.retry.key()
    }

    pub fn generation(&self) -> crate::SchedulerGenerationV1 {
        self.retry.generation()
    }

    pub fn retry_with<F>(
        &self,
        close: F,
    ) -> Result<(), WorkerRetirementErrorV1<Context, Execution, Clock>>
    where
        F: FnOnce(&WorkspaceBindingV1) -> Result<(), WorkerErrorV1>,
    {
        self.retry
            .retry(|retiring| self.worker.close_scheduler(retiring, close))
            .map_err(|error| self.worker.map_retirement_error(error))
    }
}

/// Retirement failures retain a typed retry without exposing the generic engine internals through
/// the scheduler registry itself.
pub enum WorkerRetirementErrorV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    Start(Box<WorkspaceSchedulerRetirementStartErrorV1<Context>>),
    CloseFailed {
        source: Box<WorkerErrorV1>,
        retry: Box<WorkerRetirementRetryV1<Context, Execution, Clock>>,
    },
    StaleCompletion {
        key: Box<WorkspaceSchedulerKeyV1>,
        generation: crate::SchedulerGenerationV1,
    },
    CloseFailedStale {
        source: Box<WorkerErrorV1>,
        key: Box<WorkspaceSchedulerKeyV1>,
        generation: crate::SchedulerGenerationV1,
    },
}

impl<Context, Execution, Clock> fmt::Debug for WorkerRetirementErrorV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(source) => formatter.debug_tuple("Start").field(source).finish(),
            Self::CloseFailed { source, retry } => formatter
                .debug_struct("CloseFailed")
                .field("source", source)
                .field("retry", retry)
                .finish(),
            Self::StaleCompletion { key, generation } => formatter
                .debug_struct("StaleCompletion")
                .field("key", key)
                .field("generation", generation)
                .finish(),
            Self::CloseFailedStale {
                source,
                key,
                generation,
            } => formatter
                .debug_struct("CloseFailedStale")
                .field("source", source)
                .field("key", key)
                .field("generation", generation)
                .finish(),
        }
    }
}

impl<Context, Execution, Clock> fmt::Display for WorkerRetirementErrorV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1,
    Execution: WorkerExecutionV1<Context>,
    Clock: WorkerClockV1,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(source) => source.fmt(formatter),
            Self::CloseFailed { source, .. } => {
                write!(formatter, "workspace retirement close failed: {source}")
            }
            Self::StaleCompletion { key, generation } => {
                write!(
                    formatter,
                    "workspace retirement completed for stale {key:?} generation {generation:?}"
                )
            }
            Self::CloseFailedStale {
                source,
                key,
                generation,
            } => write!(
                formatter,
                "workspace retirement close failed for stale {key:?} generation {generation:?}: {source}"
            ),
        }
    }
}

impl<Context, Execution, Clock> Error for WorkerRetirementErrorV1<Context, Execution, Clock>
where
    Context: WorkerWorkspaceContextV1 + 'static,
    Execution: WorkerExecutionV1<Context> + 'static,
    Clock: WorkerClockV1 + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Start(source) => Some(source.as_ref()),
            Self::CloseFailed { source, .. } | Self::CloseFailedStale { source, .. } => {
                Some(source.as_ref())
            }
            Self::StaleCompletion { .. } => None,
        }
    }
}

fn job_to_drive(outcome: &AdmitOutcomeV1) -> Option<&JobIdV1> {
    match outcome {
        AdmitOutcomeV1::New(receipt) => Some(receipt.job_id()),
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt)) => {
            Some(receipt.job_id())
        }
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(_)) => None,
    }
}

fn terminal_replay(outcome: &AdmitOutcomeV1) -> Option<PersistedTerminalReceiptV1> {
    match outcome {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => {
            Some(receipt.clone())
        }
        AdmitOutcomeV1::New(_)
        | AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(_)) => None,
    }
}
