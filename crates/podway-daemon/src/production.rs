//! Concrete G005 production composition.
//!
//! This module contains the daemon boundary glue only. Workspace resolution remains in
//! [`WorkspaceRuntimeManagerV1`], state remains in SQLite, and protocol routing remains in
//! [`RequestDispatcherV1Adapter`]. No adapter derives an identity from a path or persists a wire
//! document directly.

use std::{sync::Arc, time::Instant};

use podway_core::UnixMillis;
use podway_git::{
    Base64UrlPathBytesV1, DiagnosticPathDisplayV1, LosslessPathV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    IdempotencyKeyV1, JobOutputV1, JobStateV1, Rfc3339MillisV1, SessionLifecycleV1,
    SessionOutputV1, SliceCommandV1, SliceRequestV1, WorkspaceOutputV1, WorktreeSelectorWireV1,
};
use podway_store::{
    AdmitOutcomeV1, IdempotencyKeyV1 as StoreIdempotencyKeyV1, JobReceiptOrTerminalV1,
    JobStateV1 as StoreJobStateV1, JobViewV1, PersistedTerminalJobStateV1,
    PersistedTerminalReceiptV1, PersistedTerminalSessionProjectionV1, StoreContractV1,
    StoreErrorV1, WorkerIdV1, WorkspaceBindingV1,
    codec::{
        PersistedDomainErrorV1, PersistedDomainResultV1, PersistedSessionLifecycleV1,
        PersistedTerminalResultV1,
    },
};
use serde_json::{Map, Value};

use crate::{
    dispatch::{
        CatalogDispatchErrorMapperV1, DispatchErrorDetailsV1, DispatchFailureKindV1,
        DispatchFailureV1, DispatchResponseMetadataV1, DispatcherReadOutputV1,
        DispatcherReadServiceV1, DispatcherTerminalOutputV1, DispatcherTerminalResultV1,
        MutationAdmissionWorkerV1, MutationDispatchOutcomeV1, MutationWaitV1,
        RequestDispatcherV1Adapter, RequestReadWaitV1, WorkspaceRuntimeV1,
    },
    execution::{DaemonExecutionEngineV1, EmbeddedPresetProcedureProviderV1, ExecutionClockV1},
    native_execution::{
        NativeArtifactVerifierV1, NativeExecutionIdSourceV1, NativeWorkspaceRevalidatorV1,
        WallUtcExecutionClockV1,
    },
    read_service::{
        AuthoritativeReadServiceV1, MonotonicClockV1, MonotonicDeadlineV1, ReadNotificationErrorV1,
        ReadNotificationV1, ReadNotificationVersionV1, ReadServiceErrorV1, ReadWaitOutcomeV1,
        ReadWaitV1,
    },
    runtime_workspace::{
        WorkspaceRuntimeErrorV1, WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1,
        WorkspaceSchedulerContextV1,
    },
    scheduler::WorkspaceSchedulerV1,
    server::{ResponseMetadataSourceV1, SystemResponseMetadataSourceV1},
    worker::{
        DaemonWorkerV1, WorkerClockV1, WorkerCompletionModeV1, WorkerErrorV1, WorkerExecutionV1,
        WorkerWaitResultV1, WorkerWorkspaceContextV1,
    },
    workspace::{SqliteWorkspaceBindingInspectorV1, WorkspaceResolutionErrorV1},
};

/// Native wall/monotonic time and protocol response metadata for the production composition.
#[derive(Debug)]
pub struct NativeProductionClockV1 {
    started: Instant,
    metadata: SystemResponseMetadataSourceV1,
}

impl Default for NativeProductionClockV1 {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            metadata: SystemResponseMetadataSourceV1::default(),
        }
    }
}

impl ExecutionClockV1 for NativeProductionClockV1 {
    fn now(&self) -> UnixMillis {
        WallUtcExecutionClockV1.now()
    }
}

impl MonotonicClockV1 for NativeProductionClockV1 {
    fn now_millis(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}
impl WorkerClockV1 for NativeProductionClockV1 {}

impl DispatchResponseMetadataV1 for NativeProductionClockV1 {
    fn generated_at(&self) -> Rfc3339MillisV1 {
        self.metadata.generated_at()
    }
}

impl MonotonicClockV1 for Arc<NativeProductionClockV1> {
    fn now_millis(&self) -> u64 {
        self.as_ref().now_millis()
    }
}

/// One opaque resolved workspace. The scheduler is always the manager-owned durable-identity
/// generation; the response projection is captured from the same authoritative Store read used to
/// activate the request.
#[derive(Clone)]
pub struct ProductionWorkspaceV1 {
    scheduler: Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    output: WorkspaceOutputV1,
}

impl ProductionWorkspaceV1 {
    pub fn scheduler(&self) -> &Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>> {
        &self.scheduler
    }
}

/// Concrete workspace runtime adapter. Bootstrap calls only the manager bootstrap path; every
/// existing route follows the manager's Store/Git two-pass resolution path.
#[derive(Clone)]
pub struct ProductionWorkspaceRuntimeV1 {
    manager: Arc<WorkspaceRuntimeManagerV1>,
    clock: Arc<NativeProductionClockV1>,
}

impl ProductionWorkspaceRuntimeV1 {
    pub fn new(
        manager: Arc<WorkspaceRuntimeManagerV1>,
        clock: Arc<NativeProductionClockV1>,
    ) -> Self {
        Self { manager, clock }
    }

    pub fn manager(&self) -> &Arc<WorkspaceRuntimeManagerV1> {
        &self.manager
    }

    fn observation(&self) -> WorkspaceRuntimeObservationV1 {
        WorkspaceRuntimeObservationV1::new(self.clock.now(), self.clock.generated_at())
    }

    fn workspace_from_scheduler(
        &self,
        scheduler: Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    ) -> Result<ProductionWorkspaceV1, DispatchFailureV1> {
        let context = scheduler.context_snapshot();
        let view = context
            .store()
            .read_workspace_view(context.binding().identity())
            .map_err(map_store_error)?;
        let output = WorkspaceOutputV1::new(
            context.binding().identity().workspace_uuid().clone(),
            context
                .git_evidence()
                .roots()
                .worktree_root()
                .display()
                .as_str(),
            view.latest_workspace_sequence(),
        )
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
        Ok(ProductionWorkspaceV1 { scheduler, output })
    }
}

impl WorkspaceRuntimeV1 for ProductionWorkspaceRuntimeV1 {
    type Workspace = ProductionWorkspaceV1;

    fn resolve_existing(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        let expected_workspace_id = selector.expected_uuid();
        let selector = selector_from_wire(selector)?;
        let scheduler = self
            .manager
            .resolve_existing(selector, expected_workspace_id, self.observation())
            .map_err(map_runtime_error)?;
        self.workspace_from_scheduler(scheduler)
    }

    fn resolve_bootstrap(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        let selector = selector_from_wire(selector)?;
        let scheduler = self
            .manager
            .bootstrap(selector, self.observation())
            .map_err(map_runtime_error)?;
        self.workspace_from_scheduler(scheduler)
    }

    fn workspace_output(&self, workspace: &Self::Workspace) -> WorkspaceOutputV1 {
        workspace.output.clone()
    }
}

/// A context-specific production executor. It creates the existing engine with the exact SQLite
/// Store and Store options held by the active scheduler context; there is no global or fake Store.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeContextExecutionV1;
type ProductionExecutionEngineV1 = DaemonExecutionEngineV1<
    Arc<podway_store::SqliteStoreV1>,
    NativeExecutionIdSourceV1,
    WallUtcExecutionClockV1,
    EmbeddedPresetProcedureProviderV1,
    NativeArtifactVerifierV1<SqliteWorkspaceBindingInspectorV1>,
    NativeWorkspaceRevalidatorV1<SqliteWorkspaceBindingInspectorV1>,
>;

impl NativeContextExecutionV1 {
    fn engine(
        context: &WorkspaceSchedulerContextV1,
    ) -> Result<ProductionExecutionEngineV1, crate::execution::ExecutionErrorV1> {
        let options = context.store_options().clone();
        Ok(DaemonExecutionEngineV1::new(
            Arc::clone(context.store()),
            NativeExecutionIdSourceV1,
            WallUtcExecutionClockV1,
            EmbeddedPresetProcedureProviderV1,
            NativeArtifactVerifierV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
            NativeWorkspaceRevalidatorV1::new(SqliteWorkspaceBindingInspectorV1::new(options)),
        ))
    }
}

impl WorkerExecutionV1<WorkspaceSchedulerContextV1> for NativeContextExecutionV1 {
    fn admit(
        &self,
        context: &WorkspaceSchedulerContextV1,
        binding: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
    ) -> Result<AdmitOutcomeV1, crate::execution::ExecutionErrorV1> {
        if context.binding() != binding {
            return Err(
                crate::execution::ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "scheduler context binding changed during admission",
                },
            );
        }
        Self::engine(context)?.admit_for_workspace(binding, request, idempotency_key)
    }

    fn execute_next(
        &self,
        context: &WorkspaceSchedulerContextV1,
        binding: &WorkspaceBindingV1,
        worker: WorkerIdV1,
    ) -> Result<Option<podway_store::TerminalReceiptV1>, crate::execution::ExecutionErrorV1> {
        if context.binding() != binding {
            return Err(
                crate::execution::ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "scheduler context binding changed during claim",
                },
            );
        }
        Self::engine(context)?.execute_next(binding, worker)
    }
}

/// Scheduler progress is only a notification source. The read service always queries the context
/// Store again after this adapter reports a hint or a timeout.
struct SchedulerReadNotificationV1 {
    scheduler: Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    clock: Arc<NativeProductionClockV1>,
}

impl SchedulerReadNotificationV1 {
    fn matches_identity(&self, identity: &podway_store::DurableWorktreeIdentityV1) -> bool {
        self.scheduler.context_snapshot().binding().identity() == identity
    }
}

impl ReadNotificationV1 for SchedulerReadNotificationV1 {
    fn observe(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
    ) -> Result<ReadNotificationVersionV1, ReadNotificationErrorV1> {
        if !self.matches_identity(identity) {
            return Err(ReadNotificationErrorV1::Unavailable);
        }
        Ok(ReadNotificationVersionV1::new(
            self.scheduler.progress_version().get(),
        ))
    }

    fn wait_for_change(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        observed: ReadNotificationVersionV1,
        deadline: MonotonicDeadlineV1,
    ) -> Result<ReadWaitOutcomeV1, ReadNotificationErrorV1> {
        if !self.matches_identity(identity) {
            return Err(ReadNotificationErrorV1::Unavailable);
        }
        if self.scheduler.progress_version().get() != observed.get() {
            return Ok(ReadWaitOutcomeV1::Notified);
        }
        if self.clock.now_millis() >= deadline.millis() {
            return Ok(ReadWaitOutcomeV1::TimedOut);
        }

        let context = self.scheduler.context_snapshot();
        let mut notification = context.notification_version();
        loop {
            if self.scheduler.progress_version().get() != observed.get() {
                return Ok(ReadWaitOutcomeV1::Notified);
            }
            let now = self.clock.now_millis();
            if now >= deadline.millis() {
                return Ok(ReadWaitOutcomeV1::TimedOut);
            }
            context.wait_for_notification_after(
                notification,
                std::time::Duration::from_millis(deadline.millis().saturating_sub(now)),
            );
            if self.scheduler.progress_version().get() != observed.get() {
                return Ok(ReadWaitOutcomeV1::Notified);
            }
            let current_notification = context.notification_version();
            if current_notification != notification {
                return Ok(ReadWaitOutcomeV1::Notified);
            }
            notification = current_notification;
        }
    }
}

/// Concrete authoritative status/next adapter bound to one production scheduler context.
#[derive(Clone)]
pub struct ProductionReadServiceV1 {
    clock: Arc<NativeProductionClockV1>,
}

impl ProductionReadServiceV1 {
    pub fn new(clock: Arc<NativeProductionClockV1>) -> Self {
        Self { clock }
    }

    fn wait(&self, wait: RequestReadWaitV1) -> Result<ReadWaitV1, DispatchFailureV1> {
        match wait {
            RequestReadWaitV1::Immediate => Ok(ReadWaitV1::immediate()),
            RequestReadWaitV1::IdleUntil { timeout_millis } => {
                MonotonicDeadlineV1::after(self.clock.as_ref(), timeout_millis)
                    .map(ReadWaitV1::idle_until)
                    .map_err(map_read_error)
            }
        }
    }

    fn service(
        &self,
        workspace: &ProductionWorkspaceV1,
    ) -> AuthoritativeReadServiceV1<
        Arc<podway_store::SqliteStoreV1>,
        SchedulerReadNotificationV1,
        Arc<NativeProductionClockV1>,
    > {
        let context = workspace.scheduler.context_snapshot();
        AuthoritativeReadServiceV1::new(
            Arc::clone(context.store()),
            SchedulerReadNotificationV1 {
                scheduler: Arc::clone(&workspace.scheduler),
                clock: Arc::clone(&self.clock),
            },
            Arc::clone(&self.clock),
        )
    }
}

impl DispatcherReadServiceV1<ProductionWorkspaceV1> for ProductionReadServiceV1 {
    fn status(
        &self,
        workspace: &ProductionWorkspaceV1,
        wait: RequestReadWaitV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let result = self
            .service(workspace)
            .status(context.binding().identity(), self.wait(wait)?)
            .map_err(map_read_error)?;
        protocol_result_map(&result).map(|result| DispatcherReadOutputV1::new(result, Vec::new()))
    }

    fn next(
        &self,
        workspace: &ProductionWorkspaceV1,
        wait: RequestReadWaitV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let result = self
            .service(workspace)
            .next(context.binding().identity(), self.wait(wait)?)
            .map_err(map_read_error)?;
        protocol_result_map(&result).map(|result| DispatcherReadOutputV1::new(result, Vec::new()))
    }
}

/// Concrete mutation admission and drain adapter over the same scheduler pointer selected by the
/// runtime manager.
#[derive(Clone)]
pub struct ProductionMutationWorkerV1 {
    worker: DaemonWorkerV1<
        WorkspaceSchedulerContextV1,
        NativeContextExecutionV1,
        NativeProductionClockV1,
    >,
    clock: Arc<NativeProductionClockV1>,
}
/// The exact production dispatcher and worker pair used during daemon startup.
pub struct ProductionDispatcherCompositionV1 {
    dispatcher: ProductionRequestDispatcherV1,
    worker: ProductionMutationWorkerV1,
}

impl ProductionDispatcherCompositionV1 {
    pub fn dispatcher(&self) -> &ProductionRequestDispatcherV1 {
        &self.dispatcher
    }

    pub fn worker(&self) -> &ProductionMutationWorkerV1 {
        &self.worker
    }

    pub fn into_dispatcher(self) -> ProductionRequestDispatcherV1 {
        self.dispatcher
    }

    pub fn into_parts(self) -> (ProductionRequestDispatcherV1, ProductionMutationWorkerV1) {
        (self.dispatcher, self.worker)
    }
}

impl ProductionMutationWorkerV1 {
    pub fn new(worker_id: WorkerIdV1, clock: Arc<NativeProductionClockV1>) -> Self {
        Self {
            worker: DaemonWorkerV1::new(
                Arc::new(NativeContextExecutionV1),
                Arc::clone(&clock),
                worker_id,
            ),
            clock,
        }
    }

    fn completion_mode(
        &self,
        wait: MutationWaitV1,
    ) -> Result<WorkerCompletionModeV1, DispatchFailureV1> {
        match wait {
            MutationWaitV1::Detached => Ok(WorkerCompletionModeV1::Detached),
            MutationWaitV1::UntilTerminal { timeout_millis } => {
                MonotonicDeadlineV1::after(self.clock.as_ref(), timeout_millis)
                    .map(WorkerCompletionModeV1::WaitUntil)
                    .map_err(map_read_error)
            }
        }
    }

    /// Drains only schedulers returned by the runtime manager during startup recovery.
    pub fn drain_recovered_queues(
        &self,
        schedulers: impl IntoIterator<Item = Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
    ) -> Vec<Result<crate::worker::WorkerDrainReportV1, WorkerErrorV1>> {
        self.worker.drain_recovered_queues(schedulers)
    }
}

impl MutationAdmissionWorkerV1<ProductionWorkspaceV1> for ProductionMutationWorkerV1 {
    fn admit_and_wait(
        &self,
        workspace: &ProductionWorkspaceV1,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
        wait: MutationWaitV1,
    ) -> Result<MutationDispatchOutcomeV1, DispatchFailureV1> {
        let store_idempotency_key = StoreIdempotencyKeyV1::new(idempotency_key.as_str())
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let submission = self
            .worker
            .submit(
                &workspace.scheduler,
                request,
                store_idempotency_key,
                self.completion_mode(wait)?,
            )
            .map_err(|error| map_mutation_worker_error(request, error))?;
        let terminal =
            terminal_replay(submission.admission()).or_else(|| match submission.completion() {
                Some(WorkerWaitResultV1::Terminal(receipt)) => Some(receipt.as_ref()),
                Some(WorkerWaitResultV1::TimedOut(_)) | None => None,
            });
        if let Some(receipt) = terminal {
            return Ok(MutationDispatchOutcomeV1::Terminal {
                job: job_output_from_terminal_receipt(receipt)?,
                result: terminal_result(receipt, request.command())?,
            });
        }

        let job = match submission.completion() {
            Some(WorkerWaitResultV1::TimedOut(view)) => job_output(view)?,
            _ => job_output_from_context(&workspace.scheduler, submission.admission())?,
        };
        match submission.completion() {
            Some(WorkerWaitResultV1::TimedOut(_)) => {
                Ok(MutationDispatchOutcomeV1::TimedOut { job })
            }
            Some(WorkerWaitResultV1::Terminal(_)) => {
                Err(DispatchFailureV1::new(DispatchFailureKindV1::Internal))
            }
            None => Ok(MutationDispatchOutcomeV1::Detached { job }),
        }
    }
}

/// Ready-to-serve concrete dispatcher type.
pub type ProductionRequestDispatcherV1 = RequestDispatcherV1Adapter<
    ProductionWorkspaceRuntimeV1,
    ProductionReadServiceV1,
    ProductionMutationWorkerV1,
    Arc<NativeProductionClockV1>,
    CatalogDispatchErrorMapperV1,
>;

impl DispatchResponseMetadataV1 for Arc<NativeProductionClockV1> {
    fn generated_at(&self) -> Rfc3339MillisV1 {
        self.as_ref().generated_at()
    }
}

/// Builds the exact production dispatcher and worker pair over the manager-owned scheduler registry.
pub fn compose_dispatcher_with_worker_v1(
    manager: Arc<WorkspaceRuntimeManagerV1>,
    worker_id: WorkerIdV1,
) -> ProductionDispatcherCompositionV1 {
    let clock = Arc::new(NativeProductionClockV1::default());
    let worker = ProductionMutationWorkerV1::new(worker_id, Arc::clone(&clock));
    let dispatcher = RequestDispatcherV1Adapter::new(
        ProductionWorkspaceRuntimeV1::new(manager, Arc::clone(&clock)),
        ProductionReadServiceV1::new(Arc::clone(&clock)),
        worker.clone(),
        clock,
        CatalogDispatchErrorMapperV1,
    );
    ProductionDispatcherCompositionV1 { dispatcher, worker }
}

/// Builds a production dispatcher from the single manager-owned scheduler registry.
pub fn compose_dispatcher_v1(
    manager: Arc<WorkspaceRuntimeManagerV1>,
    worker_id: WorkerIdV1,
) -> ProductionRequestDispatcherV1 {
    compose_dispatcher_with_worker_v1(manager, worker_id).into_dispatcher()
}

fn selector_from_wire(
    selector: &WorktreeSelectorWireV1,
) -> Result<WorktreeSelectorV1, DispatchFailureV1> {
    let path_bytes = Base64UrlPathBytesV1::new(selector.path_bytes_base64url().to_owned())
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
    let display = DiagnosticPathDisplayV1::new(selector.display().to_owned())
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
    WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_base64url(path_bytes, display),
    )
    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))
}

fn job_output_from_context(
    scheduler: &Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    admission: &AdmitOutcomeV1,
) -> Result<JobOutputV1, DispatchFailureV1> {
    let job_id = match admission {
        AdmitOutcomeV1::New(receipt) => receipt.job_id(),
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt)) => receipt.job_id(),
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(_)) => {
            return Err(terminal_replay_integrity_failure());
        }
    };
    let context = scheduler.context_snapshot();
    let view = context
        .read_job(job_id)
        .map_err(map_store_error)?
        .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::JobNotFound))?;
    job_output(&view)
}

fn terminal_replay_integrity_failure() -> DispatchFailureV1 {
    DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
}

fn job_output_from_terminal_receipt(
    receipt: &PersistedTerminalReceiptV1,
) -> Result<JobOutputV1, DispatchFailureV1> {
    let projection = receipt
        .job_projection()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let state = match projection.state() {
        PersistedTerminalJobStateV1::Succeeded => JobStateV1::Succeeded,
        PersistedTerminalJobStateV1::Failed => JobStateV1::Failed,
        PersistedTerminalJobStateV1::Cancelled => JobStateV1::Cancelled,
    };
    JobOutputV1::new(
        receipt.job().job_id().clone(),
        receipt.job().identity_sequence(),
        state,
        rfc3339_millis(projection.submitted_at())?,
        projection.claimed_at().map(rfc3339_millis).transpose()?,
        Some(rfc3339_millis(projection.finished_at())?),
    )
    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn job_output(view: &JobViewV1) -> Result<JobOutputV1, DispatchFailureV1> {
    JobOutputV1::new(
        view.job().job_id().clone(),
        view.job().identity_sequence(),
        match view.state() {
            StoreJobStateV1::Queued => JobStateV1::Queued,
            StoreJobStateV1::Running => JobStateV1::Running,
            StoreJobStateV1::Succeeded => JobStateV1::Succeeded,
            StoreJobStateV1::Failed => JobStateV1::Failed,
            StoreJobStateV1::Cancelled => JobStateV1::Cancelled,
        },
        rfc3339_millis(view.submitted_at())?,
        view.claimed_at().map(rfc3339_millis).transpose()?,
        view.finished_at().map(rfc3339_millis).transpose()?,
    )
    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn terminal_replay(admission: &AdmitOutcomeV1) -> Option<&PersistedTerminalReceiptV1> {
    match admission {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => Some(receipt),
        AdmitOutcomeV1::New(_)
        | AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(_)) => None,
    }
}

fn terminal_result(
    receipt: &PersistedTerminalReceiptV1,
    command: &SliceCommandV1,
) -> Result<DispatcherTerminalResultV1, DispatchFailureV1> {
    match receipt.result() {
        PersistedTerminalResultV1::Success(result) => {
            let session = terminal_session_projection(result, receipt.session_projection())?;
            let result = match result {
                PersistedDomainResultV1::WorkspaceInitialized { revision, .. } => Map::from_iter([
                    ("initialized".to_owned(), Value::Bool(true)),
                    ("revision".to_owned(), Value::from(revision.get())),
                ]),
                PersistedDomainResultV1::WorkspaceReset { revision, .. } => Map::from_iter([
                    ("reset".to_owned(), Value::Bool(true)),
                    ("revision".to_owned(), Value::from(revision.get())),
                ]),
                PersistedDomainResultV1::SessionChanged {
                    revision_before,
                    revision_after,
                    changed,
                    ..
                } => Map::from_iter([
                    ("changed".to_owned(), Value::Bool(*changed)),
                    (
                        "revision_before".to_owned(),
                        Value::from(revision_before.get()),
                    ),
                    (
                        "revision_after".to_owned(),
                        Value::from(revision_after.get()),
                    ),
                ]),
                PersistedDomainResultV1::ItemChanged {
                    item_id,
                    revision_before,
                    revision_after,
                    changed,
                    ..
                } => Map::from_iter([
                    ("changed".to_owned(), Value::Bool(*changed)),
                    (
                        "item_id".to_owned(),
                        Value::String(item_id.as_str().to_owned()),
                    ),
                    (
                        "revision_before".to_owned(),
                        Value::from(revision_before.get()),
                    ),
                    (
                        "revision_after".to_owned(),
                        Value::from(revision_after.get()),
                    ),
                ]),
            };
            Ok(DispatcherTerminalResultV1::Output(
                DispatcherTerminalOutputV1::new(session, result, Vec::new()),
            ))
        }
        PersistedTerminalResultV1::Failure(error) => Ok(DispatcherTerminalResultV1::Error(
            map_domain_error(error, command),
        )),
        PersistedTerminalResultV1::Cancelled => Ok(DispatcherTerminalResultV1::Error(
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceMaintenance),
        )),
    }
}
fn terminal_session_projection(
    result: &PersistedDomainResultV1,
    persisted: Option<&PersistedTerminalSessionProjectionV1>,
) -> Result<Option<SessionOutputV1>, DispatchFailureV1> {
    let requires_session_projection = matches!(
        result,
        PersistedDomainResultV1::SessionChanged { .. }
            | PersistedDomainResultV1::ItemChanged { .. }
    );
    let persisted = match (requires_session_projection, persisted) {
        (true, Some(persisted)) => persisted,
        (true, None) | (false, Some(_)) => return Err(terminal_replay_integrity_failure()),
        (false, None) => return Ok(None),
    };
    let lifecycle = match persisted.lifecycle() {
        PersistedSessionLifecycleV1::Running => SessionLifecycleV1::Running,
        PersistedSessionLifecycleV1::Completed => SessionLifecycleV1::Completed,
        PersistedSessionLifecycleV1::Cancelled => SessionLifecycleV1::Cancelled,
    };
    SessionOutputV1::new(
        persisted.session_id().clone(),
        persisted.task_title(),
        lifecycle,
        persisted.revision_before(),
        persisted.revision_after(),
    )
    .map(Some)
    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn map_domain_error(error: &PersistedDomainErrorV1, command: &SliceCommandV1) -> DispatchFailureV1 {
    match error {
        PersistedDomainErrorV1::PreconditionFailed { expected, actual } => {
            let kind = if matches!(
                command,
                SliceCommandV1::ItemCheck(_)
                    | SliceCommandV1::ItemSet(_)
                    | SliceCommandV1::ItemAdd(_)
                    | SliceCommandV1::ItemAttachPath(_)
            ) {
                DispatchFailureKindV1::ItemRevisionConflict
            } else {
                DispatchFailureKindV1::SessionRevisionConflict
            };
            DispatchFailureV1::new(kind).with_details(
                DispatchErrorDetailsV1::default()
                    .with_expected_revision(*expected)
                    .with_current_revision(*actual),
            )
        }
        PersistedDomainErrorV1::ItemNotFound { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::ItemNotFound)
        }
        PersistedDomainErrorV1::BlockerNotCurrent => {
            DispatchFailureV1::new(DispatchFailureKindV1::BlockerNotCurrent)
        }
        PersistedDomainErrorV1::InvalidTransition { .. }
        | PersistedDomainErrorV1::InvalidState { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionNotRunning)
        }
        PersistedDomainErrorV1::RevisionOverflow { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::Internal)
        }
        PersistedDomainErrorV1::RequiredItemsMissing => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequiredItemsMissing)
        }
        PersistedDomainErrorV1::BlockersPresent => {
            DispatchFailureV1::new(DispatchFailureKindV1::BlockersPresent)
        }
        PersistedDomainErrorV1::ArtifactChanged => {
            DispatchFailureV1::new(DispatchFailureKindV1::ArtifactChanged)
        }
        PersistedDomainErrorV1::EmptyValue { .. }
        | PersistedDomainErrorV1::ValueTooLong { .. }
        | PersistedDomainErrorV1::InvalidUuid { .. }
        | PersistedDomainErrorV1::InvalidIdentifier { .. }
        | PersistedDomainErrorV1::InvalidSha256Digest => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid)
        }
    }
}

fn protocol_result_map<T: serde::Serialize>(
    result: &T,
) -> Result<Map<String, Value>, DispatchFailureV1> {
    serde_json::to_value(result)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn map_mutation_worker_error(request: &SliceRequestV1, error: WorkerErrorV1) -> DispatchFailureV1 {
    let failure = map_worker_error(error);
    if failure.kind() == DispatchFailureKindV1::SessionRevisionConflict
        && matches!(
            request.command(),
            SliceCommandV1::ItemCheck(_)
                | SliceCommandV1::ItemSet(_)
                | SliceCommandV1::ItemAdd(_)
                | SliceCommandV1::ItemAttachPath(_)
        )
    {
        failure.with_kind(DispatchFailureKindV1::ItemRevisionConflict)
    } else {
        failure
    }
}
fn map_worker_error(error: WorkerErrorV1) -> DispatchFailureV1 {
    match error {
        WorkerErrorV1::Store(error) => map_store_error(error),
        WorkerErrorV1::Execution(error) => match error {
            crate::execution::ExecutionErrorV1::Store(error) => map_store_error(error),
            crate::execution::ExecutionErrorV1::BoundaryDomain(_) => {
                DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid)
            }
            crate::execution::ExecutionErrorV1::BoundaryTransient { .. } => {
                DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
            }
            crate::execution::ExecutionErrorV1::InvalidPersistedExecution { .. }
            | crate::execution::ExecutionErrorV1::InvalidStoreValue(_) => {
                DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
            }
        },
        WorkerErrorV1::JobNotFound(_) => DispatchFailureV1::new(DispatchFailureKindV1::JobNotFound),
        WorkerErrorV1::Progress(_)
        | WorkerErrorV1::BackgroundPanicked
        | WorkerErrorV1::RecoveryRequired
        | WorkerErrorV1::RetirementRejected => {
            DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
        }
    }
}

fn map_read_error(error: ReadServiceErrorV1) -> DispatchFailureV1 {
    match error {
        ReadServiceErrorV1::Store(error) => map_store_error(error),
        ReadServiceErrorV1::JobNotFound { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::JobNotFound)
        }
        ReadServiceErrorV1::WaitTimedOut => {
            DispatchFailureV1::new(DispatchFailureKindV1::JobWaitTimeout)
        }
        ReadServiceErrorV1::Notification(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
        }
        ReadServiceErrorV1::DeadlineOverflow | ReadServiceErrorV1::TimestampOutOfRange => {
            DispatchFailureV1::new(DispatchFailureKindV1::Internal)
        }
        ReadServiceErrorV1::MissingSession => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionNotFound)
        }
        ReadServiceErrorV1::ResultShapeConversion
        | ReadServiceErrorV1::InconsistentState { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
        }
    }
}

fn map_runtime_error(error: WorkspaceRuntimeErrorV1) -> DispatchFailureV1 {
    match error {
        WorkspaceRuntimeErrorV1::Resolution(error) => map_resolution_error(error),
        WorkspaceRuntimeErrorV1::Layout(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceInitConflict)
        }
        WorkspaceRuntimeErrorV1::Store(error) => map_store_error(error),
        WorkspaceRuntimeErrorV1::Registry(_) | WorkspaceRuntimeErrorV1::Scheduler(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
        }
        WorkspaceRuntimeErrorV1::ConfigRead { .. }
        | WorkspaceRuntimeErrorV1::ConfigAdmission(_)
        | WorkspaceRuntimeErrorV1::StoreOptions(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceConfigInvalid)
        }
        WorkspaceRuntimeErrorV1::BindingDisappeared { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceNotInitialized)
        }
        WorkspaceRuntimeErrorV1::BindingIdentityMismatch { .. }
        | WorkspaceRuntimeErrorV1::BindingMismatch { .. }
        | WorkspaceRuntimeErrorV1::RebindEvidenceMismatch { .. }
        | WorkspaceRuntimeErrorV1::RevalidationKeyMismatch { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceIdentityConflict)
        }
        WorkspaceRuntimeErrorV1::RuntimeDirectoryMissing { .. }
        | WorkspaceRuntimeErrorV1::RuntimePathMismatch { .. }
        | WorkspaceRuntimeErrorV1::RuntimePathsUnsupportedPlatform => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspacePathUnsafe)
        }
    }
}

fn map_resolution_error(error: WorkspaceResolutionErrorV1) -> DispatchFailureV1 {
    match error {
        WorkspaceResolutionErrorV1::Selector { .. }
        | WorkspaceResolutionErrorV1::StoredRootPathInvalid { .. }
        | WorkspaceResolutionErrorV1::WorkspaceRootPathInvalid { .. }
        | WorkspaceResolutionErrorV1::RuntimeDirectoryPathInvalid { .. }
        | WorkspaceResolutionErrorV1::RuntimeDirectoryPathUnsupportedPlatform
        | WorkspaceResolutionErrorV1::WorkspaceRootConversion { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspacePathUnsafe)
        }
        WorkspaceResolutionErrorV1::Git { source, .. } => match source {
            podway_git::GitResolverErrorV1::NonGitRepository => {
                DispatchFailureV1::new(DispatchFailureKindV1::NotGitWorktree)
            }
            podway_git::GitResolverErrorV1::BareRepository => {
                DispatchFailureV1::new(DispatchFailureKindV1::BareGitRepository)
            }
            podway_git::GitResolverErrorV1::WorktreeDeleted => {
                DispatchFailureV1::new(DispatchFailureKindV1::WorktreeGone)
            }
            podway_git::GitResolverErrorV1::CopiedWorkspaceUuid { .. }
            | podway_git::GitResolverErrorV1::IdentityMismatch { .. }
            | podway_git::GitResolverErrorV1::MoveConflict { .. } => {
                DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceIdentityConflict)
            }
            podway_git::GitResolverErrorV1::PathEscape { .. }
            | podway_git::GitResolverErrorV1::SymlinkEscape { .. } => {
                DispatchFailureV1::new(DispatchFailureKindV1::PathOutsideWorktree)
            }
            podway_git::GitResolverErrorV1::Selector(_) => {
                DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid)
            }
            podway_git::GitResolverErrorV1::PermissionDenied { .. }
            | podway_git::GitResolverErrorV1::Io { .. }
            | podway_git::GitResolverErrorV1::Representation { .. }
            | podway_git::GitResolverErrorV1::Invariant { .. }
            | podway_git::GitResolverErrorV1::WorkspaceLayoutCleanup { .. } => {
                DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
            }
        },
        WorkspaceResolutionErrorV1::BindingInspection { source } => match source {
            crate::workspace::WorkspaceBindingInspectionErrorV1::Store(error) => {
                map_store_error(error)
            }
        },
        WorkspaceResolutionErrorV1::ExistingBindingMissing => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceNotInitialized)
        }
        WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceAlreadyInitialized)
        }
        WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { .. }
        | WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch { .. }
        | WorkspaceResolutionErrorV1::PreliminaryIdentityWasNotCandidate { .. }
        | WorkspaceResolutionErrorV1::RevalidatedIdentityStateMismatch { .. }
        | WorkspaceResolutionErrorV1::RevalidatedStoreIdentityMismatch { .. }
        | WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceIdentityConflict)
        }
    }
}

fn map_store_error(error: StoreErrorV1) -> DispatchFailureV1 {
    match error {
        StoreErrorV1::IdempotencyDigestConflictV1 { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::IdempotencyKeyReused)
        }
        StoreErrorV1::JobNotFoundV1 { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::JobNotFound)
        }
        StoreErrorV1::PreconditionConflictV1 { expected, actual } => {
            let mut details = DispatchErrorDetailsV1::default();
            if let Some(expected) = expected {
                details = details.with_expected_revision(expected);
            }
            if let Some(actual) = actual {
                details = details.with_current_revision(actual);
            }
            DispatchFailureV1::new(DispatchFailureKindV1::SessionRevisionConflict)
                .with_details(details)
        }
        StoreErrorV1::NewerStateV1 { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceSchemaUnsupported)
        }
        StoreErrorV1::StorageUnavailableV1 { .. }
        | StoreErrorV1::AlreadyClaimedV1 { .. }
        | StoreErrorV1::CancellationLostV1 { .. }
        | StoreErrorV1::ClaimStaleV1 { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
        }
        StoreErrorV1::CorruptStateV1 { .. }
        | StoreErrorV1::InternalInvariantViolationV1 { .. }
        | StoreErrorV1::InvalidStateV1(_)
        | StoreErrorV1::PrimaryOperationAndCleanupFailureV1 { .. }
        | StoreErrorV1::StorageIntegrityV1 { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
        }
    }
}

fn rfc3339_millis(value: UnixMillis) -> Result<Rfc3339MillisV1, DispatchFailureV1> {
    let seconds = value.get() / 1_000;
    let millis = value.get() % 1_000;
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_date_from_unix_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    Rfc3339MillisV1::new(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    ))
    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn civil_date_from_unix_days(days: u64) -> (i128, i128, i128) {
    let z = i128::from(days) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use podway_core::{JobId, Revision, SessionId, Sha256Digest};
    use podway_store::{
        JobReceiptV1, PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1,
        PersistedTerminalSessionProjectionV1,
    };

    #[test]
    fn terminal_job_projection_builds_output_without_a_live_job_view() {
        let job = JobReceiptV1::new(
            7,
            JobId::new("00000000-0000-4000-8000-000000000007").unwrap(),
            Sha256Digest::new(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
        );
        let projection = PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Cancelled,
            UnixMillis::new(10),
            None,
            UnixMillis::new(12),
        )
        .unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            job,
            PersistedTerminalResultV1::Cancelled,
            projection,
            None,
        )
        .unwrap();

        let output = job_output_from_terminal_receipt(&receipt).unwrap();

        assert_eq!(output.id(), receipt.job().job_id());
        assert_eq!(output.sequence(), 7);
        assert_eq!(output.state(), JobStateV1::Cancelled);
        assert_eq!(output.submitted_at().as_str(), "1970-01-01T00:00:00.010Z");
        assert!(output.claimed_at().is_none());
        assert_eq!(
            output.finished_at().map(Rfc3339MillisV1::as_str),
            Some("1970-01-01T00:00:00.012Z")
        );
    }

    #[test]
    fn terminal_session_projection_is_independent_of_current_session_state() {
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000008").unwrap();
        let result = PersistedDomainResultV1::SessionChanged {
            session_id: session_id.clone(),
            revision_before: Revision::new(4),
            revision_after: Revision::new(5),
            changed: true,
        };
        let projection = PersistedTerminalSessionProjectionV1::new(
            session_id.clone(),
            "Immutable terminal session".to_owned(),
            PersistedSessionLifecycleV1::Running,
            Revision::new(4),
            Revision::new(5),
        )
        .unwrap();

        let output = terminal_session_projection(&result, Some(&projection))
            .unwrap()
            .expect("persisted session projection must produce output");

        assert_eq!(output.id(), &session_id);
        assert_eq!(output.title(), "Immutable terminal session");
        assert_eq!(output.lifecycle(), SessionLifecycleV1::Running);
        assert_eq!(output.revision_before(), Revision::new(4));
        assert_eq!(output.revision_after(), Revision::new(5));
    }
    #[test]
    fn projectionless_terminal_replay_fails_closed() {
        let job = JobReceiptV1::new(
            9,
            JobId::new("00000000-0000-4000-8000-000000000009").unwrap(),
            Sha256Digest::new(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .unwrap(),
        );
        let missing_job_projection =
            PersistedTerminalReceiptV1::new(job.clone(), PersistedTerminalResultV1::Cancelled);
        assert_eq!(
            job_output_from_terminal_receipt(&missing_job_projection)
                .unwrap_err()
                .kind(),
            DispatchFailureKindV1::WorkspaceStateUnreadable
        );

        let session_id = SessionId::new("00000000-0000-4000-8000-000000000010").unwrap();
        let result = PersistedDomainResultV1::SessionChanged {
            session_id,
            revision_before: Revision::new(1),
            revision_after: Revision::new(2),
            changed: true,
        };
        let missing_session_projection =
            PersistedTerminalReceiptV1::new(job, PersistedTerminalResultV1::Success(result));
        assert_eq!(
            terminal_result(&missing_session_projection, &SliceCommandV1::WorkspaceInit)
                .unwrap_err()
                .kind(),
            DispatchFailureKindV1::WorkspaceStateUnreadable
        );
    }
}
