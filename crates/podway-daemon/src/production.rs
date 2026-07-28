//! Concrete G006 production composition.
//!
//! This module contains the daemon boundary glue only. Workspace resolution remains in
//! [`WorkspaceRuntimeManagerV1`], state remains in SQLite, and protocol routing remains in
//! [`RequestDispatcherV1Adapter`]. No adapter derives an identity from a path or persists a wire
//! document directly.

use std::{sync::Arc, time::Instant};

use podway_core::{
    AttemptId, CommandContextV1, DomainError, ProcedureSnapshotId, ReopenSessionV1, ResetSessionV1,
    ReturnSessionV1, Revision, SessionCommandV1, SessionId, Sha256Digest, StartReplaceSessionV1,
    StartSessionV1, UnixMillis, preview_transition_v1,
};
use podway_git::{
    Base64UrlPathBytesV1, DiagnosticPathDisplayV1, LosslessPathV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    CommandNameV1, IdempotencyKeyV1, JobOutputV1, JobStateV1, RequestIdV1, Rfc3339MillisV1,
    SessionLifecycleV1, SessionOutputV1, SliceCommandV1, SliceRequestV1,
    TerminalJobCancellationProjectionV1, TerminalJobResponseV1, TerminalJobSuccessResultV1,
    WorkspaceOutputV1, WorktreeSelectorWireV1, canonical_reset_all_identity_v1,
};
use podway_store::{
    AdmitOutcomeV1, CancelOutcomeV1, CanonicalRequestDigestV1, DurableWorktreeIdentityV1,
    IdempotencyKeyV1 as StoreIdempotencyKeyV1, JobListQueryV1, JobReceiptOrTerminalV1,
    JobStateV1 as StoreJobStateV1, JobViewV1, PersistedTerminalJobStateV1,
    PersistedTerminalReceiptV1, PersistedTerminalSessionProjectionV1, SqliteStoreOptionsV1,
    StoreContractV1, StoreErrorV1, StoreIdempotencyReadContractV1, StoreReadContractV1, WorkerIdV1,
    WorkspaceBindingV1,
    codec::{
        PersistedDomainCommandV1, PersistedDomainErrorV1, PersistedDomainResultV1,
        PersistedSessionLifecycleV1, PersistedTerminalResultV1,
    },
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use crate::{
    dispatch::{
        CatalogDispatchErrorMapperV1, DispatchErrorDetailsV1, DispatchFailureKindV1,
        DispatchFailureV1, DispatchResponseMetadataV1, DispatcherControlServiceV1,
        DispatcherJobOutputV1, DispatcherNextRequestV1, DispatcherPreviewServiceV1,
        DispatcherReadOutputV1, DispatcherReadServiceV1, DispatcherStatusRequestV1,
        DispatcherTerminalOutputV1, DispatcherTerminalResultV1, DispatcherWorkspaceOutputV1,
        MutationAdmissionWorkerV1, MutationDispatchOutcomeV1, MutationResponseContextV1,
        MutationWaitV1, RequestDispatcherV1Adapter, RequestReadWaitV1, TerminalResponseContextV1,
        WorkspaceRuntimeV1, terminal_response_envelope_v1,
    },
    execution::{
        DaemonExecutionEngineV1, ExecutionClockV1, ExecutionErrorV1, ProcedureProviderV1,
        ResetAllPreparationOutcomeV1, admitted_start_procedure_digest_v1,
    },
    native_execution::{
        NativeArtifactVerifierV1, NativeExecutionIdSourceV1, NativeProcedureProviderV1,
        NativeWorkspaceRevalidatorV1, WallUtcExecutionClockV1,
    },
    observability::ObservabilityEmitterV1,
    read_service::{
        AuthoritativeReadServiceV1, MonotonicClockV1, MonotonicDeadlineV1, ReadNotificationErrorV1,
        ReadNotificationV1, ReadNotificationVersionV1, ReadServiceErrorV1, ReadWaitOutcomeV1,
        ReadWaitV1,
    },
    runtime_workspace::{
        ResetSourceAuthorityV1, WorkspaceRuntimeErrorV1, WorkspaceRuntimeManagerV1,
        WorkspaceRuntimeObservationV1, WorkspaceSchedulerContextV1,
        WorkspaceSchedulerRevalidationV1, WorkspaceStoreReadFacadeV1, WorkspaceStoreSlotV1,
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
    fn readonly_workspace(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<ProductionWorkspaceV1, DispatchFailureV1> {
        let expected_workspace_id = selector.expected_uuid();
        let selector = selector_from_wire(selector)?;
        let resolution = self
            .manager
            .resolve_existing_readonly(selector, expected_workspace_id)
            .map_err(map_runtime_error)?;
        let scheduler = resolution
            .active_scheduler()
            .cloned()
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable))?;
        self.workspace_from_scheduler(scheduler)
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
    fn resolve_existing_readonly(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        self.readonly_workspace(selector)
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
    fn doctor(
        &self,
        selector: &WorktreeSelectorWireV1,
        deep: bool,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1> {
        let workspace = self.readonly_workspace(selector)?;
        let mut result = Map::from_iter([
            ("deep".to_owned(), Value::Bool(deep)),
            ("workspace_state_readable".to_owned(), Value::Bool(true)),
        ]);
        let warnings = if deep {
            match self.manager.revalidate_scheduler(workspace.scheduler()) {
                Ok(WorkspaceSchedulerRevalidationV1::Current) => {
                    result.insert("healthy".to_owned(), Value::Bool(true));
                    result.insert(
                        "git_store_binding_revalidated".to_owned(),
                        Value::Object(Map::from_iter([(
                            "outcome".to_owned(),
                            Value::String("current".to_owned()),
                        )])),
                    );
                    Vec::new()
                }
                Ok(WorkspaceSchedulerRevalidationV1::RetireRequired { .. }) => {
                    result.insert("healthy".to_owned(), Value::Bool(false));
                    result.insert(
                        "git_store_binding_revalidated".to_owned(),
                        Value::Object(Map::from_iter([(
                            "outcome".to_owned(),
                            Value::String("retire_required".to_owned()),
                        )])),
                    );
                    result.insert(
                        "findings".to_owned(),
                        Value::Array(vec![Value::Object(Map::from_iter([
                            (
                                "code".to_owned(),
                                Value::String("workspace_identity_changed".to_owned()),
                            ),
                            ("severity".to_owned(), Value::String("error".to_owned())),
                        ]))]),
                    );
                    Vec::new()
                }
                Err(_) => {
                    result.insert("healthy".to_owned(), Value::Bool(false));
                    result.insert(
                        "git_store_binding_revalidated".to_owned(),
                        Value::Object(Map::from_iter([(
                            "outcome".to_owned(),
                            Value::String("error".to_owned()),
                        )])),
                    );
                    result.insert(
                        "findings".to_owned(),
                        Value::Array(vec![Value::Object(Map::from_iter([
                            (
                                "code".to_owned(),
                                Value::String("git_store_binding_revalidation_failed".to_owned()),
                            ),
                            ("severity".to_owned(), Value::String("error".to_owned())),
                        ]))]),
                    );
                    Vec::new()
                }
            }
        } else {
            result.insert("healthy".to_owned(), Value::Bool(true));
            result.insert(
                "git_store_binding_revalidated".to_owned(),
                Value::Object(Map::from_iter([(
                    "outcome".to_owned(),
                    Value::String("not_requested".to_owned()),
                )])),
            );
            Vec::new()
        };
        Ok(DispatcherWorkspaceOutputV1::new(
            workspace.output,
            result,
            warnings,
        ))
    }

    fn show(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1> {
        let workspace = self.readonly_workspace(selector)?;
        Ok(DispatcherWorkspaceOutputV1::new(
            workspace.output,
            Map::from_iter([("initialized".to_owned(), Value::Bool(true))]),
            Vec::new(),
        ))
    }

    fn repair(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1> {
        let expected_workspace_id = selector.expected_uuid();
        let selector = selector_from_wire(selector)?;
        let resolved = self
            .manager
            .resolver()
            .resolve_existing(selector.clone(), expected_workspace_id)
            .map_err(map_resolution_error)?;
        let workspace_uuid = resolved.store_identity().workspace_uuid().clone();
        let registry_before = self
            .manager
            .registry()
            .lookup(&workspace_uuid)
            .map_err(|error| map_runtime_error(WorkspaceRuntimeErrorV1::Registry(error)))?
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let moved = resolved.move_metadata().relocated_from_prior_root();

        // A repair is admitted only after the resolver has bound durable SQLite identity to two
        // fresh Git observations. Supplying that resolved UUID back to activation prevents a
        // concurrent copied database from being adopted between the proof and the metadata CAS.
        let scheduler = self
            .manager
            .resolve_existing(selector, Some(&workspace_uuid), self.observation())
            .map_err(map_runtime_error)?;
        let registry_after = self
            .manager
            .registry()
            .lookup(&workspace_uuid)
            .map_err(|error| map_runtime_error(WorkspaceRuntimeErrorV1::Registry(error)))?
            .ok_or_else(|| {
                DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
            })?;
        let registry_reconciled =
            registry_before.last_known_root() != registry_after.last_known_root();
        let mut changes = Vec::new();
        if moved {
            changes.push(Value::String(
                "workspace_binding.last_validated_root".to_owned(),
            ));
        }
        if registry_reconciled {
            changes.push(Value::String("registry.last_known_root".to_owned()));
        }
        let changed = !changes.is_empty();
        let workspace = self.workspace_from_scheduler(scheduler)?;
        Ok(DispatcherWorkspaceOutputV1::new(
            workspace.output,
            Map::from_iter([
                ("changed".to_owned(), Value::Bool(changed)),
                ("changes".to_owned(), Value::Array(changes)),
                (
                    "moved_identity_proven".to_owned(),
                    Value::Bool(moved || !registry_reconciled),
                ),
            ]),
            Vec::new(),
        ))
    }
}

/// A context-specific production executor. It creates the existing engine with the scheduler-owned
/// Store slot and options; reset preparation may use only an explicitly unavailable slot.
#[derive(Clone, Debug, Default)]
pub(crate) struct NativeContextExecutionV1 {
    observability: Option<ObservabilityEmitterV1>,
}
type ProductionExecutionEngineV1 = DaemonExecutionEngineV1<
    Arc<WorkspaceStoreSlotV1>,
    NativeExecutionIdSourceV1,
    WallUtcExecutionClockV1,
    NativeProcedureProviderV1<SqliteWorkspaceBindingInspectorV1>,
    NativeArtifactVerifierV1<SqliteWorkspaceBindingInspectorV1>,
    NativeWorkspaceRevalidatorV1<SqliteWorkspaceBindingInspectorV1>,
>;

impl NativeContextExecutionV1 {
    fn new(observability: Option<ObservabilityEmitterV1>) -> Self {
        Self { observability }
    }

    fn engine(
        context: &WorkspaceSchedulerContextV1,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Result<ProductionExecutionEngineV1, crate::execution::ExecutionErrorV1> {
        let options = context.store_options().clone();
        Ok(DaemonExecutionEngineV1::with_observability(
            context.store_for_mutation(),
            NativeExecutionIdSourceV1,
            WallUtcExecutionClockV1,
            NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
            NativeArtifactVerifierV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
            NativeWorkspaceRevalidatorV1::new(SqliteWorkspaceBindingInspectorV1::new(options)),
            observability,
        ))
    }
    /// Builds a preparation-only engine for a reset whose previous Store cannot be read. Its
    /// closed slot is never supplied as idempotency authority, so this engine cannot fabricate a
    /// Store replay or admit work into the old generation.
    fn unavailable_reset_preparation_engine() -> ProductionExecutionEngineV1 {
        let options = SqliteStoreOptionsV1::new(1)
            .expect("minimum reset preparation Store options are valid");
        DaemonExecutionEngineV1::new(
            WorkspaceStoreSlotV1::unavailable_for_reset_preparation(),
            NativeExecutionIdSourceV1,
            WallUtcExecutionClockV1,
            NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
            NativeArtifactVerifierV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
            NativeWorkspaceRevalidatorV1::new(SqliteWorkspaceBindingInspectorV1::new(options)),
        )
    }
}

impl WorkerExecutionV1<WorkspaceSchedulerContextV1> for NativeContextExecutionV1 {
    fn admit(
        &self,
        context: &WorkspaceSchedulerContextV1,
        binding: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
        response_context: Option<&podway_store::PersistedResponseContextV1>,
    ) -> Result<AdmitOutcomeV1, crate::execution::ExecutionErrorV1> {
        if context.binding() != binding {
            return Err(
                crate::execution::ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "scheduler context binding changed during admission",
                },
            );
        }
        Self::engine(context, self.observability.clone())?
            .admit_for_workspace_with_response_context(
                binding,
                request,
                idempotency_key,
                response_context.cloned(),
            )
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
        Self::engine(context, self.observability.clone())?.execute_next(binding, worker)
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
            RequestReadWaitV1::AfterJobUntil {
                job_id,
                timeout_millis,
            } => MonotonicDeadlineV1::after(self.clock.as_ref(), timeout_millis)
                .map(|deadline| ReadWaitV1::after_job_until(job_id, deadline))
                .map_err(map_read_error),
        }
    }

    fn service(
        &self,
        workspace: &ProductionWorkspaceV1,
    ) -> AuthoritativeReadServiceV1<
        WorkspaceStoreReadFacadeV1,
        SchedulerReadNotificationV1,
        Arc<NativeProductionClockV1>,
    > {
        let context = workspace.scheduler.context_snapshot();
        AuthoritativeReadServiceV1::new(
            context.store(),
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
        input: DispatcherStatusRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let result = self
            .service(workspace)
            .status_guarded(
                context.binding().identity(),
                self.wait(input.wait)?,
                input.verbose,
                input.expected_session_id.as_ref(),
            )
            .map_err(map_read_error)?;
        protocol_result_map(&result).map(|result| DispatcherReadOutputV1::new(result, Vec::new()))
    }

    fn next(
        &self,
        workspace: &ProductionWorkspaceV1,
        input: DispatcherNextRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let result = self
            .service(workspace)
            .next_guarded(
                context.binding().identity(),
                self.wait(input.wait)?,
                input.expected_session_id.as_ref(),
            )
            .map_err(map_read_error)?;
        protocol_result_map(&result).map(|result| DispatcherReadOutputV1::new(result, Vec::new()))
    }
    fn job_list(
        &self,
        workspace: &ProductionWorkspaceV1,
        state: Option<JobStateV1>,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let views = self
            .service(workspace)
            .list_jobs(
                context.binding().identity(),
                JobListQueryV1::new(1_000)
                    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?,
            )
            .map_err(map_read_error)?;
        let jobs = views
            .iter()
            .filter(|view| state.is_none_or(|state| protocol_job_state(view.state()) == state))
            .map(job_result_value)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DispatcherReadOutputV1::new(
            Map::from_iter([("jobs".to_owned(), Value::Array(jobs))]),
            Vec::new(),
        ))
    }

    fn job_lookup(
        &self,
        workspace: &ProductionWorkspaceV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let key = StoreIdempotencyKeyV1::new(idempotency_key.as_str().to_owned())
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let Some(mut binding) = context
            .store()
            .read_idempotency_lookup(context.binding().identity(), &key)
            .map_err(map_store_error)?
        else {
            return Ok(DispatcherReadOutputV1::new(
                Map::from_iter([("found".to_owned(), Value::Bool(false))]),
                Vec::new(),
            ));
        };
        let view = context
            .store()
            .read_job(context.binding().identity(), binding.job_id())
            .map_err(map_store_error)?;
        let mut job = if let Some(view) = view {
            if view.job().request_digest() != binding.request_digest() {
                return Err(terminal_replay_integrity_failure());
            }
            job_result_value(&view)?
        } else {
            if binding.terminal_receipt().is_none() {
                binding = context
                    .store()
                    .read_idempotency_lookup(context.binding().identity(), &key)
                    .map_err(map_store_error)?
                    .filter(|current| {
                        current.job_id() == binding.job_id()
                            && current.request_digest() == binding.request_digest()
                    })
                    .ok_or_else(terminal_replay_integrity_failure)?;
            }
            receipt_only_job_result_value(
                binding
                    .terminal_receipt()
                    .ok_or_else(terminal_replay_integrity_failure)?,
                binding.request_digest(),
            )?
        };
        let job = job
            .as_object_mut()
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
        job.insert(
            "request_digest".to_owned(),
            Value::String(binding.request_digest().as_str().to_owned()),
        );
        Ok(DispatcherReadOutputV1::new(
            Map::from_iter([
                ("found".to_owned(), Value::Bool(true)),
                ("job".to_owned(), Value::Object(job.clone())),
            ]),
            Vec::new(),
        ))
    }

    fn job_status(
        &self,
        workspace: &ProductionWorkspaceV1,
        job_id: &podway_core::JobId,
        wait: RequestReadWaitV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let view = self
            .service(workspace)
            .job(context.binding().identity(), job_id, self.wait(wait)?)
            .map_err(map_read_error)?;
        let (job, result) = job_read_projection(&view)?;
        Ok(DispatcherJobOutputV1::new(
            job,
            Map::from_iter([("job".to_owned(), result)]),
            Vec::new(),
        ))
    }
}

/// Concrete non-durable dry-run projection over the scheduler-owned committed Store state.
#[derive(Clone)]
pub struct ProductionPreviewServiceV1 {
    clock: Arc<NativeProductionClockV1>,
}

impl ProductionPreviewServiceV1 {
    pub fn new(clock: Arc<NativeProductionClockV1>) -> Self {
        Self { clock }
    }
}

impl DispatcherPreviewServiceV1<ProductionWorkspaceV1> for ProductionPreviewServiceV1 {
    fn preview(
        &self,
        workspace: &ProductionWorkspaceV1,
        request: &SliceRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let context = workspace.scheduler.context_snapshot();
        let view = context
            .store()
            .read_workspace_view(context.binding().identity())
            .map_err(map_store_error)?;
        let now = self.clock.now();
        let provider = NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(
            context.store_options().clone(),
        ));
        let (command, expected_revision) = preview_command(
            request.command(),
            context.binding(),
            view.current_session(),
            now,
            &provider,
        )?;
        let outcome = preview_transition_v1(
            view.current_session(),
            &command,
            CommandContextV1 {
                expected_revision,
                now,
            },
        )
        .map_err(|error| map_preview_domain_error(error, request.command()))?;
        Ok(DispatcherReadOutputV1::new(
            preview_result_map(&outcome, view.current_session()),
            Vec::new(),
        ))
    }
}
/// Concrete queued-job cancellation control path. It owns no mutation-admission capability.
#[derive(Clone)]
pub struct ProductionControlServiceV1 {
    clock: Arc<NativeProductionClockV1>,
}

impl ProductionControlServiceV1 {
    pub fn new(clock: Arc<NativeProductionClockV1>) -> Self {
        Self { clock }
    }
}

impl DispatcherControlServiceV1<ProductionWorkspaceV1> for ProductionControlServiceV1 {
    fn cancel_job(
        &self,
        workspace: &ProductionWorkspaceV1,
        job_id: &podway_core::JobId,
        expected_state: JobStateV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1> {
        if expected_state != JobStateV1::Queued {
            return Err(DispatchFailureV1::new(
                DispatchFailureKindV1::JobNotCancellable,
            ));
        }
        let context = workspace.scheduler.context_snapshot();
        let view = context
            .store()
            .read_job(context.binding().identity(), job_id)
            .map_err(map_store_error)?
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::JobNotFound))?;
        if protocol_job_state(view.state()) != expected_state {
            return Err(DispatchFailureV1::new(
                DispatchFailureKindV1::JobNotCancellable,
            ));
        }
        let now = self.clock.now();
        let committed_job = JobOutputV1::new(
            view.job().job_id().clone(),
            view.job().identity_sequence(),
            JobStateV1::Cancelled,
            rfc3339_millis(view.submitted_at())?,
            None,
            Some(rfc3339_millis(now)?),
        )
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
        let cancellation = context
            .with_claim_permission(|binding| {
                context.store_for_mutation().cancel_before_claim(
                    binding.identity(),
                    job_id.clone(),
                    Revision::new(view.job().identity_sequence()),
                    now,
                )
            })
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceMaintenance))?;
        let warnings = match cancellation.map_err(map_cancel_error)? {
            CancelOutcomeV1::Cancelled(_) => {
                // The transaction has committed. Scheduler wake-ups are advisory and are reported
                // as such rather than retroactively changing the committed control result.
                let warnings = workspace.scheduler.notify_progress().err().map(|_| {
                    Map::from_iter([
                        (
                            "code".to_owned(),
                            Value::String("scheduler_notification_failed".to_owned()),
                        ),
                        ("severity".to_owned(), Value::String("warning".to_owned())),
                    ])
                });
                context.notify_after_authoritative_change();
                warnings.into_iter().collect()
            }
            CancelOutcomeV1::AlreadyTerminal(_) => {
                return Err(DispatchFailureV1::new(
                    DispatchFailureKindV1::JobNotCancellable,
                ));
            }
        };
        Ok(DispatcherJobOutputV1::new(
            committed_job,
            Map::from_iter([("cancelled".to_owned(), Value::Bool(true))]),
            warnings,
        ))
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
    manager: Arc<WorkspaceRuntimeManagerV1>,
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
    pub fn new(
        worker_id: WorkerIdV1,
        clock: Arc<NativeProductionClockV1>,
        manager: Arc<WorkspaceRuntimeManagerV1>,
    ) -> Self {
        Self::new_with_observability(worker_id, clock, manager, None)
    }

    pub fn new_with_observability(
        worker_id: WorkerIdV1,
        clock: Arc<NativeProductionClockV1>,
        manager: Arc<WorkspaceRuntimeManagerV1>,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Self {
        Self {
            worker: DaemonWorkerV1::new_with_observability(
                Arc::new(NativeContextExecutionV1::new(observability.clone())),
                Arc::clone(&clock),
                worker_id,
                observability,
            ),
            clock,
            manager,
        }
    }

    pub fn manager(&self) -> &Arc<WorkspaceRuntimeManagerV1> {
        &self.manager
    }
    fn prepare_reset_without_store(
        &self,
        source: &ResetSourceAuthorityV1,
        request: &SliceRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
        response_request_id: &RequestIdV1,
    ) -> Result<ResetAllPreparationOutcomeV1, DispatchFailureV1> {
        let proof = self
            .manager
            .unavailable_reset_store_proof(source)
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceMaintenance))?;
        NativeContextExecutionV1::unavailable_reset_preparation_engine()
            .prepare_workspace_reset_all_with_unavailable_store_and_response_request_id(
                request,
                &source.routing_identity(),
                idempotency_key,
                response_request_id.clone(),
                proof,
            )
            .map_err(map_reset_preparation_error)
    }
    fn prepare_reset(
        &self,
        source: &ResetSourceAuthorityV1,
        request: &SliceRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
        active: Option<&Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
        response_request_id: &RequestIdV1,
    ) -> Result<ResetAllPreparationOutcomeV1, DispatchFailureV1> {
        let Some(scheduler) = active else {
            return self.prepare_reset_without_store(
                source,
                request,
                idempotency_key,
                response_request_id,
            );
        };
        let context = scheduler.context_snapshot();
        let source_identity = source.routing_identity();
        if context.binding().identity() != &source_identity {
            return Err(DispatchFailureV1::new(
                DispatchFailureKindV1::WorkspaceMaintenance,
            ));
        }
        let engine = NativeContextExecutionV1::engine(context.as_ref(), None)
            .map_err(map_reset_preparation_error)?;
        match engine.prepare_workspace_reset_all_with_response_request_id(
            request,
            &source_identity,
            idempotency_key.clone(),
            response_request_id.clone(),
        ) {
            Ok(preparation) => Ok(preparation),
            Err(ExecutionErrorV1::Store(StoreErrorV1::IdempotencyDigestConflictV1 { .. })) => Err(
                DispatchFailureV1::new(DispatchFailureKindV1::IdempotencyKeyReused),
            ),
            Err(error) => Err(map_reset_preparation_error(error)),
        }
    }

    fn workspace_output(
        &self,
        scheduler: Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    ) -> Result<WorkspaceOutputV1, DispatchFailureV1> {
        Ok(
            ProductionWorkspaceRuntimeV1::new(Arc::clone(&self.manager), Arc::clone(&self.clock))
                .workspace_from_scheduler(scheduler)?
                .output,
        )
    }

    fn retire_reset_scheduler(
        &self,
        active: Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
    ) -> Result<(), DispatchFailureV1> {
        if let Some(scheduler) = active {
            self.worker
                .retire_workspace_for_maintenance(
                    self.manager.scheduler_registry(),
                    &scheduler,
                    |_| Ok(()),
                )
                .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceMaintenance))?;
        }
        Ok(())
    }

    fn reset_completion_output(
        &self,
        completion: crate::runtime_workspace::ResetAllCompletionV1,
        request: &SliceRequestV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1> {
        let scheduler = Arc::clone(completion.scheduler());
        let outcome = reread_reset_terminal(
            &scheduler,
            completion.marker().idempotency_key().as_str(),
            completion.marker().request_digest(),
            request.command(),
        )?;
        Ok((self.workspace_output(scheduler)?, outcome))
    }

    fn complete_prepared_reset(
        &self,
        transaction: &crate::runtime_workspace::WorkspaceResetMaintenanceV1<'_>,
        prepared: crate::execution::PreparedWorkspaceResetAllV1,
        active: Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
        request: &SliceRequestV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1> {
        self.retire_reset_scheduler(active)?;
        let observation =
            WorkspaceRuntimeObservationV1::new(self.clock.now(), self.clock.generated_at());
        let completion = transaction
            .complete_prepared(prepared, observation)
            .map_err(map_runtime_error)?;
        self.reset_completion_output(completion, request)
    }

    fn resume_reset_marker(
        &self,
        transaction: &crate::runtime_workspace::WorkspaceResetMaintenanceV1<'_>,
        idempotency_key: &StoreIdempotencyKeyV1,
        request_digest: &CanonicalRequestDigestV1,
        active: Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
        request: &SliceRequestV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1> {
        self.retire_reset_scheduler(active)?;
        let observation =
            WorkspaceRuntimeObservationV1::new(self.clock.now(), self.clock.generated_at());
        let completion = transaction
            .resume(idempotency_key, request_digest, observation)
            .map_err(map_runtime_error)?;
        self.reset_completion_output(completion, request)
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
        response_context: &MutationResponseContextV1,
        wait: MutationWaitV1,
    ) -> Result<MutationDispatchOutcomeV1, DispatchFailureV1> {
        let store_idempotency_key = StoreIdempotencyKeyV1::new(idempotency_key.as_str())
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let submission = self
            .worker
            .submit_with_response_context(
                &workspace.scheduler,
                request,
                store_idempotency_key,
                podway_store::PersistedResponseContextV1::new(
                    response_context.request_id().as_str(),
                    response_context.command().as_str(),
                    response_context.workspace().uuid().clone(),
                    response_context.workspace().root(),
                    response_context.workspace().latest_workspace_sequence(),
                )
                .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?,
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
                response_context: terminal_response_context(receipt)?.map(Box::new),
            });
        }

        let (job, procedure_digest) = match submission.completion() {
            Some(WorkerWaitResultV1::TimedOut(view)) => (
                job_output(view)?,
                admitted_start_procedure_digest_v1(view.execution().canonical_execution())
                    .map_err(|_| terminal_replay_integrity_failure())?,
            ),
            _ => job_output_from_context(&workspace.scheduler, submission.admission())?,
        };
        match submission.completion() {
            Some(WorkerWaitResultV1::TimedOut(_)) => {
                Ok(MutationDispatchOutcomeV1::TimedOut { job })
            }
            Some(WorkerWaitResultV1::Terminal(_)) => {
                Err(DispatchFailureV1::new(DispatchFailureKindV1::Internal))
            }
            None => Ok(MutationDispatchOutcomeV1::Detached {
                job,
                procedure_digest,
            }),
        }
    }
    fn reset_all(
        &self,
        selector: &WorktreeSelectorWireV1,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
        response_request_id: &RequestIdV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1> {
        let selector = selector_from_wire(selector)?;
        let transaction = self
            .manager
            .begin_reset_maintenance(selector)
            .map_err(map_runtime_error)?;
        let source_identity = transaction.authority().routing_identity();
        let store_idempotency_key = StoreIdempotencyKeyV1::new(idempotency_key.as_str())
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let existing_marker = transaction.discover_marker().map_err(map_runtime_error)?;
        let mut active = transaction.active_old_scheduler();
        if existing_marker.is_some() {
            let digest = reset_all_digest(request, &source_identity)?;
            transaction
                .validate_resume_request(&store_idempotency_key, &digest)
                .map_err(map_runtime_error)?;
            return self.resume_reset_marker(
                &transaction,
                &store_idempotency_key,
                &digest,
                active,
                request,
            );
        }

        // A readable Store without an active scheduler is not unavailable authority. Reactivate
        // that exact source under the held reset lease so Store-first replay cannot race admission.
        if active.is_none() && transaction.authority().persisted_identity().is_some() {
            active = transaction
                .activate_old_scheduler_for_preparation(WorkspaceRuntimeObservationV1::new(
                    self.clock.now(),
                    self.clock.generated_at(),
                ))
                .map_err(map_runtime_error)?;
        }

        match self.prepare_reset(
            transaction.authority(),
            request,
            store_idempotency_key,
            active.as_ref(),
            response_request_id,
        )? {
            ResetAllPreparationOutcomeV1::Existing(existing) => {
                let receipt =
                    terminal_replay(&existing).ok_or_else(terminal_replay_integrity_failure)?;
                let scheduler = active.ok_or_else(terminal_replay_integrity_failure)?;
                let outcome = reread_reset_terminal(
                    &scheduler,
                    idempotency_key.as_str(),
                    receipt.job().request_digest(),
                    request.command(),
                )?;
                Ok((self.workspace_output(scheduler)?, outcome))
            }
            ResetAllPreparationOutcomeV1::New(prepared) => {
                self.complete_prepared_reset(&transaction, *prepared, active, request)
            }
        }
    }
}

/// Ready-to-serve concrete dispatcher type.
pub type ProductionRequestDispatcherV1 = RequestDispatcherV1Adapter<
    ProductionWorkspaceRuntimeV1,
    ProductionReadServiceV1,
    ProductionControlServiceV1,
    ProductionPreviewServiceV1,
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
    compose_dispatcher_with_worker_and_observability_v1(manager, worker_id, None)
}

/// Builds the exact production dispatcher and worker pair with an optional non-authoritative
/// typed event producer.
pub fn compose_dispatcher_with_worker_and_observability_v1(
    manager: Arc<WorkspaceRuntimeManagerV1>,
    worker_id: WorkerIdV1,
    observability: Option<ObservabilityEmitterV1>,
) -> ProductionDispatcherCompositionV1 {
    let clock = Arc::new(NativeProductionClockV1::default());
    let worker = ProductionMutationWorkerV1::new_with_observability(
        worker_id,
        Arc::clone(&clock),
        Arc::clone(&manager),
        observability,
    );
    let dispatcher = RequestDispatcherV1Adapter::new(
        ProductionWorkspaceRuntimeV1::new(manager, Arc::clone(&clock)),
        ProductionReadServiceV1::new(Arc::clone(&clock)),
        ProductionControlServiceV1::new(Arc::clone(&clock)),
        ProductionPreviewServiceV1::new(Arc::clone(&clock)),
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
const PREVIEW_SESSION_ID_V1: &str = "00000000-0000-4000-8000-000000000201";
const PREVIEW_ATTEMPT_ID_V1: &str = "00000000-0000-4000-8000-000000000202";
const PREVIEW_SNAPSHOT_ID_V1: &str = "00000000-0000-4000-8000-000000000203";

fn preview_command(
    command: &SliceCommandV1,
    workspace: &WorkspaceBindingV1,
    prior: Option<&podway_core::SessionAggregateV1>,
    now: UnixMillis,
    provider: &impl ProcedureProviderV1,
) -> Result<(SessionCommandV1, Revision), DispatchFailureV1> {
    if let Some(expected) = preview_expected_session_id(command) {
        let actual = prior
            .map(podway_core::SessionAggregateV1::session_id)
            .cloned();
        if actual.as_ref() != Some(expected) {
            return Err(session_id_mismatch_failure(expected.clone(), actual));
        }
    }
    match command {
        SliceCommandV1::SessionStart(input) => Ok((
            SessionCommandV1::Start(preview_start(input, workspace, now, provider)?),
            Revision::ZERO,
        )),
        SliceCommandV1::SessionStartReplace(input) => {
            let _prior = prior
                .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::SessionNotFound))?;
            Ok((
                SessionCommandV1::StartReplace(StartReplaceSessionV1 {
                    expected_session_id: input.preconditions.expected_session_id.clone(),
                    confirmed: true,
                    start: preview_start(&input.start, workspace, now, provider)?,
                }),
                input.preconditions.expected_session_revision,
            ))
        }
        SliceCommandV1::SessionReturn(input) => Ok((
            SessionCommandV1::Return(ReturnSessionV1 {
                expected_attempt_id: input.preconditions.expected_attempt_id.clone(),
                destination_stage_id: input.destination_stage_id.clone(),
                reason: input.reason.clone(),
                destination_attempt_id: AttemptId::new(PREVIEW_ATTEMPT_ID_V1)
                    .expect("preview attempt ID is valid"),
            }),
            input.preconditions.expected_session_revision,
        )),
        SliceCommandV1::SessionReopen(input) => Ok((
            SessionCommandV1::Reopen(ReopenSessionV1 {
                expected_session_id: input.preconditions.expected_session_id.clone(),
                destination_stage_id: input.destination_stage_id.clone(),
                reason: input.reason.clone(),
                destination_attempt_id: AttemptId::new(PREVIEW_ATTEMPT_ID_V1)
                    .expect("preview attempt ID is valid"),
            }),
            input.preconditions.expected_session_revision,
        )),
        SliceCommandV1::SessionReset(input) => Ok((
            SessionCommandV1::Reset(ResetSessionV1 {
                expected_session_id: input.preconditions.expected_session_id.clone(),
                confirmed: true,
            }),
            input.preconditions.expected_session_revision,
        )),
        _ => Err(DispatchFailureV1::new(
            DispatchFailureKindV1::RequestInvalid,
        )),
    }
}

fn preview_expected_session_id(command: &SliceCommandV1) -> Option<&SessionId> {
    match command {
        SliceCommandV1::SessionStartReplace(input) => {
            Some(&input.preconditions.expected_session_id)
        }
        SliceCommandV1::SessionReturn(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionReopen(input) => Some(&input.preconditions.expected_session_id),
        SliceCommandV1::SessionReset(input) => Some(&input.preconditions.expected_session_id),
        _ => None,
    }
}

fn preview_start(
    input: &podway_protocol::SessionStartV1,
    workspace: &WorkspaceBindingV1,
    now: UnixMillis,
    provider: &impl ProcedureProviderV1,
) -> Result<StartSessionV1, DispatchFailureV1> {
    let snapshot_id =
        ProcedureSnapshotId::new(PREVIEW_SNAPSHOT_ID_V1).expect("preview snapshot ID is valid");
    let snapshot = match &input.source {
        podway_protocol::SessionStartSourceV1::Preset { preset } => {
            provider.load_preset_snapshot(preset, snapshot_id, now)
        }
        podway_protocol::SessionStartSourceV1::Procedure { procedure } => {
            provider.load_workspace_procedure_snapshot(workspace, procedure, snapshot_id, now)
        }
    }
    .map_err(map_preview_procedure_error)?;
    if let Some(expected) = input.expected_procedure_digest.as_ref()
        && snapshot.digest() != expected
    {
        return Err(procedure_digest_mismatch_failure(
            expected.clone(),
            snapshot.digest().clone(),
        ));
    }
    Ok(StartSessionV1 {
        task_title: input.task_title.clone(),
        snapshot,
        session_id: SessionId::new(PREVIEW_SESSION_ID_V1).expect("preview session ID is valid"),
        first_attempt_id: AttemptId::new(PREVIEW_ATTEMPT_ID_V1)
            .expect("preview attempt ID is valid"),
    })
}

fn preview_result_map(
    outcome: &podway_core::TransitionOutcomeV1,
    prior: Option<&podway_core::SessionAggregateV1>,
) -> Map<String, Value> {
    let next = outcome.next_aggregate();
    let active_before = active_attempt_projection(prior);
    let active_after = active_attempt_projection(next);
    Map::from_iter([
        ("preview".to_owned(), Value::Bool(true)),
        ("changed".to_owned(), Value::Bool(outcome.changed())),
        (
            "revision_before".to_owned(),
            outcome
                .revision_before()
                .map_or(Value::Null, |revision| Value::from(revision.get())),
        ),
        (
            "revision_after".to_owned(),
            outcome
                .revision_after()
                .map_or(Value::Null, |revision| Value::from(revision.get())),
        ),
        ("active_before".to_owned(), active_before),
        ("active_after".to_owned(), active_after.clone()),
        ("destination_attempt".to_owned(), active_after),
        (
            "affected_stages".to_owned(),
            Value::Array(
                outcome
                    .affected_stages()
                    .iter()
                    .map(|stage_id| {
                        Value::Object(Map::from_iter([
                            (
                                "stage_id".to_owned(),
                                Value::String(stage_id.as_str().to_owned()),
                            ),
                            ("before".to_owned(), stage_state_projection(prior, stage_id)),
                            ("after".to_owned(), stage_state_projection(next, stage_id)),
                            (
                                "before_attempt".to_owned(),
                                stage_attempt_projection(prior, stage_id),
                            ),
                            (
                                "after_attempt".to_owned(),
                                stage_attempt_projection(next, stage_id),
                            ),
                        ]))
                    })
                    .collect(),
            ),
        ),
    ])
}

fn active_attempt_projection(aggregate: Option<&podway_core::SessionAggregateV1>) -> Value {
    let Some(aggregate) = aggregate else {
        return Value::Null;
    };
    let Some(stage_id) = aggregate.active_stage_id() else {
        return Value::Null;
    };
    let Some(attempt_id) = aggregate.active_attempt_id() else {
        return Value::Null;
    };
    let Some(attempt) = aggregate
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == attempt_id)
    else {
        return Value::Null;
    };
    Value::Object(Map::from_iter([
        (
            "stage_id".to_owned(),
            Value::String(stage_id.as_str().to_owned()),
        ),
        (
            "attempt_id".to_owned(),
            Value::String(attempt.attempt_id().as_str().to_owned()),
        ),
        ("attempt_number".to_owned(), Value::from(attempt.number())),
    ]))
}

fn stage_state_projection(
    aggregate: Option<&podway_core::SessionAggregateV1>,
    stage_id: &podway_core::StageId,
) -> Value {
    aggregate
        .and_then(|aggregate| {
            aggregate
                .stage_progress()
                .iter()
                .find(|stage| stage.stage_id() == stage_id)
        })
        .map(|stage| Value::String(stage_progress_state_name(stage.state()).to_owned()))
        .unwrap_or(Value::Null)
}

fn stage_attempt_projection(
    aggregate: Option<&podway_core::SessionAggregateV1>,
    stage_id: &podway_core::StageId,
) -> Value {
    let Some(stage) = aggregate.and_then(|aggregate| {
        aggregate
            .stage_progress()
            .iter()
            .find(|stage| stage.stage_id() == stage_id)
    }) else {
        return Value::Null;
    };
    let Some(attempt_id) = stage.latest_attempt_id() else {
        return Value::Null;
    };
    Value::Object(Map::from_iter([
        (
            "attempt_id".to_owned(),
            Value::String(attempt_id.as_str().to_owned()),
        ),
        (
            "attempt_number".to_owned(),
            Value::from(stage.latest_attempt_number()),
        ),
    ]))
}

const fn stage_progress_state_name(state: podway_core::StageProgressState) -> &'static str {
    match state {
        podway_core::StageProgressState::Pending => "pending",
        podway_core::StageProgressState::Current => "current",
        podway_core::StageProgressState::Done => "done",
        podway_core::StageProgressState::Skipped => "skipped",
        podway_core::StageProgressState::Redo => "redo",
        podway_core::StageProgressState::Abandoned => "abandoned",
    }
}

fn map_preview_procedure_error(
    error: crate::execution::ExecutionBoundaryErrorV1,
) -> DispatchFailureV1 {
    match error {
        crate::execution::ExecutionBoundaryErrorV1::Domain(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::ProcedureInvalid)
        }
        crate::execution::ExecutionBoundaryErrorV1::WorkspaceIdentityMismatch {
            expected,
            actual,
        } => DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceUuidMismatch).with_details(
            DispatchErrorDetailsV1::default().with_workspace_uuid_mismatch(expected, actual),
        ),
        crate::execution::ExecutionBoundaryErrorV1::Transient { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
        }
    }
}

fn map_preview_domain_error(error: DomainError, command: &SliceCommandV1) -> DispatchFailureV1 {
    match error {
        DomainError::PreconditionFailed { expected, actual } => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionRevisionConflict).with_details(
                DispatchErrorDetailsV1::default()
                    .with_expected_revision(expected)
                    .with_current_revision(actual),
            )
        }
        DomainError::SessionIdentityMismatch { expected, actual } => {
            session_id_mismatch_failure(expected, actual)
        }
        DomainError::AttemptNotCurrent { expected, actual } => DispatchFailureV1::new(
            DispatchFailureKindV1::AttemptNotCurrent,
        )
        .with_details(DispatchErrorDetailsV1::default().with_attempt_mismatch(expected, actual)),
        DomainError::InvalidTransition { .. } | DomainError::InvalidState { .. } => {
            let kind = match command {
                SliceCommandV1::SessionReturn(_) => DispatchFailureKindV1::ReturnNotAllowed,
                SliceCommandV1::SessionReopen(_) => DispatchFailureKindV1::ReopenNotAllowed,
                _ => DispatchFailureKindV1::SessionNotRunning,
            };
            DispatchFailureV1::new(kind)
        }
        DomainError::RequiredItemsMissing => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequiredItemsMissing)
        }
        DomainError::BlockersPresent => {
            DispatchFailureV1::new(DispatchFailureKindV1::BlockersPresent)
        }
        DomainError::ItemNotFound { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::ItemNotFound)
        }
        DomainError::BlockerNotCurrent => {
            DispatchFailureV1::new(DispatchFailureKindV1::BlockerNotCurrent)
        }
        DomainError::ArtifactChanged => {
            DispatchFailureV1::new(DispatchFailureKindV1::ArtifactChanged)
        }
        DomainError::RevisionOverflow { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::Internal)
        }
        DomainError::EmptyValue { .. }
        | DomainError::ValueTooLong { .. }
        | DomainError::InvalidUuid { .. }
        | DomainError::InvalidIdentifier { .. }
        | DomainError::InvalidSha256Digest => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid)
        }
    }
}

fn job_output_from_context(
    scheduler: &Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    admission: &AdmitOutcomeV1,
) -> Result<(JobOutputV1, Option<Sha256Digest>), DispatchFailureV1> {
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
    let procedure_digest =
        admitted_start_procedure_digest_v1(view.execution().canonical_execution())
            .map_err(|_| terminal_replay_integrity_failure())?;
    Ok((job_output(&view)?, procedure_digest))
}

fn terminal_replay_integrity_failure() -> DispatchFailureV1 {
    DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
}
fn reread_reset_terminal(
    scheduler: &Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    idempotency_key: &str,
    request_digest: &CanonicalRequestDigestV1,
    command: &SliceCommandV1,
) -> Result<MutationDispatchOutcomeV1, DispatchFailureV1> {
    let idempotency_key = StoreIdempotencyKeyV1::new(idempotency_key)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
    scheduler.with_serialized(|context| {
        let outcome = context
            .store()
            .read_idempotent_outcome(
                context.binding().identity(),
                &idempotency_key,
                request_digest,
            )
            .map_err(map_store_error)?
            .ok_or_else(terminal_replay_integrity_failure)?;
        let receipt = terminal_replay(&outcome).ok_or_else(terminal_replay_integrity_failure)?;
        Ok(MutationDispatchOutcomeV1::Terminal {
            job: job_output_from_terminal_receipt(receipt)?,
            result: terminal_result(receipt, command)?,
            response_context: terminal_response_context(receipt)?.map(Box::new),
        })
    })
}

fn reset_all_digest(
    request: &SliceRequestV1,
    source: &DurableWorktreeIdentityV1,
) -> Result<CanonicalRequestDigestV1, DispatchFailureV1> {
    let canonical = canonical_reset_all_identity_v1(
        request,
        source.common_dir_identity(),
        source.worktree_admin_identity(),
    )
    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
    CanonicalRequestDigestV1::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn map_reset_preparation_error(error: ExecutionErrorV1) -> DispatchFailureV1 {
    match error {
        ExecutionErrorV1::Store(error) => map_store_error(error),
        ExecutionErrorV1::BoundaryDomain(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid)
        }
        ExecutionErrorV1::BoundaryTransient { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
        }
        ExecutionErrorV1::SessionIdentityMismatch { expected, actual } => {
            session_id_mismatch_failure(expected, actual)
        }
        ExecutionErrorV1::WorkspaceIdentityMismatch { expected, actual } => {
            workspace_uuid_mismatch_failure(expected, actual)
        }
        ExecutionErrorV1::ProcedureDigestMismatch { expected, actual } => {
            procedure_digest_mismatch_failure(expected, actual)
        }
        ExecutionErrorV1::InvalidPersistedExecution { .. }
        | ExecutionErrorV1::InvalidStoreValue(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
        }
    }
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
fn protocol_job_state(state: StoreJobStateV1) -> JobStateV1 {
    match state {
        StoreJobStateV1::Queued => JobStateV1::Queued,
        StoreJobStateV1::Running => JobStateV1::Running,
        StoreJobStateV1::Succeeded => JobStateV1::Succeeded,
        StoreJobStateV1::Failed => JobStateV1::Failed,
        StoreJobStateV1::Cancelled => JobStateV1::Cancelled,
    }
}

fn job_read_projection(view: &JobViewV1) -> Result<(JobOutputV1, Value), DispatchFailureV1> {
    match (view.state(), view.terminal_receipt()) {
        (StoreJobStateV1::Queued | StoreJobStateV1::Running, None)
            if view.finished_at().is_none() =>
        {
            Ok((job_output(view)?, Value::Null))
        }
        (
            StoreJobStateV1::Succeeded | StoreJobStateV1::Failed | StoreJobStateV1::Cancelled,
            Some(receipt),
        ) => terminal_job_read_projection(view, receipt),
        _ => Err(terminal_replay_integrity_failure()),
    }
}

fn terminal_job_read_projection(
    view: &JobViewV1,
    receipt: &PersistedTerminalReceiptV1,
) -> Result<(JobOutputV1, Value), DispatchFailureV1> {
    let projection = receipt
        .job_projection()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let job = job_output_from_terminal_receipt(receipt)?;
    if view.job().job_id() != receipt.job().job_id()
        || view.job().identity_sequence() != receipt.job().identity_sequence()
        || protocol_job_state(view.state()) != job.state()
        || view.submitted_at() != projection.submitted_at()
        || view.claimed_at() != projection.claimed_at()
        || view.finished_at() != Some(projection.finished_at())
    {
        return Err(terminal_replay_integrity_failure());
    }
    let terminal_response = terminal_job_response(
        receipt,
        terminal_command_kind_from_store(view.execution().command()),
    )?;
    Ok((job, terminal_response))
}

fn job_result_value(view: &JobViewV1) -> Result<Value, DispatchFailureV1> {
    let (output, terminal_response) = job_read_projection(view)?;
    let mut job = serde_json::to_value(output)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
    let object = job
        .as_object_mut()
        .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
    object.insert(
        "command".to_owned(),
        Value::String(durable_command_name(view.execution().command()).to_owned()),
    );
    object.insert("terminal_response".to_owned(), terminal_response);
    Ok(job)
}

fn receipt_only_job_result_value(
    receipt: &PersistedTerminalReceiptV1,
    request_digest: &CanonicalRequestDigestV1,
) -> Result<Value, DispatchFailureV1> {
    if receipt.job().request_digest() != request_digest {
        return Err(terminal_replay_integrity_failure());
    }
    let command = receipt
        .lookup_command()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let output = job_output_from_terminal_receipt(receipt)?;
    let terminal_response =
        terminal_job_response(receipt, terminal_command_kind_from_lookup(command))?;
    let mut job = serde_json::to_value(output)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
    let object = job
        .as_object_mut()
        .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
    object.insert(
        "command".to_owned(),
        Value::String(command.public_command_name().to_owned()),
    );
    object.insert("terminal_response".to_owned(), terminal_response);
    object.insert(
        "request_digest".to_owned(),
        Value::String(request_digest.as_str().to_owned()),
    );
    Ok(job)
}

fn terminal_command_kind_from_lookup(command: &PersistedDomainCommandV1) -> TerminalCommandKindV1 {
    terminal_command_kind_from_store(&command.command())
}

fn durable_command_name(command: &podway_store::CommandV1) -> &'static str {
    match command {
        podway_store::CommandV1::WorkspaceInitialize => "workspace.init",
        podway_store::CommandV1::WorkspaceResetAll => "workspace.reset_all",
        podway_store::CommandV1::SessionStart => "session.start",
        podway_store::CommandV1::SessionStartReplace => "session.start_replace",
        podway_store::CommandV1::SessionComplete => "session.complete",
        podway_store::CommandV1::SessionSkip => "session.skip",
        podway_store::CommandV1::SessionRetry => "session.retry",
        podway_store::CommandV1::SessionReturn => "session.return",
        podway_store::CommandV1::SessionBlock => "session.block",
        podway_store::CommandV1::SessionUnblock => "session.unblock",
        podway_store::CommandV1::SessionCancel => "session.cancel",
        podway_store::CommandV1::SessionReopen => "session.reopen",
        podway_store::CommandV1::SessionReset => "session.reset",
        podway_store::CommandV1::ItemCheck { .. } => "item.check",
        podway_store::CommandV1::ItemUncheck { .. } => "item.uncheck",
        podway_store::CommandV1::ItemSet { .. } => "item.set",
        podway_store::CommandV1::ItemAdd { .. } => "item.add",
        podway_store::CommandV1::ItemRemove { .. } => "item.remove",
        podway_store::CommandV1::ItemAttach { .. } => "item.attach",
        podway_store::CommandV1::ItemClear { .. } => "item.clear",
    }
}

#[derive(Clone, Copy)]
enum TerminalCommandKindV1 {
    Start,
    ItemMutation,
    SessionReset,
    Skip,
    Return,
    Reopen,
    Other,
}
fn terminal_command_kind_from_store(command: &podway_store::CommandV1) -> TerminalCommandKindV1 {
    match durable_command_name(command) {
        "session.start" | "session.start_replace" => TerminalCommandKindV1::Start,
        "item.check" | "item.uncheck" | "item.set" | "item.add" | "item.remove" | "item.attach"
        | "item.clear" => TerminalCommandKindV1::ItemMutation,
        "session.reset" => TerminalCommandKindV1::SessionReset,
        "session.skip" => TerminalCommandKindV1::Skip,
        "session.return" => TerminalCommandKindV1::Return,
        "session.reopen" => TerminalCommandKindV1::Reopen,
        _ => TerminalCommandKindV1::Other,
    }
}
fn validate_terminal_receipt_projection(
    receipt: &PersistedTerminalReceiptV1,
    command: TerminalCommandKindV1,
) -> Result<(), DispatchFailureV1> {
    match receipt.result() {
        PersistedTerminalResultV1::Success(result) => {
            let _ = terminal_session_projection(result, receipt.session_projection(), command)?;
        }
        PersistedTerminalResultV1::Failure(_) | PersistedTerminalResultV1::Cancelled
            if receipt.session_projection().is_some() =>
        {
            return Err(terminal_replay_integrity_failure());
        }
        PersistedTerminalResultV1::Failure(_) | PersistedTerminalResultV1::Cancelled => {}
    }
    Ok(())
}

fn terminal_job_response(
    receipt: &PersistedTerminalReceiptV1,
    command: TerminalCommandKindV1,
) -> Result<Value, DispatchFailureV1> {
    validate_terminal_receipt_projection(receipt, command)?;
    if matches!(receipt.result(), PersistedTerminalResultV1::Cancelled) {
        return serde_json::to_value(TerminalJobResponseV1::Cancelled(
            TerminalJobCancellationProjectionV1 { cancelled: true },
        ))
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
    }
    let context =
        terminal_response_context(receipt)?.ok_or_else(terminal_replay_integrity_failure)?;
    let envelope = terminal_response_envelope_v1(
        context,
        job_output_from_terminal_receipt(receipt)?,
        terminal_result_for_kind(receipt, command)?,
        false,
    )?;
    serde_json::to_value(envelope)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn terminal_job_success_result(result: &PersistedDomainResultV1) -> TerminalJobSuccessResultV1 {
    match result {
        PersistedDomainResultV1::WorkspaceInitialized { revision, .. } => {
            TerminalJobSuccessResultV1::WorkspaceInitialized {
                revision: *revision,
            }
        }
        PersistedDomainResultV1::WorkspaceReset { revision, .. } => {
            TerminalJobSuccessResultV1::WorkspaceReset {
                revision: *revision,
            }
        }
        PersistedDomainResultV1::SessionChanged {
            revision_before,
            revision_after,
            changed,
            ..
        } => TerminalJobSuccessResultV1::SessionChanged {
            changed: *changed,
            revision_before: *revision_before,
            revision_after: *revision_after,
        },
        PersistedDomainResultV1::ItemChanged {
            item_id,
            revision_before,
            revision_after,
            changed,
            ..
        } => TerminalJobSuccessResultV1::ItemChanged {
            item_id: item_id.clone(),
            changed: *changed,
            revision_before: *revision_before,
            revision_after: *revision_after,
        },
    }
}

fn map_cancel_error(error: StoreErrorV1) -> DispatchFailureV1 {
    match error {
        StoreErrorV1::AlreadyClaimedV1 { .. } | StoreErrorV1::CancellationLostV1 { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::JobNotCancellable)
        }
        error => map_store_error(error),
    }
}

fn terminal_replay(admission: &AdmitOutcomeV1) -> Option<&PersistedTerminalReceiptV1> {
    match admission {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => Some(receipt),
        AdmitOutcomeV1::New(_)
        | AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(_)) => None,
    }
}

fn terminal_response_context(
    receipt: &PersistedTerminalReceiptV1,
) -> Result<Option<TerminalResponseContextV1>, DispatchFailureV1> {
    let Some(context) = receipt.response_context() else {
        return Ok(None);
    };
    let projection = receipt
        .job_projection()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let request_id =
        RequestIdV1::new(context.request_id()).map_err(|_| terminal_replay_integrity_failure())?;
    let command =
        CommandNameV1::new(context.command()).map_err(|_| terminal_replay_integrity_failure())?;
    let workspace = WorkspaceOutputV1::new(
        context.workspace_uuid().clone(),
        context.workspace_root(),
        context.workspace_sequence(),
    )
    .map_err(|_| terminal_replay_integrity_failure())?;
    Ok(Some(TerminalResponseContextV1::new(
        request_id,
        command,
        rfc3339_millis(projection.finished_at())?,
        workspace,
    )))
}

fn terminal_result(
    receipt: &PersistedTerminalReceiptV1,
    command: &SliceCommandV1,
) -> Result<DispatcherTerminalResultV1, DispatchFailureV1> {
    terminal_result_for_kind(receipt, terminal_command_kind(command))
}

fn terminal_result_for_kind(
    receipt: &PersistedTerminalReceiptV1,
    command: TerminalCommandKindV1,
) -> Result<DispatcherTerminalResultV1, DispatchFailureV1> {
    validate_terminal_receipt_projection(receipt, command)?;
    match receipt.result() {
        PersistedTerminalResultV1::Success(result) => {
            let session =
                terminal_session_projection(result, receipt.session_projection(), command)?;
            let mut result = terminal_success_result_map(&terminal_job_success_result(result));
            if matches!(command, TerminalCommandKindV1::Start) {
                let procedure_digest = receipt
                    .session_projection()
                    .and_then(PersistedTerminalSessionProjectionV1::procedure_digest)
                    .ok_or_else(terminal_replay_integrity_failure)?;
                result.insert("procedure_digest".to_owned(), json!(procedure_digest));
            }
            Ok(DispatcherTerminalResultV1::Output(
                DispatcherTerminalOutputV1::new(session, result, Vec::new()),
            ))
        }
        PersistedTerminalResultV1::Failure(error) => Ok(DispatcherTerminalResultV1::Error(
            map_terminal_domain_error(error, command),
        )),
        PersistedTerminalResultV1::Cancelled => Ok(DispatcherTerminalResultV1::Error(
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceMaintenance),
        )),
    }
}

fn terminal_success_result_map(result: &TerminalJobSuccessResultV1) -> Map<String, Value> {
    match result {
        TerminalJobSuccessResultV1::WorkspaceInitialized { revision } => Map::from_iter([
            ("initialized".to_owned(), Value::Bool(true)),
            ("revision".to_owned(), Value::from(revision.get())),
        ]),
        TerminalJobSuccessResultV1::WorkspaceReset { revision } => Map::from_iter([
            ("reset".to_owned(), Value::Bool(true)),
            ("revision".to_owned(), Value::from(revision.get())),
        ]),
        TerminalJobSuccessResultV1::SessionChanged {
            changed,
            revision_before,
            revision_after,
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
        TerminalJobSuccessResultV1::ItemChanged {
            item_id,
            changed,
            revision_before,
            revision_after,
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
    }
}
fn terminal_session_projection(
    result: &PersistedDomainResultV1,
    persisted: Option<&PersistedTerminalSessionProjectionV1>,
    command: TerminalCommandKindV1,
) -> Result<Option<SessionOutputV1>, DispatchFailureV1> {
    match (result, persisted) {
        (
            PersistedDomainResultV1::SessionChanged {
                changed: true,
                revision_before,
                revision_after,
                ..
            },
            None,
        ) if matches!(command, TerminalCommandKindV1::SessionReset)
            && revision_before.get() > 0
            && revision_after.get() == 0 =>
        {
            Ok(None)
        }
        (
            PersistedDomainResultV1::SessionChanged { .. }
            | PersistedDomainResultV1::ItemChanged { .. },
            Some(persisted),
        ) => {
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
        (
            PersistedDomainResultV1::SessionChanged { .. }
            | PersistedDomainResultV1::ItemChanged { .. },
            None,
        )
        | (_, Some(_)) => Err(terminal_replay_integrity_failure()),
        (_, None) => Ok(None),
    }
}

fn terminal_command_kind(command: &SliceCommandV1) -> TerminalCommandKindV1 {
    match command {
        SliceCommandV1::ItemCheck(_)
        | SliceCommandV1::ItemUncheck(_)
        | SliceCommandV1::ItemSet(_)
        | SliceCommandV1::ItemAdd(_)
        | SliceCommandV1::ItemRemove(_)
        | SliceCommandV1::ItemAttach(_)
        | SliceCommandV1::ItemClear(_) => TerminalCommandKindV1::ItemMutation,
        SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_) => {
            TerminalCommandKindV1::Start
        }
        SliceCommandV1::SessionReset(_) => TerminalCommandKindV1::SessionReset,
        SliceCommandV1::SessionSkip(_) => TerminalCommandKindV1::Skip,
        SliceCommandV1::SessionReturn(_) => TerminalCommandKindV1::Return,
        SliceCommandV1::SessionReopen(_) => TerminalCommandKindV1::Reopen,
        _ => TerminalCommandKindV1::Other,
    }
}

fn map_terminal_domain_error(
    error: &PersistedDomainErrorV1,
    command: TerminalCommandKindV1,
) -> DispatchFailureV1 {
    match error {
        PersistedDomainErrorV1::PreconditionFailed { expected, actual } => {
            let kind = match command {
                TerminalCommandKindV1::ItemMutation => DispatchFailureKindV1::ItemRevisionConflict,
                TerminalCommandKindV1::SessionReset
                | TerminalCommandKindV1::Skip
                | TerminalCommandKindV1::Return
                | TerminalCommandKindV1::Reopen
                | TerminalCommandKindV1::Start
                | TerminalCommandKindV1::Other => DispatchFailureKindV1::SessionRevisionConflict,
            };
            DispatchFailureV1::new(kind).with_details(
                DispatchErrorDetailsV1::default()
                    .with_expected_revision(*expected)
                    .with_current_revision(*actual),
            )
        }
        PersistedDomainErrorV1::SessionIdentityMismatch { expected, actual } => {
            session_id_mismatch_failure(expected.clone(), actual.clone())
        }
        PersistedDomainErrorV1::AttemptNotCurrent { expected, actual } => {
            DispatchFailureV1::new(DispatchFailureKindV1::AttemptNotCurrent).with_details(
                DispatchErrorDetailsV1::default()
                    .with_attempt_mismatch(expected.clone(), actual.clone()),
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
            let kind = match command {
                TerminalCommandKindV1::Skip => DispatchFailureKindV1::StageNotSkippable,
                TerminalCommandKindV1::Return => DispatchFailureKindV1::ReturnNotAllowed,
                TerminalCommandKindV1::Reopen => DispatchFailureKindV1::ReopenNotAllowed,
                TerminalCommandKindV1::ItemMutation
                | TerminalCommandKindV1::SessionReset
                | TerminalCommandKindV1::Start
                | TerminalCommandKindV1::Other => DispatchFailureKindV1::SessionNotRunning,
            };
            DispatchFailureV1::new(kind)
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
                | SliceCommandV1::ItemUncheck(_)
                | SliceCommandV1::ItemSet(_)
                | SliceCommandV1::ItemAdd(_)
                | SliceCommandV1::ItemRemove(_)
                | SliceCommandV1::ItemAttach(_)
                | SliceCommandV1::ItemClear(_)
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
            crate::execution::ExecutionErrorV1::SessionIdentityMismatch { expected, actual } => {
                session_id_mismatch_failure(expected, actual)
            }
            crate::execution::ExecutionErrorV1::WorkspaceIdentityMismatch { expected, actual } => {
                workspace_uuid_mismatch_failure(expected, actual)
            }
            crate::execution::ExecutionErrorV1::ProcedureDigestMismatch { expected, actual } => {
                procedure_digest_mismatch_failure(expected, actual)
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
        ReadServiceErrorV1::SessionIdentityMismatch { expected, actual } => {
            session_id_mismatch_failure(expected, actual)
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
        WorkspaceRuntimeErrorV1::MaintenanceInProgress
        | WorkspaceRuntimeErrorV1::ResetSchedulerRetirement
        | WorkspaceRuntimeErrorV1::ResetMarkerConflict
        | WorkspaceRuntimeErrorV1::ResetRegistryPredecessorStale => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceMaintenance)
        }
        WorkspaceRuntimeErrorV1::ResetIdempotencyConflict { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::IdempotencyKeyReused)
        }
        WorkspaceRuntimeErrorV1::ResetSourceNotRegistered => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceNotInitialized)
        }
        WorkspaceRuntimeErrorV1::ResetSourceAmbiguous => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceIdentityConflict)
        }
        WorkspaceRuntimeErrorV1::RuntimeDirectory(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspacePathUnsafe)
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
        WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { expected, actual } => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceUuidMismatch).with_details(
                DispatchErrorDetailsV1::default().with_workspace_uuid_mismatch(expected, actual),
            )
        }
        WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch { .. }
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
        StoreErrorV1::SessionIdentityConflictV1 {
            expected: Some(expected),
            actual,
        } => session_id_mismatch_failure(expected, actual),
        StoreErrorV1::SessionIdentityConflictV1 {
            expected: None,
            actual: Some(_),
        } => DispatchFailureV1::new(DispatchFailureKindV1::SessionAlreadyExists),
        StoreErrorV1::SessionIdentityConflictV1 {
            expected: None,
            actual: None,
        } => DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable),
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

fn session_id_mismatch_failure(
    expected: SessionId,
    actual: Option<SessionId>,
) -> DispatchFailureV1 {
    DispatchFailureV1::new(DispatchFailureKindV1::SessionIdMismatch)
        .with_details(DispatchErrorDetailsV1::default().with_session_id_mismatch(expected, actual))
}

fn workspace_uuid_mismatch_failure(
    expected: podway_core::WorkspaceId,
    actual: podway_core::WorkspaceId,
) -> DispatchFailureV1 {
    DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceUuidMismatch).with_details(
        DispatchErrorDetailsV1::default().with_workspace_uuid_mismatch(expected, actual),
    )
}

fn procedure_digest_mismatch_failure(
    expected: Sha256Digest,
    actual: Sha256Digest,
) -> DispatchFailureV1 {
    DispatchFailureV1::new(DispatchFailureKindV1::ProcedureDigestMismatch).with_details(
        DispatchErrorDetailsV1::default().with_procedure_digest_mismatch(expected, actual),
    )
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
    use podway_config::{ProcedureFormatV1, ProcedureWarningPolicyV1, parse_procedure_v1};
    use podway_core::{
        AttemptId, CompleteSessionV1, ItemId, JobId, ProcedureSnapshotId, ProcedureSourceLabelV1,
        Revision, SessionAggregateV1, SessionCommandV1, SessionId, Sha256Digest, WorkspaceId,
        preview_transition_v1,
    };
    use podway_store::{
        ClaimedExecutionV1, DurableWorktreeIdentityV1, JobReceiptV1, JobViewV1,
        PersistedResponseContextV1, PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1,
        PersistedTerminalSessionProjectionV1, RevisionAttemptItemPreconditionsV1,
        ValidatedWorkspaceRootV1,
    };

    fn fixture_job(sequence: u64) -> JobReceiptV1 {
        JobReceiptV1::new(
            sequence,
            JobId::new(format!("00000000-0000-4000-8000-{sequence:012x}")).unwrap(),
            Sha256Digest::new(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
        )
    }

    fn terminal_job_projection(
        state: PersistedTerminalJobStateV1,
    ) -> PersistedTerminalJobProjectionV1 {
        PersistedTerminalJobProjectionV1::new(
            state,
            UnixMillis::new(10),
            (state != PersistedTerminalJobStateV1::Cancelled).then(|| UnixMillis::new(11)),
            UnixMillis::new(12),
        )
        .unwrap()
    }

    #[test]
    fn identity_boundary_mappers_preserve_expected_actual_and_absence() {
        let expected_workspace = WorkspaceId::new("00000000-0000-4000-8000-000000000031").unwrap();
        let actual_workspace = WorkspaceId::new("00000000-0000-4000-8000-000000000032").unwrap();
        let workspace_failure = map_worker_error(WorkerErrorV1::Execution(
            ExecutionErrorV1::WorkspaceIdentityMismatch {
                expected: expected_workspace.clone(),
                actual: actual_workspace.clone(),
            },
        ));
        assert_eq!(
            workspace_failure.kind(),
            DispatchFailureKindV1::WorkspaceUuidMismatch
        );
        assert_eq!(
            workspace_failure.into_details().into_json(false),
            Map::from_iter([
                (
                    "schema".to_owned(),
                    json!("podway.workspace-uuid-mismatch-details/v1"),
                ),
                (
                    "expected_workspace_uuid".to_owned(),
                    json!(expected_workspace.as_str()),
                ),
                (
                    "actual_workspace_uuid".to_owned(),
                    json!(actual_workspace.as_str()),
                ),
                ("admission".to_owned(), json!({"admitted": false})),
            ])
        );

        let expected_session = SessionId::new("00000000-0000-4000-8000-000000000033").unwrap();
        let read_failure = map_read_error(ReadServiceErrorV1::SessionIdentityMismatch {
            expected: expected_session.clone(),
            actual: None,
        });
        assert_eq!(
            read_failure.kind(),
            DispatchFailureKindV1::SessionIdMismatch
        );
        assert_eq!(
            read_failure.into_details().into_json(false)["actual_session_id"],
            Value::Null
        );

        let store_failure = map_store_error(StoreErrorV1::SessionIdentityConflictV1 {
            expected: Some(expected_session),
            actual: None,
        });
        assert_eq!(
            store_failure.kind(),
            DispatchFailureKindV1::SessionIdMismatch
        );
        let start_conflict = map_store_error(StoreErrorV1::SessionIdentityConflictV1 {
            expected: None,
            actual: Some(SessionId::new("00000000-0000-4000-8000-000000000034").unwrap()),
        });
        assert_eq!(
            start_conflict.kind(),
            DispatchFailureKindV1::SessionAlreadyExists
        );
    }

    #[test]
    fn pstrt001_digest_mismatch_mapper_preserves_closed_details() {
        let expected = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
        let actual = Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap();
        let failure = map_worker_error(WorkerErrorV1::Execution(
            ExecutionErrorV1::ProcedureDigestMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            },
        ));
        assert_eq!(
            failure.kind(),
            DispatchFailureKindV1::ProcedureDigestMismatch
        );
        assert_eq!(
            failure.into_details().into_json(false),
            Map::from_iter([
                (
                    "schema".to_owned(),
                    json!("podway.procedure-digest-mismatch-details/v1"),
                ),
                (
                    "expected_procedure_digest".to_owned(),
                    json!(expected.as_str()),
                ),
                ("actual_procedure_digest".to_owned(), json!(actual.as_str()),),
                ("admission".to_owned(), json!({"admitted": false})),
            ])
        );
    }

    fn terminal_view(
        command: podway_store::CommandV1,
        state: StoreJobStateV1,
        receipt: PersistedTerminalReceiptV1,
    ) -> JobViewV1 {
        let persisted_command = PersistedDomainCommandV1::from_command(&command);
        let response_command = persisted_command.public_command_name().to_owned();
        let identity_sequence = receipt.job().identity_sequence();
        let enriched = receipt
            .clone()
            .with_lookup_command(persisted_command)
            .and_then(|receipt| {
                receipt.with_response_context(
                    PersistedResponseContextV1::new(
                        format!("00000000-0000-4000-8000-{identity_sequence:012x}"),
                        response_command,
                        WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                        "/safe/worktree",
                        identity_sequence,
                    )
                    .unwrap(),
                )
            });
        let receipt = enriched.unwrap_or(receipt);
        let projection = receipt.job_projection().unwrap();
        JobViewV1::new(
            ClaimedExecutionV1::new(
                command,
                RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
            ),
            receipt.job().clone(),
            state,
            projection.submitted_at(),
            projection.claimed_at(),
            Some(projection.finished_at()),
            Some(receipt),
        )
    }

    fn terminal_session_result(session_id: SessionId) -> PersistedDomainResultV1 {
        PersistedDomainResultV1::SessionChanged {
            session_id,
            revision_before: Revision::new(4),
            revision_after: Revision::new(5),
            changed: true,
        }
    }
    fn terminal_session_clear_result(session_id: SessionId) -> PersistedDomainResultV1 {
        PersistedDomainResultV1::SessionChanged {
            session_id,
            revision_before: Revision::new(4),
            revision_after: Revision::ZERO,
            changed: true,
        }
    }

    fn fixture_terminal_session_projection(
        session_id: SessionId,
    ) -> PersistedTerminalSessionProjectionV1 {
        PersistedTerminalSessionProjectionV1::new(
            session_id,
            "Stored immutable terminal session".to_owned(),
            PersistedSessionLifecycleV1::Running,
            Revision::new(4),
            Revision::new(5),
        )
        .unwrap()
    }
    fn preview_binding() -> WorkspaceBindingV1 {
        let digest = Sha256Digest::new(
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap();
        WorkspaceBindingV1::new(
            DurableWorktreeIdentityV1::new(
                digest.clone(),
                WorkspaceId::new("00000000-0000-4000-8000-000000000030").unwrap(),
                digest,
            ),
            ValidatedWorkspaceRootV1::from_encoded("podway.unix-path/v1:2f776f726b74726565")
                .unwrap(),
        )
    }

    fn preview_snapshot() -> podway_core::ProcedureSnapshotV1 {
        parse_procedure_v1(
            r#"{
                "schema": "podway.procedure/v1",
                "id": "preview-fixture",
                "version": "1",
                "name": "Preview fixture",
                "stages": [
                    {"id": "first", "title": "First", "instructions": [], "items": []},
                    {"id": "second", "title": "Second", "instructions": [], "items": []}
                ],
                "rework": {"allow_return_to": "any_previous"}
            }"#,
            ProcedureFormatV1::Json,
        )
        .unwrap()
        .into_snapshot_v1(
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000031").unwrap(),
            ProcedureSourceLabelV1::file("preview-fixture").unwrap(),
            UnixMillis::new(1),
            ProcedureWarningPolicyV1::Accept,
        )
        .unwrap()
    }

    fn preview_running_session() -> SessionAggregateV1 {
        SessionAggregateV1::start(
            SessionId::new("00000000-0000-4000-8000-000000000032").unwrap(),
            "Preview fixture",
            preview_snapshot(),
            AttemptId::new("00000000-0000-4000-8000-000000000033").unwrap(),
            UnixMillis::new(10),
        )
        .unwrap()
    }

    fn preview_apply(
        prior: &SessionAggregateV1,
        command: SessionCommandV1,
        now: u64,
    ) -> SessionAggregateV1 {
        preview_transition_v1(
            Some(prior),
            &command,
            CommandContextV1 {
                expected_revision: prior.revision(),
                now: UnixMillis::new(now),
            },
        )
        .unwrap()
        .next_aggregate()
        .unwrap()
        .clone()
    }

    fn preview_second_stage_session() -> SessionAggregateV1 {
        let first = preview_running_session();
        preview_apply(
            &first,
            SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: first.active_attempt_id().unwrap().clone(),
                next_attempt_id: Some(
                    AttemptId::new("00000000-0000-4000-8000-000000000034").unwrap(),
                ),
                local_artifact_verifications: Vec::new(),
            }),
            11,
        )
    }

    fn preview_completed_session() -> SessionAggregateV1 {
        let second = preview_second_stage_session();
        preview_apply(
            &second,
            SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: second.active_attempt_id().unwrap().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            12,
        )
    }

    fn production_preview_map(
        command: SliceCommandV1,
        binding: &WorkspaceBindingV1,
        prior: Option<&SessionAggregateV1>,
    ) -> Result<Map<String, Value>, DispatchFailureV1> {
        let (domain_command, expected_revision) = preview_command(
            &command,
            binding,
            prior,
            UnixMillis::new(30),
            &crate::execution::EmbeddedPresetProcedureProviderV1,
        )?;
        let outcome = preview_transition_v1(
            prior,
            &domain_command,
            CommandContextV1 {
                expected_revision,
                now: UnixMillis::new(30),
            },
        )
        .map_err(|error| map_preview_domain_error(error, &command))?;
        Ok(preview_result_map(&outcome, prior))
    }

    fn preview_start_command(title: &str) -> SliceCommandV1 {
        SliceCommandV1::SessionStart(podway_protocol::SessionStartV1 {
            source: podway_protocol::SessionStartSourceV1::Preset {
                preset: "bug-fix".to_owned(),
            },
            expected_procedure_digest: None,
            task_title: title.to_owned(),
            dry_run: true,
        })
    }

    fn assert_preview_projection(
        result: &Map<String, Value>,
        revision_before: Value,
        revision_after: Value,
        destination_stage: Option<&str>,
        destination_attempt_number: Option<u64>,
        affected_stages: &[&str],
    ) {
        assert_eq!(result.get("preview"), Some(&Value::Bool(true)));
        assert_eq!(result.get("changed"), Some(&Value::Bool(true)));
        assert_eq!(result.get("revision_before"), Some(&revision_before));
        assert_eq!(result.get("revision_after"), Some(&revision_after));
        match destination_stage {
            Some(stage_id) => {
                assert_eq!(
                    result["destination_attempt"]["stage_id"],
                    Value::String(stage_id.to_owned())
                );
                assert_eq!(
                    result["destination_attempt"]["attempt_id"],
                    Value::String(PREVIEW_ATTEMPT_ID_V1.to_owned())
                );
                assert_eq!(
                    result["destination_attempt"]["attempt_number"],
                    Value::from(destination_attempt_number.unwrap())
                );
            }
            None => assert_eq!(result["destination_attempt"], Value::Null),
        }
        assert_eq!(
            result["affected_stages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|stage| stage["stage_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            affected_stages.to_vec()
        );
    }

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
    fn receipt_only_lookup_projects_terminal_response_and_rejects_legacy_receipts() {
        let digest = Sha256Digest::new(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            JobReceiptV1::new(
                7,
                JobId::new("00000000-0000-4000-8000-000000000017").unwrap(),
                digest.clone(),
            ),
            PersistedTerminalResultV1::Cancelled,
            PersistedTerminalJobProjectionV1::new(
                PersistedTerminalJobStateV1::Cancelled,
                UnixMillis::new(10),
                None,
                UnixMillis::new(12),
            )
            .unwrap(),
            None,
        )
        .unwrap();
        assert!(receipt_only_job_result_value(&receipt, &digest).is_err());

        let receipt = receipt
            .with_lookup_command(PersistedDomainCommandV1::WorkspaceInitialize)
            .unwrap();
        let output = receipt_only_job_result_value(&receipt, &digest).unwrap();
        assert_eq!(output["id"], receipt.job().job_id().as_str());
        assert_eq!(output["sequence"], 7);
        assert_eq!(output["state"], "cancelled");
        assert_eq!(output["command"], "workspace.init");
        assert_eq!(output["terminal_response"]["kind"], "cancelled");
        assert_eq!(output["request_digest"], digest.as_str());
        assert!(
            receipt_only_job_result_value(
                &receipt,
                &Sha256Digest::new(
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .unwrap()
            )
            .is_err()
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

        let output =
            terminal_session_projection(&result, Some(&projection), TerminalCommandKindV1::Other)
                .unwrap()
                .expect("persisted session projection must produce output");

        assert_eq!(output.id(), &session_id);
        assert_eq!(output.title(), "Immutable terminal session");
        assert_eq!(output.lifecycle(), SessionLifecycleV1::Running);
        assert_eq!(output.revision_before(), Revision::new(4));
        assert_eq!(output.revision_after(), Revision::new(5));
    }
    #[test]
    fn pstrt004_terminal_job_replay_exposes_the_admitted_procedure_digest() {
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000018").unwrap();
        let digest = Sha256Digest::new(
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .unwrap();
        let projection = PersistedTerminalSessionProjectionV1::new(
            session_id.clone(),
            "Immutable start digest".to_owned(),
            PersistedSessionLifecycleV1::Running,
            Revision::ZERO,
            Revision::new(1),
        )
        .unwrap()
        .with_procedure_digest(digest.clone());
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(18),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                session_id,
                revision_before: Revision::ZERO,
                revision_after: Revision::new(1),
                changed: true,
            }),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            Some(projection),
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionStart)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                "00000000-0000-4000-8000-000000000018",
                "session.start",
                WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                "/safe/worktree",
                18,
            )
            .unwrap(),
        )
        .unwrap();

        let output = terminal_job_response(&receipt, TerminalCommandKindV1::Start).unwrap();
        assert_eq!(output["schema"], "podway.output/v1");
        assert_eq!(output["result"]["procedure_digest"], json!(digest));
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
            terminal_result(
                &missing_session_projection,
                &SliceCommandV1::WorkspaceInit(podway_protocol::WorkspaceInitV1 { repair: false }),
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::WorkspaceStateUnreadable
        );
    }
    #[test]
    fn session_reset_terminal_without_projection_renders_immediate_replay_and_job_read() {
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000011").unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(11),
            PersistedTerminalResultV1::Success(terminal_session_clear_result(session_id.clone())),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            None,
        )
        .unwrap();
        let reset = SliceCommandV1::SessionReset(podway_protocol::SessionResetV1 {
            confirmed: true,
            dry_run: false,
            preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                expected_session_id: session_id,
                expected_session_revision: Revision::new(4),
            },
        });

        let PersistedTerminalResultV1::Success(result) = receipt.result() else {
            panic!("fixture terminal receipt must succeed");
        };
        assert_eq!(
            terminal_result(&receipt, &reset),
            Ok(DispatcherTerminalResultV1::Output(
                DispatcherTerminalOutputV1::new(
                    None,
                    terminal_success_result_map(&terminal_job_success_result(result)),
                    Vec::new(),
                ),
            )),
        );
        assert_eq!(
            terminal_result(
                &receipt,
                &SliceCommandV1::WorkspaceInit(podway_protocol::WorkspaceInitV1 { repair: false }),
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::WorkspaceStateUnreadable
        );
        let invalid_view = terminal_view(
            podway_store::CommandV1::WorkspaceInitialize,
            StoreJobStateV1::Succeeded,
            PersistedTerminalReceiptV1::new_with_projections(
                fixture_job(12),
                PersistedTerminalResultV1::Success(terminal_session_clear_result(
                    SessionId::new("00000000-0000-4000-8000-000000000011").unwrap(),
                )),
                terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
                None,
            )
            .unwrap(),
        );
        assert_eq!(
            job_read_projection(&invalid_view).unwrap_err().kind(),
            DispatchFailureKindV1::WorkspaceStateUnreadable
        );

        let view = terminal_view(
            podway_store::CommandV1::SessionReset,
            StoreJobStateV1::Succeeded,
            receipt,
        );
        let (_, terminal) = job_read_projection(&view).unwrap();
        assert_eq!(terminal["schema"], "podway.output/v1");
        assert_eq!(terminal["result"]["changed"], true);
        assert_eq!(terminal["result"]["revision_after"], 0);
        assert_eq!(terminal["session"], Value::Null);
    }
    #[test]
    fn job_reads_project_complete_terminal_success_receipts() {
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000020").unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(20),
            PersistedTerminalResultV1::Success(terminal_session_result(session_id.clone())),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            Some(fixture_terminal_session_projection(session_id)),
        )
        .unwrap();
        let view = terminal_view(
            podway_store::CommandV1::SessionComplete,
            StoreJobStateV1::Succeeded,
            receipt,
        );

        let (job, terminal) = job_read_projection(&view).unwrap();

        assert_eq!(job.state(), JobStateV1::Succeeded);
        assert_eq!(terminal["schema"], "podway.output/v1");
        assert_eq!(terminal["result"]["changed"], true);
        assert_eq!(
            terminal["session"]["title"],
            "Stored immutable terminal session"
        );
        assert_eq!(terminal["session"]["revision_before"], 4);
        assert_eq!(terminal["session"]["revision_after"], 5);
    }

    #[test]
    fn job_reads_render_failed_receipts_with_public_catalog_errors() {
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(21),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::PreconditionFailed {
                expected: Revision::new(4),
                actual: Revision::new(5),
            }),
            terminal_job_projection(PersistedTerminalJobStateV1::Failed),
            None,
        )
        .unwrap();
        let view = terminal_view(
            podway_store::CommandV1::ItemCheck {
                item_id: ItemId::new("proof").unwrap(),
            },
            StoreJobStateV1::Failed,
            receipt,
        );

        let (_, terminal) = job_read_projection(&view).unwrap();

        assert_eq!(terminal["schema"], "podway.error/v1");
        assert_eq!(terminal["code"], "ITEM_REVISION_CONFLICT");
        assert_eq!(
            terminal["message"],
            "The item changed after it was observed."
        );
        assert_eq!(terminal["retryable"], true);
        assert_eq!(terminal["exit_code"], 4);
        assert_eq!(terminal["details"]["expected_revision"], 4);
        assert_eq!(terminal["details"]["current_revision"], 5);

        let expected = SessionId::new("00000000-0000-4000-8000-000000000026").unwrap();
        let actual = SessionId::new("00000000-0000-4000-8000-000000000027").unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(26),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::SessionIdentityMismatch {
                expected: expected.clone(),
                actual: Some(actual.clone()),
            }),
            terminal_job_projection(PersistedTerminalJobStateV1::Failed),
            None,
        )
        .unwrap();
        let view = terminal_view(
            podway_store::CommandV1::SessionComplete,
            StoreJobStateV1::Failed,
            receipt,
        );

        let (_, terminal) = job_read_projection(&view).unwrap();

        assert_eq!(terminal["schema"], "podway.error/v1");
        assert_eq!(terminal["code"], "SESSION_ID_MISMATCH");
        assert_eq!(terminal["retryable"], false);
        assert_eq!(terminal["exit_code"], 4);
        assert_eq!(
            terminal["details"],
            json!({
                "schema": "podway.session-id-mismatch-details/v1",
                "expected_session_id": expected.as_str(),
                "actual_session_id": actual.as_str(),
                "admission": {
                    "admitted": true,
                    "job_id": fixture_job(26).job_id().as_str(),
                    "workspace_sequence": 26
                }
            })
        );

        let expected_attempt = AttemptId::new("00000000-0000-4000-8000-000000000028").unwrap();
        let actual_attempt = AttemptId::new("00000000-0000-4000-8000-000000000029").unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(28),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::AttemptNotCurrent {
                expected: expected_attempt.clone(),
                actual: Some(actual_attempt.clone()),
            }),
            terminal_job_projection(PersistedTerminalJobStateV1::Failed),
            None,
        )
        .unwrap();
        let view = terminal_view(
            podway_store::CommandV1::ItemCheck {
                item_id: ItemId::new("proof").unwrap(),
            },
            StoreJobStateV1::Failed,
            receipt,
        );

        let (_, terminal) = job_read_projection(&view).unwrap();

        assert_eq!(terminal["schema"], "podway.error/v1");
        assert_eq!(terminal["code"], "ATTEMPT_NOT_CURRENT");
        assert_eq!(terminal["retryable"], true);
        assert_eq!(terminal["exit_code"], 4);
        assert_eq!(
            terminal["details"]["admission"],
            json!({
                "admitted": true,
                "job_id": fixture_job(28).job_id().as_str(),
                "workspace_sequence": 28
            })
        );
        assert_eq!(
            terminal["details"]["expected_attempt_id"],
            expected_attempt.as_str()
        );
        assert_eq!(
            terminal["details"]["actual_attempt_id"],
            actual_attempt.as_str()
        );
    }

    #[test]
    fn job_reads_render_cancellation_and_active_jobs_without_terminal_receipts() {
        let cancelled_receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(22),
            PersistedTerminalResultV1::Cancelled,
            terminal_job_projection(PersistedTerminalJobStateV1::Cancelled),
            None,
        )
        .unwrap();
        let cancelled = terminal_view(
            podway_store::CommandV1::SessionStart,
            StoreJobStateV1::Cancelled,
            cancelled_receipt,
        );
        let (_, terminal) = job_read_projection(&cancelled).unwrap();
        assert_eq!(
            terminal,
            serde_json::json!({"kind": "cancelled", "payload": {"cancelled": true}})
        );

        let active_job = fixture_job(23);
        let active = JobViewV1::new(
            ClaimedExecutionV1::new(
                podway_store::CommandV1::SessionStart,
                RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
            ),
            active_job,
            StoreJobStateV1::Queued,
            UnixMillis::new(20),
            None,
            None,
            None,
        );
        let (job, terminal) = job_read_projection(&active).unwrap();
        assert_eq!(job.state(), JobStateV1::Queued);
        assert_eq!(terminal, Value::Null);
    }

    #[test]
    fn job_reads_fail_closed_when_store_state_and_terminal_receipt_disagree() {
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(24),
            PersistedTerminalResultV1::Cancelled,
            terminal_job_projection(PersistedTerminalJobStateV1::Cancelled),
            None,
        )
        .unwrap();
        let mismatched = terminal_view(
            podway_store::CommandV1::SessionStart,
            StoreJobStateV1::Succeeded,
            receipt,
        );

        assert_eq!(
            job_read_projection(&mismatched).unwrap_err().kind(),
            DispatchFailureKindV1::WorkspaceStateUnreadable
        );
    }

    #[test]
    fn terminal_job_reads_replay_the_immutable_persisted_projection() {
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000025").unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(25),
            PersistedTerminalResultV1::Success(terminal_session_result(session_id.clone())),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            Some(fixture_terminal_session_projection(session_id)),
        )
        .unwrap();
        let view = terminal_view(
            podway_store::CommandV1::SessionComplete,
            StoreJobStateV1::Succeeded,
            receipt,
        );

        let first = job_result_value(&view).unwrap();
        let replay = job_result_value(&view).unwrap();

        assert_eq!(replay, first);
        assert_eq!(
            replay["terminal_response"]["session"]["title"],
            "Stored immutable terminal session"
        );
        assert!(
            replay["terminal_response"]["result"]
                .get("session_id")
                .is_none()
        );
    }
    #[test]
    fn production_preview_projects_all_dry_run_variants_from_core_outcomes() {
        let binding = preview_binding();

        let start =
            production_preview_map(preview_start_command("Preview start"), &binding, None).unwrap();
        assert_preview_projection(
            &start,
            Value::Null,
            Value::from(1),
            Some("reproduce"),
            Some(1),
            &["reproduce"],
        );

        let running = preview_running_session();
        let start_replace = production_preview_map(
            SliceCommandV1::SessionStartReplace(podway_protocol::SessionStartReplaceV1 {
                start: podway_protocol::SessionStartV1 {
                    source: podway_protocol::SessionStartSourceV1::Preset {
                        preset: "bug-fix".to_owned(),
                    },
                    expected_procedure_digest: None,
                    task_title: "Preview replacement".to_owned(),
                    dry_run: true,
                },
                confirmed: true,
                preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                    expected_session_id: running.session_id().clone(),
                    expected_session_revision: running.revision(),
                },
            }),
            &binding,
            Some(&running),
        )
        .unwrap();
        assert_preview_projection(
            &start_replace,
            Value::from(1),
            Value::from(1),
            Some("reproduce"),
            Some(1),
            &["reproduce"],
        );

        let second = preview_second_stage_session();
        let returned = production_preview_map(
            SliceCommandV1::SessionReturn(podway_protocol::SessionReturnV1 {
                destination_stage_id: podway_core::StageId::new("first").unwrap(),
                reason: "Return to first".to_owned(),
                dry_run: true,
                preconditions: podway_protocol::SessionMutationPreconditionsWireV1 {
                    expected_session_id: second.session_id().clone(),
                    expected_attempt_id: second.active_attempt_id().unwrap().clone(),
                    expected_session_revision: second.revision(),
                },
            }),
            &binding,
            Some(&second),
        )
        .unwrap();
        assert_preview_projection(
            &returned,
            Value::from(2),
            Value::from(3),
            Some("first"),
            Some(2),
            &["first", "second"],
        );

        let completed = preview_completed_session();
        let reopened = production_preview_map(
            SliceCommandV1::SessionReopen(podway_protocol::SessionReopenV1 {
                destination_stage_id: podway_core::StageId::new("first").unwrap(),
                reason: "Reopen first".to_owned(),
                dry_run: true,
                preconditions: podway_protocol::SessionRevisionPreconditionsWireV1 {
                    expected_session_id: completed.session_id().clone(),
                    expected_session_revision: completed.revision(),
                },
            }),
            &binding,
            Some(&completed),
        )
        .unwrap();
        assert_preview_projection(
            &reopened,
            Value::from(3),
            Value::from(4),
            Some("first"),
            Some(2),
            &["first", "second"],
        );

        let reset = production_preview_map(
            SliceCommandV1::SessionReset(podway_protocol::SessionResetV1 {
                confirmed: true,
                dry_run: true,
                preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                    expected_session_id: running.session_id().clone(),
                    expected_session_revision: running.revision(),
                },
            }),
            &binding,
            Some(&running),
        )
        .unwrap();
        assert_preview_projection(
            &reset,
            Value::from(1),
            Value::Null,
            None,
            None,
            &["first", "second"],
        );
    }

    #[test]
    fn production_preview_preserves_variant_specific_public_errors() {
        let binding = preview_binding();
        let running = preview_running_session();

        assert_eq!(
            production_preview_map(
                preview_start_command("Existing session"),
                &binding,
                Some(&running)
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::SessionNotRunning
        );
        assert_eq!(
            production_preview_map(
                SliceCommandV1::SessionStartReplace(podway_protocol::SessionStartReplaceV1 {
                    start: podway_protocol::SessionStartV1 {
                        source: podway_protocol::SessionStartSourceV1::Preset {
                            preset: "bug-fix".to_owned(),
                        },
                        expected_procedure_digest: None,
                        task_title: "Missing session".to_owned(),
                        dry_run: true,
                    },
                    confirmed: true,
                    preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                        expected_session_id: running.session_id().clone(),
                        expected_session_revision: running.revision(),
                    },
                }),
                &binding,
                None,
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::SessionIdMismatch
        );
        assert_eq!(
            production_preview_map(
                SliceCommandV1::SessionReturn(podway_protocol::SessionReturnV1 {
                    destination_stage_id: podway_core::StageId::new("second").unwrap(),
                    reason: "Invalid forward return".to_owned(),
                    dry_run: true,
                    preconditions: podway_protocol::SessionMutationPreconditionsWireV1 {
                        expected_session_id: running.session_id().clone(),
                        expected_attempt_id: running.active_attempt_id().unwrap().clone(),
                        expected_session_revision: running.revision(),
                    },
                }),
                &binding,
                Some(&running),
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::ReturnNotAllowed
        );
        assert_eq!(
            production_preview_map(
                SliceCommandV1::SessionReopen(podway_protocol::SessionReopenV1 {
                    destination_stage_id: podway_core::StageId::new("first").unwrap(),
                    reason: "Running sessions cannot reopen".to_owned(),
                    dry_run: true,
                    preconditions: podway_protocol::SessionRevisionPreconditionsWireV1 {
                        expected_session_id: running.session_id().clone(),
                        expected_session_revision: running.revision(),
                    },
                }),
                &binding,
                Some(&running),
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::ReopenNotAllowed
        );
        assert_eq!(
            production_preview_map(
                SliceCommandV1::SessionReset(podway_protocol::SessionResetV1 {
                    confirmed: true,
                    dry_run: true,
                    preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                        expected_session_id: running.session_id().clone(),
                        expected_session_revision: Revision::ZERO,
                    },
                }),
                &binding,
                Some(&running),
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::SessionRevisionConflict
        );

        let foreign_session_id = SessionId::new("00000000-0000-4000-8000-000000000099").unwrap();
        assert_eq!(
            production_preview_map(
                SliceCommandV1::SessionReset(podway_protocol::SessionResetV1 {
                    confirmed: true,
                    dry_run: true,
                    preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                        expected_session_id: foreign_session_id,
                        expected_session_revision: running.revision(),
                    },
                }),
                &binding,
                Some(&running),
            )
            .unwrap_err()
            .kind(),
            DispatchFailureKindV1::SessionIdMismatch
        );
    }
}
