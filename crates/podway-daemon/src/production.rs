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
    StartSessionV1, TraceSequenceV2, UnixMillis, preview_transition_v1,
};
use podway_git::{
    Base64UrlPathBytesV1, DiagnosticPathDisplayV1, LosslessPathV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    CommandNameV1, IdempotencyKeyV1, JobOutputV1, JobStateV1, OutputEnvelopeInputV2,
    OutputEnvelopeV2, ProcedureV2MutationRequestV1, ProcedureV2StartRequestV1, RequestEnvelopeV1,
    RequestIdV1, ResponseEnvelopeV1, ResponseEnvelopeV2, Rfc3339MillisV1, SessionLifecycleV1,
    SessionOutputV1, SessionStartSourceV1, SliceCommandV1, SliceRequestV1,
    TerminalJobCancellationProjectionV1, TerminalJobResponseV1, TerminalJobSuccessResultV1,
    WorkspaceOutputV1, WorktreeSelectorWireV1, canonical_reset_all_identity_v1,
    decode_response_payload_v2,
};
use podway_store::{
    AdmitOutcomeV1, CancelOutcomeV1, CanonicalRequestDigestV1, DurableWorktreeIdentityV1,
    EpochMillisV1, GraphStartCurrentTaskV2, GraphWorkspaceViewV2,
    IdempotencyKeyV1 as StoreIdempotencyKeyV1, JobListQueryV1, JobReceiptOrTerminalV1,
    JobReceiptV1, JobStateV1 as StoreJobStateV1, JobViewV1, PersistedGraphMutationFailureV2,
    PersistedGraphTerminalOperationV2, PersistedTerminalJobStateV1, PersistedTerminalReceiptV1,
    PersistedTerminalSessionProjectionV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreErrorV1, StoreIdempotencyReadContractV1, StoreReadContractV1,
    StoreReconciliationReadContractV1, WorkerIdV1, WorkspaceBindingV1,
    codec::{
        PersistedDomainCommandV1, PersistedDomainErrorV1, PersistedDomainResultV1,
        PersistedSessionLifecycleV1, PersistedTerminalResultV1,
    },
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use crate::{
    development_v2::DevelopmentV2AdmissionGateV1,
    dispatch::{
        CatalogDispatchErrorMapperV1, DevelopmentV2AdmissionProofV1, DispatchErrorDetailsV1,
        DispatchFailureKindV1, DispatchFailureV1, DispatchResponseMetadataV1,
        DispatcherControlServiceV1, DispatcherJobOutputV1, DispatcherNextRequestV1,
        DispatcherPreviewServiceV1, DispatcherReadOutputV1, DispatcherReadServiceV1,
        DispatcherReconciliationOutputV1, DispatcherStatusRequestV1, DispatcherTerminalOutputV1,
        DispatcherTerminalResultV1, DispatcherWorkspaceOutputV1, MutationAdmissionWorkerV1,
        MutationDispatchOutcomeV1, MutationResponseContextV1, MutationWaitV1,
        RequestDispatcherV1Adapter, RequestReadWaitV1, TerminalResponseContextV1,
        WorkspaceRuntimeV1, terminal_response_envelope_v1,
    },
    execution::{
        DaemonExecutionEngineV1, ExecutionClockV1, ExecutionErrorV1, ExecutionIdSourceV1,
        ProcedureProviderV1, ProcedureV2SourceAdmissionErrorV1, ProcedureV2StartPreparationErrorV1,
        ResetAllPreparationOutcomeV1, admitted_procedure_v2_start_projection_v1,
        admitted_start_procedure_digest_v1, bind_initial_goal_for_start_v2,
        prepare_custom_procedure_v2_start, prepare_preset_procedure_v2_start,
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
        ReadonlyReconciliationResolutionV1, ResetSourceAuthorityV1, WorkspaceRuntimeErrorV1,
        WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1, WorkspaceSchedulerContextV1,
        WorkspaceSchedulerRevalidationV1, WorkspaceStoreReadFacadeV1, WorkspaceStoreSlotV1,
    },
    scheduler::WorkspaceSchedulerV1,
    server::{DaemonRequestV1, ResponseMetadataSourceV1, SystemResponseMetadataSourceV1},
    v2_read_service::{
        GraphStatusTierV2, GraphViewErrorV2, project_graph_next_v2, project_graph_status_v2,
    },
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
    development_v2_admission: DevelopmentV2AdmissionGateV1,
}

impl ProductionWorkspaceRuntimeV1 {
    pub fn new(
        manager: Arc<WorkspaceRuntimeManagerV1>,
        clock: Arc<NativeProductionClockV1>,
    ) -> Self {
        Self {
            manager,
            clock,
            development_v2_admission: DevelopmentV2AdmissionGateV1::closed(),
        }
    }

    pub(crate) fn with_development_v2_admission(
        mut self,
        admission: DevelopmentV2AdmissionGateV1,
    ) -> Self {
        self.development_v2_admission = admission;
        self
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

    fn development_v2_admission(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Option<DevelopmentV2AdmissionProofV1> {
        if !self.development_v2_admission.process_is_eligible() {
            return None;
        }
        let expected_workspace_id = selector.expected_uuid();
        let Ok(selector) = selector_from_wire(selector) else {
            return None;
        };
        let Ok(resolution) = self
            .manager
            .resolve_existing_readonly(selector, expected_workspace_id)
        else {
            return None;
        };
        self.development_v2_admission
            .permits_workspace(resolution.binding(), resolution.worktree())
            .then(DevelopmentV2AdmissionProofV1::granted_for_runtime)
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
        Self::engine(context, self.observability.clone())?
            .execute_next_with_graph_v2(binding, worker)
    }

    fn admit_procedure_v2_start(
        &self,
        context: &WorkspaceSchedulerContextV1,
        binding: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
        response_context: Option<&podway_store::PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ProcedureV2StartPreparationErrorV1> {
        if context.binding() != binding {
            return Err(ProcedureV2StartPreparationErrorV1::Execution(
                ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "scheduler context binding changed during Procedure v2 admission",
                },
            ));
        }
        Self::engine(context, self.observability.clone())
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?
            .admit_procedure_v2_start_for_workspace_with_response_context(
                binding,
                request,
                idempotency_key,
                response_context.cloned(),
            )
    }

    fn admit_procedure_v2_typed_start(
        &self,
        context: &WorkspaceSchedulerContextV1,
        binding: &WorkspaceBindingV1,
        request: &ProcedureV2StartRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
        response_context: Option<&podway_store::PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ProcedureV2StartPreparationErrorV1> {
        if context.binding() != binding {
            return Err(ProcedureV2StartPreparationErrorV1::Execution(
                ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "scheduler context changed during typed Procedure v2 start admission",
                },
            ));
        }
        Self::engine(context, self.observability.clone())
            .map_err(ProcedureV2StartPreparationErrorV1::Execution)?
            .admit_procedure_v2_typed_start_for_workspace_with_response_context(
                binding,
                request,
                idempotency_key,
                response_context.cloned(),
            )
    }

    fn admit_procedure_v2_mutation(
        &self,
        context: &WorkspaceSchedulerContextV1,
        binding: &WorkspaceBindingV1,
        request: &SliceRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
        response_context: Option<&podway_store::PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        if context.binding() != binding {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "scheduler context binding changed during Procedure v2 admission",
            });
        }
        Self::engine(context, self.observability.clone())?
            .admit_procedure_v2_mutation_for_workspace_with_response_context(
                binding,
                request,
                idempotency_key,
                response_context.cloned(),
            )
    }

    fn admit_procedure_v2_typed_mutation(
        &self,
        context: &WorkspaceSchedulerContextV1,
        binding: &WorkspaceBindingV1,
        request: &ProcedureV2MutationRequestV1,
        idempotency_key: StoreIdempotencyKeyV1,
        response_context: Option<&podway_store::PersistedResponseContextV1>,
    ) -> Result<Option<AdmitOutcomeV1>, ExecutionErrorV1> {
        if context.binding() != binding {
            return Err(ExecutionErrorV1::InvalidPersistedExecution {
                reason: "scheduler context binding changed during typed Procedure v2 admission",
            });
        }
        Self::engine(context, self.observability.clone())?
            .admit_procedure_v2_typed_mutation_for_workspace_with_response_context(
                binding,
                request,
                idempotency_key,
                response_context.cloned(),
            )
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
    manager: Arc<WorkspaceRuntimeManagerV1>,
    clock: Arc<NativeProductionClockV1>,
}

impl ProductionReadServiceV1 {
    pub fn new(
        manager: Arc<WorkspaceRuntimeManagerV1>,
        clock: Arc<NativeProductionClockV1>,
    ) -> Self {
        Self { manager, clock }
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
        selector: &WorktreeSelectorWireV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<DispatcherReconciliationOutputV1, DispatchFailureV1> {
        let expected_workspace_id = selector.expected_uuid();
        let selector = selector_from_wire(selector)?;
        let resolution = self
            .manager
            .resolve_reconciliation_readonly(selector, expected_workspace_id)
            .map_err(map_runtime_error)?;
        let key = StoreIdempotencyKeyV1::new(idempotency_key.as_str().to_owned())
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let resolution = match resolution {
            ReadonlyReconciliationResolutionV1::Missing => {
                return Ok(DispatcherReconciliationOutputV1::new(
                    None,
                    Map::from_iter([("found".to_owned(), Value::Bool(false))]),
                    Vec::new(),
                ));
            }
            ReadonlyReconciliationResolutionV1::ResetMarker { marker, worktree } => {
                if marker.idempotency_key() != &key {
                    return Ok(DispatcherReconciliationOutputV1::new(
                        None,
                        Map::from_iter([("found".to_owned(), Value::Bool(false))]),
                        Vec::new(),
                    ));
                }
                let workspace = WorkspaceOutputV1::new(
                    marker.target_workspace_uuid().clone(),
                    worktree.roots().worktree_root().display().as_str(),
                    1,
                )
                .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
                let job = JobOutputV1::new(
                    marker.operation_id().clone(),
                    1,
                    JobStateV1::Running,
                    rfc3339_millis(marker.submitted_at_ms())?,
                    None,
                    None,
                )
                .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
                let mut job = serde_json::to_value(job)
                    .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
                let job = job
                    .as_object_mut()
                    .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
                job.insert(
                    "command".to_owned(),
                    Value::String("workspace.reset_all".to_owned()),
                );
                job.insert(
                    "request_digest".to_owned(),
                    Value::String(marker.request_digest().as_str().to_owned()),
                );
                job.insert("terminal_response".to_owned(), Value::Null);
                return Ok(DispatcherReconciliationOutputV1::new(
                    Some(workspace),
                    Map::from_iter([
                        ("found".to_owned(), Value::Bool(true)),
                        ("job".to_owned(), Value::Object(job.clone())),
                    ]),
                    Vec::new(),
                ));
            }
            ReadonlyReconciliationResolutionV1::Store(resolution) => resolution,
        };
        let snapshot = if let Some(scheduler) = resolution.active_scheduler() {
            let context = scheduler.context_snapshot();
            context
                .store()
                .read_reconciliation_snapshot(resolution.binding().identity(), &key)
                .map_err(map_store_error)?
        } else {
            SqliteStoreV1::inspect_reconciliation_snapshot(
                resolution.database_path(),
                resolution.binding().identity(),
                resolution.store_options(),
                &key,
                EpochMillisV1::new(self.clock.now().get()),
            )
            .map_err(map_store_error)?
        };
        let workspace = WorkspaceOutputV1::new(
            resolution.binding().identity().workspace_uuid().clone(),
            resolution
                .worktree()
                .roots()
                .worktree_root()
                .display()
                .as_str(),
            snapshot.latest_workspace_sequence(),
        )
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
        let Some(binding) = snapshot.lookup() else {
            return Ok(DispatcherReconciliationOutputV1::new(
                Some(workspace),
                Map::from_iter([("found".to_owned(), Value::Bool(false))]),
                Vec::new(),
            ));
        };
        let mut job = if let Some(view) = snapshot.job() {
            if view.job().request_digest() != binding.request_digest() {
                return Err(terminal_replay_integrity_failure());
            }
            job_result_value(view)?
        } else {
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
        let mut result = Map::from_iter([
            ("found".to_owned(), Value::Bool(true)),
            ("job".to_owned(), Value::Object(job.clone())),
        ]);
        if job
            .get("terminal_response")
            .is_some_and(terminal_response_requires_v2_wrapper)
        {
            result.insert(
                "schema".to_owned(),
                Value::String("podway.job-lookup-result/v2".to_owned()),
            );
        }
        Ok(DispatcherReconciliationOutputV1::new(
            Some(workspace),
            result,
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
        let uses_v2 = terminal_response_requires_v2_wrapper(&result);
        let mut result = Map::from_iter([("job".to_owned(), result)]);
        if uses_v2 {
            result.insert(
                "schema".to_owned(),
                Value::String("podway.job-result/v2".to_owned()),
            );
        }
        Ok(DispatcherJobOutputV1::new(job, result, Vec::new()))
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
        podway_store::install_terminal_envelope_sealer_v1(seal_terminal_receipt_v1);
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

    fn read_wait_v2(&self, wait: RequestReadWaitV1) -> Result<ReadWaitV1, DispatchFailureV1> {
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
        marker: &crate::workspace::ResetMarkerV1,
        idempotency_key: &StoreIdempotencyKeyV1,
        request_digest: &CanonicalRequestDigestV1,
        active: Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
        request: &SliceRequestV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1> {
        self.retire_reset_scheduler(active)
            .map_err(|failure| failure.with_admission_identity(marker.operation_id().clone(), 1))?;
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
                .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?
                .with_frozen_public_terminal_envelope(),
                self.completion_mode(wait)?,
            )
            .map_err(|error| map_mutation_worker_error(request, error))?;
        let admission = admission_receipt(submission.admission()).clone();
        let outcome = (|| {
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
        })();
        outcome.map_err(|failure| {
            failure
                .with_admission_identity(admission.job_id().clone(), admission.identity_sequence())
        })
    }

    fn dispatch_development_v2(
        &self,
        _proof: DevelopmentV2AdmissionProofV1,
        request: &RequestEnvelopeV1,
        daemon_request: &DaemonRequestV1,
    ) -> Result<Option<ResponseEnvelopeV2>, DispatchFailureV1> {
        if let DaemonRequestV1::ProcedureV2Mutation(typed_request) = daemon_request {
            if !matches!(
                typed_request.command().command_name(),
                "session.decide"
                    | "session.rework"
                    | "goal.define"
                    | "goal.revise"
                    | "goal.assess_criterion"
            ) {
                return Ok(None);
            }
            let terminal_kind = match typed_request.command().command_name() {
                "session.decide" => TerminalCommandKindV1::Decision,
                "session.rework" => TerminalCommandKindV1::Rework,
                "goal.define" => TerminalCommandKindV1::GoalDefinition,
                "goal.revise" => TerminalCommandKindV1::GoalRevision,
                "goal.assess_criterion" => TerminalCommandKindV1::CriterionAssessment,
                _ => unreachable!("typed mutation was filtered above"),
            };
            let runtime = ProductionWorkspaceRuntimeV1::new(
                Arc::clone(&self.manager),
                Arc::clone(&self.clock),
            );
            let workspace = runtime.resolve_existing(typed_request.selector())?;
            let idempotency_key = StoreIdempotencyKeyV1::new(
                request
                    .idempotency_key()
                    .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?
                    .as_str(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
            let response_context = podway_store::PersistedResponseContextV1::new(
                request.request_id().as_str(),
                request.command().as_str(),
                workspace.output.uuid().clone(),
                workspace.output.root(),
                workspace.output.latest_workspace_sequence(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?
            .with_frozen_public_terminal_envelope();
            let completion_mode = if request.options().detach() {
                WorkerCompletionModeV1::Detached
            } else {
                WorkerCompletionModeV1::WaitUntil(
                    MonotonicDeadlineV1::after(
                        self.clock.as_ref(),
                        request.options().wait_timeout_ms(),
                    )
                    .map_err(map_read_error)?,
                )
            };
            let Some(submission) = self
                .worker
                .submit_procedure_v2_typed_mutation_with_response_context(
                    &workspace.scheduler,
                    typed_request,
                    idempotency_key,
                    response_context,
                    completion_mode,
                )
                .map_err(map_worker_error)?
            else {
                return Ok(None);
            };
            let terminal =
                terminal_replay(submission.admission()).or_else(|| match submission.completion() {
                    Some(WorkerWaitResultV1::Terminal(receipt)) => Some(receipt.as_ref()),
                    Some(WorkerWaitResultV1::TimedOut(_)) | None => None,
                });
            if let Some(receipt) = terminal {
                return terminal_direct_response_v2(receipt, terminal_kind, request.request_id())
                    .map(Some);
            }
            if let Some(WorkerWaitResultV1::TimedOut(view)) = submission.completion() {
                let job = job_output(view)?;
                return Err(
                    DispatchFailureV1::new(DispatchFailureKindV1::JobWaitTimeout).with_job(&job),
                );
            }
            let job = job_output_only_from_context(&workspace.scheduler, submission.admission())?;
            let result = json!({
                "schema": "podway.detached-admission-result/v2",
                "detached": true,
                "admission": {
                    "admitted": true,
                    "job_id": job.id(),
                    "workspace_sequence": job.sequence(),
                },
            });
            return OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at: self.clock.generated_at(),
                workspace: Some(workspace.output),
                job: Some(job),
                session: None,
                result: result
                    .as_object()
                    .expect("Procedure v2 detached result is an object")
                    .clone(),
                warnings: Vec::new(),
            })
            .map(ResponseEnvelopeV2::OutputV2)
            .map(Some)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
        }
        if let DaemonRequestV1::ProcedureV2Start(typed_request) = daemon_request {
            if !typed_request.command().is_mutation() {
                let (start, initial_goal, expected_current) = match typed_request.command() {
                    podway_protocol::ProcedureV2StartCommandV1::SessionStart(input) => (
                        &input.start,
                        input.initial_goal.as_ref(),
                        GraphStartCurrentTaskV2::Absent,
                    ),
                    podway_protocol::ProcedureV2StartCommandV1::SessionStartReplace(input) => (
                        &input.start.start,
                        input.start.initial_goal.as_ref(),
                        GraphStartCurrentTaskV2::Exact {
                            session_id: input.preconditions.expected_session_id.clone(),
                            session_revision: input.preconditions.expected_session_revision,
                        },
                    ),
                };
                let Some(initial_goal) = initial_goal else {
                    return Ok(None);
                };
                let selector = selector_from_wire(typed_request.selector())?;
                let readonly = self
                    .manager
                    .resolve_existing_readonly(selector, typed_request.selector().expected_uuid())
                    .map_err(map_runtime_error)?;
                let provider = NativeProcedureProviderV1::new(
                    SqliteWorkspaceBindingInspectorV1::new(readonly.store_options().clone()),
                );
                let ids = NativeExecutionIdSourceV1;
                let now = self.clock.now();
                let state = match &start.source {
                    SessionStartSourceV1::Procedure { procedure } => {
                        prepare_custom_procedure_v2_start(
                            &provider,
                            readonly.binding(),
                            procedure,
                            start.expected_procedure_digest.as_ref(),
                            &start.task_title,
                            ids.next_session_id(),
                            ids.next_attempt_id(),
                            ids.next_procedure_snapshot_id(),
                            now,
                        )
                        .map(Some)
                    }
                    SessionStartSourceV1::Preset { preset } => prepare_preset_procedure_v2_start(
                        &provider,
                        preset,
                        &start.task_title,
                        ids.next_session_id(),
                        ids.next_attempt_id(),
                        ids.next_procedure_snapshot_id(),
                        now,
                    ),
                }
                .map_err(map_procedure_v2_start_preparation_error)?;
                let Some(state) = state else {
                    return Ok(None);
                };
                let state = bind_initial_goal_for_start_v2(state, initial_goal, now)
                    .map_err(map_procedure_v2_start_preparation_error)?;
                let actual_current = SqliteStoreV1::inspect_graph_start_current_task_v2(
                    readonly.database_path(),
                    readonly.binding().identity(),
                    readonly.store_options(),
                    podway_store::EpochMillisV1::new(now.get()),
                )
                .map_err(map_store_error)?;
                validate_graph_start_dry_run_fence_v2(&expected_current, &actual_current)?;
                let result = json!({
                    "schema":"podway.session-start-result/v2", "procedure_schema":"podway.procedure/v2",
                    "procedure_digest":state.snapshot().digest(), "dry_run":true,
                    "goal_tracking":state.snapshot().goal_tracking(), "goal_defined":true,
                });
                return OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                    request_id: request.request_id().clone(),
                    command: request.command().clone(),
                    generated_at: self.clock.generated_at(),
                    workspace: None,
                    job: None,
                    session: None,
                    result: result
                        .as_object()
                        .expect("typed start preview is an object")
                        .clone(),
                    warnings: Vec::new(),
                })
                .map(ResponseEnvelopeV2::OutputV2)
                .map(Some)
                .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
            }
            let runtime = ProductionWorkspaceRuntimeV1::new(
                Arc::clone(&self.manager),
                Arc::clone(&self.clock),
            );
            let workspace = runtime.resolve_existing(typed_request.selector())?;
            let idempotency_key = StoreIdempotencyKeyV1::new(
                request
                    .idempotency_key()
                    .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?
                    .as_str(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
            let response_context = podway_store::PersistedResponseContextV1::new(
                request.request_id().as_str(),
                request.command().as_str(),
                workspace.output.uuid().clone(),
                workspace.output.root(),
                workspace.output.latest_workspace_sequence(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?
            .with_frozen_public_terminal_envelope();
            let completion_mode = if request.options().detach() {
                WorkerCompletionModeV1::Detached
            } else {
                WorkerCompletionModeV1::WaitUntil(
                    MonotonicDeadlineV1::after(
                        self.clock.as_ref(),
                        request.options().wait_timeout_ms(),
                    )
                    .map_err(map_read_error)?,
                )
            };
            let Some(submission) = self
                .worker
                .submit_procedure_v2_typed_start_with_response_context(
                    &workspace.scheduler,
                    typed_request,
                    idempotency_key,
                    response_context,
                    completion_mode,
                )
                .map_err(map_worker_error)?
            else {
                return Ok(None);
            };
            let terminal =
                terminal_replay(submission.admission()).or_else(|| match submission.completion() {
                    Some(WorkerWaitResultV1::Terminal(receipt)) => Some(receipt.as_ref()),
                    Some(WorkerWaitResultV1::TimedOut(_)) | None => None,
                });
            if let Some(receipt) = terminal {
                return terminal_direct_response_v2(
                    receipt,
                    TerminalCommandKindV1::Start,
                    request.request_id(),
                )
                .map(Some);
            }
            if let Some(WorkerWaitResultV1::TimedOut(view)) = submission.completion() {
                return Err(
                    DispatchFailureV1::new(DispatchFailureKindV1::JobWaitTimeout)
                        .with_job(&job_output(view)?),
                );
            }
            let job = job_output_only_from_context(&workspace.scheduler, submission.admission())?;
            let result = json!({
                "schema": "podway.detached-admission-result/v2", "detached": true,
                "admission": { "admitted": true, "job_id": job.id(), "workspace_sequence": job.sequence() },
            });
            return OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at: self.clock.generated_at(),
                workspace: Some(workspace.output),
                job: Some(job),
                session: None,
                result: result
                    .as_object()
                    .expect("detached result is an object")
                    .clone(),
                warnings: Vec::new(),
            })
            .map(ResponseEnvelopeV2::OutputV2)
            .map(Some)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
        }
        let Some(slice_request) = daemon_request.legacy() else {
            return Ok(None);
        };
        if let SliceCommandV1::SessionReset(input) = slice_request.command()
            && input.dry_run
        {
            let selector = selector_from_wire(slice_request.selector())?;
            let readonly = self
                .manager
                .resolve_existing_readonly(selector, slice_request.selector().expected_uuid())
                .map_err(map_runtime_error)?;
            let view = SqliteStoreV1::inspect_graph_workspace_view_v2(
                readonly.database_path(),
                readonly.binding().identity(),
                readonly.store_options(),
                EpochMillisV1::new(self.clock.now().get()),
            )
            .map_err(map_store_error)?;
            let Some(state) = view.graph_state() else {
                return Ok(None);
            };
            validate_graph_view_session_v2(&view, Some(&input.preconditions.expected_session_id))?;
            if state.trace().revision() != input.preconditions.expected_session_revision {
                return Err(
                    DispatchFailureV1::new(DispatchFailureKindV1::SessionRevisionConflict)
                        .with_details(
                            DispatchErrorDetailsV1::default()
                                .with_expected_revision(
                                    input.preconditions.expected_session_revision,
                                )
                                .with_current_revision(state.trace().revision()),
                        ),
                );
            }
            let workspace = WorkspaceOutputV1::new(
                view.identity().workspace_uuid().clone(),
                readonly
                    .worktree()
                    .roots()
                    .worktree_root()
                    .display()
                    .as_str(),
                view.latest_workspace_sequence(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
            let result = json!({
                "schema": "podway.stage-transition-result/v2",
                "admission": { "admitted": false },
                "transition": "reset",
                "reset": true,
                "revision": state.trace().revision(),
            });
            return OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at: self.clock.generated_at(),
                workspace: Some(workspace),
                job: None,
                session: None,
                result: result
                    .as_object()
                    .expect("Procedure v2 reset dry-run result is an object")
                    .clone(),
                warnings: Vec::new(),
            })
            .map(ResponseEnvelopeV2::OutputV2)
            .map(Some)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
        }
        if matches!(
            slice_request.command(),
            SliceCommandV1::SessionStatus(_) | SliceCommandV1::SessionNext(_)
        ) {
            let selector = selector_from_wire(slice_request.selector())?;
            let readonly = self
                .manager
                .resolve_existing_readonly(selector, slice_request.selector().expected_uuid())
                .map_err(map_runtime_error)?;
            let (wait, expected_session_id) = match slice_request.command() {
                SliceCommandV1::SessionStatus(input) => (
                    RequestReadWaitV1::from_query_wait(
                        &input.wait,
                        request.options().wait_timeout_ms(),
                    ),
                    input.preconditions.expected_session_id.as_ref(),
                ),
                SliceCommandV1::SessionNext(input) => (
                    RequestReadWaitV1::from_query_wait(
                        &input.wait,
                        request.options().wait_timeout_ms(),
                    ),
                    input.preconditions.expected_session_id.as_ref(),
                ),
                _ => unreachable!("non-read requests returned above"),
            };
            let view = if let Some(scheduler) = readonly.active_scheduler().cloned() {
                let context = scheduler.context_snapshot();
                AuthoritativeReadServiceV1::new(
                    context.store(),
                    SchedulerReadNotificationV1 {
                        scheduler,
                        clock: Arc::clone(&self.clock),
                    },
                    Arc::clone(&self.clock),
                )
                .graph_workspace_view_v2(
                    context.binding().identity(),
                    self.read_wait_v2(wait.clone())?,
                    expected_session_id,
                )
                .map_err(map_read_error)?
            } else {
                let durable_wait = self.read_wait_v2(wait)?;
                loop {
                    let inspected_job = match &durable_wait {
                        ReadWaitV1::AfterJobUntil { job_id, .. } => Some(job_id),
                        ReadWaitV1::Immediate | ReadWaitV1::IdleUntil(_) => None,
                    };
                    let (view, job_state) =
                        SqliteStoreV1::inspect_graph_workspace_view_and_job_state_v2(
                            readonly.database_path(),
                            readonly.binding().identity(),
                            readonly.store_options(),
                            inspected_job,
                            EpochMillisV1::new(self.clock.now().get()),
                        )
                        .map_err(map_store_error)?;
                    if view.graph_state().is_none() {
                        return Ok(None);
                    }
                    validate_graph_view_session_v2(&view, expected_session_id)?;
                    let deadline = match &durable_wait {
                        ReadWaitV1::Immediate => break view,
                        ReadWaitV1::IdleUntil(deadline)
                            if view.queued_job_count() == 0 && view.running_job_id().is_none() =>
                        {
                            break view;
                        }
                        ReadWaitV1::AfterJobUntil { deadline, .. } => {
                            let Some(job_state) = job_state else {
                                return Err(DispatchFailureV1::new(
                                    DispatchFailureKindV1::JobNotFound,
                                ));
                            };
                            if matches!(
                                job_state,
                                StoreJobStateV1::Succeeded
                                    | StoreJobStateV1::Failed
                                    | StoreJobStateV1::Cancelled
                            ) {
                                break view;
                            }
                            *deadline
                        }
                        ReadWaitV1::IdleUntil(deadline) => *deadline,
                    };
                    let now = self.clock.now_millis();
                    if now >= deadline.millis() {
                        return Err(DispatchFailureV1::new(
                            DispatchFailureKindV1::JobWaitTimeout,
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(
                        deadline.millis().saturating_sub(now).min(50),
                    ));
                }
            };
            if view.graph_state().is_none() {
                return Ok(None);
            }
            let result = match slice_request.command() {
                SliceCommandV1::SessionStatus(input) => {
                    let tier = if input.compact {
                        GraphStatusTierV2::Compact
                    } else if input.verbose {
                        GraphStatusTierV2::Verbose
                    } else {
                        GraphStatusTierV2::Standard
                    };
                    project_graph_status_v2(
                        &view,
                        tier,
                        input.history_before.map(TraceSequenceV2::new),
                    )
                }
                SliceCommandV1::SessionNext(_) => project_graph_next_v2(&view),
                _ => unreachable!("non-read requests returned above"),
            }
            .map_err(map_graph_view_error_v2)?;
            let workspace = WorkspaceOutputV1::new(
                view.identity().workspace_uuid().clone(),
                readonly
                    .worktree()
                    .roots()
                    .worktree_root()
                    .display()
                    .as_str(),
                view.latest_workspace_sequence(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
            return OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at: self.clock.generated_at(),
                workspace: Some(workspace),
                job: None,
                session: None,
                result,
                warnings: Vec::new(),
            })
            .map(ResponseEnvelopeV2::OutputV2)
            .map(Some)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
        }
        if matches!(
            slice_request.command(),
            SliceCommandV1::SessionComplete(_)
                | SliceCommandV1::SessionSkip(_)
                | SliceCommandV1::SessionRetry(_)
                | SliceCommandV1::SessionBlock(_)
                | SliceCommandV1::SessionUnblock(_)
                | SliceCommandV1::SessionCancel(_)
                | SliceCommandV1::SessionReset(_)
                | SliceCommandV1::ItemCheck(_)
                | SliceCommandV1::ItemUncheck(_)
                | SliceCommandV1::ItemSet(_)
                | SliceCommandV1::ItemAdd(_)
                | SliceCommandV1::ItemRemove(_)
                | SliceCommandV1::ItemAttach(_)
                | SliceCommandV1::ItemClear(_)
        ) {
            let runtime = ProductionWorkspaceRuntimeV1::new(
                Arc::clone(&self.manager),
                Arc::clone(&self.clock),
            );
            let workspace = runtime.resolve_existing(slice_request.selector())?;
            let idempotency_key = StoreIdempotencyKeyV1::new(
                request
                    .idempotency_key()
                    .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?
                    .as_str(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
            let response_context = podway_store::PersistedResponseContextV1::new(
                request.request_id().as_str(),
                request.command().as_str(),
                workspace.output.uuid().clone(),
                workspace.output.root(),
                workspace.output.latest_workspace_sequence(),
            )
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?
            .with_frozen_public_terminal_envelope();
            let completion_mode = if request.options().detach() {
                WorkerCompletionModeV1::Detached
            } else {
                WorkerCompletionModeV1::WaitUntil(
                    MonotonicDeadlineV1::after(
                        self.clock.as_ref(),
                        request.options().wait_timeout_ms(),
                    )
                    .map_err(map_read_error)?,
                )
            };
            let Some(submission) = self
                .worker
                .submit_procedure_v2_mutation_with_response_context(
                    &workspace.scheduler,
                    slice_request,
                    idempotency_key,
                    response_context,
                    completion_mode,
                )
                .map_err(|error| map_mutation_worker_error(slice_request, error))?
            else {
                return Ok(None);
            };
            let terminal =
                terminal_replay(submission.admission()).or_else(|| match submission.completion() {
                    Some(WorkerWaitResultV1::Terminal(receipt)) => Some(receipt.as_ref()),
                    Some(WorkerWaitResultV1::TimedOut(_)) | None => None,
                });
            if let Some(receipt) = terminal {
                return terminal_direct_response_v2(
                    receipt,
                    terminal_command_kind(slice_request.command()),
                    request.request_id(),
                )
                .map(Some);
            }
            if let Some(WorkerWaitResultV1::TimedOut(view)) = submission.completion() {
                let job = job_output(view)?;
                return Err(
                    DispatchFailureV1::new(DispatchFailureKindV1::JobWaitTimeout).with_job(&job),
                );
            }
            let job = job_output_only_from_context(&workspace.scheduler, submission.admission())?;
            let result = json!({
                "schema": "podway.detached-admission-result/v2",
                "detached": true,
                "admission": {
                    "admitted": true,
                    "job_id": job.id(),
                    "workspace_sequence": job.sequence(),
                },
            });
            return OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at: self.clock.generated_at(),
                workspace: Some(workspace.output),
                job: Some(job),
                session: None,
                result: result
                    .as_object()
                    .expect("Procedure v2 detached result is an object")
                    .clone(),
                warnings: Vec::new(),
            })
            .map(ResponseEnvelopeV2::OutputV2)
            .map(Some)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
        }
        if !matches!(
            slice_request.command(),
            SliceCommandV1::SessionStart(_) | SliceCommandV1::SessionStartReplace(_)
        ) {
            return Ok(None);
        }
        let start = match slice_request.command() {
            SliceCommandV1::SessionStart(start) => start,
            SliceCommandV1::SessionStartReplace(replace) => &replace.start,
            _ => unreachable!("non-start requests returned above"),
        };
        if start.dry_run {
            let selector = selector_from_wire(slice_request.selector())?;
            let readonly = self
                .manager
                .resolve_existing_readonly(selector, slice_request.selector().expected_uuid())
                .map_err(map_runtime_error)?;
            let provider = NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(
                readonly.store_options().clone(),
            ));
            let ids = NativeExecutionIdSourceV1;
            let now = self.clock.now();
            let session_id = ids.next_session_id();
            let attempt_id = ids.next_attempt_id();
            let snapshot_id = ids.next_procedure_snapshot_id();
            let state = match &start.source {
                SessionStartSourceV1::Procedure { procedure } => {
                    match prepare_custom_procedure_v2_start(
                        &provider,
                        readonly.binding(),
                        procedure,
                        start.expected_procedure_digest.as_ref(),
                        &start.task_title,
                        session_id,
                        attempt_id,
                        snapshot_id,
                        now,
                    ) {
                        Ok(state) => Ok(Some(state)),
                        Err(ProcedureV2StartPreparationErrorV1::Source(
                            ProcedureV2SourceAdmissionErrorV1::NotProcedureV2,
                        )) => Ok(None),
                        Err(error) => Err(error),
                    }
                }
                SessionStartSourceV1::Preset { preset } => prepare_preset_procedure_v2_start(
                    &provider,
                    preset,
                    &start.task_title,
                    session_id,
                    attempt_id,
                    snapshot_id,
                    now,
                ),
            }
            .map_err(map_procedure_v2_start_preparation_error)?;
            let Some(state) = state else {
                return Ok(None);
            };
            let expected_current = match slice_request.command() {
                SliceCommandV1::SessionStart(_) => GraphStartCurrentTaskV2::Absent,
                SliceCommandV1::SessionStartReplace(replace) => GraphStartCurrentTaskV2::Exact {
                    session_id: replace.preconditions.expected_session_id.clone(),
                    session_revision: replace.preconditions.expected_session_revision,
                },
                _ => unreachable!("non-start requests returned above"),
            };
            let actual_current = SqliteStoreV1::inspect_graph_start_current_task_v2(
                readonly.database_path(),
                readonly.binding().identity(),
                readonly.store_options(),
                podway_store::EpochMillisV1::new(now.get()),
            )
            .map_err(map_store_error)?;
            validate_graph_start_dry_run_fence_v2(&expected_current, &actual_current)?;
            let result = json!({
                "schema": "podway.session-start-result/v2",
                "procedure_schema": "podway.procedure/v2",
                "procedure_digest": state.snapshot().digest(),
                "dry_run": true,
                "goal_tracking": state.snapshot().goal_tracking(),
                "goal_defined": false,
            });
            return OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at: self.clock.generated_at(),
                workspace: None,
                job: None,
                session: None,
                result: result
                    .as_object()
                    .expect("Procedure v2 dry-run result is an object")
                    .clone(),
                warnings: Vec::new(),
            })
            .map(ResponseEnvelopeV2::OutputV2)
            .map(Some)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal));
        }
        let runtime =
            ProductionWorkspaceRuntimeV1::new(Arc::clone(&self.manager), Arc::clone(&self.clock));
        let workspace = runtime.resolve_existing(slice_request.selector())?;
        let idempotency_key = StoreIdempotencyKeyV1::new(
            request
                .idempotency_key()
                .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?
                .as_str(),
        )
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let response_context = podway_store::PersistedResponseContextV1::new(
            request.request_id().as_str(),
            request.command().as_str(),
            workspace.output.uuid().clone(),
            workspace.output.root(),
            workspace.output.latest_workspace_sequence(),
        )
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?
        .with_frozen_public_terminal_envelope();
        let completion_mode = if request.options().detach() {
            WorkerCompletionModeV1::Detached
        } else {
            WorkerCompletionModeV1::WaitUntil(
                MonotonicDeadlineV1::after(
                    self.clock.as_ref(),
                    request.options().wait_timeout_ms(),
                )
                .map_err(map_read_error)?,
            )
        };
        let Some(submission) = self
            .worker
            .submit_procedure_v2_start_with_response_context(
                &workspace.scheduler,
                slice_request,
                idempotency_key,
                response_context,
                completion_mode,
            )
            .map_err(map_worker_error)?
        else {
            return Ok(None);
        };
        let terminal =
            terminal_replay(submission.admission()).or_else(|| match submission.completion() {
                Some(WorkerWaitResultV1::Terminal(receipt)) => Some(receipt.as_ref()),
                Some(WorkerWaitResultV1::TimedOut(_)) | None => None,
            });
        if let Some(receipt) = terminal {
            return terminal_direct_response_v2(
                receipt,
                TerminalCommandKindV1::Start,
                request.request_id(),
            )
            .map(Some);
        }
        if let Some(WorkerWaitResultV1::TimedOut(view)) = submission.completion() {
            let job = job_output(view)?;
            return Err(
                DispatchFailureV1::new(DispatchFailureKindV1::JobWaitTimeout).with_job(&job),
            );
        }
        let (job, _) = job_output_from_context(&workspace.scheduler, submission.admission())?;
        let view = workspace
            .scheduler
            .context_snapshot()
            .read_job(job.id())
            .map_err(map_store_error)?
            .ok_or_else(terminal_replay_integrity_failure)?;
        let projection =
            admitted_procedure_v2_start_projection_v1(view.execution().canonical_execution())
                .map_err(|_| terminal_replay_integrity_failure())?;
        let result = json!({
            "schema": "podway.detached-admission-result/v2",
            "detached": true,
            "procedure_digest": projection.procedure_digest,
            "admission": {
                "admitted": true,
                "job_id": job.id(),
                "workspace_sequence": job.sequence(),
            },
        });
        OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
            request_id: request.request_id().clone(),
            command: request.command().clone(),
            generated_at: self.clock.generated_at(),
            workspace: Some(workspace.output),
            job: Some(job),
            session: None,
            result: result
                .as_object()
                .expect("Procedure v2 start result is an object")
                .clone(),
            warnings: Vec::new(),
        })
        .map(ResponseEnvelopeV2::OutputV2)
        .map(Some)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
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
        if let Some(marker) = existing_marker {
            let digest = reset_all_digest(request, &source_identity)?;
            transaction
                .validate_resume_request(&store_idempotency_key, &digest)
                .map_err(map_runtime_error)?;
            return self.resume_reset_marker(
                &transaction,
                &marker,
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
    compose_dispatcher_with_worker_observability_and_development_v2_v1(
        manager,
        worker_id,
        observability,
        DevelopmentV2AdmissionGateV1::closed(),
    )
}

pub(crate) fn compose_dispatcher_with_worker_observability_and_development_v2_v1(
    manager: Arc<WorkspaceRuntimeManagerV1>,
    worker_id: WorkerIdV1,
    observability: Option<ObservabilityEmitterV1>,
    development_v2_admission: DevelopmentV2AdmissionGateV1,
) -> ProductionDispatcherCompositionV1 {
    let clock = Arc::new(NativeProductionClockV1::default());
    let worker = ProductionMutationWorkerV1::new_with_observability(
        worker_id,
        Arc::clone(&clock),
        Arc::clone(&manager),
        observability,
    );
    let dispatcher = RequestDispatcherV1Adapter::new(
        ProductionWorkspaceRuntimeV1::new(Arc::clone(&manager), Arc::clone(&clock))
            .with_development_v2_admission(development_v2_admission),
        ProductionReadServiceV1::new(manager, Arc::clone(&clock)),
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
            SessionCommandV1::Start(preview_start(
                input,
                workspace,
                now,
                provider,
                "session.start",
            )?),
            Revision::ZERO,
        )),
        SliceCommandV1::SessionStartReplace(input) => {
            let _prior = prior
                .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::SessionNotFound))?;
            Ok((
                SessionCommandV1::StartReplace(StartReplaceSessionV1 {
                    expected_session_id: input.preconditions.expected_session_id.clone(),
                    confirmed: true,
                    start: preview_start(
                        &input.start,
                        workspace,
                        now,
                        provider,
                        "session.start_replace",
                    )?,
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
    capability: &'static str,
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
    .map_err(|error| map_preview_procedure_error(error, capability))?;
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
    capability: &'static str,
) -> DispatchFailureV1 {
    match error {
        crate::execution::ExecutionBoundaryErrorV1::Domain(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::ProcedureInvalid)
        }
        crate::execution::ExecutionBoundaryErrorV1::ProcedureV2Unsupported => {
            unsupported_v2_capability_failure(capability)
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

fn unsupported_v2_capability_failure(capability: &'static str) -> DispatchFailureV1 {
    DispatchFailureV1::new(DispatchFailureKindV1::UnsupportedV2Capability).with_details(
        DispatchErrorDetailsV1::default()
            .with_unsupported_v2_capability(capability, "podway.session-start-result/v2"),
    )
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
        DomainError::BlockerLimitReached {
            maximum_open_blockers,
        } => DispatchFailureV1::new(DispatchFailureKindV1::BlockerLimitReached).with_details(
            DispatchErrorDetailsV1::default().with_blocker_limit(maximum_open_blockers),
        ),
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

fn job_output_only_from_context(
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
    scheduler
        .context_snapshot()
        .read_job(job_id)
        .map_err(map_store_error)?
        .as_ref()
        .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::JobNotFound))
        .and_then(job_output)
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
        ExecutionErrorV1::UnsupportedV2Capability {
            capability,
            required_result_schema,
        } => DispatchFailureV1::new(DispatchFailureKindV1::UnsupportedV2Capability).with_details(
            DispatchErrorDetailsV1::default()
                .with_unsupported_v2_capability(capability, required_result_schema),
        ),
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

fn terminal_response_requires_v2_wrapper(response: &Value) -> bool {
    // V2PLT-007 rejects every Procedure v2 mutation before durable admission, so only imported or
    // future terminal receipts can currently require a v2 wrapper. Enabling queued/running v2 jobs
    // must add an immutable request-version discriminator to the persisted job identity instead of
    // extending this terminal-response inference.
    response.get("schema").and_then(Value::as_str) == Some("podway.output/v2")
        || response
            .get("details")
            .and_then(|details| details.get("schema"))
            .and_then(Value::as_str)
            == Some("podway.v2-runtime-error-details/v1")
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
        podway_store::CommandV1::SessionDecide => "session.decide",
        podway_store::CommandV1::SessionRework => "session.rework",
        podway_store::CommandV1::GoalDefine => "goal.define",
        podway_store::CommandV1::GoalRevise => "goal.revise",
        podway_store::CommandV1::GoalAssessCriterion => "goal.assess_criterion",
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
    Decision,
    Rework,
    GoalDefinition,
    GoalRevision,
    CriterionAssessment,
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
        "session.decide" => TerminalCommandKindV1::Decision,
        "session.rework" => TerminalCommandKindV1::Rework,
        "goal.define" => TerminalCommandKindV1::GoalDefinition,
        "goal.revise" => TerminalCommandKindV1::GoalRevision,
        "goal.assess_criterion" => TerminalCommandKindV1::CriterionAssessment,
        _ => TerminalCommandKindV1::Other,
    }
}
fn validate_terminal_receipt_projection(
    receipt: &PersistedTerminalReceiptV1,
    command: TerminalCommandKindV1,
) -> Result<(), DispatchFailureV1> {
    if let Some(graph) = receipt.graph_session_projection() {
        return match (graph.operation(), receipt.result()) {
            (
                None
                | Some(PersistedGraphTerminalOperationV2::Decide { .. })
                | Some(PersistedGraphTerminalOperationV2::Rework { .. })
                | Some(PersistedGraphTerminalOperationV2::GoalDefine { .. })
                | Some(PersistedGraphTerminalOperationV2::GoalRevise { .. })
                | Some(PersistedGraphTerminalOperationV2::GoalAssessCriterion { .. })
                | Some(PersistedGraphTerminalOperationV2::Complete { .. })
                | Some(PersistedGraphTerminalOperationV2::Skip { .. })
                | Some(PersistedGraphTerminalOperationV2::Retry { .. })
                | Some(PersistedGraphTerminalOperationV2::Block { .. })
                | Some(PersistedGraphTerminalOperationV2::Unblock { .. })
                | Some(PersistedGraphTerminalOperationV2::Cancel { .. })
                | Some(PersistedGraphTerminalOperationV2::Reset { .. }),
                PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                    ..
                }),
            )
            | (
                Some(PersistedGraphTerminalOperationV2::ItemMutation { .. }),
                PersistedTerminalResultV1::Success(PersistedDomainResultV1::ItemChanged { .. }),
            )
            | (
                Some(PersistedGraphTerminalOperationV2::Failure { .. }),
                PersistedTerminalResultV1::Failure(_),
            ) => Ok(()),
            _ => Err(terminal_replay_integrity_failure()),
        };
    }
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
    if let Some(envelope) = receipt.public_terminal_envelope() {
        validate_frozen_terminal_envelope(receipt, envelope)?;
        return Ok(envelope.clone());
    }
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

fn terminal_direct_response_v2(
    receipt: &PersistedTerminalReceiptV1,
    command: TerminalCommandKindV1,
    request_id: &RequestIdV1,
) -> Result<ResponseEnvelopeV2, DispatchFailureV1> {
    let mut value = terminal_job_response(receipt, command)?;
    value
        .as_object_mut()
        .ok_or_else(terminal_replay_integrity_failure)?
        .insert(
            "request_id".to_owned(),
            Value::String(request_id.as_str().to_owned()),
        );
    let encoded = serde_json::to_vec(&value)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
    decode_response_payload_v2(&encoded).map_err(|_| terminal_replay_integrity_failure())
}

fn validate_frozen_terminal_envelope(
    receipt: &PersistedTerminalReceiptV1,
    envelope: &Value,
) -> Result<(), DispatchFailureV1> {
    let object = envelope
        .as_object()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let context = receipt
        .response_context()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let schema = object.get("schema").and_then(Value::as_str);
    if !matches!(
        schema,
        Some("podway.output/v1" | "podway.output/v2" | "podway.error/v1")
    ) || object.get("request_id").and_then(Value::as_str) != Some(context.request_id())
        || object.get("command").and_then(Value::as_str) != Some(context.command())
    {
        return Err(terminal_replay_integrity_failure());
    }
    let expected_workspace = json!({
        "uuid": context.workspace_uuid(),
        "root": context.workspace_root(),
        "latest_workspace_sequence": context.workspace_sequence(),
    });
    if object.get("workspace") != Some(&expected_workspace) {
        return Err(terminal_replay_integrity_failure());
    }
    let job = job_output_from_terminal_receipt(receipt)?;
    match schema {
        Some("podway.output/v1") => {
            if object.get("job")
                != Some(
                    &serde_json::to_value(job)
                        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?,
                )
            {
                return Err(terminal_replay_integrity_failure());
            }
        }
        Some("podway.output/v2") => {
            validate_frozen_v2_terminal_envelope(receipt, envelope, &job)?;
        }
        Some("podway.error/v1") => {
            validate_frozen_terminal_error(receipt, envelope, &job)?;
        }
        _ => return Err(terminal_replay_integrity_failure()),
    }
    Ok(())
}

fn validate_frozen_v2_terminal_envelope(
    receipt: &PersistedTerminalReceiptV1,
    envelope: &Value,
    expected_job: &JobOutputV1,
) -> Result<(), DispatchFailureV1> {
    let encoded = serde_json::to_vec(envelope).map_err(|_| terminal_replay_integrity_failure())?;
    let ResponseEnvelopeV2::OutputV2(output) =
        decode_response_payload_v2(&encoded).map_err(|_| terminal_replay_integrity_failure())?
    else {
        return Err(terminal_replay_integrity_failure());
    };
    let expected_command = receipt
        .lookup_command()
        .map(PersistedDomainCommandV1::public_command_name)
        .ok_or_else(terminal_replay_integrity_failure)?;
    if output.command().as_str() != expected_command
        || output.job() != Some(expected_job)
        || expected_job.finished_at() != Some(output.generated_at())
        || !matches!(receipt.result(), PersistedTerminalResultV1::Success(_))
    {
        return Err(terminal_replay_integrity_failure());
    }
    let admission = output
        .result()
        .get("admission")
        .and_then(Value::as_object)
        .ok_or_else(terminal_replay_integrity_failure)?;
    if admission.get("admitted") != Some(&Value::Bool(true))
        || admission.get("job_id").and_then(Value::as_str) != Some(expected_job.id().as_str())
        || admission.get("workspace_sequence").and_then(Value::as_u64)
            != Some(expected_job.sequence())
    {
        return Err(terminal_replay_integrity_failure());
    }
    validate_frozen_v2_result_projection(receipt, output.result())?;
    Ok(())
}

fn validate_frozen_terminal_error(
    receipt: &PersistedTerminalReceiptV1,
    envelope: &Value,
    expected_job: &JobOutputV1,
) -> Result<(), DispatchFailureV1> {
    let encoded = serde_json::to_vec(envelope).map_err(|_| terminal_replay_integrity_failure())?;
    let ResponseEnvelopeV2::Error(error) =
        decode_response_payload_v2(&encoded).map_err(|_| terminal_replay_integrity_failure())?
    else {
        return Err(terminal_replay_integrity_failure());
    };
    let PersistedTerminalResultV1::Failure(persisted_error) = receipt.result() else {
        return Err(terminal_replay_integrity_failure());
    };
    let context =
        terminal_response_context(receipt)?.ok_or_else(terminal_replay_integrity_failure)?;
    let expected_failure = match receipt
        .graph_session_projection()
        .and_then(|graph| graph.operation())
    {
        Some(PersistedGraphTerminalOperationV2::Failure { error }) => {
            map_graph_mutation_failure_v2(error)
        }
        Some(
            PersistedGraphTerminalOperationV2::Decide { .. }
            | PersistedGraphTerminalOperationV2::Rework { .. }
            | PersistedGraphTerminalOperationV2::GoalDefine { .. }
            | PersistedGraphTerminalOperationV2::GoalRevise { .. }
            | PersistedGraphTerminalOperationV2::GoalAssessCriterion { .. }
            | PersistedGraphTerminalOperationV2::Complete { .. }
            | PersistedGraphTerminalOperationV2::Skip { .. }
            | PersistedGraphTerminalOperationV2::Retry { .. }
            | PersistedGraphTerminalOperationV2::Block { .. }
            | PersistedGraphTerminalOperationV2::Unblock { .. }
            | PersistedGraphTerminalOperationV2::Cancel { .. }
            | PersistedGraphTerminalOperationV2::Reset { .. }
            | PersistedGraphTerminalOperationV2::ItemMutation { .. },
        ) => return Err(terminal_replay_integrity_failure()),
        None => {
            let command = receipt
                .lookup_command()
                .ok_or_else(terminal_replay_integrity_failure)?;
            map_terminal_domain_error(persisted_error, terminal_command_kind_from_lookup(command))
        }
    };
    let expected = terminal_response_envelope_v1(
        context,
        expected_job.clone(),
        DispatcherTerminalResultV1::Error(expected_failure),
        false,
    )?;
    let ResponseEnvelopeV1::Error(expected) = expected else {
        return Err(terminal_replay_integrity_failure());
    };
    let mut actual =
        serde_json::to_value(error).map_err(|_| terminal_replay_integrity_failure())?;
    let mut expected =
        serde_json::to_value(expected).map_err(|_| terminal_replay_integrity_failure())?;
    actual
        .as_object_mut()
        .and_then(|value| value.remove("message"));
    expected
        .as_object_mut()
        .and_then(|value| value.remove("message"));
    if actual != expected {
        return Err(terminal_replay_integrity_failure());
    }
    Ok(())
}

fn validate_frozen_v2_result_projection(
    receipt: &PersistedTerminalReceiptV1,
    result: &Map<String, Value>,
) -> Result<(), DispatchFailureV1> {
    let schema = result.get("schema").and_then(Value::as_str);
    let session = receipt.session_projection();
    let graph_session = receipt.graph_session_projection();
    let matches_projection = match (schema, receipt.result()) {
        (
            Some("podway.session-start-result/v2"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                session_id,
                revision_after,
                ..
            }),
        ) => {
            result.get("session_id").and_then(Value::as_str) == Some(session_id.as_str())
                && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
                && (session.is_some_and(|projection| {
                    projection.session_id() == session_id
                        && result.get("procedure_digest").and_then(Value::as_str)
                            == projection.procedure_digest().map(|digest| digest.as_str())
                }) || graph_session.is_some_and(|projection| {
                    projection.session_id() == session_id
                        && result.get("procedure_digest").and_then(Value::as_str)
                            == Some(projection.procedure_digest().as_str())
                        && result.get("entry_graph_node_id").and_then(Value::as_str)
                            == Some(projection.entry_graph_node_id().as_str())
                        && result.get("goal_tracking").and_then(Value::as_bool)
                            == Some(projection.goal_tracking())
                        && result.get("goal_defined").and_then(Value::as_bool)
                            == Some(projection.goal_defined())
                }))
        }
        (
            Some("podway.decision-result/v1"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                revision_after,
                ..
            }),
        ) => graph_session.is_some_and(|projection| {
            let Some(PersistedGraphTerminalOperationV2::Decide {
                record,
                target_attempt_id,
            }) = projection.operation()
            else {
                return false;
            };
            let Some(record_object) = record.as_object() else {
                return false;
            };
            projection.lifecycle() == PersistedSessionLifecycleV1::Running
                && projection.revision_after() == *revision_after
                && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
                && result.get("session_state").and_then(Value::as_str) == Some("running")
                && result.get("record") == Some(record)
                && result.get("graph_node_id") == record_object.get("graph_node_id")
                && result.get("attempt_id") == record_object.get("attempt_id")
                && result.get("attempt_number") == record_object.get("attempt_number")
                && result.get("option_id") == record_object.get("option_id")
                && result.get("effect") == record_object.get("effect")
                && result.get("target_graph_node_id") == record_object.get("target_graph_node_id")
                && result.get("target_attempt_id").and_then(Value::as_str)
                    == Some(target_attempt_id.as_str())
        }),
        (
            Some("podway.rework-result/v1"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                revision_after,
                ..
            }),
        ) => graph_session.is_some_and(|projection| {
            let Some(PersistedGraphTerminalOperationV2::Rework { record }) = projection.operation()
            else {
                return false;
            };
            let Some(record) = record.as_object() else {
                return false;
            };
            projection.lifecycle() == PersistedSessionLifecycleV1::Running
                && projection.revision_after() == *revision_after
                && result.len() == 8
                && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
                && result.get("from_graph_node_id") == record.get("from_graph_node_id")
                && result.get("to_graph_node_id") == record.get("to_graph_node_id")
                && result.get("target_attempt_id") == record.get("target_attempt_id")
                && result.get("reason") == record.get("reason")
                && result.get("reactivated") == record.get("reactivated")
        }),
        (
            Some("podway.stage-transition-result/v2"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                revision_after,
                ..
            }),
        ) => {
            if let Some(projection) = graph_session {
                (|| {
                    let lifecycle = match projection.lifecycle() {
                        PersistedSessionLifecycleV1::Running => "running",
                        PersistedSessionLifecycleV1::Completed => "completed",
                        PersistedSessionLifecycleV1::Cancelled => "cancelled",
                    };
                    let common = result.get("revision").and_then(Value::as_u64)
                        == Some(revision_after.get())
                        && result.get("session_state").and_then(Value::as_str) == Some(lifecycle);
                    match projection.operation() {
                        Some(PersistedGraphTerminalOperationV2::Complete {
                            from_graph_node_id,
                            from_attempt_id,
                            to_graph_node_id,
                            to_attempt_id,
                        }) => {
                            common
                                && result.get("transition").and_then(Value::as_str)
                                    == Some("complete")
                                && result.get("from_graph_node_id").and_then(Value::as_str)
                                    == Some(from_graph_node_id.as_str())
                                && result.get("from_attempt_id").and_then(Value::as_str)
                                    == Some(from_attempt_id.as_str())
                                && result.get("to_graph_node_id").and_then(Value::as_str)
                                    == to_graph_node_id.as_ref().map(|value| value.as_str())
                                && result.get("to_attempt_id").and_then(Value::as_str)
                                    == to_attempt_id.as_ref().map(|value| value.as_str())
                        }
                        Some(PersistedGraphTerminalOperationV2::Skip {
                            from_graph_node_id,
                            from_attempt_id,
                            to_graph_node_id,
                            to_attempt_id,
                            reason,
                        }) => {
                            common
                                && result.get("transition").and_then(Value::as_str) == Some("skip")
                                && result.get("from_graph_node_id").and_then(Value::as_str)
                                    == Some(from_graph_node_id.as_str())
                                && result.get("from_attempt_id").and_then(Value::as_str)
                                    == Some(from_attempt_id.as_str())
                                && result.get("to_graph_node_id").and_then(Value::as_str)
                                    == to_graph_node_id.as_ref().map(|value| value.as_str())
                                && result.get("to_attempt_id").and_then(Value::as_str)
                                    == to_attempt_id.as_ref().map(|value| value.as_str())
                                && result.get("reason").and_then(Value::as_str) == reason.as_deref()
                        }
                        Some(PersistedGraphTerminalOperationV2::Retry {
                            graph_node_id,
                            from_attempt_id,
                            to_attempt_id,
                            reason,
                        }) => {
                            common
                                && lifecycle == "running"
                                && result.get("transition").and_then(Value::as_str) == Some("retry")
                                && result.get("from_graph_node_id").and_then(Value::as_str)
                                    == Some(graph_node_id.as_str())
                                && result.get("to_graph_node_id").and_then(Value::as_str)
                                    == Some(graph_node_id.as_str())
                                && result.get("from_attempt_id").and_then(Value::as_str)
                                    == Some(from_attempt_id.as_str())
                                && result.get("to_attempt_id").and_then(Value::as_str)
                                    == Some(to_attempt_id.as_str())
                                && result.get("reason").and_then(Value::as_str)
                                    == Some(reason.as_str())
                        }
                        Some(PersistedGraphTerminalOperationV2::Block {
                            graph_node_id,
                            attempt_id,
                            blocker_id,
                            reason,
                        }) => {
                            common
                                && lifecycle == "running"
                                && result.get("transition").and_then(Value::as_str) == Some("block")
                                && result.get("from_graph_node_id").and_then(Value::as_str)
                                    == Some(graph_node_id.as_str())
                                && result.get("from_attempt_id").and_then(Value::as_str)
                                    == Some(attempt_id.as_str())
                                && result.get("blocker_id").and_then(Value::as_str)
                                    == Some(blocker_id.as_str())
                                && result.get("reason").and_then(Value::as_str)
                                    == Some(reason.as_str())
                        }
                        Some(PersistedGraphTerminalOperationV2::Unblock {
                            graph_node_id,
                            attempt_id,
                            all,
                            blocker_ids,
                        }) => {
                            common
                                && lifecycle == "running"
                                && result.get("transition").and_then(Value::as_str)
                                    == Some("unblock")
                                && result.get("from_graph_node_id").and_then(Value::as_str)
                                    == Some(graph_node_id.as_str())
                                && result.get("from_attempt_id").and_then(Value::as_str)
                                    == Some(attempt_id.as_str())
                                && if *all {
                                    result.get("all").and_then(Value::as_bool) == Some(true)
                                        && result.get("blocker_id").is_none()
                                } else {
                                    let [blocker_id] = blocker_ids.as_slice() else {
                                        return false;
                                    };
                                    result.get("blocker_id").and_then(Value::as_str)
                                        == Some(blocker_id.as_str())
                                        && result.get("all").is_none()
                                }
                        }
                        Some(PersistedGraphTerminalOperationV2::Cancel {
                            graph_node_id,
                            attempt_id,
                            ..
                        }) => {
                            common
                                && lifecycle == "cancelled"
                                && result.get("transition").and_then(Value::as_str)
                                    == Some("cancel")
                                && result.get("from_graph_node_id").and_then(Value::as_str)
                                    == Some(graph_node_id.as_str())
                                && result.get("from_attempt_id").and_then(Value::as_str)
                                    == Some(attempt_id.as_str())
                                && result.get("reason").is_none()
                        }
                        Some(PersistedGraphTerminalOperationV2::Reset { session_id }) => {
                            graph_session
                                .is_some_and(|projection| projection.session_id() == session_id)
                                && result.get("revision").and_then(Value::as_u64)
                                    == Some(revision_after.get())
                                && result.get("transition").and_then(Value::as_str) == Some("reset")
                                && result.get("reset").and_then(Value::as_bool) == Some(true)
                                && result.get("session_state").is_none()
                        }
                        _ => false,
                    }
                })()
            } else {
                let transition = receipt
                    .lookup_command()
                    .and_then(expected_stage_transition_v2);
                let base_matches = result.get("revision").and_then(Value::as_u64)
                    == Some(revision_after.get())
                    && result.get("transition").and_then(Value::as_str) == transition;
                if transition == Some("reset") {
                    base_matches
                        && session.is_none()
                        && result.get("reset").and_then(Value::as_bool) == Some(true)
                } else {
                    base_matches
                        && session.is_some_and(|projection| {
                            let lifecycle = match projection.lifecycle() {
                                PersistedSessionLifecycleV1::Running => "running",
                                PersistedSessionLifecycleV1::Completed => "completed",
                                PersistedSessionLifecycleV1::Cancelled => "cancelled",
                            };
                            result.get("session_state").and_then(Value::as_str) == Some(lifecycle)
                                && projection.revision_after() == *revision_after
                        })
                }
            }
        }
        (
            Some("podway.item-mutation-result/v2"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::ItemChanged {
                item_id,
                revision_after,
                changed,
                ..
            }),
        ) => {
            if let Some(projection) = graph_session {
                let Some(PersistedGraphTerminalOperationV2::ItemMutation {
                    graph_node_id,
                    attempt_id,
                    attempt_number,
                    item_id: operation_item_id,
                    value_digest,
                }) = projection.operation()
                else {
                    return Err(terminal_replay_integrity_failure());
                };
                result.get("item_id").and_then(Value::as_str) == Some(item_id.as_str())
                    && operation_item_id == item_id
                    && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
                    && result.get("changed").and_then(Value::as_bool) == Some(*changed)
                    && result.get("graph_node_id").and_then(Value::as_str)
                        == Some(graph_node_id.as_str())
                    && result.get("attempt_id").and_then(Value::as_str) == Some(attempt_id.as_str())
                    && result.get("attempt_number").and_then(Value::as_u64) == Some(*attempt_number)
                    && result.get("value_digest").and_then(Value::as_str)
                        == value_digest.as_ref().map(|value| value.as_str())
            } else {
                result.get("item_id").and_then(Value::as_str) == Some(item_id.as_str())
                    && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
                    && result.get("changed").and_then(Value::as_bool) == Some(*changed)
            }
        }
        (
            Some("podway.goal-definition-result/v1"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                revision_after,
                ..
            }),
        ) => graph_session
            .and_then(|projection| projection.operation())
            .is_some_and(|operation| {
                let PersistedGraphTerminalOperationV2::GoalDefine { record } = operation else {
                    return false;
                };
                record.as_object().is_some_and(|record| {
                    record
                        .iter()
                        .all(|(key, value)| result.get(key) == Some(value))
                }) && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
            }),
        (
            Some("podway.criterion-assessment-result/v1"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                revision_after,
                ..
            }),
        ) => graph_session
            .and_then(|projection| projection.operation())
            .is_some_and(|operation| {
                let PersistedGraphTerminalOperationV2::GoalAssessCriterion { record } = operation
                else {
                    return false;
                };
                let Some(record) = record.as_object() else {
                    return false;
                };
                let Some(stored_result) = record.get("result").and_then(Value::as_object) else {
                    return false;
                };
                let Some(public_result) = result.get("result").and_then(Value::as_object) else {
                    return false;
                };
                let citations_match = stored_result
                    .get("citations")
                    .and_then(Value::as_array)
                    .zip(public_result.get("citations").and_then(Value::as_array))
                    .is_some_and(|(stored, public)| {
                        stored.len() == public.len()
                            && stored.iter().zip(public).all(|(stored, public)| {
                                let Some(stored) = stored.as_object() else {
                                    return false;
                                };
                                match stored.get("kind").and_then(Value::as_str) {
                                    Some("evidence") => {
                                        public.get("reference_graph_node_id")
                                            == stored.get("graph_node_id")
                                    }
                                    Some("item") => {
                                        public.get("local_item_id") == stored.get("item_id")
                                    }
                                    _ => false,
                                }
                            })
                    });
                citations_match
                    && result.get("graph_node_id") == record.get("graph_node_id")
                    && result.get("attempt_id") == record.get("attempt_id")
                    && result.get("goal_revision") == record.get("goal_revision")
                    && result.get("mode") == record.get("mode")
                    && result.get("complete") == record.get("complete")
                    && result.get("determined_outcome") == record.get("determined_outcome")
                    && public_result.get("criterion_id") == stored_result.get("criterion_id")
                    && public_result.get("status") == stored_result.get("status")
                    && public_result.get("reason") == stored_result.get("reason")
                    && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
            }),
        (
            Some("podway.goal-revision-result/v1"),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                revision_after,
                ..
            }),
        ) => graph_session
            .and_then(|projection| projection.operation())
            .is_some_and(|operation| {
                let PersistedGraphTerminalOperationV2::GoalRevise { record, .. } = operation else {
                    return false;
                };
                record.as_object().is_some_and(|record| {
                    record
                        .iter()
                        .all(|(key, value)| result.get(key) == Some(value))
                }) && result.get("revision").and_then(Value::as_u64) == Some(revision_after.get())
            }),
        _ => false,
    };
    if matches_projection {
        Ok(())
    } else {
        Err(terminal_replay_integrity_failure())
    }
}

fn expected_stage_transition_v2(command: &PersistedDomainCommandV1) -> Option<&'static str> {
    match command {
        PersistedDomainCommandV1::SessionComplete => Some("complete"),
        PersistedDomainCommandV1::SessionSkip => Some("skip"),
        PersistedDomainCommandV1::SessionRetry => Some("retry"),
        PersistedDomainCommandV1::SessionBlock => Some("block"),
        PersistedDomainCommandV1::SessionUnblock => Some("unblock"),
        PersistedDomainCommandV1::SessionCancel => Some("cancel"),
        PersistedDomainCommandV1::SessionReset => Some("reset"),
        PersistedDomainCommandV1::WorkspaceInitialize
        | PersistedDomainCommandV1::WorkspaceResetAll
        | PersistedDomainCommandV1::SessionStart
        | PersistedDomainCommandV1::SessionStartReplace
        | PersistedDomainCommandV1::SessionReturn
        | PersistedDomainCommandV1::SessionReopen
        | PersistedDomainCommandV1::SessionDecide
        | PersistedDomainCommandV1::SessionRework
        | PersistedDomainCommandV1::GoalDefine
        | PersistedDomainCommandV1::GoalRevise
        | PersistedDomainCommandV1::GoalAssessCriterion
        | PersistedDomainCommandV1::ItemCheck { .. }
        | PersistedDomainCommandV1::ItemUncheck { .. }
        | PersistedDomainCommandV1::ItemSet { .. }
        | PersistedDomainCommandV1::ItemAdd { .. }
        | PersistedDomainCommandV1::ItemRemove { .. }
        | PersistedDomainCommandV1::ItemAttach { .. }
        | PersistedDomainCommandV1::ItemClear { .. } => None,
    }
}

pub(crate) fn seal_terminal_receipt_v1(
    receipt: &PersistedTerminalReceiptV1,
) -> Result<Value, StoreErrorV1> {
    if receipt.graph_session_projection().is_some() {
        return graph_terminal_envelope_v2(receipt).map_err(|_| {
            StoreErrorV1::InternalInvariantViolationV1 {
                invariant: podway_store::StoreInvariantV1::TransitionMutationShape,
            }
        });
    }
    let command = receipt
        .lookup_command()
        .ok_or(StoreErrorV1::InternalInvariantViolationV1 {
            invariant: podway_store::StoreInvariantV1::TransitionMutationShape,
        })?;
    terminal_job_response(receipt, terminal_command_kind_from_lookup(command)).map_err(|_| {
        StoreErrorV1::InternalInvariantViolationV1 {
            invariant: podway_store::StoreInvariantV1::TransitionMutationShape,
        }
    })
}

fn graph_terminal_envelope_v2(
    receipt: &PersistedTerminalReceiptV1,
) -> Result<Value, DispatchFailureV1> {
    let graph = receipt
        .graph_session_projection()
        .ok_or_else(terminal_replay_integrity_failure)?;
    match graph.operation() {
        None => serde_json::to_value(graph_start_terminal_envelope_v2(receipt)?)
            .map_err(|_| terminal_replay_integrity_failure()),
        Some(PersistedGraphTerminalOperationV2::Decide {
            record,
            target_attempt_id,
        }) => {
            if graph.lifecycle() != PersistedSessionLifecycleV1::Running {
                return Err(terminal_replay_integrity_failure());
            }
            let record_object = record
                .as_object()
                .ok_or_else(terminal_replay_integrity_failure)?;
            let result = json!({
                "schema": "podway.decision-result/v1",
                "admission": graph_admission_value_v2(receipt)?,
                "graph_node_id": record_object.get("graph_node_id").ok_or_else(terminal_replay_integrity_failure)?,
                "attempt_id": record_object.get("attempt_id").ok_or_else(terminal_replay_integrity_failure)?,
                "attempt_number": record_object.get("attempt_number").ok_or_else(terminal_replay_integrity_failure)?,
                "option_id": record_object.get("option_id").ok_or_else(terminal_replay_integrity_failure)?,
                "effect": record_object.get("effect").ok_or_else(terminal_replay_integrity_failure)?,
                "target_graph_node_id": record_object.get("target_graph_node_id").ok_or_else(terminal_replay_integrity_failure)?,
                "target_attempt_id": target_attempt_id,
                "revision": graph.revision_after(),
                "session_state": "running",
                "record": record,
            })
            .as_object()
            .expect("graph decision result is an object")
            .clone();
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Rework { record }) => {
            if graph.lifecycle() != PersistedSessionLifecycleV1::Running {
                return Err(terminal_replay_integrity_failure());
            }
            let record = record
                .as_object()
                .ok_or_else(terminal_replay_integrity_failure)?;
            let result = json!({
                "schema": "podway.rework-result/v1",
                "admission": graph_admission_value_v2(receipt)?,
                "from_graph_node_id": record.get("from_graph_node_id").ok_or_else(terminal_replay_integrity_failure)?,
                "to_graph_node_id": record.get("to_graph_node_id").ok_or_else(terminal_replay_integrity_failure)?,
                "target_attempt_id": record.get("target_attempt_id").ok_or_else(terminal_replay_integrity_failure)?,
                "reason": record.get("reason").ok_or_else(terminal_replay_integrity_failure)?,
                "reactivated": record.get("reactivated").ok_or_else(terminal_replay_integrity_failure)?,
                "revision": graph.revision_after(),
            })
            .as_object()
            .expect("graph rework result is an object")
            .clone();
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::GoalDefine { record }) => {
            let mut result = record
                .as_object()
                .cloned()
                .ok_or_else(terminal_replay_integrity_failure)?;
            result.insert(
                "schema".to_owned(),
                json!("podway.goal-definition-result/v1"),
            );
            result.insert("admission".to_owned(), graph_admission_value_v2(receipt)?);
            result.insert("revision".to_owned(), json!(graph.revision_after()));
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::GoalRevise { record, .. }) => {
            let mut result = record
                .as_object()
                .cloned()
                .ok_or_else(terminal_replay_integrity_failure)?;
            result.insert("schema".to_owned(), json!("podway.goal-revision-result/v1"));
            result.insert("admission".to_owned(), graph_admission_value_v2(receipt)?);
            result.insert("revision".to_owned(), json!(graph.revision_after()));
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::GoalAssessCriterion { record }) => {
            let record = record
                .as_object()
                .ok_or_else(terminal_replay_integrity_failure)?;
            let result_record = record
                .get("result")
                .and_then(Value::as_object)
                .ok_or_else(terminal_replay_integrity_failure)?;
            let citations = result_record
                .get("citations")
                .and_then(Value::as_array)
                .ok_or_else(terminal_replay_integrity_failure)?
                .iter()
                .map(|citation| {
                    let citation = citation
                        .as_object()
                        .ok_or_else(terminal_replay_integrity_failure)?;
                    match citation.get("kind").and_then(Value::as_str) {
                        Some("evidence") => Ok(json!({
                            "reference_graph_node_id": citation
                                .get("graph_node_id")
                                .ok_or_else(terminal_replay_integrity_failure)?,
                        })),
                        Some("item") => Ok(json!({
                            "local_item_id": citation
                                .get("item_id")
                                .ok_or_else(terminal_replay_integrity_failure)?,
                        })),
                        _ => Err(terminal_replay_integrity_failure()),
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut public_record = result_record.clone();
            public_record.insert("citations".to_owned(), Value::Array(citations));
            let mut result = json!({
                "schema": "podway.criterion-assessment-result/v1",
                "admission": graph_admission_value_v2(receipt)?,
                "graph_node_id": record.get("graph_node_id").ok_or_else(terminal_replay_integrity_failure)?,
                "attempt_id": record.get("attempt_id").ok_or_else(terminal_replay_integrity_failure)?,
                "goal_revision": record.get("goal_revision").ok_or_else(terminal_replay_integrity_failure)?,
                "mode": record.get("mode").ok_or_else(terminal_replay_integrity_failure)?,
                "result": public_record,
                "complete": record.get("complete").ok_or_else(terminal_replay_integrity_failure)?,
                "revision": graph.revision_after(),
            })
            .as_object()
            .expect("criterion assessment result is an object")
            .clone();
            if let Some(outcome) = record.get("determined_outcome") {
                result.insert("determined_outcome".to_owned(), outcome.clone());
            }
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Complete {
            from_graph_node_id,
            from_attempt_id,
            to_graph_node_id,
            to_attempt_id,
        }) => {
            let session_state = match graph.lifecycle() {
                PersistedSessionLifecycleV1::Running => "running",
                PersistedSessionLifecycleV1::Completed => "completed",
                PersistedSessionLifecycleV1::Cancelled => {
                    return Err(terminal_replay_integrity_failure());
                }
            };
            let mut result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "complete",
                "from_graph_node_id": from_graph_node_id,
                "from_attempt_id": from_attempt_id,
                "revision": graph.revision_after(),
                "session_state": session_state,
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph transition result is an object")
            .clone();
            match (to_graph_node_id, to_attempt_id) {
                (Some(graph_node_id), Some(attempt_id)) if session_state == "running" => {
                    result.insert("to_graph_node_id".to_owned(), json!(graph_node_id));
                    result.insert("to_attempt_id".to_owned(), json!(attempt_id));
                }
                (None, None) if session_state == "completed" => {}
                _ => return Err(terminal_replay_integrity_failure()),
            }
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Skip {
            from_graph_node_id,
            from_attempt_id,
            to_graph_node_id,
            to_attempt_id,
            reason,
        }) => {
            let session_state = match graph.lifecycle() {
                PersistedSessionLifecycleV1::Running => "running",
                PersistedSessionLifecycleV1::Completed => "completed",
                PersistedSessionLifecycleV1::Cancelled => {
                    return Err(terminal_replay_integrity_failure());
                }
            };
            let mut result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "skip",
                "from_graph_node_id": from_graph_node_id,
                "from_attempt_id": from_attempt_id,
                "revision": graph.revision_after(),
                "session_state": session_state,
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph transition result is an object")
            .clone();
            match (to_graph_node_id, to_attempt_id) {
                (Some(graph_node_id), Some(attempt_id)) if session_state == "running" => {
                    result.insert("to_graph_node_id".to_owned(), json!(graph_node_id));
                    result.insert("to_attempt_id".to_owned(), json!(attempt_id));
                }
                (None, None) if session_state == "completed" => {}
                _ => return Err(terminal_replay_integrity_failure()),
            }
            if let Some(reason) = reason {
                result.insert("reason".to_owned(), json!(reason));
            }
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Retry {
            graph_node_id,
            from_attempt_id,
            to_attempt_id,
            reason,
        }) => {
            if graph.lifecycle() != PersistedSessionLifecycleV1::Running
                || from_attempt_id == to_attempt_id
            {
                return Err(terminal_replay_integrity_failure());
            }
            let result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "retry",
                "from_graph_node_id": graph_node_id,
                "from_attempt_id": from_attempt_id,
                "to_graph_node_id": graph_node_id,
                "to_attempt_id": to_attempt_id,
                "reason": reason,
                "revision": graph.revision_after(),
                "session_state": "running",
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph retry result is an object")
            .clone();
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Block {
            graph_node_id,
            attempt_id,
            blocker_id,
            reason,
        }) => {
            if graph.lifecycle() != PersistedSessionLifecycleV1::Running {
                return Err(terminal_replay_integrity_failure());
            }
            let result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "block",
                "from_graph_node_id": graph_node_id,
                "from_attempt_id": attempt_id,
                "blocker_id": blocker_id,
                "reason": reason,
                "revision": graph.revision_after(),
                "session_state": "running",
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph block result is an object")
            .clone();
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Unblock {
            graph_node_id,
            attempt_id,
            all,
            blocker_ids,
        }) => {
            if graph.lifecycle() != PersistedSessionLifecycleV1::Running {
                return Err(terminal_replay_integrity_failure());
            }
            let mut result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "unblock",
                "from_graph_node_id": graph_node_id,
                "from_attempt_id": attempt_id,
                "revision": graph.revision_after(),
                "session_state": "running",
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph unblock result is an object")
            .clone();
            if *all {
                result.insert("all".to_owned(), Value::Bool(true));
            } else {
                let [blocker_id] = blocker_ids.as_slice() else {
                    return Err(terminal_replay_integrity_failure());
                };
                result.insert("blocker_id".to_owned(), json!(blocker_id));
            }
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Cancel {
            graph_node_id,
            attempt_id,
            ..
        }) => {
            if graph.lifecycle() != PersistedSessionLifecycleV1::Cancelled {
                return Err(terminal_replay_integrity_failure());
            }
            let result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "cancel",
                "from_graph_node_id": graph_node_id,
                "from_attempt_id": attempt_id,
                "revision": graph.revision_after(),
                "session_state": "cancelled",
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph cancel result is an object")
            .clone();
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Reset { .. }) => {
            let result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "reset",
                "reset": true,
                "revision": graph.revision_after(),
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph reset result is an object")
            .clone();
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::ItemMutation {
            graph_node_id,
            attempt_id,
            attempt_number,
            item_id,
            value_digest,
        }) => {
            let PersistedTerminalResultV1::Success(PersistedDomainResultV1::ItemChanged {
                changed,
                ..
            }) = receipt.result()
            else {
                return Err(terminal_replay_integrity_failure());
            };
            let mut result = json!({
                "schema": "podway.item-mutation-result/v2",
                "changed": changed,
                "graph_node_id": graph_node_id,
                "attempt_id": attempt_id,
                "attempt_number": attempt_number,
                "item_id": item_id,
                "revision": graph.revision_after(),
                "admission": graph_admission_value_v2(receipt)?,
            })
            .as_object()
            .expect("graph item result is an object")
            .clone();
            if let Some(value_digest) = value_digest {
                result.insert("value_digest".to_owned(), json!(value_digest));
            }
            graph_success_terminal_envelope_v2(receipt, result)
        }
        Some(PersistedGraphTerminalOperationV2::Failure { error }) => {
            let context = terminal_response_context(receipt)?
                .ok_or_else(terminal_replay_integrity_failure)?;
            serde_json::to_value(terminal_response_envelope_v1(
                context,
                job_output_from_terminal_receipt(receipt)?,
                DispatcherTerminalResultV1::Error(map_graph_mutation_failure_v2(error)),
                false,
            )?)
            .map_err(|_| terminal_replay_integrity_failure())
        }
    }
}

fn graph_admission_value_v2(
    receipt: &PersistedTerminalReceiptV1,
) -> Result<Value, DispatchFailureV1> {
    let job = job_output_from_terminal_receipt(receipt)?;
    Ok(json!({
        "admitted": true,
        "job_id": job.id(),
        "workspace_sequence": job.sequence(),
    }))
}

fn graph_success_terminal_envelope_v2(
    receipt: &PersistedTerminalReceiptV1,
    result: Map<String, Value>,
) -> Result<Value, DispatchFailureV1> {
    let context = receipt
        .response_context()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let projection = receipt
        .job_projection()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let output = OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
        request_id: RequestIdV1::new(context.request_id())
            .map_err(|_| terminal_replay_integrity_failure())?,
        command: CommandNameV1::new(context.command())
            .map_err(|_| terminal_replay_integrity_failure())?,
        generated_at: rfc3339_millis(projection.finished_at())?,
        workspace: Some(
            WorkspaceOutputV1::new(
                context.workspace_uuid().clone(),
                context.workspace_root(),
                context.workspace_sequence(),
            )
            .map_err(|_| terminal_replay_integrity_failure())?,
        ),
        job: Some(job_output_from_terminal_receipt(receipt)?),
        session: None,
        result,
        warnings: Vec::new(),
    })
    .map_err(|_| terminal_replay_integrity_failure())?;
    serde_json::to_value(output).map_err(|_| terminal_replay_integrity_failure())
}

fn graph_start_terminal_envelope_v2(
    receipt: &PersistedTerminalReceiptV1,
) -> Result<OutputEnvelopeV2, DispatchFailureV1> {
    let graph = receipt
        .graph_session_projection()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let context = receipt
        .response_context()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let projection = receipt
        .job_projection()
        .ok_or_else(terminal_replay_integrity_failure)?;
    let job = job_output_from_terminal_receipt(receipt)?;
    let result = json!({
        "schema": "podway.session-start-result/v2",
        "procedure_schema": "podway.procedure/v2",
        "procedure_digest": graph.procedure_digest(),
        "dry_run": false,
        "goal_tracking": graph.goal_tracking(),
        "goal_defined": graph.goal_defined(),
        "admission": {
            "admitted": true,
            "job_id": job.id(),
            "workspace_sequence": job.sequence(),
        },
        "session_id": graph.session_id(),
        "revision": graph.revision_after(),
        "entry_graph_node_id": graph.entry_graph_node_id(),
    });
    OutputEnvelopeV2::new(podway_protocol::OutputEnvelopeInputV2 {
        request_id: RequestIdV1::new(context.request_id())
            .map_err(|_| terminal_replay_integrity_failure())?,
        command: CommandNameV1::new(context.command())
            .map_err(|_| terminal_replay_integrity_failure())?,
        generated_at: rfc3339_millis(projection.finished_at())?,
        workspace: Some(
            WorkspaceOutputV1::new(
                context.workspace_uuid().clone(),
                context.workspace_root(),
                context.workspace_sequence(),
            )
            .map_err(|_| terminal_replay_integrity_failure())?,
        ),
        job: Some(job),
        session: None,
        result: result
            .as_object()
            .expect("graph start result is an object")
            .clone(),
        warnings: Vec::new(),
    })
    .map_err(|_| terminal_replay_integrity_failure())
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
                | TerminalCommandKindV1::Decision
                | TerminalCommandKindV1::Rework
                | TerminalCommandKindV1::GoalDefinition
                | TerminalCommandKindV1::GoalRevision
                | TerminalCommandKindV1::CriterionAssessment
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
                | TerminalCommandKindV1::Decision
                | TerminalCommandKindV1::Rework
                | TerminalCommandKindV1::GoalDefinition
                | TerminalCommandKindV1::GoalRevision
                | TerminalCommandKindV1::CriterionAssessment
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
        PersistedDomainErrorV1::BlockerLimitReached {
            maximum_open_blockers,
        } => usize::try_from(*maximum_open_blockers).map_or_else(
            |_| DispatchFailureV1::new(DispatchFailureKindV1::Internal),
            |maximum| {
                DispatchFailureV1::new(DispatchFailureKindV1::BlockerLimitReached)
                    .with_details(DispatchErrorDetailsV1::default().with_blocker_limit(maximum))
            },
        ),
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

fn map_graph_mutation_failure_v2(error: &PersistedGraphMutationFailureV2) -> DispatchFailureV1 {
    match error {
        PersistedGraphMutationFailureV2::GoalTrackingNotEnabled => {
            DispatchFailureV1::new(DispatchFailureKindV1::GoalTrackingNotEnabled)
                .with_details(DispatchErrorDetailsV1::default().with_goal_tracking_not_enabled())
        }
        PersistedGraphMutationFailureV2::SessionGoalAlreadyDefined { goal_revision } => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionGoalAlreadyDefined).with_details(
                DispatchErrorDetailsV1::default().with_session_goal_already_defined(*goal_revision),
            )
        }
        PersistedGraphMutationFailureV2::GoalRevisionStale {
            expected_goal_revision,
            actual_goal_revision,
        } => DispatchFailureV1::new(DispatchFailureKindV1::GoalRevisionStale).with_details(
            DispatchErrorDetailsV1::default()
                .with_goal_revision_stale(*expected_goal_revision, *actual_goal_revision),
        ),
        PersistedGraphMutationFailureV2::GoalRevisionTargetNotAllowed {
            target_graph_node_id,
        } => DispatchFailureV1::new(DispatchFailureKindV1::GoalRevisionTargetNotAllowed)
            .with_details(
                DispatchErrorDetailsV1::default()
                    .with_goal_revision_target_not_allowed(target_graph_node_id.clone()),
            ),
        PersistedGraphMutationFailureV2::GoalRevisionTargetNotRevisionSafe {
            target_graph_node_id,
        } => DispatchFailureV1::new(DispatchFailureKindV1::GoalRevisionTargetNotRevisionSafe)
            .with_details(
                DispatchErrorDetailsV1::default()
                    .with_goal_revision_target_not_revision_safe(target_graph_node_id.clone()),
            ),
        PersistedGraphMutationFailureV2::ReactivationFlagRequired => {
            DispatchFailureV1::new(DispatchFailureKindV1::ReactivationFlagRequired)
                .with_details(DispatchErrorDetailsV1::default().with_reactivation_flag_required())
        }
        PersistedGraphMutationFailureV2::SessionNotRunning => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionNotRunning)
        }
        PersistedGraphMutationFailureV2::SessionCancelled => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionCancelled)
        }
        PersistedGraphMutationFailureV2::SessionRevisionConflict { expected, actual } => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionRevisionConflict).with_details(
                DispatchErrorDetailsV1::default()
                    .with_expected_revision(*expected)
                    .with_current_revision(*actual),
            )
        }
        PersistedGraphMutationFailureV2::AttemptNotCurrent { expected, actual } => {
            DispatchFailureV1::new(DispatchFailureKindV1::AttemptNotCurrent).with_details(
                DispatchErrorDetailsV1::default()
                    .with_attempt_mismatch(expected.clone(), actual.clone()),
            )
        }
        PersistedGraphMutationFailureV2::GraphNodeTypeMismatch {
            graph_node_id,
            actual,
        } => DispatchFailureV1::new(DispatchFailureKindV1::GraphNodeTypeMismatch).with_details(
            DispatchErrorDetailsV1::default()
                .with_graph_node_type_mismatch(graph_node_id.clone(), actual),
        ),
        PersistedGraphMutationFailureV2::OptionNotAllowed {
            graph_node_id,
            option_id,
            allowed_option_ids,
        } => DispatchFailureV1::new(DispatchFailureKindV1::OptionNotAllowed).with_details(
            DispatchErrorDetailsV1::default().with_option_not_allowed(
                graph_node_id.clone(),
                option_id.clone(),
                allowed_option_ids.clone(),
            ),
        ),
        PersistedGraphMutationFailureV2::RouteNotAllowed {
            graph_node_id,
            option_id,
        } => DispatchFailureV1::new(DispatchFailureKindV1::RouteNotAllowed).with_details(
            DispatchErrorDetailsV1::default()
                .with_route_not_allowed(graph_node_id.clone(), option_id.clone()),
        ),
        PersistedGraphMutationFailureV2::ManualReworkTargetNotAllowed {
            target_graph_node_id,
        } => DispatchFailureV1::new(DispatchFailureKindV1::ManualReworkTargetNotAllowed)
            .with_details(
                DispatchErrorDetailsV1::default()
                    .with_manual_rework_target_not_allowed(target_graph_node_id.clone()),
            ),
        PersistedGraphMutationFailureV2::ManualReworkTargetNotOnTrace {
            target_graph_node_id,
        } => DispatchFailureV1::new(DispatchFailureKindV1::ManualReworkTargetNotOnTrace)
            .with_details(
                DispatchErrorDetailsV1::default()
                    .with_manual_rework_target_not_on_trace(target_graph_node_id.clone()),
            ),
        PersistedGraphMutationFailureV2::DecisionReasonMissing { graph_node_id } => {
            DispatchFailureV1::new(DispatchFailureKindV1::DecisionReasonMissing).with_details(
                DispatchErrorDetailsV1::default()
                    .with_decision_reason_missing(graph_node_id.clone()),
            )
        }
        PersistedGraphMutationFailureV2::EvidenceReferenceUnresolved {
            graph_node_id,
            source_graph_node_ids,
        } => DispatchFailureV1::new(DispatchFailureKindV1::EvidenceReferenceUnresolved)
            .with_details(
                DispatchErrorDetailsV1::default().with_evidence_reference_unresolved(
                    graph_node_id.clone(),
                    source_graph_node_ids.clone(),
                ),
            ),
        PersistedGraphMutationFailureV2::EvidenceReferenceStale {
            graph_node_id,
            source_graph_node_id,
            expected_source_attempt_id,
            current_source_attempt_id,
        } => DispatchFailureV1::new(DispatchFailureKindV1::EvidenceReferenceStale).with_details(
            DispatchErrorDetailsV1::default().with_evidence_reference_stale(
                graph_node_id.clone(),
                source_graph_node_id.clone(),
                expected_source_attempt_id.clone(),
                current_source_attempt_id.clone(),
            ),
        ),
        PersistedGraphMutationFailureV2::GoalAssessmentDecisionRequiresAssessment { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid)
        }
        PersistedGraphMutationFailureV2::GoalAssessmentDecisionRequired { .. }
        | PersistedGraphMutationFailureV2::CriterionResultAlreadyRecorded { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid)
        }
        PersistedGraphMutationFailureV2::CriterionNotFound { criterion_id } => {
            DispatchFailureV1::new(DispatchFailureKindV1::CriterionNotFound).with_details(
                DispatchErrorDetailsV1::default().with_criterion_not_found(criterion_id.clone()),
            )
        }
        PersistedGraphMutationFailureV2::CriterionModeMixed {
            criterion_id,
            expected_mode,
            actual_status,
        } => DispatchFailureV1::new(DispatchFailureKindV1::CriterionModeMixed).with_details(
            DispatchErrorDetailsV1::default().with_criterion_mode_mixed(
                criterion_id.clone(),
                expected_mode,
                actual_status,
            ),
        ),
        PersistedGraphMutationFailureV2::CriterionCitationInvalid {
            criterion_id,
            citation,
        } => {
            let public_citation = citation.as_object().and_then(|citation| {
                match citation.get("kind").and_then(Value::as_str) {
                    Some("evidence") => citation
                        .get("graph_node_id")
                        .map(|value| json!({"reference_graph_node_id":value})),
                    Some("item") => citation
                        .get("item_id")
                        .map(|value| json!({"local_item_id":value})),
                    _ => None,
                }
            });
            public_citation.map_or_else(
                || DispatchFailureV1::new(DispatchFailureKindV1::Internal),
                |citation| {
                    DispatchFailureV1::new(DispatchFailureKindV1::CriterionCitationInvalid)
                        .with_details(
                            DispatchErrorDetailsV1::default()
                                .with_criterion_citation_invalid(criterion_id.clone(), citation),
                        )
                },
            )
        }
        PersistedGraphMutationFailureV2::SkipNotAllowed { .. }
        | PersistedGraphMutationFailureV2::SkipReasonRequired { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::StageNotSkippable)
        }
        PersistedGraphMutationFailureV2::BlockerIdAlreadyUsed { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::Internal)
        }
        PersistedGraphMutationFailureV2::TooManyOpenBlockers { maximum } => {
            usize::try_from(*maximum).map_or_else(
                |_| DispatchFailureV1::new(DispatchFailureKindV1::Internal),
                |maximum| {
                    DispatchFailureV1::new(DispatchFailureKindV1::BlockerLimitReached)
                        .with_details(DispatchErrorDetailsV1::default().with_blocker_limit(maximum))
                },
            )
        }
        PersistedGraphMutationFailureV2::BlockerNotFound { .. }
        | PersistedGraphMutationFailureV2::NoOpenBlockers => {
            DispatchFailureV1::new(DispatchFailureKindV1::BlockerNotFound)
        }
        PersistedGraphMutationFailureV2::BlockerNotCurrent { .. }
        | PersistedGraphMutationFailureV2::BlockerAlreadyResolved { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::BlockerNotCurrent)
        }
        PersistedGraphMutationFailureV2::ItemNotFound { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::ItemNotFound)
        }
        PersistedGraphMutationFailureV2::ItemRevisionConflict { expected, actual } => {
            DispatchFailureV1::new(DispatchFailureKindV1::ItemRevisionConflict).with_details(
                DispatchErrorDetailsV1::default()
                    .with_expected_revision(*expected)
                    .with_current_revision(*actual),
            )
        }
        PersistedGraphMutationFailureV2::ItemTypeMismatch => {
            DispatchFailureV1::new(DispatchFailureKindV1::ItemTypeMismatch)
        }
        PersistedGraphMutationFailureV2::ItemConstraintFailed => {
            DispatchFailureV1::new(DispatchFailureKindV1::ItemConstraintFailed)
        }
        PersistedGraphMutationFailureV2::ListValueNotFound => {
            DispatchFailureV1::new(DispatchFailureKindV1::ListValueNotFound)
        }
        PersistedGraphMutationFailureV2::ListValueDuplicate => {
            DispatchFailureV1::new(DispatchFailureKindV1::ListValueDuplicate)
        }
        PersistedGraphMutationFailureV2::RequiredItemsMissing { .. } => {
            DispatchFailureV1::new(DispatchFailureKindV1::RequiredItemsMissing)
        }
        PersistedGraphMutationFailureV2::BlockersPresent => {
            DispatchFailureV1::new(DispatchFailureKindV1::BlockersPresent)
        }
        PersistedGraphMutationFailureV2::SessionGoalMissing => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionGoalMissing)
                .with_details(DispatchErrorDetailsV1::default().with_session_goal_missing())
        }
        PersistedGraphMutationFailureV2::FreshGoalAssessmentMissing { goal_revision } => {
            DispatchFailureV1::new(DispatchFailureKindV1::FreshGoalAssessmentMissing).with_details(
                DispatchErrorDetailsV1::default()
                    .with_fresh_goal_assessment_missing(*goal_revision),
            )
        }
        PersistedGraphMutationFailureV2::ArtifactChanged => {
            DispatchFailureV1::new(DispatchFailureKindV1::ArtifactChanged)
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
        WorkerErrorV1::AfterAdmission { admission, source } => {
            let receipt = admission_receipt(&admission);
            map_worker_error(*source)
                .with_admission_identity(receipt.job_id().clone(), receipt.identity_sequence())
        }
        WorkerErrorV1::Store(error) => map_store_error(error),
        WorkerErrorV1::Execution(error) => match error {
            crate::execution::ExecutionErrorV1::Store(error) => map_store_error(error),
            crate::execution::ExecutionErrorV1::UnsupportedV2Capability {
                capability,
                required_result_schema,
            } => DispatchFailureV1::new(DispatchFailureKindV1::UnsupportedV2Capability)
                .with_details(
                    DispatchErrorDetailsV1::default()
                        .with_unsupported_v2_capability(capability, required_result_schema),
                ),
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
        WorkerErrorV1::ProcedureV2Preparation(error) => {
            map_procedure_v2_start_preparation_error(error)
        }
        WorkerErrorV1::JobNotFound(_) => DispatchFailureV1::new(DispatchFailureKindV1::JobNotFound),
        WorkerErrorV1::Progress(_)
        | WorkerErrorV1::BackgroundPanicked
        | WorkerErrorV1::RecoveryRequired
        | WorkerErrorV1::RetirementRejected => {
            DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
        }
    }
}

fn map_procedure_v2_start_preparation_error(
    error: ProcedureV2StartPreparationErrorV1,
) -> DispatchFailureV1 {
    match error {
        ProcedureV2StartPreparationErrorV1::Source(
            ProcedureV2SourceAdmissionErrorV1::SchemaInvalid { diagnostic_codes },
        ) => DispatchFailureV1::new(DispatchFailureKindV1::ProcedureV2SchemaInvalid).with_details(
            DispatchErrorDetailsV1::default().with_procedure_v2_schema_invalid(diagnostic_codes),
        ),
        ProcedureV2StartPreparationErrorV1::DigestConfirmationRequired { procedure_digest } => {
            DispatchFailureV1::new(DispatchFailureKindV1::DigestConfirmationRequired).with_details(
                DispatchErrorDetailsV1::default()
                    .with_digest_confirmation_required(procedure_digest),
            )
        }
        ProcedureV2StartPreparationErrorV1::ProcedureDigestMismatch { expected, actual } => {
            procedure_digest_mismatch_failure(expected, actual)
        }
        ProcedureV2StartPreparationErrorV1::Execution(error) => {
            map_worker_error(WorkerErrorV1::Execution(error))
        }
        ProcedureV2StartPreparationErrorV1::Source(
            ProcedureV2SourceAdmissionErrorV1::Rejected(error),
        ) => match error {
            crate::execution::ExecutionBoundaryErrorV1::Domain(_) => {
                DispatchFailureV1::new(DispatchFailureKindV1::ProcedureInvalid)
            }
            crate::execution::ExecutionBoundaryErrorV1::Transient { .. } => {
                DispatchFailureV1::new(DispatchFailureKindV1::DaemonUnavailable)
            }
            crate::execution::ExecutionBoundaryErrorV1::ProcedureV2Unsupported => {
                DispatchFailureV1::new(DispatchFailureKindV1::UnsupportedV2Capability)
            }
            crate::execution::ExecutionBoundaryErrorV1::WorkspaceIdentityMismatch {
                expected,
                actual,
            } => workspace_uuid_mismatch_failure(expected, actual),
        },
        ProcedureV2StartPreparationErrorV1::Source(
            ProcedureV2SourceAdmissionErrorV1::NotProcedureV2,
        ) => DispatchFailureV1::new(DispatchFailureKindV1::ProcedureInvalid),
        ProcedureV2StartPreparationErrorV1::PinnedPresetDigestMismatch { expected, actual } => {
            procedure_digest_mismatch_failure(expected, actual)
        }
        ProcedureV2StartPreparationErrorV1::GoalMutation(
            podway_store::GraphMutationErrorV2::GoalTrackingNotEnabled,
        ) => DispatchFailureV1::new(DispatchFailureKindV1::GoalTrackingNotEnabled)
            .with_details(DispatchErrorDetailsV1::default().with_goal_tracking_not_enabled()),
        ProcedureV2StartPreparationErrorV1::GoalMutation(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::Internal)
        }
        ProcedureV2StartPreparationErrorV1::Domain(_)
        | ProcedureV2StartPreparationErrorV1::InvalidStoreValue(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::Internal)
        }
    }
}

fn validate_graph_start_dry_run_fence_v2(
    expected: &GraphStartCurrentTaskV2,
    actual: &GraphStartCurrentTaskV2,
) -> Result<(), DispatchFailureV1> {
    if expected == actual {
        return Ok(());
    }
    match (expected, actual) {
        (GraphStartCurrentTaskV2::Absent, GraphStartCurrentTaskV2::Exact { .. }) => Err(
            DispatchFailureV1::new(DispatchFailureKindV1::SessionAlreadyExists),
        ),
        (
            GraphStartCurrentTaskV2::Exact {
                session_id: expected_id,
                session_revision: expected_revision,
            },
            GraphStartCurrentTaskV2::Exact {
                session_id: actual_id,
                session_revision: actual_revision,
            },
        ) if expected_id == actual_id => Err(DispatchFailureV1::new(
            DispatchFailureKindV1::SessionRevisionConflict,
        )
        .with_details(
            DispatchErrorDetailsV1::default()
                .with_expected_revision(*expected_revision)
                .with_current_revision(*actual_revision),
        )),
        (
            GraphStartCurrentTaskV2::Exact {
                session_id: expected_id,
                ..
            },
            GraphStartCurrentTaskV2::Exact {
                session_id: actual_id,
                ..
            },
        ) => Err(session_id_mismatch_failure(
            expected_id.clone(),
            Some(actual_id.clone()),
        )),
        (
            GraphStartCurrentTaskV2::Exact {
                session_id: expected_id,
                ..
            },
            GraphStartCurrentTaskV2::Absent,
        ) => Err(session_id_mismatch_failure(expected_id.clone(), None)),
        (GraphStartCurrentTaskV2::Absent, GraphStartCurrentTaskV2::Absent) => Ok(()),
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

fn map_graph_view_error_v2(error: GraphViewErrorV2) -> DispatchFailureV1 {
    match error {
        GraphViewErrorV2::MissingGraphSession => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionNotFound)
        }
        GraphViewErrorV2::TerminalSessionHasNoNext => {
            DispatchFailureV1::new(DispatchFailureKindV1::SessionNotRunning)
        }
        GraphViewErrorV2::TimestampOutOfRange => {
            DispatchFailureV1::new(DispatchFailureKindV1::Internal)
        }
        GraphViewErrorV2::PendingMutationsForCompact
        | GraphViewErrorV2::InvalidSnapshot
        | GraphViewErrorV2::InconsistentState(_) => {
            DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceStateUnreadable)
        }
    }
}

fn validate_graph_view_session_v2(
    view: &GraphWorkspaceViewV2,
    expected_session_id: Option<&SessionId>,
) -> Result<(), DispatchFailureV1> {
    let actual = view
        .graph_state()
        .map(|state| state.trace().session_id().clone());
    if let Some(expected) = expected_session_id
        && actual.as_ref() != Some(expected)
    {
        return Err(session_id_mismatch_failure(expected.clone(), actual));
    }
    Ok(())
}

fn map_runtime_error(error: WorkspaceRuntimeErrorV1) -> DispatchFailureV1 {
    match error {
        WorkspaceRuntimeErrorV1::ResetAdmissionOutcomeUnknown {
            idempotency_key, ..
        } => DispatchFailureV1::new(DispatchFailureKindV1::MutationOutcomeUnknown).with_details(
            DispatchErrorDetailsV1::default().with_unknown_outcome(
                IdempotencyKeyV1::new(idempotency_key.as_str())
                    .expect("Store idempotency keys satisfy the protocol bound"),
            ),
        ),
        WorkspaceRuntimeErrorV1::ResetAdmitted { marker, source } => {
            map_runtime_error(*source).with_admission_identity(marker.operation_id().clone(), 1)
        }
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
        StoreErrorV1::AdmissionCommittedV1 { receipt, source } => map_store_error(*source)
            .with_admission_identity(receipt.job_id().clone(), receipt.identity_sequence()),
        StoreErrorV1::AdmissionOutcomeUnknownV1 { idempotency_key } => {
            DispatchFailureV1::new(DispatchFailureKindV1::MutationOutcomeUnknown).with_details(
                DispatchErrorDetailsV1::default().with_unknown_outcome(
                    IdempotencyKeyV1::new(idempotency_key.as_str())
                        .expect("Store idempotency keys satisfy the protocol bound"),
                ),
            )
        }
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
        StoreErrorV1::ProcedureV2PreconditionFailedV1 { failure } => {
            map_graph_mutation_failure_v2(&failure)
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

fn admission_receipt(admission: &AdmitOutcomeV1) -> &JobReceiptV1 {
    match admission {
        AdmitOutcomeV1::New(receipt)
        | AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt)) => receipt,
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => receipt.job(),
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
        AttemptId, CompleteSessionV1, GraphNodeId, ItemId, JobId, ProcedureSnapshotId,
        ProcedureSourceLabelV1, ReasonV2, Revision, SessionAggregateV1, SessionCommandV1,
        SessionId, Sha256Digest, WorkspaceId, preview_transition_v1,
    };
    use podway_store::{
        ClaimedExecutionV1, DurableWorktreeIdentityV1, JobReceiptV1, JobViewV1,
        PersistedGraphTerminalSessionProjectionV2, PersistedResponseContextV1,
        PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1,
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

    #[test]
    fn v2run001_pinned_preset_digest_mismatch_preserves_stable_details() {
        let expected = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
        let actual = Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap();
        let failure = map_procedure_v2_start_preparation_error(
            ProcedureV2StartPreparationErrorV1::PinnedPresetDigestMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            },
        );
        assert_eq!(
            failure.kind(),
            DispatchFailureKindV1::ProcedureDigestMismatch
        );
        assert_eq!(
            failure.into_details().into_json(true),
            Map::from_iter([
                (
                    "schema".to_owned(),
                    json!("podway.procedure-digest-mismatch-details/v1"),
                ),
                ("expected_procedure_digest".to_owned(), json!(expected)),
                ("actual_procedure_digest".to_owned(), json!(actual)),
                ("admission".to_owned(), json!({"admitted": false})),
            ])
        );
    }

    #[test]
    fn blocker_limit_domain_errors_preserve_the_public_limit() {
        let command =
            SliceCommandV1::WorkspaceInit(podway_protocol::WorkspaceInitV1 { repair: false });
        let preview = map_preview_domain_error(
            DomainError::BlockerLimitReached {
                maximum_open_blockers: 1_024,
            },
            &command,
        );
        assert_eq!(preview.kind(), DispatchFailureKindV1::BlockerLimitReached);
        assert_eq!(
            preview.into_details().into_json(true),
            Map::from_iter([
                ("maximum_open_blockers".to_owned(), json!(1024)),
                ("admission".to_owned(), json!({"admitted": false})),
            ])
        );

        let terminal = map_terminal_domain_error(
            &PersistedDomainErrorV1::BlockerLimitReached {
                maximum_open_blockers: 1_024,
            },
            TerminalCommandKindV1::Other,
        );
        assert_eq!(terminal.kind(), DispatchFailureKindV1::BlockerLimitReached);
        assert_eq!(
            terminal.into_details().into_json(false),
            Map::from_iter([("maximum_open_blockers".to_owned(), json!(1024))])
        );
    }

    #[test]
    fn v2plt007_source_declared_v2_preserves_capability_and_pre_admission_details() {
        let expected = json!({
            "schema": "podway.v2-runtime-error-details/v1",
            "kind": "UNSUPPORTED_V2_CAPABILITY",
            "capability": "session.start",
            "required_result_schema": "podway.session-start-result/v2",
            "contract_manifest_digest": podway_protocol::build_identity_v1()
                .contract_manifest_digest(),
            "admission": {"admitted": false},
        })
        .as_object()
        .unwrap()
        .clone();

        let preview = map_preview_procedure_error(
            crate::execution::ExecutionBoundaryErrorV1::ProcedureV2Unsupported,
            "session.start",
        );
        assert_eq!(
            preview.kind(),
            DispatchFailureKindV1::UnsupportedV2Capability
        );
        assert_eq!(preview.into_details().into_json(false), expected);

        let admission = map_worker_error(WorkerErrorV1::Execution(
            crate::execution::ExecutionErrorV1::UnsupportedV2Capability {
                capability: "session.start",
                required_result_schema: "podway.session-start-result/v2",
            },
        ));
        assert_eq!(
            admission.kind(),
            DispatchFailureKindV1::UnsupportedV2Capability
        );
        assert_eq!(admission.into_details().into_json(true), expected);
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
    fn recon002_post_commit_store_errors_preserve_admission_identity() {
        let failure = map_store_error(StoreErrorV1::AdmissionCommittedV1 {
            receipt: fixture_job(7),
            source: Box::new(StoreErrorV1::StorageUnavailableV1 {
                reason: podway_store::StoreUnavailableReasonV1::Recovery,
            }),
        });

        assert_eq!(
            failure.into_details().into_json(true)["admission"],
            json!({
                "admitted": true,
                "job_id": "00000000-0000-4000-8000-000000000007",
                "workspace_sequence": 7,
            })
        );

        let unknown = map_store_error(StoreErrorV1::AdmissionOutcomeUnknownV1 {
            idempotency_key: StoreIdempotencyKeyV1::new("unknown-admission").unwrap(),
        });
        assert_eq!(
            unknown.into_details().into_json(true),
            json!({
                "schema": "podway.mutation-outcome-unknown-details/v1",
                "outcome": "unknown",
                "idempotency_key": "unknown-admission",
                "reconcile": {
                    "command": "job.lookup",
                    "idempotency_key": "unknown-admission",
                },
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn reset_marker_failures_preserve_unknown_and_admitted_public_evidence() {
        let key = StoreIdempotencyKeyV1::new("reset-admission-boundary").unwrap();
        let marker = crate::workspace::ResetMarkerV1::new(
            JobId::new("00000000-0000-4000-8000-000000000071").unwrap(),
            key.clone(),
            CanonicalRequestDigestV1::new(format!("sha256:{}", "7".repeat(64))).unwrap(),
            WorkspaceId::new("00000000-0000-4000-8000-000000000072").unwrap(),
            WorkspaceId::new("00000000-0000-4000-8000-000000000073").unwrap(),
            UnixMillis::new(1_700_000_000_123),
        );
        let admitted = map_runtime_error(WorkspaceRuntimeErrorV1::ResetAdmitted {
            marker: Box::new(marker.clone()),
            source: Box::new(WorkspaceRuntimeErrorV1::RuntimeDirectory(
                crate::workspace::ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform,
            )),
        });
        assert_eq!(
            admitted.into_details().into_json(true)["admission"],
            json!({
                "admitted": true,
                "job_id": marker.operation_id().as_str(),
                "workspace_sequence": 1,
            })
        );

        let unknown = map_runtime_error(WorkspaceRuntimeErrorV1::ResetAdmissionOutcomeUnknown {
            idempotency_key: key,
            source: crate::workspace::ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform,
        });
        assert_eq!(
            unknown.kind(),
            DispatchFailureKindV1::MutationOutcomeUnknown
        );
        assert_eq!(
            unknown.into_details().into_json(true)["idempotency_key"],
            "reset-admission-boundary"
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
    fn recon003_terminal_replay_prefers_the_frozen_public_envelope() {
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(19),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::WorkspaceInitialized {
                workspace_id: WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                revision: Revision::ZERO,
            }),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            None,
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::WorkspaceInitialize)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                "00000000-0000-4000-8000-000000000019",
                "workspace.init",
                WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                "/safe/worktree",
                19,
            )
            .unwrap(),
        )
        .unwrap();
        let mut frozen = terminal_job_response(&receipt, TerminalCommandKindV1::Other).unwrap();
        frozen["warnings"] = json!([{"code": "frozen-renderer"}]);
        let receipt = receipt
            .with_public_terminal_envelope(frozen.clone())
            .unwrap();

        assert_eq!(
            terminal_job_response(&receipt, TerminalCommandKindV1::Other).unwrap(),
            frozen
        );
    }

    fn procedure_v2_terminal_fixture(sequence: u64) -> (PersistedTerminalReceiptV1, Value) {
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000080").unwrap();
        let session_projection = PersistedTerminalSessionProjectionV1::new(
            session_id.clone(),
            "Stored immutable terminal session".to_owned(),
            PersistedSessionLifecycleV1::Completed,
            Revision::new(4),
            Revision::new(5),
        )
        .unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(sequence),
            PersistedTerminalResultV1::Success(terminal_session_result(session_id.clone())),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            Some(session_projection),
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionComplete)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                format!("00000000-0000-4000-8000-{sequence:012x}"),
                "session.complete",
                WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                "/safe/worktree",
                sequence,
            )
            .unwrap(),
        )
        .unwrap();
        let job_output = job_output_from_terminal_receipt(&receipt).unwrap();
        let generated_at = job_output.finished_at().unwrap().as_str().to_owned();
        let job = serde_json::to_value(job_output).unwrap();
        let envelope = json!({
            "schema": "podway.output/v2",
            "request_id": format!("00000000-0000-4000-8000-{sequence:012x}"),
            "command": "session.complete",
            "generated_at": generated_at,
            "workspace": {
                "uuid": "00000000-0000-4000-8000-000000000099",
                "root": "/safe/worktree",
                "latest_workspace_sequence": sequence,
            },
            "job": job,
            "result": {
                "schema": "podway.stage-transition-result/v2",
                "admission": {
                    "admitted": true,
                    "job_id": format!("00000000-0000-4000-8000-{sequence:012x}"),
                    "workspace_sequence": sequence,
                },
                "transition": "complete",
                "from_graph_node_id": "work",
                "from_attempt_id": "00000000-0000-4000-8000-000000000081",
                "revision": 5,
                "session_state": "completed",
            },
            "warnings": [],
        });
        (receipt, envelope)
    }

    #[test]
    fn v2plt007_terminal_replay_accepts_a_complete_correlated_v2_envelope() {
        let (receipt, frozen) = procedure_v2_terminal_fixture(80);
        validate_frozen_terminal_envelope(&receipt, &frozen).unwrap();
        let receipt = receipt
            .with_public_terminal_envelope(frozen.clone())
            .unwrap();

        assert_eq!(
            terminal_job_response(&receipt, TerminalCommandKindV1::Other).unwrap(),
            frozen
        );
    }

    #[test]
    fn v2run004_retry_terminal_seals_required_same_node_fields_and_reason() {
        let sequence = 83;
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000080").unwrap();
        let graph_node_id = GraphNodeId::new("work").unwrap();
        let from_attempt_id = AttemptId::new("00000000-0000-4000-8000-000000000081").unwrap();
        let to_attempt_id = AttemptId::new("00000000-0000-4000-8000-000000000082").unwrap();
        let reason = ReasonV2::new("Retry from a clean attempt.").unwrap();
        let operation = PersistedGraphTerminalOperationV2::retry(
            graph_node_id.clone(),
            from_attempt_id.clone(),
            to_attempt_id.clone(),
            reason.clone(),
        )
        .unwrap();
        let graph_projection = PersistedGraphTerminalSessionProjectionV2::new(
            session_id.clone(),
            "Stored immutable retry session".to_owned(),
            PersistedSessionLifecycleV1::Running,
            Revision::new(4),
            Revision::new(5),
            Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            graph_node_id.clone(),
            false,
            false,
        )
        .unwrap()
        .with_operation(operation)
        .unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_graph_projection(
            fixture_job(sequence),
            PersistedTerminalResultV1::Success(terminal_session_result(session_id)),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            graph_projection,
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionRetry)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                format!("00000000-0000-4000-8000-{sequence:012x}"),
                "session.retry",
                WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                "/safe/worktree",
                sequence,
            )
            .unwrap(),
        )
        .unwrap();

        let envelope = seal_terminal_receipt_v1(&receipt).unwrap();
        assert_eq!(envelope["result"]["transition"], "retry");
        assert_eq!(
            envelope["result"]["from_graph_node_id"],
            graph_node_id.as_str()
        );
        assert_eq!(
            envelope["result"]["to_graph_node_id"],
            graph_node_id.as_str()
        );
        assert_eq!(
            envelope["result"]["from_attempt_id"],
            from_attempt_id.as_str()
        );
        assert_eq!(envelope["result"]["to_attempt_id"], to_attempt_id.as_str());
        assert_eq!(envelope["result"]["reason"], reason.as_str());
        validate_frozen_terminal_envelope(&receipt, &envelope).unwrap();
    }

    #[test]
    fn v2run005_skip_terminal_seals_successor_fields_and_optional_reason() {
        let sequence = 84;
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000080").unwrap();
        let from_graph_node_id = GraphNodeId::new("optional-work").unwrap();
        let to_graph_node_id = GraphNodeId::new("review").unwrap();
        let from_attempt_id = AttemptId::new("00000000-0000-4000-8000-000000000081").unwrap();
        let to_attempt_id = AttemptId::new("00000000-0000-4000-8000-000000000082").unwrap();
        let reason = ReasonV2::new("The optional work is unnecessary.").unwrap();
        let operation = PersistedGraphTerminalOperationV2::skip(
            from_graph_node_id.clone(),
            from_attempt_id.clone(),
            Some(to_graph_node_id.clone()),
            Some(to_attempt_id.clone()),
            Some(reason.clone()),
        )
        .unwrap();
        let graph_projection = PersistedGraphTerminalSessionProjectionV2::new(
            session_id.clone(),
            "Stored immutable skip session".to_owned(),
            PersistedSessionLifecycleV1::Running,
            Revision::new(4),
            Revision::new(5),
            Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            GraphNodeId::new("entry").unwrap(),
            false,
            false,
        )
        .unwrap()
        .with_operation(operation)
        .unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_graph_projection(
            fixture_job(sequence),
            PersistedTerminalResultV1::Success(terminal_session_result(session_id)),
            terminal_job_projection(PersistedTerminalJobStateV1::Succeeded),
            graph_projection,
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionSkip)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                format!("00000000-0000-4000-8000-{sequence:012x}"),
                "session.skip",
                WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                "/safe/worktree",
                sequence,
            )
            .unwrap(),
        )
        .unwrap();

        let envelope = seal_terminal_receipt_v1(&receipt).unwrap();
        assert_eq!(envelope["result"]["transition"], "skip");
        assert_eq!(
            envelope["result"]["from_graph_node_id"],
            from_graph_node_id.as_str()
        );
        assert_eq!(
            envelope["result"]["to_graph_node_id"],
            to_graph_node_id.as_str()
        );
        assert_eq!(
            envelope["result"]["from_attempt_id"],
            from_attempt_id.as_str()
        );
        assert_eq!(envelope["result"]["to_attempt_id"], to_attempt_id.as_str());
        assert_eq!(envelope["result"]["reason"], reason.as_str());
        validate_frozen_terminal_envelope(&receipt, &envelope).unwrap();
    }

    #[test]
    fn v2plt007_terminal_replay_rejects_open_or_mismatched_v2_receipts() {
        let (receipt, frozen) = procedure_v2_terminal_fixture(81);
        let invalid = [
            {
                let mut value = frozen.clone();
                value["result"]["unexpected"] = json!(true);
                value
            },
            {
                let mut value = frozen.clone();
                value["result"]["admission"]["job_id"] =
                    json!("00000000-0000-4000-8000-000000000099");
                value
            },
            {
                let mut value = frozen.clone();
                value["job"]["finished_at"] = json!("2026-08-09T00:00:00.000Z");
                value
            },
            {
                let mut value = frozen.clone();
                value["command"] = json!("session.skip");
                value
            },
            {
                let mut value = frozen.clone();
                value["warnings"] = json!([{"code":"incomplete"}]);
                value
            },
            {
                let mut value = frozen.clone();
                value["result"]["revision"] = json!(6);
                value
            },
            {
                let mut value = frozen.clone();
                value["result"]["transition"] = json!("skip");
                value
            },
        ];

        for envelope in invalid {
            let receipt = receipt
                .clone()
                .with_public_terminal_envelope(envelope)
                .unwrap();
            assert_eq!(
                terminal_job_response(&receipt, TerminalCommandKindV1::Other)
                    .unwrap_err()
                    .kind(),
                DispatchFailureKindV1::WorkspaceStateUnreadable
            );
        }
    }

    fn terminal_error_fixture(sequence: u64) -> (PersistedTerminalReceiptV1, Value) {
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(sequence),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::PreconditionFailed {
                expected: Revision::new(4),
                actual: Revision::new(5),
            }),
            terminal_job_projection(PersistedTerminalJobStateV1::Failed),
            None,
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionComplete)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                format!("00000000-0000-4000-8000-{sequence:012x}"),
                "session.complete",
                WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                "/safe/worktree",
                sequence,
            )
            .unwrap(),
        )
        .unwrap();
        let envelope = terminal_job_response(&receipt, TerminalCommandKindV1::Other).unwrap();
        (receipt, envelope)
    }

    #[test]
    fn v2plt007_terminal_error_replay_is_typed_and_failure_bound() {
        let (failure, valid) = terminal_error_fixture(82);
        validate_frozen_terminal_envelope(&failure, &valid).unwrap();
        let mut improved_message = valid.clone();
        improved_message["message"] = json!("The observed session revision changed.");
        validate_frozen_terminal_envelope(&failure, &improved_message).unwrap();

        let (success, _) = procedure_v2_terminal_fixture(82);
        assert!(validate_frozen_terminal_envelope(&success, &valid).is_err());

        let alternate_receipt = PersistedTerminalReceiptV1::new_with_projections(
            fixture_job(82),
            PersistedTerminalResultV1::Failure(PersistedDomainErrorV1::SessionIdentityMismatch {
                expected: SessionId::new("00000000-0000-4000-8000-000000000090").unwrap(),
                actual: Some(SessionId::new("00000000-0000-4000-8000-000000000091").unwrap()),
            }),
            terminal_job_projection(PersistedTerminalJobStateV1::Failed),
            None,
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionComplete)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                "00000000-0000-4000-8000-000000000052",
                "session.complete",
                WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap(),
                "/safe/worktree",
                82,
            )
            .unwrap(),
        )
        .unwrap();
        let alternate =
            terminal_job_response(&alternate_receipt, TerminalCommandKindV1::Other).unwrap();
        assert!(validate_frozen_terminal_envelope(&failure, &alternate).is_err());

        let invalid = [
            {
                let mut value = valid.clone();
                value["details"]["unknown"] = json!(true);
                value
            },
            {
                let mut value = valid.clone();
                value["code"] = json!("UNSUPPORTED_V2_CAPABILITY");
                value
            },
            {
                let mut value = valid.clone();
                value["retryable"] = json!(!value["retryable"].as_bool().unwrap());
                value
            },
            {
                let mut value = valid.clone();
                value["exit_code"] = json!(6);
                value
            },
        ];
        for envelope in invalid {
            assert!(validate_frozen_terminal_envelope(&failure, &envelope).is_err());
        }
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
