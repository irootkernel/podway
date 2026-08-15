//! Production daemon startup, recovery, serving, and deterministic endpoint shutdown.
//!
//! This module deliberately does not own service lifecycle or signal handling. It composes the
//! existing endpoint, workspace runtime manager, production dispatcher, and bounded Unix transport
//! into one daemon process boundary.

use std::{
    error::Error,
    fmt,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use podway_core::WorkspaceId;
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{SqliteStoreOptionsV1, WorkerIdV1};

use crate::{
    development_v2::{DevelopmentV2AdmissionGateV1, ProcedureV2AdmissionGateV1},
    dispatch::DispatchResponseMetadataV1,
    endpoint::{EndpointErrorV1, SingletonEndpointGuardV1, SingletonEndpointV1},
    execution::ExecutionClockV1,
    observability::{EventOperationV1, EventOutcomeV1, EventRecordV1, ObservabilityEmitterV1},
    peer::{NativePeerCredentialSourceV1, PeerUidVerifierV1},
    production::{
        NativeProductionClockV1, ProductionMutationWorkerV1, ProductionRequestDispatcherV1,
        compose_dispatcher_with_worker_observability_and_procedure_v2_v1,
    },
    registry::{RegistryErrorV1, WorkspaceRegistryEntryV1, WorkspaceRegistryV1},
    runtime_workspace::{
        WorkspaceRuntimeErrorV1, WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1,
    },
    server::{
        BoundedAcceptLoopV1, DaemonProcessIdentityV1, ServerAcceptLoopErrorV1,
        ServerTransportTimeoutsV1, ShutdownAdmissionV1, UnixServerTransportV1,
    },
    worker::WorkerErrorV1,
    workspace::WorkspaceResolutionErrorV1,
};

/// Bounded production settings for one daemon process.
#[derive(Clone, Debug)]
pub struct ProductionDaemonRuntimeConfigV1 {
    worker_id: WorkerIdV1,
    maximum_in_flight_connections: NonZeroUsize,
    transport_timeouts: ServerTransportTimeoutsV1,
    process_identity: Option<DaemonProcessIdentityV1>,
    dev_mode: bool,
    managed_dev_workspace_root: Option<PathBuf>,
    procedure_v2_admission: ProcedureV2AdmissionGateV1,
}

impl ProductionDaemonRuntimeConfigV1 {
    pub fn new(
        worker_id: WorkerIdV1,
        maximum_in_flight_connections: NonZeroUsize,
        transport_timeouts: ServerTransportTimeoutsV1,
    ) -> Self {
        Self {
            worker_id,
            maximum_in_flight_connections,
            transport_timeouts,
            process_identity: None,
            dev_mode: false,
            managed_dev_workspace_root: None,
            procedure_v2_admission: ProcedureV2AdmissionGateV1::public(),
        }
    }

    pub fn with_process_identity(mut self, identity: DaemonProcessIdentityV1) -> Self {
        self.process_identity = Some(identity);
        self
    }

    pub fn with_dev_mode(mut self) -> Self {
        self.dev_mode = true;
        self
    }

    pub fn with_managed_dev_workspace_root(mut self, root: &Path) -> Self {
        self.managed_dev_workspace_root = Some(root.to_path_buf());
        self
    }

    /// Evaluates the development-only Procedure v2 process topology. Invalid provenance keeps the
    /// daemon operational for v1 but leaves the future v2 handler seam closed.
    pub fn with_development_v2_admission(
        mut self,
        paths: &ServiceRuntimePathsV1,
        current_executable: &Path,
    ) -> Self {
        self.procedure_v2_admission = ProcedureV2AdmissionGateV1::development(
            DevelopmentV2AdmissionGateV1::from_process(self.dev_mode, paths, current_executable),
        );
        self
    }

    pub fn worker_id(&self) -> &WorkerIdV1 {
        &self.worker_id
    }

    pub const fn maximum_in_flight_connections(&self) -> NonZeroUsize {
        self.maximum_in_flight_connections
    }

    pub const fn transport_timeouts(&self) -> ServerTransportTimeoutsV1 {
        self.transport_timeouts
    }

    pub fn process_identity(&self) -> Option<&DaemonProcessIdentityV1> {
        self.process_identity.as_ref()
    }

    pub const fn dev_mode(&self) -> bool {
        self.dev_mode
    }
}

/// One recovered registry entry that was revalidated through Git and the Store before use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredWorkspaceReportV1 {
    workspace_uuid: WorkspaceId,
    requeued_job_count: u32,
    drained_terminal_job_count: u64,
}

impl RecoveredWorkspaceReportV1 {
    fn new(
        workspace_uuid: WorkspaceId,
        requeued_job_count: u32,
        drained_terminal_job_count: u64,
    ) -> Self {
        Self {
            workspace_uuid,
            requeued_job_count,
            drained_terminal_job_count,
        }
    }

    pub fn workspace_uuid(&self) -> &WorkspaceId {
        &self.workspace_uuid
    }

    pub const fn requeued_job_count(&self) -> u32 {
        self.requeued_job_count
    }

    pub const fn drained_terminal_job_count(&self) -> u64 {
        self.drained_terminal_job_count
    }
}

/// A non-authoritative registry entry that could not safely regain daemon authority at startup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnavailableWorkspaceReportV1 {
    workspace_uuid: WorkspaceId,
    reason: WorkspaceRecoveryUnavailableReasonV1,
}

impl UnavailableWorkspaceReportV1 {
    fn new(workspace_uuid: WorkspaceId, reason: WorkspaceRecoveryUnavailableReasonV1) -> Self {
        Self {
            workspace_uuid,
            reason,
        }
    }

    pub fn workspace_uuid(&self) -> &WorkspaceId {
        &self.workspace_uuid
    }

    pub const fn reason(&self) -> WorkspaceRecoveryUnavailableReasonV1 {
        self.reason
    }
}

/// A sanitized classification for a registry entry that was not recovered.
///
/// Registry roots are only discovery metadata. A failure here never causes another entry's Store
/// state to be copied, rebound, or treated as this entry's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceRecoveryUnavailableReasonV1 {
    WorktreeGone,
    RegisteredRootInvalid,
    WorkspaceNotInitialized,
    WorkspaceIdentityConflict,
    WorkspaceConfigurationInvalid,
    WorkspaceStateUnreadable,
    DaemonUnavailable,
}

impl fmt::Display for WorkspaceRecoveryUnavailableReasonV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorktreeGone => formatter.write_str("worktree is unavailable"),
            Self::RegisteredRootInvalid => {
                formatter.write_str("registered workspace root is invalid")
            }
            Self::WorkspaceNotInitialized => formatter.write_str("workspace state is unavailable"),
            Self::WorkspaceIdentityConflict => {
                formatter.write_str("workspace identity does not match durable state")
            }
            Self::WorkspaceConfigurationInvalid => {
                formatter.write_str("workspace configuration is unavailable")
            }
            Self::WorkspaceStateUnreadable => {
                formatter.write_str("workspace state cannot be recovered")
            }
            Self::DaemonUnavailable => formatter.write_str("workspace recovery is unavailable"),
        }
    }
}

/// The outcome of recovering one metadata-only registry document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionDaemonRecoveryReportV1 {
    workspaces: Vec<WorkspaceRecoveryEntryV1>,
}

impl ProductionDaemonRecoveryReportV1 {
    fn new(workspaces: Vec<WorkspaceRecoveryEntryV1>) -> Self {
        Self { workspaces }
    }

    pub fn workspaces(&self) -> &[WorkspaceRecoveryEntryV1] {
        &self.workspaces
    }

    pub fn recovered_workspace_count(&self) -> usize {
        self.workspaces
            .iter()
            .filter(|entry| matches!(entry, WorkspaceRecoveryEntryV1::Recovered(_)))
            .count()
    }

    pub fn unavailable_workspace_count(&self) -> usize {
        self.workspaces
            .iter()
            .filter(|entry| matches!(entry, WorkspaceRecoveryEntryV1::Unavailable(_)))
            .count()
    }
}

/// A per-workspace startup recovery outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceRecoveryEntryV1 {
    Recovered(RecoveredWorkspaceReportV1),
    Unavailable(UnavailableWorkspaceReportV1),
}

/// Startup errors that prevent a daemon from being bound and ready to serve.
#[derive(Debug)]
pub enum ProductionDaemonStartupErrorV1 {
    EndpointAcquire(EndpointErrorV1),
    RegistryLoad(RegistryErrorV1),
    RegistryLoadAndEndpointCleanup {
        registry: Box<RegistryErrorV1>,
        cleanup: Box<EndpointErrorV1>,
    },
}

impl fmt::Display for ProductionDaemonStartupErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndpointAcquire(_) => formatter.write_str("cannot acquire daemon endpoint"),
            Self::RegistryLoad(_) => formatter.write_str("cannot load workspace registry"),
            Self::RegistryLoadAndEndpointCleanup { .. } => formatter
                .write_str("cannot load workspace registry and cannot clean up daemon endpoint"),
        }
    }
}

impl Error for ProductionDaemonStartupErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EndpointAcquire(source) => Some(source),
            Self::RegistryLoad(source) => Some(source),
            Self::RegistryLoadAndEndpointCleanup { registry, .. } => Some(registry),
        }
    }
}

/// Errors while serving or deterministically releasing an already-bound endpoint.
#[derive(Debug)]
pub enum ProductionDaemonRuntimeErrorV1 {
    AcceptLoop(ServerAcceptLoopErrorV1),
    EndpointShutdown(EndpointErrorV1),
    AcceptLoopAndEndpointShutdown {
        accept_loop: Box<ServerAcceptLoopErrorV1>,
        endpoint_shutdown: Box<EndpointErrorV1>,
    },
}

impl fmt::Display for ProductionDaemonRuntimeErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AcceptLoop(_) => formatter.write_str("daemon accept loop failed"),
            Self::EndpointShutdown(_) => formatter.write_str("daemon endpoint shutdown failed"),
            Self::AcceptLoopAndEndpointShutdown { .. } => {
                formatter.write_str("daemon accept loop and endpoint shutdown both failed")
            }
        }
    }
}

impl Error for ProductionDaemonRuntimeErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::AcceptLoop(source) => Some(source),
            Self::EndpointShutdown(source) => Some(source),
            Self::AcceptLoopAndEndpointShutdown { accept_loop, .. } => Some(accept_loop),
        }
    }
}

/// The successful terminal outcome of [`ProductionDaemonRuntimeV1::run`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionDaemonShutdownReportV1 {
    recovered_workspace_count: usize,
    unavailable_workspace_count: usize,
}

impl ProductionDaemonShutdownReportV1 {
    fn from_recovery(recovery: &ProductionDaemonRecoveryReportV1) -> Self {
        Self {
            recovered_workspace_count: recovery.recovered_workspace_count(),
            unavailable_workspace_count: recovery.unavailable_workspace_count(),
        }
    }

    pub const fn recovered_workspace_count(&self) -> usize {
        self.recovered_workspace_count
    }

    pub const fn unavailable_workspace_count(&self) -> usize {
        self.unavailable_workspace_count
    }
}

/// A cloneable capability that stops new socket admissions and lets admitted handlers drain.
#[derive(Clone, Debug)]
pub struct ProductionDaemonShutdownHandleV1 {
    admission: ShutdownAdmissionV1,
}

impl ProductionDaemonShutdownHandleV1 {
    pub fn request_shutdown(&self) {
        self.admission.request_shutdown();
    }

    pub fn is_accepting(&self) -> bool {
        self.admission.is_accepting()
    }
}

type ProductionTransportV1 =
    UnixServerTransportV1<NativePeerCredentialSourceV1, ProductionRequestDispatcherV1>;
type ProductionAcceptLoopV1 =
    BoundedAcceptLoopV1<NativePeerCredentialSourceV1, ProductionRequestDispatcherV1>;

/// A bound production daemon that owns its endpoint and one durable-identity scheduler manager.
pub struct ProductionDaemonRuntimeV1 {
    endpoint: SingletonEndpointGuardV1,
    manager: Arc<WorkspaceRuntimeManagerV1>,
    accept_loop: ProductionAcceptLoopV1,
    shutdown: ProductionDaemonShutdownHandleV1,
    recovery_report: ProductionDaemonRecoveryReportV1,
    observability: Option<ObservabilityEmitterV1>,
}

impl ProductionDaemonRuntimeV1 {
    /// Acquires the singleton endpoint before loading recoverable state, then recovers each
    /// metadata-only registry entry independently through the real Git/SQLite resolution path.
    pub fn bind(
        paths: &ServiceRuntimePathsV1,
        inspection_options: SqliteStoreOptionsV1,
        configuration: ProductionDaemonRuntimeConfigV1,
    ) -> Result<Self, ProductionDaemonStartupErrorV1> {
        Self::bind_with_observability(paths, inspection_options, configuration, None)
    }

    /// Binds a daemon with an optional non-authoritative typed event producer.
    pub fn bind_with_observability(
        paths: &ServiceRuntimePathsV1,
        inspection_options: SqliteStoreOptionsV1,
        configuration: ProductionDaemonRuntimeConfigV1,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Result<Self, ProductionDaemonStartupErrorV1> {
        let endpoint = match SingletonEndpointV1::acquire(paths) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                emit_observation(
                    &observability,
                    EventOperationV1::DaemonStart,
                    EventOutcomeV1::Failed,
                );
                return Err(ProductionDaemonStartupErrorV1::EndpointAcquire(error));
            }
        };
        let manager = Arc::new(WorkspaceRuntimeManagerV1::with_observability_and_scope(
            paths,
            inspection_options,
            observability.clone(),
            configuration.managed_dev_workspace_root.clone(),
        ));
        let registry = match manager.registry().load() {
            Ok(registry) => registry,
            Err(registry) => {
                emit_observation(
                    &observability,
                    EventOperationV1::DaemonStart,
                    EventOutcomeV1::Failed,
                );
                return Err(shutdown_after_registry_load_failure(endpoint, registry));
            }
        };

        let composition = compose_dispatcher_with_worker_observability_and_procedure_v2_v1(
            Arc::clone(&manager),
            configuration.worker_id().clone(),
            observability.clone(),
            configuration.procedure_v2_admission.clone(),
        );
        let (dispatcher, worker) = composition.into_parts();
        let recovery_report =
            recover_registered_workspaces(&manager, &worker, registry, &observability);
        let admission = ShutdownAdmissionV1::new();
        let mut transport = ProductionTransportV1::new_with_observability(
            PeerUidVerifierV1::for_current_user(),
            dispatcher,
            configuration.transport_timeouts(),
            observability.clone(),
        );
        if let Some(identity) = configuration.process_identity().cloned() {
            transport = transport
                .with_process_identity(identity.with_effective_socket_path(endpoint.socket_path()));
        }
        if configuration.dev_mode() {
            transport = transport.with_dev_shutdown(admission.clone());
        }
        let transport = Arc::new(transport);
        let accept_loop = ProductionAcceptLoopV1::new_with_observability(
            transport,
            admission.clone(),
            configuration.maximum_in_flight_connections(),
            observability.clone(),
        );

        emit_observation(
            &observability,
            EventOperationV1::DaemonStart,
            EventOutcomeV1::Succeeded,
        );
        Ok(Self {
            endpoint,
            manager,
            accept_loop,
            shutdown: ProductionDaemonShutdownHandleV1 { admission },
            recovery_report,
            observability,
        })
    }

    pub fn manager(&self) -> &Arc<WorkspaceRuntimeManagerV1> {
        &self.manager
    }

    pub fn recovery_report(&self) -> &ProductionDaemonRecoveryReportV1 {
        &self.recovery_report
    }

    pub fn shutdown_handle(&self) -> ProductionDaemonShutdownHandleV1 {
        self.shutdown.clone()
    }

    pub fn socket_path(&self) -> &Path {
        self.endpoint.socket_path()
    }

    /// Serves until the shutdown handle closes admission, joins every admitted handler, and then
    /// releases only the endpoint socket identity that this runtime owns.
    pub fn run(self) -> Result<ProductionDaemonShutdownReportV1, ProductionDaemonRuntimeErrorV1> {
        let Self {
            endpoint,
            manager: _,
            accept_loop,
            shutdown: _,
            recovery_report,
            observability,
        } = self;
        let accept_result = accept_loop.run(endpoint.listener());
        let shutdown_result = endpoint.shutdown();
        emit_observation(
            &observability,
            EventOperationV1::DaemonStop,
            daemon_stop_outcome(accept_result.is_ok(), shutdown_result.is_ok()),
        );

        match (accept_result, shutdown_result) {
            (Ok(()), Ok(())) => Ok(ProductionDaemonShutdownReportV1::from_recovery(
                &recovery_report,
            )),
            (Err(accept_loop), Ok(())) => {
                Err(ProductionDaemonRuntimeErrorV1::AcceptLoop(accept_loop))
            }
            (Ok(()), Err(endpoint_shutdown)) => Err(
                ProductionDaemonRuntimeErrorV1::EndpointShutdown(endpoint_shutdown),
            ),
            (Err(accept_loop), Err(endpoint_shutdown)) => Err(
                ProductionDaemonRuntimeErrorV1::AcceptLoopAndEndpointShutdown {
                    accept_loop: Box::new(accept_loop),
                    endpoint_shutdown: Box::new(endpoint_shutdown),
                },
            ),
        }
    }
}

fn emit_observation(
    observability: &Option<ObservabilityEmitterV1>,
    operation: EventOperationV1,
    outcome: EventOutcomeV1,
) {
    if let Some(observability) = observability {
        observability.emit(EventRecordV1::new(operation, outcome));
    }
}
const fn daemon_stop_outcome(
    accept_loop_succeeded: bool,
    endpoint_shutdown_succeeded: bool,
) -> EventOutcomeV1 {
    if accept_loop_succeeded && endpoint_shutdown_succeeded {
        EventOutcomeV1::Succeeded
    } else {
        EventOutcomeV1::Failed
    }
}

fn shutdown_after_registry_load_failure(
    endpoint: SingletonEndpointGuardV1,
    registry: RegistryErrorV1,
) -> ProductionDaemonStartupErrorV1 {
    match endpoint.shutdown() {
        Ok(()) => ProductionDaemonStartupErrorV1::RegistryLoad(registry),
        Err(cleanup) => ProductionDaemonStartupErrorV1::RegistryLoadAndEndpointCleanup {
            registry: Box::new(registry),
            cleanup: Box::new(cleanup),
        },
    }
}

fn recover_registered_workspaces(
    manager: &Arc<WorkspaceRuntimeManagerV1>,
    worker: &ProductionMutationWorkerV1,
    registry: WorkspaceRegistryV1,
    observability: &Option<ObservabilityEmitterV1>,
) -> ProductionDaemonRecoveryReportV1 {
    let clock = NativeProductionClockV1::default();
    let mut outcomes = Vec::with_capacity(registry.workspaces().len());
    let mut recovered = Vec::new();

    for entry in registry.workspaces() {
        let workspace_uuid = entry.workspace_uuid().clone();
        let index = outcomes.len();
        let selector = match selector_from_registry_entry(entry) {
            Ok(selector) => selector,
            Err(reason) => {
                outcomes.push(Some(WorkspaceRecoveryEntryV1::Unavailable(
                    UnavailableWorkspaceReportV1::new(workspace_uuid, reason),
                )));
                continue;
            }
        };
        let observation = WorkspaceRuntimeObservationV1::new(clock.now(), clock.generated_at());
        match manager.begin_reset_maintenance(selector.clone()) {
            Ok(transaction) => match transaction.discover_marker() {
                Ok(Some(marker)) => {
                    let completion = if transaction.authority().registry_previous_workspace_uuid()
                        != &workspace_uuid
                    {
                        Err(WorkspaceRuntimeErrorV1::ResetRegistryPredecessorStale)
                    } else {
                        transaction.resume(
                            marker.idempotency_key(),
                            marker.request_digest(),
                            observation,
                        )
                    };
                    match completion {
                        Ok(completion) => {
                            let scheduler = Arc::clone(completion.scheduler());
                            let target_workspace_uuid = scheduler
                                .context_snapshot()
                                .binding()
                                .identity()
                                .workspace_uuid()
                                .clone();
                            let requeued_job_count = scheduler
                                .context_snapshot()
                                .store()
                                .startup_recovery_report()
                                .requeued_job_count();
                            outcomes.push(None);
                            recovered.push((
                                index,
                                target_workspace_uuid,
                                requeued_job_count,
                                scheduler,
                            ));
                        }
                        Err(error) => outcomes.push(Some(WorkspaceRecoveryEntryV1::Unavailable(
                            UnavailableWorkspaceReportV1::new(
                                workspace_uuid,
                                unavailable_reason_from_runtime_error(error),
                            ),
                        ))),
                    }
                }
                Ok(None) => {
                    drop(transaction);
                    match manager.resolve_existing(selector, Some(&workspace_uuid), observation) {
                        Ok(scheduler) => {
                            let requeued_job_count = scheduler
                                .context_snapshot()
                                .store()
                                .startup_recovery_report()
                                .requeued_job_count();
                            outcomes.push(None);
                            recovered.push((index, workspace_uuid, requeued_job_count, scheduler));
                        }
                        Err(error) => outcomes.push(Some(WorkspaceRecoveryEntryV1::Unavailable(
                            UnavailableWorkspaceReportV1::new(
                                workspace_uuid,
                                unavailable_reason_from_runtime_error(error),
                            ),
                        ))),
                    }
                }
                Err(error) => outcomes.push(Some(WorkspaceRecoveryEntryV1::Unavailable(
                    UnavailableWorkspaceReportV1::new(
                        workspace_uuid,
                        unavailable_reason_from_runtime_error(error),
                    ),
                ))),
            },
            Err(error) => outcomes.push(Some(WorkspaceRecoveryEntryV1::Unavailable(
                UnavailableWorkspaceReportV1::new(
                    workspace_uuid,
                    unavailable_reason_from_runtime_error(error),
                ),
            ))),
        }
    }

    let drains = worker.drain_recovered_queues(
        recovered
            .iter()
            .map(|(_, _, _, scheduler)| Arc::clone(scheduler)),
    );
    for ((index, workspace_uuid, requeued_job_count, _), drain) in recovered.into_iter().zip(drains)
    {
        let entry = match drain {
            Ok(report) => WorkspaceRecoveryEntryV1::Recovered(RecoveredWorkspaceReportV1::new(
                workspace_uuid,
                requeued_job_count,
                report.terminal_job_count(),
            )),
            Err(error) => WorkspaceRecoveryEntryV1::Unavailable(UnavailableWorkspaceReportV1::new(
                workspace_uuid,
                unavailable_reason_from_worker_error(error),
            )),
        };
        outcomes[index] = Some(entry);
    }

    let report = ProductionDaemonRecoveryReportV1::new(outcomes.into_iter().flatten().collect());
    emit_observation(
        observability,
        EventOperationV1::IntegrityCheck,
        if report.unavailable_workspace_count() == 0 {
            EventOutcomeV1::Succeeded
        } else {
            EventOutcomeV1::Failed
        },
    );
    report
}

fn selector_from_registry_entry(
    entry: &WorkspaceRegistryEntryV1,
) -> Result<WorktreeSelectorV1, WorkspaceRecoveryUnavailableReasonV1> {
    let display = DiagnosticPathDisplayV1::new("registered workspace")
        .map_err(|_| WorkspaceRecoveryUnavailableReasonV1::RegisteredRootInvalid)?;
    let path = LosslessPathV1::from_raw_bytes(entry.last_known_root().unix_bytes(), display)
        .map_err(|_| WorkspaceRecoveryUnavailableReasonV1::RegisteredRootInvalid)?;
    WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, path)
        .map_err(|_| WorkspaceRecoveryUnavailableReasonV1::RegisteredRootInvalid)
}

fn unavailable_reason_from_runtime_error(
    error: WorkspaceRuntimeErrorV1,
) -> WorkspaceRecoveryUnavailableReasonV1 {
    match error {
        WorkspaceRuntimeErrorV1::Resolution(error) => {
            unavailable_reason_from_resolution_error(error)
        }
        WorkspaceRuntimeErrorV1::ConfigRead { .. }
        | WorkspaceRuntimeErrorV1::ConfigAdmission(_)
        | WorkspaceRuntimeErrorV1::StoreOptions(_) => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceConfigurationInvalid
        }
        WorkspaceRuntimeErrorV1::Store(_) | WorkspaceRuntimeErrorV1::BindingDisappeared { .. } => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceStateUnreadable
        }
        WorkspaceRuntimeErrorV1::MaintenanceInProgress
        | WorkspaceRuntimeErrorV1::ResetAdmissionOutcomeUnknown { .. }
        | WorkspaceRuntimeErrorV1::ResetAdmitted { .. }
        | WorkspaceRuntimeErrorV1::ResetSchedulerRetirement
        | WorkspaceRuntimeErrorV1::ResetMarkerConflict
        | WorkspaceRuntimeErrorV1::ResetIdempotencyConflict { .. }
        | WorkspaceRuntimeErrorV1::ResetRegistryPredecessorStale
        | WorkspaceRuntimeErrorV1::RuntimeDirectory(_) => {
            WorkspaceRecoveryUnavailableReasonV1::DaemonUnavailable
        }
        WorkspaceRuntimeErrorV1::ResetSourceNotRegistered => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceNotInitialized
        }
        WorkspaceRuntimeErrorV1::BindingIdentityMismatch { .. }
        | WorkspaceRuntimeErrorV1::BindingMismatch { .. }
        | WorkspaceRuntimeErrorV1::RebindEvidenceMismatch { .. }
        | WorkspaceRuntimeErrorV1::ResetSourceAmbiguous
        | WorkspaceRuntimeErrorV1::RevalidationKeyMismatch { .. } => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceIdentityConflict
        }
        WorkspaceRuntimeErrorV1::RuntimeDirectoryMissing { .. }
        | WorkspaceRuntimeErrorV1::RuntimePathMismatch { .. }
        | WorkspaceRuntimeErrorV1::RuntimePathsUnsupportedPlatform => {
            WorkspaceRecoveryUnavailableReasonV1::RegisteredRootInvalid
        }
        WorkspaceRuntimeErrorV1::Registry(RegistryErrorV1::WorkspaceRootOccupied { .. }) => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceIdentityConflict
        }
        WorkspaceRuntimeErrorV1::Layout(_)
        | WorkspaceRuntimeErrorV1::Registry(_)
        | WorkspaceRuntimeErrorV1::Scheduler(_) => {
            WorkspaceRecoveryUnavailableReasonV1::DaemonUnavailable
        }
    }
}

fn unavailable_reason_from_resolution_error(
    error: WorkspaceResolutionErrorV1,
) -> WorkspaceRecoveryUnavailableReasonV1 {
    match error {
        WorkspaceResolutionErrorV1::Git {
            source:
                podway_git::GitResolverErrorV1::CopiedWorkspaceUuid { .. }
                | podway_git::GitResolverErrorV1::IdentityMismatch { .. }
                | podway_git::GitResolverErrorV1::MoveConflict { .. },
            ..
        }
        | WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { .. }
        | WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch { .. }
        | WorkspaceResolutionErrorV1::PreliminaryIdentityWasNotCandidate { .. }
        | WorkspaceResolutionErrorV1::RevalidatedIdentityStateMismatch { .. }
        | WorkspaceResolutionErrorV1::RevalidatedStoreIdentityMismatch { .. }
        | WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceIdentityConflict
        }
        WorkspaceResolutionErrorV1::Git {
            source: podway_git::GitResolverErrorV1::Selector(_),
            ..
        }
        | WorkspaceResolutionErrorV1::ManagedDevScopeViolation
        | WorkspaceResolutionErrorV1::Selector { .. }
        | WorkspaceResolutionErrorV1::StoredRootPathInvalid { .. }
        | WorkspaceResolutionErrorV1::WorkspaceRootPathInvalid { .. }
        | WorkspaceResolutionErrorV1::RuntimeDirectoryPathInvalid { .. }
        | WorkspaceResolutionErrorV1::RuntimeDirectoryPathUnsupportedPlatform
        | WorkspaceResolutionErrorV1::WorkspaceRootConversion { .. } => {
            WorkspaceRecoveryUnavailableReasonV1::RegisteredRootInvalid
        }
        WorkspaceResolutionErrorV1::ExistingBindingMissing => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceNotInitialized
        }
        WorkspaceResolutionErrorV1::BindingInspection { .. }
        | WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent => {
            WorkspaceRecoveryUnavailableReasonV1::DaemonUnavailable
        }
        WorkspaceResolutionErrorV1::Git { .. } => {
            WorkspaceRecoveryUnavailableReasonV1::WorktreeGone
        }
    }
}

fn unavailable_reason_from_worker_error(
    error: WorkerErrorV1,
) -> WorkspaceRecoveryUnavailableReasonV1 {
    match error {
        WorkerErrorV1::AfterAdmission { source, .. } => {
            unavailable_reason_from_worker_error(*source)
        }
        WorkerErrorV1::Store(_)
        | WorkerErrorV1::Execution(_)
        | WorkerErrorV1::ProcedureV2Preparation(_)
        | WorkerErrorV1::RecoveryRequired => {
            WorkspaceRecoveryUnavailableReasonV1::WorkspaceStateUnreadable
        }
        WorkerErrorV1::Progress(_)
        | WorkerErrorV1::JobNotFound(_)
        | WorkerErrorV1::BackgroundPanicked
        | WorkerErrorV1::RetirementRejected => {
            WorkspaceRecoveryUnavailableReasonV1::DaemonUnavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EventOutcomeV1, daemon_stop_outcome};

    #[test]
    fn daemon_stop_requires_complete_runtime_success() {
        for (accept_loop_succeeded, endpoint_shutdown_succeeded, expected) in [
            (true, true, EventOutcomeV1::Succeeded),
            (false, true, EventOutcomeV1::Failed),
            (true, false, EventOutcomeV1::Failed),
            (false, false, EventOutcomeV1::Failed),
        ] {
            assert_eq!(
                daemon_stop_outcome(accept_loop_succeeded, endpoint_shutdown_succeeded),
                expected
            );
        }
    }
}
